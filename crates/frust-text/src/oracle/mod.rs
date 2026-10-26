//! The font oracle: frust's own answer to "which face draws this cluster?".
//!
//! It replaces fontique's collection, query and system backends (decisions
//! D3-D5). Per unresolved cluster it consults, in order, the app-font
//! registry, its fallback cache, the platform backend, and the generic
//! defaults; every candidate must cover the cluster in its `cmap` and be
//! drawable by frust-engine ([`RenderCapabilities`]), and the answer is a plain
//! frust-owned [`FaceRef`].
//!
//! # Boundary (D14)
//!
//! Nothing in `oracle/` names a shaping-engine type. The crate-internal API is
//! [`Oracle::resolve`] over a [`FallbackRequest`] and [`Oracle::default_face`];
//! the adapter that turns those answers into a shaper's font selection lives
//! outside this module, so a later shaper swap leaves `oracle/` untouched. The
//! request and cache keys use frust-owned hashable mirrors of the style types
//! ([`GenericKey`], [`cache::CacheKey`]) because `crate::style`'s
//! `GenericSlot`/`FamilyName`/`FontWeight` are not `Hash`.
//!
//! # Module map
//!
//! - [`registry`]: the app-font registry (the process-wide app-font and
//!   generic-fallback records, parsed per context with read-fonts).
//! - [`cache`]: the in-memory LRU of system answers (misses included) and its
//!   persistent, stamped, write-behind copy, placed by [`CacheLocation`].
//! - [`selector`]: the resolve pipeline, coverage, renderability and synthesis.
//! - One backend per platform, each cfg-gated on its `mod` line below
//!   (`apple`, `android`, `fontconfig`, `dwrite`, `web`); `apple`, `android` and
//!   `dwrite` ask the OS, `web` answers `None` by design, and `fontconfig` is a
//!   stub until its card fills it in.
//! - `synthetic` (tests only): a scripted backend over in-memory faces.
//!
//! # No FFI here
//!
//! frust-text is on CODE_STANDARDS' unsafe-free list, with one sanctioned
//! zone (decision D-6): the `apple`, `android`, `fontconfig` and `dwrite`
//! backend files, each opening it with `#![allow(unsafe_code)]`. [`FaceStore`]
//! reads font files with `std::fs::read` (no memory mapping);
//! [`FaceLocation::Blob`] lets a backend hand over an already-built buffer
//! instead.

// `deny`, not `forbid`: `forbid` cannot be overridden by an inner `allow`, and
// the D-6 zone files must be able to open it. Every other file under `oracle/`
// stays unsafe-free, enforced by the guard tests below.
#![deny(unsafe_code)]
#![cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the resolve pipeline's first production caller is the shaping adapter (l-01)"
    )
)]

pub(crate) mod cache;
pub(crate) mod registry;
mod selector;
#[cfg(test)]
pub(crate) mod synthetic;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_vendor = "apple")]
mod apple;
#[cfg(windows)]
mod dwrite;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod fontconfig;
#[cfg(target_family = "wasm")]
mod web;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use peniko::Blob;
use serde::{Deserialize, Serialize};

use crate::style::{FamilyName, FontFamily, FontStyle, FontWeight, GenericSlot};

pub(crate) use registry::AppFontRegistry;

/// What frust-engine can draw, which decides whether a candidate face is
/// usable (the renderability filter).
///
/// A face is accepted when at least one of its glyph formats is enabled here;
/// a face whose only glyph data is a disabled format is *refused* (counted in
/// the oracle's stats) and resolution moves on to the next candidate — it is
/// never swapped for a "similar" face.
///
/// [`Default`] describes today's engine: outlines (`glyf`/`CFF `/`CFF2`), COLR
/// color glyphs and bitmap strikes (`CBDT`/`sbix`/`EBDT`) everywhere, and
/// `hvgl`-only faces (drawn through CoreText outlines) on Apple targets only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RenderCapabilities {
    /// Vector outlines: `glyf`, `CFF ` or `CFF2`.
    pub outlines: bool,
    /// COLR (v0/v1) color glyphs.
    pub colr: bool,
    /// Bitmap strikes: `CBDT`, `sbix` or `EBDT`.
    pub bitmap: bool,
    /// `hvgl` glyph data, drawable only through CoreText on Apple platforms.
    pub hvgl_via_coretext: bool,
}

impl Default for RenderCapabilities {
    fn default() -> Self {
        Self {
            outlines: true,
            colr: true,
            bitmap: true,
            hvgl_via_coretext: cfg!(target_vendor = "apple"),
        }
    }
}

