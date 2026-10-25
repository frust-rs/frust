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
//!   persistent, version-stamped copy under frust-paths' cache directory.
//! - [`selector`]: the resolve pipeline, coverage, renderability and synthesis.
//! - One backend per platform, each cfg-gated on its `mod` line below
//!   (`apple`, `android`, `fontconfig`, `dwrite`, `web`); every one is a stub
//!   answering `None` until its own card fills it in.
//! - `synthetic` (tests only): a scripted backend over in-memory faces.
//!
//! # No FFI here
//!
//! frust-text is on CODE_STANDARDS' unsafe-free list, and this module is
//! compiled with `forbid(unsafe_code)`. [`FaceStore`] reads font files with
//! `std::fs::read` (no memory mapping); [`FaceLocation::Blob`] lets a backend
//! hand over an already-built buffer instead, which is the seam a future
//! sanctioned mapping zone (decision D-6, pending) would use.

#![forbid(unsafe_code)]
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
use std::time::{Duration, Instant};

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
}

impl FaceSource {
    /// A plain path answer with no coordinates or hint.
    pub(crate) fn path(path: impl Into<PathBuf>, index: u32) -> Self {
        Self {
            location: FaceLocation::Path(path.into()),
            index,
            variations: Vec::new(),
            synthesis_hint: None,
        }
    }

    /// A plain in-memory answer with no coordinates or hint.
    pub(crate) fn blob(blob: Blob<u8>, index: u32) -> Self {
        Self {
            location: FaceLocation::Blob(blob),
            index,
            variations: Vec::new(),
            synthesis_hint: None,
        }
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
pub(crate) fn persistent_cache_path() -> Option<PathBuf> {
    let base = frust_paths::cache_dir()?;
    Some(
        base.join(frust_paths::app_stem())
            .join("frust-text")
            .join(cache::PERSIST_FILE_NAME),
    )
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
    cache_read: Duration,
}

impl std::fmt::Debug for Oracle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Oracle")
            .field("capabilities", &self.capabilities)
            .field("stats", &self.stats)
            .field("cache_read", &self.cache_read)
            .finish_non_exhaustive()
    }
}

impl Oracle {
    /// Builds an oracle over `backend`, loading the persistent cache from
    /// `persist` when given (the load's duration is [`Self::cache_read_time`]).
    pub(crate) fn new(
        backend: Box<dyn Backend>,
        capabilities: RenderCapabilities,
        persist: Option<PathBuf>,
    ) -> Self {
        let started = Instant::now();
        let mut cache = cache::FallbackCache::new(cache::MEMORY_CAPACITY);
        if let Some(path) = persist {
            let stamp = cache::VersionStamp::current(backend.os_build_id());
            cache.attach_persistent(path, stamp);
        }
        let cache_read = started.elapsed();
        Self {
            backend,
            registry: AppFontRegistry::default(),
            cache,
            store: FaceStore::default(),
            facts: HashMap::new(),
            capabilities,
            stats: OracleStats::default(),
            cache_read,
        }
    }

    /// How long [`Self::new`] spent reading the persistent cache.
    pub(crate) fn cache_read_time(&self) -> Duration {
        self.cache_read
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

    /// Writes the persistent cache now if it changed since the last write.
    pub(crate) fn persist(&mut self) -> std::io::Result<()> {
        self.cache.persist()
    }
}

impl Drop for Oracle {
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

    #[test]
    fn the_oracle_contains_no_unsafe_code() {
        let forbidden = [
            concat!("un", "safe {"),
            concat!("un", "safe fn"),
            concat!("un", "safe impl"),
            concat!("un", "safe extern"),
            concat!("un", "safe trait"),
            concat!("un", "safe("),
        ];
        for (path, text) in oracle_sources() {
            for word in forbidden {
                assert!(
                    !text.contains(word),
                    "{} contains `{word}`: frust-text is unsafe-free",
                    path.display()
                );
            }
        }
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