impl RenderCapabilities {
    /// Whether a face carrying `formats` has at least one drawable format.
    pub(crate) fn can_draw(&self, formats: &DrawFormats) -> bool {
        (self.outlines && formats.outlines)
            || (self.colr && formats.colr)
            || (self.bitmap && formats.bitmap)
            || (self.hvgl_via_coretext && formats.hvgl)
    }
}

/// The glyph formats a face carries, read from its table directory.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DrawFormats {
    pub(crate) outlines: bool,
    pub(crate) colr: bool,
    pub(crate) bitmap: bool,
    pub(crate) hvgl: bool,
}

/// Frust-owned, hashable mirror of [`GenericSlot`] for cache keys and
/// serialisation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum GenericKey {
    Monospace,
    SansSerif,
    Serif,
    SystemUi,
    Emoji,
}

impl From<GenericSlot> for GenericKey {
    fn from(slot: GenericSlot) -> Self {
        match slot {
            GenericSlot::Monospace => Self::Monospace,
            GenericSlot::SansSerif => Self::SansSerif,
            GenericSlot::Serif => Self::Serif,
            GenericSlot::SystemUi => Self::SystemUi,
            GenericSlot::Emoji => Self::Emoji,
        }
    }
}

/// An ISO 15924 script tag (`Latn`, `Hani`, `Zyyy`, ...).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct ScriptTag(pub(crate) [u8; 4]);

impl ScriptTag {
    /// `Zyyy`: characters shared by several scripts (spaces, digits, punctuation).
    pub(crate) const COMMON: Self = Self(*b"Zyyy");
}

/// One font-variation axis setting (`wght`, `wdth`, `ital`, `slnt`, `opsz`, ...).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct VariationSetting {
    pub(crate) tag: [u8; 4],
    pub(crate) value: f32,
}

/// How to draw a face so it looks like the requested attributes: variation
/// coordinates to apply, plus faux bold / faux oblique when the face has no
/// axis for the difference. Frust-owned (D14); the shaping adapter translates
/// it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Synthesis {
    /// Faux bold: the face is lighter than requested and has no `wght` axis.
    pub(crate) embolden: bool,
    /// Faux oblique angle in degrees, when the face is upright, slant was
    /// requested and it has no `ital`/`slnt` axis.
    pub(crate) skew: Option<f32>,
    /// Variation coordinates, in application order.
    pub(crate) variations: Vec<VariationSetting>,
}

impl Synthesis {
    /// The value set for `tag`, if any.
    pub(crate) fn variation(&self, tag: [u8; 4]) -> Option<f32> {
        self.variations
            .iter()
            .find(|v| v.tag == tag)
            .map(|v| v.value)
    }
}

/// One cluster's fallback question.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FallbackRequest {
    /// The cluster's characters, in text order.
    pub(crate) chars: String,
    pub(crate) script: ScriptTag,
    /// BCP 47 locale (`ja`, `zh-Hant`, ...), when the text carries one.
    pub(crate) locale: Option<String>,
    pub(crate) weight: FontWeight,
    pub(crate) slant: FontStyle,
    /// Width as a ratio of normal (`1.0`), the CSS `font-stretch` scale.
    pub(crate) width: f32,
    /// The style's family chain: named families and generic slots, in order.
    pub(crate) families: Vec<FamilyName>,
    pub(crate) renderability: RenderCapabilities,
}

impl FallbackRequest {
    /// A request for `chars` with regular attributes and `family`'s chain.
    pub(crate) fn new(
        chars: impl Into<String>,
        script: ScriptTag,
        family: &FontFamily,
        renderability: RenderCapabilities,
    ) -> Self {
        Self {
            chars: chars.into(),
            script,
            locale: None,
            weight: FontWeight::REGULAR,
            slant: FontStyle::Normal,
            width: 1.0,
            families: family_chain(family),
            renderability,
        }
    }

    /// The generic slots named in the chain, or `[SystemUi]` when it names none.
    pub(crate) fn generics(&self) -> Vec<GenericSlot> {
        let generics: Vec<GenericSlot> = self
            .families
            .iter()
            .filter_map(|f| match f {
                FamilyName::Generic(g) => Some(*g),
                FamilyName::Named(_) => None,
            })
            .collect();
        if generics.is_empty() {
            vec![GenericSlot::SystemUi]
        } else {
            generics
        }
    }
}

/// Flattens a style's [`FontFamily`] into the chain the oracle walks.
pub(crate) fn family_chain(family: &FontFamily) -> Vec<FamilyName> {
    match family {
        FontFamily::SystemUi => vec![FamilyName::Generic(GenericSlot::SystemUi)],
        FontFamily::Named(names) => names.iter().cloned().map(FamilyName::Named).collect(),
        FontFamily::NamedWithGeneric(chain) => chain.clone(),
    }
}

/// Where a backend found a face's bytes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FaceLocation {
    /// A font file on disk, read once by [`FaceStore`].
    Path(PathBuf),
    /// Bytes the backend already holds.
    Blob(Blob<u8>),
}

/// A backend's answer: the face's bytes, its index in a collection, and what
/// the backend knows about drawing it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FaceSource {
    pub(crate) location: FaceLocation,
    /// Face index inside a TTC/OTC (`0` for a single-face file).
    pub(crate) index: u32,
    /// Instance coordinates the backend matched (CoreText's
    /// `CTFontCopyVariation`, a named instance); applied before synthesis.
    pub(crate) variations: Vec<VariationSetting>,
    /// A backend-provided synthesis decision (e.g. Android's matcher reporting
    /// faux bold is needed); replaces the oracle's own weight/slant rule.
    pub(crate) synthesis_hint: Option<Synthesis>,
    /// The PostScript name (name ID 6 of the face, or of the fvar named
    /// instance) as the OS resolved it; `None` when the backend does not know it.
    pub(crate) postscript_name: Option<String>,
    /// A dot-prefixed system-private face (e.g. `.SF NS`): passed through
    /// with the flag set, never substituted.
    pub(crate) hidden: bool,
    /// DirectWrite's `MapCharacters` scale factor; `None` on every other backend.
    pub(crate) scale: Option<f32>,
}

impl FaceSource {
    /// An answer at `location` with no coordinates, hint, name or scale.
    pub(crate) fn new(location: FaceLocation, index: u32) -> Self {
        Self {
            location,
            index,
            variations: Vec::new(),
            synthesis_hint: None,
            postscript_name: None,
            hidden: false,
            scale: None,
        }
    }

    /// A plain path answer with no coordinates or hint.
    pub(crate) fn path(path: impl Into<PathBuf>, index: u32) -> Self {
        Self::new(FaceLocation::Path(path.into()), index)
    }

    /// A plain in-memory answer with no coordinates or hint.
    pub(crate) fn blob(blob: Blob<u8>, index: u32) -> Self {
        Self::new(FaceLocation::Blob(blob), index)
    }
}

/// A face's identity: which bytes, which face in them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FaceKey {
    pub(crate) file: FileId,
    pub(crate) index: u32,
}

/// The file half of a [`FaceKey`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum FileId {
    Path(PathBuf),
    /// A [`Blob`]'s unique id.
    Blob(u64),
}

impl FaceKey {
    pub(crate) fn of(source: &FaceSource) -> Self {
        let file = match &source.location {
            FaceLocation::Path(path) => FileId::Path(path.clone()),
            FaceLocation::Blob(blob) => FileId::Blob(blob.id()),
        };
        Self {
            file,
            index: source.index,
        }
    }
}

/// The oracle's answer: the bytes to shape and draw, the face index, and how
/// to synthesise the requested attributes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FaceRef {
    pub(crate) blob: Blob<u8>,
    pub(crate) index: u32,
    pub(crate) synthesis: Synthesis,
    /// The PostScript name the OS resolved, when a backend reported one.
    pub(crate) postscript_name: Option<String>,
    /// A dot-prefixed system-private face, carried forward for the shaper and engine.
    pub(crate) hidden: bool,
}

/// Reads each font file once and hands out shared [`Blob`]s.
///
/// An unreadable path is remembered as such, so a broken backend answer costs
/// one failed read, not one per cluster.
#[derive(Debug, Default)]
pub(crate) struct FaceStore {
    files: HashMap<PathBuf, Option<Blob<u8>>>,
    reads: usize,
}

impl FaceStore {
    /// The bytes and face index `source` names, reading its file on first use.
    pub(crate) fn load(&mut self, source: &FaceSource) -> Option<(Blob<u8>, u32)> {
        let blob = match &source.location {
            FaceLocation::Blob(blob) => blob.clone(),
            FaceLocation::Path(path) => self.read(path)?,
        };
        Some((blob, source.index))
    }

    fn read(&mut self, path: &Path) -> Option<Blob<u8>> {
        if let Some(cached) = self.files.get(path) {
            return cached.clone();
        }
        self.reads += 1;
        let blob = match std::fs::read(path) {
            Ok(bytes) => Some(Blob::from(bytes)),
            Err(err) => {
                log::debug!("frust-text: font file {} unreadable: {err}", path.display());
                None
            }
        };
        self.files.insert(path.to_path_buf(), blob.clone());
        blob
    }

    /// How many file reads (successful or not) this store has performed.
    pub(crate) fn file_reads(&self) -> usize {
        self.reads
    }
}

/// A platform's font-fallback source. One implementation per OS, chosen by
/// cfg in [`platform_backend`]; `synthetic::SyntheticBackend` in tests.
///
/// `Send` because a shell builds its `TextContext` on a pre-init thread.
pub(crate) trait Backend: Send {
    /// The system's face for `req`'s cluster, or `None` when it has none. The
    /// oracle checks coverage and renderability itself, so a backend may answer
    /// with a face that turns out unusable.
    fn resolve(&mut self, req: &FallbackRequest) -> Option<FaceSource>;
    /// The system's default face for a generic slot.
    fn default_family(&self, generic: GenericSlot) -> Option<FaceSource>;
    /// Identifies the installed OS build; any change invalidates the
    /// persistent cache wholesale.
    fn os_build_id(&self) -> String;
}

/// The backend for the target being compiled.
pub(crate) fn platform_backend() -> Box<dyn Backend> {
    #[cfg(target_vendor = "apple")]
    let backend = apple::CoreTextBackend::new();
    #[cfg(target_os = "android")]
    let backend = android::NdkFontBackend::new();
    #[cfg(windows)]
    let backend = dwrite::DirectWriteBackend::new();
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    let backend = fontconfig::FontconfigBackend::new();
    #[cfg(target_family = "wasm")]
    let backend = web::WebBackend::new();
    #[cfg(not(any(
        target_vendor = "apple",
        target_os = "android",
        windows,
        target_os = "linux",
        target_os = "freebsd",
        target_family = "wasm"
    )))]
    let backend = NoBackend;
    Box::new(backend)
}

/// The backend of a target with no font API wired: every question is a miss.
#[cfg_attr(
    any(
        target_vendor = "apple",
        target_os = "android",
        windows,
        target_os = "linux",
        target_os = "freebsd",
        target_family = "wasm"
    ),
    allow(dead_code, reason = "only targets without a platform backend use it")
)]
struct NoBackend;

impl Backend for NoBackend {
    fn resolve(&mut self, _req: &FallbackRequest) -> Option<FaceSource> {
        None
    }
    fn default_family(&self, _generic: GenericSlot) -> Option<FaceSource> {
        None
    }
    fn os_build_id(&self) -> String {
        String::from("none")
    }
}

/// Where the persistent fallback cache lives for this process, if anywhere:
/// `<frust-paths cache dir>/<app stem>/frust-text/font-fallback.json`.
///
/// `None` until frust-paths can answer — on Android, until the shell's
/// `nativeInitPlatform` installed the app's directories — which is why
/// [`CacheLocation::Lazy`] asks again at the cache's next miss or flush.
pub(crate) fn persistent_cache_path() -> Option<PathBuf> {
    let base = frust_paths::cache_dir()?;
    Some(
        base.join(frust_paths::app_stem())
            .join("frust-text")
            .join(cache::PERSIST_FILE_NAME),
    )
}

/// The persistent cache file inside an explicitly chosen directory.
pub(crate) fn cache_file_in(dir: &Path) -> PathBuf {
    dir.join(cache::PERSIST_FILE_NAME)
}

/// Where an [`Oracle`] keeps its persistent fallback cache.
#[derive(Debug)]
pub(crate) enum CacheLocation {
    /// Memory only: nothing is read or written.
    Disabled,
    /// This file, read at the first miss or flush.
    File(PathBuf),
    /// The file this resolver names, asked at the first miss or flush and
    /// again at later ones while it answers `None`.
    Lazy(cache::PathResolver),
}

impl CacheLocation {
    /// The platform default: [`persistent_cache_path`], resolved lazily.
    pub(crate) fn platform() -> Self {
        Self::Lazy(cache::PathResolver::new(persistent_cache_path))
    }
}

/// The system and user font directories whose mtimes stamp the persistent
/// cache, so installing or removing a font expires its cached misses. A
/// directory's mtime changes when an entry directly inside it does; fonts in
/// nested folders are covered by the OS build id and each face's file stamp.
pub(crate) fn font_dirs() -> Vec<PathBuf> {
    #[cfg_attr(
        not(any(
            target_vendor = "apple",
            windows,
            target_os = "linux",
            target_os = "freebsd"
        )),
        allow(unused_mut)
    )]
    let mut dirs: Vec<PathBuf> = Vec::new();
    let env_dir = |var: &str| {
        std::env::var_os(var)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/System/Library/Fonts"));
        dirs.push(PathBuf::from("/Library/Fonts"));
        dirs.extend(env_dir("HOME").map(|home| home.join("Library/Fonts")));
    }
    #[cfg(all(target_vendor = "apple", not(target_os = "macos")))]
    dirs.push(PathBuf::from("/System/Library/Fonts"));
    #[cfg(target_os = "android")]
    dirs.extend(["/system/fonts", "/product/fonts", "/data/fonts/files"].map(PathBuf::from));
    #[cfg(windows)]
    {
        let windir = env_dir("WINDIR").unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        dirs.push(windir.join("Fonts"));
        dirs.extend(env_dir("LOCALAPPDATA").map(|local| local.join(r"Microsoft\Windows\Fonts")));
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        // fontconfig rewrites its caches whenever the font set changes.
        let user = env_dir("XDG_CACHE_HOME").or_else(|| env_dir("HOME").map(|h| h.join(".cache")));
        dirs.extend(user.map(|cache| cache.join("fontconfig")));
        dirs.push(PathBuf::from("/var/cache/fontconfig"));
    }
    let _ = env_dir;
    dirs
}

/// Counters for the resolve pipeline, read by tests and perf traces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct OracleStats {
    /// Answers served by an app face named in the family chain.
    pub(crate) app_hits: usize,
    /// Cache lookups that found an answer (a face or a cached miss).
    pub(crate) cache_hits: usize,
    /// Cache lookups that found nothing, so the backend was asked.
    pub(crate) cache_misses: usize,
    /// Calls to [`Backend::resolve`].
    pub(crate) backend_calls: usize,
    /// Answers served by a generic default (system or app generic fallback).
    pub(crate) generic_hits: usize,
    /// Candidates refused because frust-engine cannot draw them.
    pub(crate) refused: usize,
    /// Candidates rejected because their `cmap` misses a cluster character.
    pub(crate) uncovered: usize,
    /// Requests nothing could answer.
    pub(crate) unresolved: usize,
}

/// Per-face facts the selector reads once per face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FaceFacts {
    pub(crate) formats: DrawFormats,
    pub(crate) attrs: FaceAttrs,
}

/// A face's own style attributes and variation axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FaceAttrs {
    pub(crate) weight: f32,
    pub(crate) style: FontStyle,
    /// Width ratio (`1.0` = normal), from `OS/2.usWidthClass`.
    pub(crate) width: f32,
    pub(crate) has_wght: bool,
    pub(crate) has_wdth: bool,
    pub(crate) has_ital: bool,
    pub(crate) has_slnt: bool,
}

impl Default for FaceAttrs {
    fn default() -> Self {
        Self {
            weight: FontWeight::REGULAR.value(),
            style: FontStyle::Normal,
            width: 1.0,
            has_wght: false,
            has_wdth: false,
            has_ital: false,
            has_slnt: false,
        }
    }
}

/// The font oracle: owns the platform backend, this context's app-font
/// registry, the fallback cache and the face store. See the module docs.
pub(crate) struct Oracle {
    backend: Box<dyn Backend>,
    registry: AppFontRegistry,
    cache: cache::FallbackCache,
    store: FaceStore,
    facts: HashMap<FaceKey, Option<FaceFacts>>,
    capabilities: RenderCapabilities,
    stats: OracleStats,
}

impl std::fmt::Debug for Oracle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Oracle")
            .field("capabilities", &self.capabilities)
            .field("stats", &self.stats)
            .field("cache_read", &self.cache.load_time())
            .finish_non_exhaustive()
    }
}

impl Oracle {
    /// Builds an oracle over `backend` whose persistent cache is the file
    /// `persist` (read at the first miss), or memory only for `None`.
    pub(crate) fn new(
        backend: Box<dyn Backend>,
        capabilities: RenderCapabilities,
        persist: Option<PathBuf>,
    ) -> Self {
        let location = match persist {
            Some(path) => CacheLocation::File(path),
            None => CacheLocation::Disabled,
        };
        Self::with_cache_location(backend, capabilities, location)
    }

    /// Builds an oracle over `backend` with its persistent cache at
    /// `location`. Nothing is read here: the file is resolved and loaded at
    /// the first cache miss or flush (see [`cache`]).
    pub(crate) fn with_cache_location(
        backend: Box<dyn Backend>,
        capabilities: RenderCapabilities,
        location: CacheLocation,
    ) -> Self {
        let mut cache = cache::FallbackCache::new(cache::MEMORY_CAPACITY);
        let resolver = match location {
            CacheLocation::Disabled => None,
            CacheLocation::File(path) => Some(cache::PathResolver::fixed(path)),
            CacheLocation::Lazy(resolver) => Some(resolver),
        };
        if let Some(resolver) = resolver {
            let stamp = cache::VersionStamp::current(backend.os_build_id());
            cache.attach_lazy(resolver, stamp);
        }
        Self {
            backend,
            registry: AppFontRegistry::default(),
            cache,
            store: FaceStore::default(),
            facts: HashMap::new(),
            capabilities,
            stats: OracleStats::default(),
        }
    }

    /// How long reading the persistent cache took (zero until the first miss
    /// or flush has read it).
    pub(crate) fn cache_read_time(&self) -> Duration {
        self.cache.load_time()
    }

    /// The capabilities this oracle was built for (the default for requests).
    pub(crate) fn capabilities(&self) -> RenderCapabilities {
        self.capabilities
    }

    /// This context's app-font registry.
    pub(crate) fn registry_mut(&mut self) -> &mut AppFontRegistry {
        &mut self.registry
    }

    /// The resolve pipeline's counters.
    pub(crate) fn stats(&self) -> OracleStats {
        self.stats
    }

    /// The fallback cache (tests read its counters and bound).
    pub(crate) fn cache(&self) -> &cache::FallbackCache {
        &self.cache
    }

    /// The face store (tests read its file-read count).
    pub(crate) fn store(&self) -> &FaceStore {
        &self.store
    }

    /// The fallback cache, mutably (tests tune its write-behind triggers).
    pub(crate) fn cache_mut(&mut self) -> &mut cache::FallbackCache {
        &mut self.cache
    }

    /// Writes the persistent cache now if it changed since the last write —
    /// the hook a shell calls when the app is paused or backgrounded, where
    /// `Drop` may never run. Write-behind covers the time in between.
    pub(crate) fn flush(&mut self) -> std::io::Result<()> {
        self.cache.persist()
    }

    /// [`Self::flush`].
    pub(crate) fn persist(&mut self) -> std::io::Result<()> {
        self.flush()
    }
}

impl Drop for Oracle {
    /// The final flush.
    fn drop(&mut self) {
        if let Err(err) = self.cache.persist() {
            log::debug!("frust-text: font fallback cache not written: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `.rs` file under `src/oracle/`, with its source text.
    fn oracle_sources() -> Vec<(PathBuf, String)> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/oracle");
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("src/oracle is readable") {
            let path = entry.expect("dir entry").path();
            if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).expect("source is UTF-8");
                out.push((path, text));
            }
        }
        assert!(
            out.len() >= 10,
            "expected every oracle module, found {}",
            out.len()
        );
        out
    }

    #[test]
    fn the_oracle_names_no_shaping_engine_type() {
        // Built with `concat!` so this file does not match its own scan.
        let forbidden = [
            concat!("parley", "_engine"),
            concat!("harf", "rust"),
            concat!("parley", "::"),
        ];
        for (path, text) in oracle_sources() {
            for word in forbidden {
                assert!(
                    !text.contains(word),
                    "{} mentions `{word}`: the oracle must stay shaper-agnostic (D14)",
                    path.display()
                );
            }
        }
    }

    /// The word the guard scans for, spelled so this file does not match itself.
    const UNSAFE_WORD: &str = concat!("un", "safe");

    /// The sanctioned unsafe zone (decision D-6): the only oracle files that may
    /// contain `unsafe`. `store.rs` is reserved for the file-mapping zone and may
    /// not exist yet.
    const UNSAFE_ZONE: [&str; 5] = [
        "apple.rs",
        "android.rs",
        "fontconfig.rs",
        "dwrite.rs",
        "store.rs",
    ];

    /// The marker every zone file's leading `//!` header carries.
    const ZONE_MARKER: &str = concat!("Sanctioned un", "safe zone (decision D-6)");

    /// The attribute that opens the zone for one module.
    const ZONE_ALLOW: &str = "#![allow(unsafe_code)]";

    fn is_zone(path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| UNSAFE_ZONE.contains(&name))
    }

    /// Whether `code` contains [`UNSAFE_WORD`] as a whole word, so identifiers
    /// such as `unsafe_code` do not count.
    fn has_unsafe_token(code: &str) -> bool {
        let is_ident = |c: char| c.is_alphanumeric() || c == '_';
        code.match_indices(UNSAFE_WORD).any(|(at, _)| {
            let before = code[..at].chars().next_back();
            let after = code[at + UNSAFE_WORD.len()..].chars().next();
            !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
        })
    }

    #[test]
    fn no_unsafe_token_outside_the_zone() {
        for (path, text) in oracle_sources() {
            if is_zone(&path) {
                continue;
            }
            for line in text.lines() {
                // Prose may say "unsafe"; only the code before a `//` counts.
                let code = line.split_once("//").map_or(line, |(code, _)| code);
                assert!(
                    !has_unsafe_token(code),
                    "{} uses `{UNSAFE_WORD}` outside the D-6 zone: {line}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn every_zone_file_opens_the_zone_with_a_header() {
        let zone_files: Vec<_> = oracle_sources()
            .into_iter()
            .filter(|(path, _)| is_zone(path))
            .collect();
        assert!(
            zone_files.len() >= 4,
            "expected the four backend zone files, found {}",
            zone_files.len()
        );
        for (path, text) in zone_files {
            let allow = text.lines().position(|line| line == ZONE_ALLOW);
            let first_item = text
                .lines()
                .position(|line| line.starts_with("use ") || line.starts_with("mod "));
            assert!(
                allow.is_some() && allow < first_item,
                "{} must open with `{ZONE_ALLOW}` before its items",
                path.display()
            );
            assert!(
                text.lines()
                    .take_while(|line| line.starts_with("//!"))
                    .any(|line| line.contains(ZONE_MARKER)),
                "{} lacks the `{ZONE_MARKER}` header",
                path.display()
            );
        }
    }

    #[test]
    fn the_unsafe_token_scan_matches_whole_words_only() {
        assert!(has_unsafe_token(concat!("un", "safe { f() }")));
        assert!(has_unsafe_token(concat!("pub ", "un", "safe fn f()")));
        assert!(!has_unsafe_token("#![deny(unsafe_code)]"));
        assert!(!has_unsafe_token("let is_unsafe_free = true;"));
    }

    #[test]
    fn default_capabilities_match_todays_engine() {
        let caps = RenderCapabilities::default();
        assert!(caps.outlines && caps.colr && caps.bitmap);
        assert_eq!(caps.hvgl_via_coretext, cfg!(target_vendor = "apple"));
        let hvgl_only = DrawFormats {
            hvgl: true,
            ..DrawFormats::default()
        };
        assert_eq!(caps.can_draw(&hvgl_only), cfg!(target_vendor = "apple"));
    }

    #[test]
    fn the_platform_backend_constructs_and_names_the_os_build() {
        let backend = platform_backend();
        assert!(!backend.os_build_id().is_empty());
    }

    #[test]
    fn face_source_new_defaults_the_os_facts() {
        let src = FaceSource::new(FaceLocation::Path(PathBuf::from("a.ttf")), 3);
        assert_eq!(src.index, 3);
        assert!(src.variations.is_empty());
        assert_eq!(src.synthesis_hint, None);
        assert_eq!(src.postscript_name, None);
        assert!(!src.hidden);
        assert_eq!(src.scale, None);
    }

    #[cfg(target_family = "wasm")]
    #[test]
    fn the_platform_stub_backend_answers_nothing() {
        let mut backend = platform_backend();
        let req = FallbackRequest::new(
            "a",
            ScriptTag(*b"Latn"),
            &FontFamily::SystemUi,
            RenderCapabilities::default(),
        );
        assert_eq!(backend.resolve(&req), None);
        assert_eq!(backend.default_family(GenericSlot::SansSerif), None);
        assert!(!backend.os_build_id().is_empty());
    }

    fn latin_request(chars: &str) -> FallbackRequest {
        FallbackRequest::new(
            chars,
            ScriptTag(*b"Latn"),
            &FontFamily::SystemUi,
            RenderCapabilities::default(),
        )
    }

    /// A synthetic backend answering `font` for "a" and "b", and its call log.
    fn backend_over(font: &Path) -> (synthetic::SyntheticBackend, synthetic::Calls) {
        let mut backend = synthetic::SyntheticBackend::new("build-1");
        backend.cover_with(FaceSource::path(font, 0), "ab");
        let calls = backend.calls();
        (backend, calls)
    }

    #[test]
    fn an_oracle_built_before_the_cache_dir_resolves_still_persists() {
        use std::sync::{Arc, Mutex};
        let dir = synthetic::TempDir::new("oracle-lazy");
        let font = dir.write("a.ttf", &synthetic::FaceSpec::new("A").chars("ab").build());
        let file = cache_file_in(&dir.path().join("cache"));
        // Android: the context is built before `nativeInitPlatform`.
        let installed: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
        let resolver = {
            let installed = Arc::clone(&installed);
            cache::PathResolver::new(move || installed.lock().unwrap().clone())
        };
        let (backend, _) = backend_over(&font);
        let mut oracle = Oracle::with_cache_location(
            Box::new(backend),
            RenderCapabilities::default(),
            CacheLocation::Lazy(resolver),
        );
        assert_eq!(oracle.cache_read_time(), Duration::ZERO, "nothing read yet");
        assert!(oracle.resolve(&latin_request("a")).is_some());
        assert!(oracle.cache().attached_path().is_none(), "no path yet");
        *installed.lock().unwrap() = Some(file.clone());
        oracle.flush().expect("written once the path resolves");
        assert_eq!(oracle.cache().attached_path(), Some(file.as_path()));
        drop(oracle);

        let (backend, calls) = backend_over(&font);
        let mut next = Oracle::new(Box::new(backend), RenderCapabilities::default(), Some(file));
        assert!(next.resolve(&latin_request("a")).is_some());
        assert_eq!(calls.resolves(), 0, "answered from the persistent cache");
        assert!(
            next.cache().attached_path().is_some(),
            "read at the first miss"
        );
    }

    #[test]
    fn the_oracle_writes_behind_flushes_on_demand_and_on_drop() {
        let dir = synthetic::TempDir::new("oracle-write-behind");
        let font = dir.write("a.ttf", &synthetic::FaceSpec::new("A").chars("ab").build());
        let file = cache_file_in(dir.path());
        let open = || {
            let (backend, _) = backend_over(&font);
            let mut oracle = Oracle::new(
                Box::new(backend),
                RenderCapabilities::default(),
                Some(file.clone()),
            );
            oracle
                .cache_mut()
                .set_write_behind(2, Duration::from_secs(3600));
            oracle
        };

        let mut oracle = open();
        assert!(oracle.resolve(&latin_request("a")).is_some());
        assert!(!file.exists(), "one answer is below the threshold");
        assert!(oracle.resolve(&latin_request("b")).is_some());
        assert!(
            file.exists(),
            "the second answer wrote behind, no Drop needed"
        );
        std::fs::remove_file(&file).unwrap();

        assert!(oracle.resolve(&latin_request("ж")).is_none());
        oracle.flush().unwrap();
        assert!(file.exists(), "an explicit flush writes the dirty cache");
        std::fs::remove_file(&file).unwrap();
        oracle.flush().unwrap();
        assert!(!file.exists(), "a clean flush writes nothing");

        assert!(oracle.resolve(&latin_request("ab")).is_some());
        drop(oracle);
        assert!(file.exists(), "Drop is the final flush");
    }

    #[test]
    fn the_platform_location_is_resolved_lazily() {
        // Constructing it asks frust-paths nothing; the resolver runs at first use.
        assert!(matches!(CacheLocation::platform(), CacheLocation::Lazy(_)));
        let path = persistent_cache_path();
        if let Some(path) = path {
            assert!(path.ends_with(Path::new("frust-text").join(cache::PERSIST_FILE_NAME)));
        }
    }

    #[test]
    fn a_disabled_cache_location_never_resolves_a_path() {
        let mut oracle = Oracle::with_cache_location(
            Box::new(NoBackend),
            RenderCapabilities::default(),
            CacheLocation::Disabled,
        );
        assert!(oracle.resolve(&latin_request("a")).is_none());
        oracle.flush().unwrap();
        assert!(!oracle.cache().is_persistent());
        assert!(oracle.cache().attached_path().is_none());
    }

    #[test]
    fn the_stamped_font_dirs_are_absolute_platform_directories() {
        let dirs = font_dirs();
        assert!(dirs.iter().all(|d| d.is_absolute()), "{dirs:?}");
        if cfg!(target_os = "macos") {
            for expected in ["/System/Library/Fonts", "/Library/Fonts"] {
                assert!(dirs.contains(&PathBuf::from(expected)), "{dirs:?}");
            }
        }
        if cfg!(target_family = "wasm") {
            assert!(dirs.is_empty());
        }
    }

    #[test]
    fn face_store_reads_a_file_once() {
        let dir = synthetic::TempDir::new("store");
        let path = dir.write("a.ttf", &synthetic::FaceSpec::new("A").chars("a").build());
        let mut store = FaceStore::default();
        let src = FaceSource::path(&path, 0);
        let (first, _) = store.load(&src).expect("readable");
        let (second, _) = store.load(&src).expect("readable");
        assert_eq!(first.id(), second.id(), "the same Blob is handed out");
        assert_eq!(store.file_reads(), 1);
        let missing = FaceSource::path(dir.path().join("missing.ttf"), 0);
        assert!(store.load(&missing).is_none());
        assert!(store.load(&missing).is_none());
        assert_eq!(store.file_reads(), 2, "an unreadable path is read once too");
    }
}
