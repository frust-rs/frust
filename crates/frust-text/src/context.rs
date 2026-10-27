//! [`TextContext`]: the heavyweight, `!Sync` owner of the text pipeline's font
//! and shaping state.
//!
//! Created once and threaded through the app/render loop. It owns the frust
//! font oracle ([`crate::oracle::Oracle`], built with the platform backend for
//! the target), the shaping driver's state and the shape caches.
//!
//! # Layout over the shaping driver
//!
//! [`TextContext::layout`] shapes through [`shape`] (frust's own shaping path
//! over the oracle: a width-independent [`shape::Shaped`]) and lays it out with
//! the frust line layout ([`crate::layout`]); [`TextContext::layout_bounded`]'s
//! line cap and ellipsis are native to that layout. [`TextContext::shape`]
//! returns the bare [`shape::Shaped`] through its own cache — the
//! [`crate::TextEditor`]'s path, which breaks the lines itself. The driver is
//! declared here because the context owns its state.
//!
//! # App fonts across independent contexts
//!
//! A shell owns *the* context widgets shape through at layout time, but a
//! widget that must shape outside the layout pass legitimately owns a private
//! one (`frust_widgets::textinput`). The registry's process-wide app-font
//! record (`oracle::registry`) keeps those two in agreement about registered
//! app fonts — see its docs for the layering rationale.
//!
//! # Generic-family fallback registry
//!
//! The registry's generic-fallback record is the process-wide counterpart for
//! the *generic* font-family map (the `SystemUi`/`SansSerif`/`Monospace`/
//! `Serif`/`Emoji` slots) rather than a named family — see
//! [`register_generic_fallback`] for why a host needs this seam on `wasm32`.
//!
//! The oracle's persistent font cache is placed by [`TextContext::builder`];
//! unconfigured contexts honour `FRUST_TEXT_FONT_CACHE` in debug builds.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use web_time::Instant;

use peniko::Blob;

use crate::layout::{ELLIPSIS, Ellipsis, TextLayout};
use crate::oracle::registry::{FamilySummary, RegistryMirror};
use crate::oracle::{self, CacheLocation, Oracle, RenderCapabilities};
use crate::shape_cache::{
    DEFAULT_CAPACITY, LaidOut, LayoutCache, ShapeCacheStats, ShapeKey, ShapedCache,
};
use crate::style::{GenericSlot, TextOverflow, TextStyle};

#[path = "shape/mod.rs"]
pub(crate) mod shape;

/// A font family registered via [`TextContext::register_fonts`], reported back
/// to the caller so it can resolve widget styles against the exact name(s)
/// the registry assigned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredFamily {
    /// The family name the registered face(s) resolve by (the font's own
    /// `name` table: its typographic family, else its family).
    pub name: String,
    /// How many faces (weights/styles) were registered into this family from
    /// the supplied data.
    pub face_count: usize,
}

/// Errors from [`TextContext::register_fonts`].
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum FontError {
    /// The supplied bytes contained no parseable font faces (invalid, empty,
    /// or unrecognized data).
    #[error("font data contained no parseable faces")]
    NoFacesFound,
}

/// Registers `data` (raw font bytes) as a fallback face for each generic
/// family slot in `generics`, applied to every [`TextContext`] — existing,
/// on its next [`TextContext::sync_app_fonts`], and future, seeded for free
/// by [`TextContext::new`] — through the oracle's registry, which records it
/// process-wide.
///
/// # Why this exists
///
/// On `wasm32-unknown-unknown` the platform has no system-font backend, so
/// the generic-family map is empty: [`crate::FontFamily::SystemUi`] (this
/// crate's default family) and any bare [`GenericSlot`] then resolve to
/// nothing and shape zero glyph runs. This function is the seam a host (a
/// shell) calls once at startup with a bundled face to give a generic slot
/// something to resolve to on that target; a platform whose backend already
/// populates the generic map is unaffected unless it opts in too, since the
/// registered face is only ever *added* to the slot — never a replacement.
///
/// A stack that names a concrete family
/// ([`crate::FontFamily::Named`]/[`crate::FontFamily::NamedWithGeneric`],
/// registered via [`TextContext::register_fonts`]) still resolves before a
/// generic fallback, since a named lookup is always tried first.
///
/// No-op (records nothing) when `generics` is empty. Bytes with no parseable
/// face register zero families when eventually applied, so this is a
/// harmless no-op then too — the same "no error surface" contract
/// [`TextContext::sync_app_fonts`] already follows for app fonts.
pub fn register_generic_fallback(data: Vec<u8>, generics: &[GenericSlot]) {
    oracle::registry::record_generic_fallback(data, generics);
}

/// The registry's second collection: there is none — the oracle is the only
/// font collection a context has.
struct NoMirror;

impl RegistryMirror for NoMirror {
    fn app_font(&mut self, _blob: &Blob<u8>) -> Vec<FamilySummary> {
        Vec::new()
    }

    fn generic_fallback(&mut self, _blob: &Blob<u8>, _generics: &[GenericSlot]) -> bool {
        false
    }
}

/// Owns the font oracle, the shaping driver and the shape caches.
///
/// This is deliberately not `Clone`/`Sync`: it is expensive per-instance state
/// meant to be constructed once and borrowed mutably for each layout pass.
/// Glyph runs carry [`peniko::Brush`], the same paint vocabulary as
/// [`frust_scene`].
pub struct TextContext {
    /// Width-independent shape cache: a bounded LRU of shaped paragraphs and
    /// their line tables keyed by (text, style), so a width change re-runs
    /// the line layout only and repeated content shapes once. See
    /// [`crate::shape_cache`].
    shape_cache: LayoutCache,
    /// The shaping driver's analyzer/shaper state ([`shape::ShapeContext`]).
    shape_cx: shape::ShapeContext,
    /// The shaping driver's cache: one [`shape::Shaped`] per (text, style),
    /// the [`crate::TextEditor`]'s shaping.
    shaped_cache: ShapedCache,
    /// The frust font oracle: platform backend, fallback cache, and this
    /// context's app-font registry (with its watermarks into the process-wide
    /// app-font and generic-fallback records).
    oracle: Oracle,
}

/// The number of lines a layout actually puts content on, discounting the
/// empty line the line layout opens after a text-terminating `'\n'`.
///
/// A hard break's newline belongs to the line it terminates, and a further,
/// zero-width line follows for the caret position after it (as parley does)
/// — so `"Hello\n"` reports two lines while painting one. Counting that phantom
/// line as overflow is what made `text("Hello\n").max_lines(1)` render
/// "Hello…": a false ellipsis on text that fits. Only a *trailing* empty line
/// at `text_len` is discounted; an empty line in the middle of the text
/// (`"a\n\nb"`) is a real blank line the caller asked for.
fn visible_line_count(layout: &TextLayout, text_len: usize) -> usize {
    let count = layout.line_count();
    if count <= 1 {
        return count;
    }
    match layout.line_info(count - 1) {
        Some(last) if last.range.is_empty() && last.range.end >= text_len => count - 1,
        _ => count,
    }
}

/// Where [`TextContext::new_with_timings`] spent its time (startup evidence).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NewTimings {
    /// The font system's start-up: the oracle's construction (its persistent
    /// fallback cache is read later, at the first miss).
    pub font_system: Duration,
    /// [`TextContext::sync_app_fonts`] — seeding the app fonts and generic fallbacks.
    pub app_sync: Duration,
}

/// The debug-build override for a context nobody configured: `off`, or an
/// absolute directory to keep the font cache in.
const FONT_CACHE_ENV: &str = "FRUST_TEXT_FONT_CACHE";

/// Where a context's persistent font cache goes, as configured.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum FontCacheChoice {
    /// The platform cache directory (or [`FONT_CACHE_ENV`]'s, in a debug build).
    #[default]
    Default,
    Disabled,
    Dir(PathBuf),
}

/// [`FONT_CACHE_ENV`]'s value as a choice; `None` when unset or unusable.
fn env_font_cache(value: Option<&OsStr>) -> Option<FontCacheChoice> {
    let value = value?;
    if value == "off" {
        return Some(FontCacheChoice::Disabled);
    }
    let dir = Path::new(value);
    if dir.is_absolute() {
        Some(FontCacheChoice::Dir(dir.to_path_buf()))
    } else {
        log::debug!("frust-text: {FONT_CACHE_ENV} ignored: not `off` or an absolute path");
        None
    }
}

/// The oracle's cache location for `choice`; only [`FontCacheChoice::Default`]
/// consults the environment, and only in a debug build.
fn cache_location(choice: FontCacheChoice) -> CacheLocation {
    let choice = match choice {
        FontCacheChoice::Default if cfg!(debug_assertions) => {
            env_font_cache(std::env::var_os(FONT_CACHE_ENV).as_deref()).unwrap_or_default()
        }
        other => other,
    };
    match choice {
        FontCacheChoice::Disabled => CacheLocation::Disabled,
        FontCacheChoice::Dir(dir) => CacheLocation::File(oracle::cache_file_in(&dir)),
        FontCacheChoice::Default => default_cache_location(),
    }
}

/// An unconfigured context's location. This crate's own unit tests get a
/// per-process scratch directory, so none reaches the user's cache; the web
/// has no cache directory, so nothing is attempted there.
fn default_cache_location() -> CacheLocation {
    #[cfg(test)]
    let location = CacheLocation::File(oracle::cache_file_in(
        &std::env::temp_dir().join(format!("frust-text-test-font-cache-{}", std::process::id())),
    ));
    #[cfg(not(test))]
    let location = if cfg!(target_family = "wasm") {
        CacheLocation::Disabled
    } else {
        CacheLocation::platform()
    };
    location
}

/// Configures a [`TextContext`] before building it; see [`TextContext::builder`].
#[derive(Clone, Debug, Default)]
#[must_use]
pub struct TextContextBuilder {
    capabilities: RenderCapabilities,
    font_cache: FontCacheChoice,
}

impl TextContextBuilder {
    /// The glyph formats the engine draws (default: today's engine).
    pub fn render_capabilities(mut self, capabilities: RenderCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Keeps the font oracle's fallback cache in memory: nothing is read from
    /// or written to disk. Tests use this.
    pub fn without_font_cache(mut self) -> Self {
        self.font_cache = FontCacheChoice::Disabled;
        self
    }

    /// Keeps the persistent font cache in `dir` instead of the platform cache
    /// directory (created at the first write).
    pub fn font_cache_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.font_cache = FontCacheChoice::Dir(dir.into());
        self
    }

    /// Builds the context; see [`TextContext::new`].
    pub fn build(self) -> TextContext {
        self.build_with_timings().0
    }

    /// [`Self::build`] that also reports where its time went.
    pub fn build_with_timings(self) -> (TextContext, NewTimings) {
        TextContext::build(self.capabilities, self.font_cache)
    }
}

impl TextContext {
    /// A builder for a context with non-default capabilities or font-cache
    /// placement; [`Self::new`] is `TextContext::builder().build()`.
    pub fn builder() -> TextContextBuilder {
        TextContextBuilder::default()
    }

    /// Builds a context with system fonts available, seeded with every app font
    /// registered so far.
    ///
    /// The oracle's platform backend answers from the system's fonts (Core
    /// Text on Apple targets); no manual font registration is required for the
    /// default [`crate::FontFamily::SystemUi`] to resolve.
    ///
    /// The seeding step ([`Self::sync_app_fonts`]) is what makes a *private*
    /// context (a `TextInput`'s) shape with the same app fonts the shell-owned
    /// context does, however late it is constructed — see the oracle
    /// registry's app-font record. It also applies every pending
    /// [`register_generic_fallback`] entry, so a generic-family fallback a
    /// shell registered before this call is already live in the returned
    /// context.
    ///
    /// The context's font oracle is built with this target's platform backend
    /// and the default [`RenderCapabilities`]; [`Self::with_capabilities`]
    /// takes others.
    pub fn new() -> Self {
        Self::new_with_timings().0
    }

    /// [`Self::new`] for an engine that draws a different set of glyph formats
    /// than today's default (see [`RenderCapabilities`]): the font oracle
    /// refuses faces the engine cannot draw rather than handing them to it.
    pub fn with_capabilities(capabilities: RenderCapabilities) -> Self {
        Self::builder().render_capabilities(capabilities).build()
    }

    /// [`Self::new`] that also reports where its time went. `new` delegates
    /// here, so the work and its order are identical.
    pub fn new_with_timings() -> (Self, NewTimings) {
        Self::builder().build_with_timings()
    }

    /// Writes the font oracle's persistent fallback cache now if it holds
    /// unwritten answers.
    ///
    /// The cache writes behind on its own (after a burst of new answers, or
    /// once idle) and on drop, but a mobile app can be killed in the
    /// background with neither: a shell calls this from its pause/background
    /// hook. A no-op for a context built without a font cache.
    pub fn flush_font_cache(&mut self) -> std::io::Result<()> {
        self.oracle.flush()
    }

    /// What frust-engine can draw, as this context's font oracle assumes.
    pub fn render_capabilities(&self) -> RenderCapabilities {
        self.oracle.capabilities()
    }

    fn build(capabilities: RenderCapabilities, font_cache: FontCacheChoice) -> (Self, NewTimings) {
        let oracle_started = Instant::now();
        let oracle = Oracle::with_cache_location(
            oracle::platform_backend(),
            capabilities,
            cache_location(font_cache),
        );
        let font_system = oracle_started.elapsed();
        log::trace!("frust-text: font system {font_system:?}");
        let mut cx = Self {
            shape_cache: LayoutCache::new(DEFAULT_CAPACITY),
            shape_cx: shape::ShapeContext::default(),
            shaped_cache: ShapedCache::new(DEFAULT_CAPACITY),
            oracle,
        };
        let sync_started = Instant::now();
        cx.sync_app_fonts();
        let app_sync = sync_started.elapsed();
        (
            cx,
            NewTimings {
                font_system,
                app_sync,
            },
        )
    }

    /// Lays out `text` with `style`, wrapping to `max_width` when supplied.
    ///
    /// `max_width` is in the same logical-pixel units as `style.size` (the
    /// layout scale is fixed at `1.0` here — physical-pixel scaling is applied
    /// downstream via the scene transform). Passing `None` produces a single
    /// unwrapped line per hard break in `text`. An empty `text` yields a layout
    /// with no glyph runs, zero width and one line of the style's height.
    ///
    /// `style.align` positions every line within the layout's width (the
    /// widest line's when `max_width` is unbounded). A word wider than
    /// `max_width` overflows its line rather than breaking (parley's default).
    ///
    /// Shaping goes through the shape cache: the same (text, style) shapes
    /// once, and a width change re-runs only the line layout on the cached
    /// shaping ([`crate::shape_cache::LayoutCache::get`]).
    pub fn layout(&mut self, text: &str, style: &TextStyle, max_width: Option<f32>) -> TextLayout {
        let key = ShapeKey::new(text, style);
        if let Some(layout) = self.shape_cache.get(&key, max_width) {
            return layout;
        }
        // An empty text is sized by a space it never draws (parley's rule), so
        // an empty label still occupies one line of its style's height.
        let shaped_text = if text.is_empty() { " " } else { text };
        let shaped = Arc::new(self.shape_cx.shape(&mut self.oracle, shaped_text, style));
        let laid_out = LaidOut::new(text, shaped, style, max_width);
        let layout = laid_out.layout();
        self.shape_cache.insert(key, laid_out);
        layout
    }

    /// Lays out `text` exactly like [`Self::layout`], then caps it to
    /// `max_lines` (when `Some`), applying `overflow` to whatever is cut.
    ///
    /// `max_lines = None` delegates straight to [`Self::layout`]. A bounded
    /// call lays out the full text once (shape-cache backed) and, when it
    /// overflows, re-runs the line layout on the same shaping with the line
    /// cap: nothing is reshaped, and the earlier lines come out exactly as in
    /// the full layout (breaking is greedy). An ellipsis is shaped once per
    /// style, through the same cache.
    ///
    /// # Algorithm
    ///
    /// A layout overflows `max_lines` in one of two ways, both checked
    /// against the full layout:
    /// - **extra lines**: line-breaking produced more than `max_lines`
    ///   *content* lines (wrapping, or `max_lines` hard `\n` breaks in the
    ///   source) — see [`visible_line_count`] for why the raw line count is
    ///   not that number when the text ends in `\n`.
    /// - **an overrun**: exactly `max_lines` lines came out, but the last
    ///   visible one is itself wider than `max_width` — a run with no break
    ///   opportunity (one long unspaced word) overflows rather than breaking.
    ///
    /// Neither condition holds → the text already fits (this is also why an
    /// exact-fit line, width `== max_width`, never gets truncated: the
    /// comparison is a strict `>`), and the full layout is returned as-is,
    /// *unless* it still carries the empty line opened after a terminal
    /// `'\n'` — then that line is dropped, so a fitting layout's reported line
    /// count and height always match what paints.
    ///
    /// On overflow, [`TextOverflow::Clip`] only ever drops whole trailing
    /// lines; an overrun-only case (no extra lines to drop) is left
    /// untouched, since Clip never character-trims. [`TextOverflow::Ellipsis`]
    /// does the same line drop, then refills the last visible line from the
    /// text after it (past its soft-wrap point, up to the next mandatory
    /// break) and keeps the longest grapheme-boundary prefix, whitespace at
    /// the cut dropped, that fits `max_width` with [`ELLIPSIS`] (see
    /// `crate::layout`'s truncation); with no `max_width` the whole line is
    /// kept and marked.
    pub fn layout_bounded(
        &mut self,
        text: &str,
        style: &TextStyle,
        max_width: Option<f32>,
        max_lines: Option<usize>,
        overflow: TextOverflow,
    ) -> TextLayout {
        let Some(max_lines) = max_lines else {
            return self.layout(text, style, max_width);
        };
        if max_lines == 0 {
            // No visible lines at all — never index `max_lines - 1` below.
            return self.layout("", style, max_width);
        }

        let full = self.layout(text, style, max_width);
        let visible_lines = visible_line_count(&full, text.len());
        let has_extra_lines = visible_lines > max_lines;
        let Some(last_line) = full.line_info(max_lines - 1) else {
            // Fewer than `max_lines` lines exist at all: nothing overflowed.
            return full;
        };
        let width_overflows = matches!(max_width, Some(w) if last_line.width > w);

        if !has_extra_lines && !width_overflows {
            if full.line_count() == visible_lines {
                return full;
            }
            // The text fits, but the full layout still counts the empty line
            // after a terminal `'\n'` in its line count and height: cap it at
            // the visible lines.
            return full.truncated(visible_lines, None);
        }

        match overflow {
            TextOverflow::Clip if !has_extra_lines => full,
            TextOverflow::Clip => full.truncated(max_lines, None),
            TextOverflow::Ellipsis => {
                let mut glyph = [0u8; 4];
                let ellipsis = Ellipsis {
                    shaped: Arc::clone(
                        self.layout(ELLIPSIS.encode_utf8(&mut glyph), style, None)
                            .shaped(),
                    ),
                    style_index: 0,
                };
                full.truncated(max_lines, Some(ellipsis))
            }
        }
    }

    /// The shape cache's instrumentation counters (shapes performed,
    /// line-break-only relayouts, full hits, evictions).
    ///
    /// The observable hook the shape-cache tests assert against, and a
    /// perf signal otherwise. Plain scalar data — no `parley`/`vello`/`wgpu`
    /// type leaks through (scene-layer purity).
    pub fn shape_cache_stats(&self) -> ShapeCacheStats {
        self.shape_cache.stats()
    }

    /// Shapes `text` with `style` through the frust shaping driver (fonts from
    /// this context's oracle), width-independently and cached: a repeat of the
    /// same (text, style) is served without analysis or shaping. The
    /// [`crate::TextEditor`] shapes through here on every edit.
    pub(crate) fn shape(&mut self, text: &str, style: &TextStyle) -> Arc<shape::Shaped> {
        let key = ShapeKey::new(text, style);
        if let Some(shaped) = self.shaped_cache.get_shaped(&key) {
            return shaped;
        }
        let shaped = Arc::new(self.shape_cx.shape(&mut self.oracle, text, style));
        self.shaped_cache.insert_shaped(key, Arc::clone(&shaped));
        shaped
    }

    /// The shaping driver's cache counters ([`Self::shape`]).
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "the shaping cache's counters are read by tests only"
        )
    )]
    pub(crate) fn shaped_cache_stats(&self) -> ShapeCacheStats {
        self.shaped_cache.stats()
    }

    /// Registers font faces from raw bytes (TTF/OTF, or a TTC/OTC
    /// collection) so they resolve by family name via
    /// [`crate::FontFamily::named`]/[`crate::FontFamily::stack`].
    ///
    /// Registers into the font oracle's app-font registry (parsed with
    /// read-fonts). A registered family **shadows** a same-named system
    /// family (app faces are consulted before the system, and a provided
    /// family is never asked of the system), so bundling a family already
    /// present on the platform (e.g. "Roboto") deterministically wins over the
    /// platform's own copy.
    ///
    /// Always clears the shape cache on success — a same-named registered
    /// family changes shaping without changing the cache key, so any layout
    /// shaped before this call could otherwise be served stale. Returns
    /// [`FontError::NoFacesFound`] (no panic) for invalid/empty data, an
    /// empty byte slice, or bytes with no parseable face.
    ///
    /// **Caller-visible relayout contract**: registering fonts after a shell
    /// has already laid out text does not retroactively re-shape anything
    /// still cached elsewhere (e.g. a widget's own retained layout) — a shell
    /// calling this must force `ChangeFlags::LAYOUT | PAINT` the same way a
    /// theme swap does (see `docs/ARCHITECTURE.md`'s Theme delivery), so the
    /// next layout pass re-shapes against the newly registered faces. This
    /// crate only owns the shape-cache half of that contract.
    ///
    /// An accepted payload is also recorded process-wide (the registry's
    /// app-font record), so every `TextContext` constructed later — and every
    /// existing one that calls [`Self::sync_app_fonts`] — resolves the same
    /// family. That is what carries an app font from a shell's drain into a
    /// widget-owned private context.
    pub fn register_fonts(&mut self, data: Vec<u8>) -> Result<Vec<RegisteredFamily>, FontError> {
        let blob = Blob::from(data);
        let families = self
            .oracle
            .registry_mut()
            .register(blob, &mut NoMirror)
            .map_err(|_| FontError::NoFacesFound)?
            .into_iter()
            .map(|family| RegisteredFamily {
                name: family.name,
                face_count: family.face_count,
            })
            .collect();
        self.clear_shape_cache();
        Ok(families)
    }

    /// Registers every app font this context is missing and applies every
    /// [`register_generic_fallback`] entry it is missing (the oracle
    /// registry's process-wide records), returning whether either actually
    /// changed this context's resolution.
    ///
    /// Cheap when there is nothing to do — a lock and a length compare per
    /// record, no allocation and no font parsing — so a widget owning a
    /// private context can call it once per layout pass to pick up a font (or
    /// fallback) registered *after* the context was built (the shells'
    /// per-frame late drain).
    ///
    /// A `true` return carries the same caller-visible relayout contract as
    /// [`Self::register_fonts`]: this context's shape cache is cleared, but a
    /// layout retained *outside* it (a [`crate::TextEditor`]'s, until its next
    /// edit) must be re-shaped by its owner.
    pub fn sync_app_fonts(&mut self) -> bool {
        let applied = self.oracle.registry_mut().sync(&mut NoMirror);
        if applied {
            self.clear_shape_cache();
        }
        applied
    }

    /// Drops every cached shaped layout, forcing the next [`Self::layout`]
    /// call for any (text, style) pair to re-shape from scratch.
    ///
    /// Exposed (not just an internal helper) for the shell late-drain path
    /// (a shell registering fonts after startup, once widgets may already
    /// hold cached layouts elsewhere) — [`Self::register_fonts`] already
    /// calls this internally on success, so a caller registering fonts
    /// doesn't need to call it separately.
    pub fn clear_shape_cache(&mut self) {
        self.shape_cache = LayoutCache::new(DEFAULT_CAPACITY);
        self.shaped_cache = ShapedCache::new(DEFAULT_CAPACITY);
    }
}

impl Default for TextContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape_cache::DEFAULT_CAPACITY;
    use crate::style::TextAlign;
    use peniko::Color;
    use std::sync::Mutex;

    /// Every glyph of `runs` as (id, x, y), in emission order.
    fn glyphs_of(runs: &[frust_scene::GlyphRun]) -> Vec<(u32, f32, f32)> {
        runs.iter()
            .flat_map(|r| r.glyphs.iter().map(|g| (g.id, g.x, g.y)))
            .collect()
    }

    /// A context whose font cache stays in memory: the builder opt-out every
    /// test here uses (the scratch-dir default is only the safety net).
    fn test_cx() -> TextContext {
        TextContext::builder().without_font_cache().build()
    }

    fn oracle_request(chars: &str) -> crate::oracle::FallbackRequest {
        crate::oracle::FallbackRequest::new(
            chars,
            crate::oracle::ScriptTag(*b"Latn"),
            &crate::FontFamily::SystemUi,
            RenderCapabilities::default(),
        )
    }

    #[test]
    fn the_builder_opt_out_keeps_the_font_cache_in_memory() {
        let mut cx = test_cx();
        assert!(!cx.oracle.cache().is_persistent());
        let _ = cx.oracle.resolve(&oracle_request("a"));
        cx.flush_font_cache().expect("nothing to write");
        assert!(cx.oracle.cache().attached_path().is_none());
    }

    #[test]
    fn a_font_cache_dir_receives_the_cache_at_the_first_flush() {
        let dir = oracle::synthetic::TempDir::new("context-cache-dir");
        let cache_dir = dir.path().join("fonts");
        let mut cx = TextContext::builder()
            .font_cache_dir(&cache_dir)
            .render_capabilities(RenderCapabilities::default())
            .build();
        assert!(cx.oracle.cache().is_persistent());
        assert!(
            cx.oracle.cache().attached_path().is_none(),
            "resolved lazily"
        );
        // A face on a backend that knows "a", a cached miss on one that doesn't.
        let _ = cx.oracle.resolve(&oracle_request("a"));
        let file = oracle::cache_file_in(&cache_dir);
        assert_eq!(cx.oracle.cache().attached_path(), Some(file.as_path()));
        cx.flush_font_cache().expect("written");
        assert!(file.exists());
        assert!(!cx.oracle.cache().is_dirty());
    }

    #[test]
    fn two_contexts_over_one_cache_dir_share_their_answers() {
        let dir = oracle::synthetic::TempDir::new("context-shared-cache");
        let cache_dir = dir.path().join("fonts");
        let build = || TextContext::builder().font_cache_dir(&cache_dir).build();
        // Two contexts, as a shell's and a text input's private one.
        let (mut one, mut two) = (build(), build());
        let a = oracle_request("a");
        let b = oracle_request("\u{0436}");

        let _ = one.oracle.resolve(&a);
        assert_eq!(one.oracle.stats().backend_calls, 1);
        one.flush_font_cache().expect("written");
        let _ = two.oracle.resolve(&a);
        assert_eq!(
            two.oracle.stats().backend_calls,
            0,
            "the second context is answered by the first one's write-back"
        );
        assert_eq!(two.oracle.stats().cache_hits, 1);

        let _ = two.oracle.resolve(&b);
        assert_eq!(two.oracle.stats().backend_calls, 1);
        two.flush_font_cache().expect("written");
        let _ = one.oracle.resolve(&b);
        assert_eq!(
            one.oracle.stats().backend_calls,
            1,
            "and the first by the second's"
        );

        // The file holds both answers: an independent reader finds them.
        let file = oracle::cache_file_in(&cache_dir);
        let stamp = oracle::cache::VersionStamp::current(oracle::platform_backend().os_build_id());
        let mut disk = oracle::cache::FallbackCache::new(16);
        assert_eq!(
            disk.attach_persistent(file, stamp),
            oracle::cache::LoadOutcome::Loaded(2)
        );
        for request in [&a, &b] {
            let key = oracle::cache::CacheKey::from_request(request);
            assert!(disk.get(&key).is_some(), "{:?} is on disk", request.chars);
        }
    }

    #[test]
    fn the_final_flush_writes_answers_another_context_recorded() {
        let dir = oracle::synthetic::TempDir::new("context-flush-any");
        let cache_dir = dir.path().join("fonts");
        let build = || TextContext::builder().font_cache_dir(&cache_dir).build();
        let (mut a, mut b) = (build(), build());
        let (x, y) = (oracle_request("a"), oracle_request("\u{0436}"));
        let file = oracle::cache_file_in(&cache_dir);

        // B records and writes one answer; A attaches to the file by reading it.
        let _ = b.oracle.resolve(&x);
        b.flush_font_cache().expect("written");
        let _ = a.oracle.resolve(&x);
        assert_eq!(a.oracle.stats().backend_calls, 0, "A recorded nothing");
        assert!(!a.oracle.cache().is_dirty());

        // Nothing behind: A's flush writes nothing.
        let before = std::fs::metadata(&file)
            .and_then(|m| m.modified())
            .expect("mtime");
        a.flush_font_cache().expect("no-op");
        let after = std::fs::metadata(&file)
            .and_then(|m| m.modified())
            .expect("mtime");
        assert_eq!(before, after, "a flush with nothing behind writes nothing");

        // B records a second answer and never flushes; A's flush writes it.
        let _ = b.oracle.resolve(&y);
        assert!(!a.oracle.cache().is_dirty(), "A itself recorded nothing");
        a.flush_font_cache().expect("written");

        let stamp = oracle::cache::VersionStamp::current(oracle::platform_backend().os_build_id());
        let mut disk = oracle::cache::FallbackCache::new(16);
        assert_eq!(
            disk.attach_persistent(file, stamp),
            oracle::cache::LoadOutcome::Loaded(2)
        );
        for request in [&x, &y] {
            let key = oracle::cache::CacheKey::from_request(request);
            assert!(disk.get(&key).is_some(), "{:?} is on disk", request.chars);
        }
    }

    #[test]
    fn the_environment_override_parses_off_and_absolute_dirs_only() {
        assert_eq!(env_font_cache(None), None);
        assert_eq!(
            env_font_cache(Some(OsStr::new("off"))),
            Some(FontCacheChoice::Disabled)
        );
        let abs = std::env::temp_dir().join("frust-font-cache");
        assert_eq!(
            env_font_cache(Some(abs.as_os_str())),
            Some(FontCacheChoice::Dir(abs.clone()))
        );
        assert_eq!(env_font_cache(Some(OsStr::new("relative/dir"))), None);
        assert_eq!(env_font_cache(Some(OsStr::new(""))), None);
    }

    #[test]
    fn explicit_choices_win_and_the_test_default_is_a_scratch_dir() {
        assert!(matches!(
            cache_location(FontCacheChoice::Disabled),
            CacheLocation::Disabled
        ));
        let dir = std::env::temp_dir().join("chosen");
        let CacheLocation::File(file) = cache_location(FontCacheChoice::Dir(dir.clone())) else {
            panic!("an explicit directory is used as given");
        };
        assert_eq!(file, oracle::cache_file_in(&dir));
        // Unless FRUST_TEXT_FONT_CACHE says otherwise, this crate's tests never
        // resolve the user's cache directory.
        if std::env::var_os(FONT_CACHE_ENV).is_none() {
            let CacheLocation::File(file) = default_cache_location() else {
                panic!("tests default to a scratch file");
            };
            assert!(file.starts_with(std::env::temp_dir()), "{}", file.display());
            // Never the platform cache file's directory. (A prefix test against
            // `frust_paths::cache_dir()` is wrong on Windows, where the temp
            // dir itself lives under the user's local app-data dir.)
            if let Some(platform) = oracle::persistent_cache_path() {
                assert_ne!(file.parent(), platform.parent(), "{}", file.display());
            }
        }
    }

    #[test]
    fn new_with_timings_builds_the_same_context_as_new() {
        let (mut timed, timings) = TextContext::new_with_timings();
        let mut plain = TextContext::new();
        let style = style(16.0);
        let a = timed.layout("Hello", &style, None);
        let b = plain.layout("Hello", &style, None);
        assert_eq!(a.size(), b.size());
        // Both phases ran: the font-system scan is never free.
        assert!(timings.font_system > Duration::ZERO);
        assert!(timings.font_system + timings.app_sync < Duration::from_secs(60));
    }

    /// A body-text style at `size`.
    fn style(size: f32) -> TextStyle {
        TextStyle::new(size, Color::BLACK)
    }

    /// The registered-font fixture, shared with `tests/register_fonts.rs`.
    const TUFFY: &[u8] = include_bytes!("../tests/fonts/Tuffy-Subset.ttf");

    /// The raw bytes of the face a shaped run of `text` actually resolved to.
    fn shaped_font_bytes(cx: &mut TextContext, text: &str, sty: &TextStyle) -> Vec<u8> {
        let layout = cx.layout(text, sty, None);
        let runs = layout.to_scene_runs(kurbo::Point::ORIGIN);
        runs.first()
            .expect("expected at least one glyph run")
            .font
            .font()
            .data
            .as_ref()
            .to_vec()
    }

    #[test]
    fn an_app_font_reaches_contexts_built_later_and_older_ones_on_sync() {
        // The seam under test: `register_fonts` is what a shell's
        // font drain calls, and a widget-owned context (a `TextInput`'s) is
        // built from `new()` long after that drain. Both legs below are
        // monotone — the record is append-only and never reset, so nothing
        // here depends on the order tests run in.
        let s = TextStyle {
            family: crate::FontFamily::named("Tuffy"),
            ..style(24.0)
        };

        // A context that exists *before* the registration.
        let mut older = test_cx();

        // The shell's drain.
        let mut shell = test_cx();
        shell
            .register_fonts(TUFFY.to_vec())
            .expect("valid TTF bytes must register");

        // A context built after it needs no explicit sync.
        let mut later = test_cx();
        assert!(
            !later.sync_app_fonts(),
            "a freshly built context is already current with the app-font record"
        );
        assert!(
            shaped_font_bytes(&mut later, "0123456789", &s) == TUFFY,
            "a context built after the registration must shape with the app font"
        );

        // An older one picks it up on the explicit sync a per-frame caller makes.
        older.sync_app_fonts();
        assert!(
            shaped_font_bytes(&mut older, "0123456789", &s) == TUFFY,
            "an already-built context must pick the app font up on sync_app_fonts"
        );
    }

    /// The generic-fallback registry, exercised against an oracle whose
    /// backend has no fonts at all — the empty-generic-family-map shape
    /// `wasm32` has (see [`register_generic_fallback`]'s docs) — since a real
    /// host's system backend already resolves `SystemUi`, so it could never
    /// reproduce the defect this seam fixes.
    #[test]
    fn generic_fallback_maps_requested_slots_and_is_idempotent() {
        use crate::oracle::registry::PendingGenericFallback;

        // Entries built locally, driven straight through the registry's
        // `apply_generic_entries` rather than the real
        // `register_generic_fallback` seam — that seam writes to the
        // process-wide generic-fallback record, which is never drained, so a
        // real call here would keep applying this face to every other test's
        // (real-system-font) `TextContext` built later in this same test
        // binary process and could change which face they resolve.
        let entries = vec![PendingGenericFallback {
            blob: Blob::from(TUFFY.to_vec()),
            generics: vec![GenericSlot::SystemUi, GenericSlot::SansSerif],
        }];

        let mut cx = test_cx();
        cx.oracle = Oracle::new(
            Box::new(oracle::synthetic::SyntheticBackend::new("systemless")),
            RenderCapabilities::default(),
            None,
        );
        let applied = cx
            .oracle
            .registry_mut()
            .apply_generic_entries(&entries, &mut NoMirror);
        assert!(
            applied,
            "registering a parseable face must report at least one applied family"
        );

        // The synthetic backend answers nothing, so the app generic fallback
        // is the answer — for the requested slots only.
        let face = cx
            .oracle
            .default_face(GenericSlot::SystemUi)
            .expect("the oracle resolves SystemUi to the registered fallback");
        assert_eq!(face.blob.data(), TUFFY);
        assert!(
            cx.oracle.default_face(GenericSlot::Monospace).is_none(),
            "Monospace must stay unmapped — only SystemUi/SansSerif were requested"
        );

        // The defect under test: SystemUi-styled text on an otherwise-empty
        // system collection must still shape into glyph runs.
        let layout = cx.layout("0123", &style(20.0), None);
        let runs = layout.to_scene_runs(kurbo::Point::ORIGIN);
        assert!(
            !runs.is_empty(),
            "SystemUi text must shape with the registered fallback face even \
             when the system has no fonts"
        );

        // Idempotence: a second apply with nothing new pending must not
        // re-append the same face to the slot.
        let applied_again = cx
            .oracle
            .registry_mut()
            .apply_generic_entries(&entries, &mut NoMirror);
        assert!(
            !applied_again,
            "nothing new is pending — a repeat apply must be a no-op"
        );
        assert_eq!(
            cx.oracle
                .registry_mut()
                .generic_faces(GenericSlot::SystemUi)
                .count(),
            1,
            "a repeat apply must not double-register the fallback face"
        );
    }

    #[test]
    fn register_generic_fallback_is_a_noop_with_no_generics() {
        // Deliberately the *empty-generics* case only: `register_generic_fallback`
        // writes to the process-wide, never-drained generic-fallback record,
        // so a call naming a real slot here would keep applying for the rest of
        // this test binary's run — see the isolated-collection test above,
        // which exercises the real registration/append behavior without that
        // leak.
        let before = oracle::registry::generic_fallback_count();
        register_generic_fallback(vec![1, 2, 3], &[]);
        assert_eq!(
            before,
            oracle::registry::generic_fallback_count(),
            "no generic slots requested — nothing should be queued"
        );
    }

    #[test]
    fn a_registered_app_font_reaches_the_oracle_too() {
        let mut cx = test_cx();
        let families = cx
            .register_fonts(TUFFY.to_vec())
            .expect("valid TTF bytes must register");
        assert_eq!(families[0].name, "Tuffy");
        let request = crate::oracle::FallbackRequest::new(
            "0123",
            crate::oracle::ScriptTag(*b"Zyyy"),
            &crate::FontFamily::named("Tuffy"),
            cx.render_capabilities(),
        );
        let face = cx
            .oracle
            .resolve(&request)
            .expect("the oracle resolves the app family");
        assert_eq!(face.blob.data(), TUFFY);
        assert_eq!(cx.oracle.stats().app_hits, 1);
    }

    #[test]
    fn with_capabilities_reaches_the_oracle() {
        let caps = RenderCapabilities {
            bitmap: false,
            ..RenderCapabilities::default()
        };
        let cx = TextContext::with_capabilities(caps);
        assert_eq!(cx.render_capabilities(), caps);
        assert_eq!(
            test_cx().render_capabilities(),
            RenderCapabilities::default()
        );
    }

    #[test]
    fn same_text_two_widths_shapes_once() {
        // The core claim: shaping is width-independent. Laying out
        // the same text+style at two different widths shapes exactly once; the
        // width change re-runs line-breaking only, and a repeated width is a
        // full reuse.
        let mut cx = test_cx();
        let text = "Hello from Frust, the pure Rust mobile UI toolkit";
        let s = style(16.0);

        let _ = cx.layout(text, &s, Some(200.0)); // miss → shape
        let _ = cx.layout(text, &s, Some(80.0)); // width change → line-break only
        let _ = cx.layout(text, &s, Some(80.0)); // same width → full hit

        let stats = cx.shape_cache_stats();
        assert_eq!(stats.shapes, 1, "shaping must run exactly once");
        assert_eq!(stats.line_breaks, 1, "the differing width re-breaks once");
        assert_eq!(stats.hits, 1, "the repeated width is a full reuse");
    }

    /// A context holding the bundled Latin subset as an app font, and its family.
    fn noto_cx() -> (TextContext, TextStyle) {
        let mut cx = test_cx();
        let families = cx
            .register_fonts(include_bytes!("../../../testing/fonts/NotoSans-Subset.ttf").to_vec())
            .expect("the subset registers");
        let s = TextStyle {
            family: crate::FontFamily::named(families[0].name.clone()),
            ..style(18.0)
        };
        (cx, s)
    }

    #[test]
    fn the_shaped_cache_hit_skips_analysis_and_shaping() {
        let (mut cx, s) = noto_cx();
        let first = cx.shape("Hello, hello", &s);
        let (analyses, passes) = (cx.shape_cx.analyses(), cx.shape_cx.passes());
        assert_eq!((analyses, passes), (1, 1));
        let again = cx.shape("Hello, hello", &s);
        assert!(Arc::ptr_eq(&first, &again), "the cached value is shared");
        assert_eq!(cx.shape_cx.analyses(), analyses, "a hit runs no analysis");
        assert_eq!(cx.shape_cx.passes(), passes, "a hit runs no shaper");
        let stats = cx.shaped_cache_stats();
        assert_eq!((stats.shapes, stats.hits), (1, 1));
        assert_eq!(
            cx.shape_cache_stats(),
            ShapeCacheStats::default(),
            "the layout cache is separate"
        );
        cx.clear_shape_cache();
        let _ = cx.shape("Hello, hello", &s);
        assert_eq!(cx.shape_cx.passes(), passes + 1, "cleared → shaped again");
    }

    #[test]
    fn a_width_change_relays_out_without_reshaping() {
        let (mut cx, s) = noto_cx();
        let text = "Hello hello, Hello hello Hello, hello";
        let first = cx.layout(text, &s, Some(200.0));
        let (analyses, passes) = (cx.shape_cx.analyses(), cx.shape_cx.passes());
        for width in [Some(80.0), Some(120.0), None, Some(200.0)] {
            let again = cx.layout(text, &s, width);
            assert!(Arc::ptr_eq(again.shaped(), first.shaped()), "one shaping");
        }
        assert_eq!(
            cx.shape_cx.analyses(),
            analyses,
            "no analysis on a width change"
        );
        assert_eq!(cx.shape_cx.passes(), passes, "no shaping on a width change");
        let stats = cx.shape_cache_stats();
        assert_eq!((stats.shapes, stats.line_breaks, stats.hits), (1, 4, 0));
    }

    #[test]
    fn style_variations_widen_the_layout() {
        let mut cx = test_cx();
        let families = cx
            .register_fonts(
                include_bytes!("../../../plugins/beui/fonts/geist/Geist-Variable.ttf").to_vec(),
            )
            .expect("Geist registers");
        let s = TextStyle {
            family: crate::FontFamily::named(families[0].name.clone()),
            ..style(18.0)
        };
        let plain = cx.layout("Hamburg", &s, None).size().width;
        let heavy = s.clone().with_variation(*b"wght", 800.0);
        assert_ne!(
            ShapeKey::new("Hamburg", &s),
            ShapeKey::new("Hamburg", &heavy)
        );
        let wide = cx.layout("Hamburg", &heavy, None).size().width;
        assert!(wide > plain, "wght 800 widens Geist: {plain} vs {wide}");
    }

    #[test]
    fn text_change_forces_a_fresh_shape() {
        let mut cx = test_cx();
        let s = style(16.0);
        let _ = cx.layout("hello", &s, None);
        let _ = cx.layout("world", &s, None);
        assert_eq!(
            cx.shape_cache_stats().shapes,
            2,
            "a different string is a distinct key → fresh shape (no stale reuse)"
        );
    }

    #[test]
    fn style_and_color_changes_force_fresh_shapes() {
        let mut cx = test_cx();
        let _ = cx.layout("hello", &style(16.0), None);
        // Size change.
        let _ = cx.layout("hello", &style(24.0), None);
        // Color change (the glyph brush is baked into the shaped layout, so a
        // color change must not reuse an earlier shape — the theme-swap
        // correctness contract at the shaping layer).
        let _ = cx.layout("hello", &TextStyle::new(16.0, Color::WHITE), None);
        assert_eq!(cx.shape_cache_stats().shapes, 3);
    }

    #[test]
    fn invalidation_correct_across_all_mutation_orders() {
        // Property-style: whatever the interleaving of text/style/width, every
        // layout the cache returns matches a freshly-shaped reference — the
        // stale-text/style/width failure mode is what this kills.
        let styles = [style(16.0), style(28.0)];
        let texts = ["alpha beta", "gamma delta epsilon zeta eta"];
        let widths = [None, Some(60.0), Some(140.0)];

        let mut cx = test_cx();
        for _round in 0..3 {
            for t in &texts {
                for s in &styles {
                    for w in &widths {
                        let cached = cx.layout(t, s, *w).size();
                        // A pristine context shapes this exact combination fresh.
                        let mut reference = test_cx();
                        let fresh = reference.layout(t, s, *w).size();
                        assert_eq!(
                            cached, fresh,
                            "cached layout for (text={t:?}, size={}, width={w:?}) \
                             is stale — got {cached:?}, expected {fresh:?}",
                            s.size
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn cache_is_bounded_and_evicts_least_recently_used() {
        let mut cx = test_cx();
        let s = style(16.0);
        let overflow = 8;
        let n = DEFAULT_CAPACITY + overflow;

        // Fill past capacity with distinct strings (entry 0 is the oldest).
        for i in 0..n {
            let _ = cx.layout(&format!("entry number {i}"), &s, None);
        }
        let stats = cx.shape_cache_stats();
        assert_eq!(stats.shapes, n as u64, "each distinct string shapes once");
        assert_eq!(
            stats.evictions, overflow as u64,
            "capacity overflow evicts exactly the surplus, no unbounded growth"
        );

        // The most recently used entry is still cached → a full hit.
        let hits_before = cx.shape_cache_stats().hits;
        let _ = cx.layout(&format!("entry number {}", n - 1), &s, None);
        assert_eq!(
            cx.shape_cache_stats().hits,
            hits_before + 1,
            "the most-recently-used entry survives eviction"
        );

        // The oldest entry was evicted → re-requesting it re-shapes.
        let shapes_before = cx.shape_cache_stats().shapes;
        let _ = cx.layout("entry number 0", &s, None);
        assert_eq!(
            cx.shape_cache_stats().shapes,
            shapes_before + 1,
            "an evicted entry is re-shaped, not served stale"
        );
    }

    // --- Paragraph alignment ---

    /// A style at `align`, otherwise default.
    fn aligned_style(align: TextAlign) -> TextStyle {
        TextStyle {
            align,
            ..style(16.0)
        }
    }

    /// Lays out `text` at `max_width` and returns each line's minimum glyph
    /// `x` (its rendered left edge), in line order.
    ///
    /// Glyphs on the same line share a `y` (`crate::convert`'s coordinate
    /// contract), so grouping by `y` recovers per-line positions from the
    /// flat glyph-run output — the only origin-independent signal that
    /// alignment (baked into `positioned_glyphs()` by parley) actually moved
    /// a line, as opposed to just the style being set.
    fn line_min_x(cx: &mut TextContext, text: &str, style: &TextStyle, max_width: f32) -> Vec<f32> {
        let layout = cx.layout(text, style, Some(max_width));
        let runs = layout.to_scene_runs(kurbo::Point::ORIGIN);
        let mut by_y: Vec<(f32, f32)> = Vec::new();
        for run in &runs {
            for g in &run.glyphs {
                match by_y.iter_mut().find(|(y, _)| (*y - g.y).abs() < 0.01) {
                    Some((_, min_x)) => *min_x = min_x.min(g.x),
                    None => by_y.push((g.y, g.x)),
                }
            }
        }
        by_y.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        by_y.into_iter().map(|(_, x)| x).collect()
    }

    /// Two hard-broken lines of very different length, so alignment moves
    /// them by clearly different, non-accidental amounts. The long line is
    /// short enough to stay well under `max_width` (400px) even under a
    /// pessimistically wide glyph metric, so it never itself soft-wraps —
    /// keeping the line count at exactly two regardless of the host's
    /// resolved system font.
    const TWO_LINES: &str = "A\nBBBBBBBBBB";

    #[test]
    fn center_and_right_align_position_wrapped_lines_correctly() {
        // Asserts on per-line origins, not on the style merely being set —
        // this is what catches the v1 defect where `Align(CENTER, text(..))`
        // centred only the block, leaving every line hugging the leading
        // edge.
        let mut cx = test_cx();
        let max_width = 400.0;

        let start_x = line_min_x(
            &mut cx,
            TWO_LINES,
            &aligned_style(TextAlign::Start),
            max_width,
        );
        let center_x = line_min_x(
            &mut cx,
            TWO_LINES,
            &aligned_style(TextAlign::Center),
            max_width,
        );
        let right_x = line_min_x(
            &mut cx,
            TWO_LINES,
            &aligned_style(TextAlign::Right),
            max_width,
        );

        assert_eq!(start_x.len(), 2, "expected two hard-broken lines");
        assert_eq!(center_x.len(), 2);
        assert_eq!(right_x.len(), 2);

        // Start (default, v1-compatible): every line hugs the left edge.
        assert!(
            start_x[0].abs() < 0.5 && start_x[1].abs() < 0.5,
            "start-aligned lines must hug the left edge: {start_x:?}"
        );

        // Center: both lines move off the left edge, and the short line ("A")
        // centers further right than the long line, since each line is
        // centered independently within the 400px container.
        assert!(
            center_x[0] > 1.0 && center_x[1] > 1.0,
            "center-aligned lines must move off the left edge: {center_x:?}"
        );
        assert!(
            center_x[0] > center_x[1] + 1.0,
            "the shorter line must center further right than the longer one: {center_x:?}"
        );

        // Right: same relationship — the short line's left edge sits further
        // right than the long line's, since both trailing edges align.
        assert!(
            right_x[0] > right_x[1] + 1.0,
            "the shorter line's right-aligned left edge must sit further right: {right_x:?}"
        );
    }

    #[test]
    fn alignment_survives_a_width_change_through_the_shape_cache_rebreak() {
        // The resize regression the adversarial pass flagged: `LayoutCache::get`
        // had its own hardcoded `Alignment::Start` on the re-break path, so a
        // resized layout would silently revert to `Start` even though the
        // initial (from-scratch) layout in this fn correctly centered.
        let mut cx = test_cx();
        let centered = aligned_style(TextAlign::Center);

        // First pass: shapes and breaks from scratch at 400px.
        let _ = cx.layout(TWO_LINES, &centered, Some(400.0));

        // Second pass at a different width: a shape-cache hit that re-breaks
        // (not a fresh shape) — exactly the path `LayoutCache::get` owns.
        let x = line_min_x(&mut cx, TWO_LINES, &centered, 500.0);
        assert_eq!(x.len(), 2);
        assert!(
            x[0] > x[1] + 1.0,
            "center alignment must survive the width-change re-break: {x:?}"
        );

        let stats = cx.shape_cache_stats();
        assert_eq!(stats.shapes, 1, "the width change must not re-shape");
        assert_eq!(
            stats.line_breaks, 1,
            "sanity: this really went through the re-break path"
        );
    }

    #[test]
    fn same_text_different_alignment_does_not_collide_in_the_shape_cache() {
        // The `ShapeKey` extension regression: two texts identical but for
        // alignment must shape (and render) independently, not share one
        // cache entry.
        let mut cx = test_cx();
        let max_width = 400.0;

        let start_x = line_min_x(
            &mut cx,
            TWO_LINES,
            &aligned_style(TextAlign::Start),
            max_width,
        );
        let center_x = line_min_x(
            &mut cx,
            TWO_LINES,
            &aligned_style(TextAlign::Center),
            max_width,
        );

        assert_ne!(
            start_x, center_x,
            "a cache collision would make the second (center) request come back \
             identical to the first (start)"
        );
        assert!(
            start_x[0].abs() < 0.5,
            "the start-aligned request must render correctly despite sharing text \
             with a differently-aligned request: {start_x:?}"
        );
        assert!(
            center_x[0] > center_x[1] + 1.0,
            "the center-aligned request must render correctly despite sharing text \
             with a differently-aligned request: {center_x:?}"
        );

        let stats = cx.shape_cache_stats();
        assert_eq!(
            stats.shapes, 2,
            "distinct alignment must be a distinct shape, not a collision"
        );
    }

    #[test]
    fn default_alignment_matches_pre_findings_39_start_behavior() {
        // Byte-for-byte parity: the default style's layout is unchanged from
        // before this retrofit (both hardcoded call sites now apply
        // `TextAlign::Start`, exactly what they hardcoded before).
        let mut cx = test_cx();
        let default_x = line_min_x(&mut cx, TWO_LINES, &style(16.0), 400.0);
        let explicit_start_x =
            line_min_x(&mut cx, TWO_LINES, &aligned_style(TextAlign::Start), 400.0);
        assert_eq!(default_x, explicit_start_x);
        assert!(default_x.iter().all(|x| x.abs() < 0.5));
    }

    // --- max_lines / TextOverflow truncation ---

    /// A long, multi-word phrase that reliably soft-wraps to several lines
    /// under a narrow `max_width` (shared with this file's other wrap tests).
    const WRAPPING_TEXT: &str = "Hello from Frust, the pure Rust mobile UI toolkit";

    /// Groups a [`TextLayout`]'s painted glyphs by line (glyphs sharing a
    /// `y` are the same line — the coordinate contract [`line_min_x`] also
    /// relies on) and returns each line's minimum `x`, in line order.
    fn min_x_per_line(layout: &TextLayout) -> Vec<f32> {
        let runs = layout.to_scene_runs(kurbo::Point::ORIGIN);
        let mut by_y: Vec<(f32, f32)> = Vec::new();
        for run in &runs {
            for g in &run.glyphs {
                match by_y.iter_mut().find(|(y, _)| (*y - g.y).abs() < 0.01) {
                    Some((_, min_x)) => *min_x = min_x.min(g.x),
                    None => by_y.push((g.y, g.x)),
                }
            }
        }
        by_y.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        by_y.into_iter().map(|(_, x)| x).collect()
    }

    #[test]
    fn layout_bounded_with_no_max_lines_matches_plain_layout() {
        // The zero-cost, unchanged-behavior contract: `max_lines: None`
        // delegates straight to `layout`, so no existing caller (none of
        // which pass `max_lines`) can regress.
        let mut cx = test_cx();
        let s = style(16.0);
        let plain = cx.layout(WRAPPING_TEXT, &s, Some(200.0)).size();
        let bounded = cx
            .layout_bounded(WRAPPING_TEXT, &s, Some(200.0), None, TextOverflow::Ellipsis)
            .size();
        assert_eq!(plain, bounded);
    }

    #[test]
    fn single_line_fits_is_left_unmodified() {
        // "fits": comfortable width, well under the box — no truncation, no
        // ellipsis; byte-for-byte the same shape as a plain `layout` call.
        let mut cx = test_cx();
        let s = style(16.0);
        let text = "short";
        let plain = cx.layout(text, &s, Some(400.0)).size();
        let bounded = cx
            .layout_bounded(text, &s, Some(400.0), Some(1), TextOverflow::Ellipsis)
            .size();
        assert_eq!(plain, bounded);
    }

    #[test]
    fn single_line_exact_fit_is_not_truncated() {
        // The `>` (not `>=`) boundary: a width equal to the line's own
        // natural width is a fit, not an overflow.
        let mut cx = test_cx();
        let s = style(16.0);
        let text = "exact";
        let natural = cx.layout(text, &s, None).size().width;
        // `ceil()` keeps the bound at or a hair above the natural width —
        // avoids f64->f32 rounding noise making a bit-identical bound look
        // like a sub-pixel overflow.
        let max_width = natural.ceil() as f32;
        let bounded = cx.layout_bounded(text, &s, Some(max_width), Some(1), TextOverflow::Ellipsis);
        assert_eq!(bounded.line_count(), 1);
        assert_eq!(
            bounded.size().width,
            cx.layout(text, &s, Some(max_width)).size().width,
            "an exactly-fitting line must render identically to an untruncated layout"
        );
    }

    #[test]
    fn single_line_overflow_truncates_and_fits_the_bound() {
        // `max_lines(1)` forces a phrase that would otherwise soft-wrap onto
        // one truncated line.
        let mut cx = test_cx();
        let s = style(16.0);
        let max_width = 80.0;

        let plain = cx.layout(WRAPPING_TEXT, &s, Some(max_width));
        assert!(
            plain.line_count() > 1,
            "fixture sanity: expected this phrase to soft-wrap at {max_width}px, got {} line(s)",
            plain.line_count()
        );

        let bounded = cx.layout_bounded(
            WRAPPING_TEXT,
            &s,
            Some(max_width),
            Some(1),
            TextOverflow::Ellipsis,
        );
        assert_eq!(bounded.line_count(), 1, "max_lines(1) must yield one line");
        let width = bounded.line_info(0).expect("one line").width;
        assert!(
            width <= max_width,
            "the truncated+ellipsized line must fit the bound: {width} > {max_width}"
        );
    }

    #[test]
    fn max_lines_two_wrapped_truncates_only_the_last_visible_line() {
        let mut cx = test_cx();
        let s = style(16.0);
        let max_width = 60.0;

        let plain = cx.layout(WRAPPING_TEXT, &s, Some(max_width));
        assert!(
            plain.line_count() > 2,
            "fixture sanity: expected >2 wrapped lines at {max_width}px, got {}",
            plain.line_count()
        );
        let plain_first_line_width = plain.line_info(0).expect("line 0").width;

        let bounded = cx.layout_bounded(
            WRAPPING_TEXT,
            &s,
            Some(max_width),
            Some(2),
            TextOverflow::Ellipsis,
        );
        assert_eq!(bounded.line_count(), 2, "max_lines(2) must yield two lines");
        assert_eq!(
            bounded.line_info(0).expect("line 0").width,
            plain_first_line_width,
            "the greedy line-breaker's earlier line must survive the truncation \
             of a later line verbatim"
        );
        let last_width = bounded.line_info(1).expect("line 1").width;
        assert!(
            last_width <= max_width,
            "the truncated+ellipsized last visible line must fit the bound: \
             {last_width} > {max_width}"
        );
    }

    #[test]
    fn clip_drops_trailing_lines_without_touching_the_last_visible_line() {
        let mut cx = test_cx();
        let s = style(16.0);
        let max_width = 60.0;

        let plain = cx.layout(WRAPPING_TEXT, &s, Some(max_width));
        assert!(plain.line_count() > 1, "fixture sanity");
        let plain_first_line_width = plain.line_info(0).expect("line 0").width;

        let clipped = cx.layout_bounded(
            WRAPPING_TEXT,
            &s,
            Some(max_width),
            Some(1),
            TextOverflow::Clip,
        );
        assert_eq!(clipped.line_count(), 1);
        assert_eq!(
            clipped.line_info(0).expect("line 0").width,
            plain_first_line_width,
            "Clip drops trailing lines but never character-trims the last \
             visible one — its content, and so its width, must be identical \
             to the untruncated layout's own first line"
        );

        let ellipsized = cx.layout_bounded(
            WRAPPING_TEXT,
            &s,
            Some(max_width),
            Some(1),
            TextOverflow::Ellipsis,
        );
        assert_eq!(ellipsized.line_count(), 1);
        // Ellipsis appends '…', which Clip never does — the two modes' last
        // lines for the same overflowing input must not coincide.
        assert_ne!(
            clipped.line_info(0).expect("line 0").width,
            ellipsized.line_info(0).expect("line 0").width,
            "Clip and Ellipsis must produce visibly different last lines for \
             the same overflowing input"
        );
    }

    #[test]
    fn ellipsis_wider_than_the_box_still_renders_without_panicking() {
        let mut cx = test_cx();
        let s = style(16.0);
        // Narrower than a single glyph at this size — even a bare '…' can't
        // fit; the truncation walk must still return *something* rather than
        // panicking or yielding an empty layout.
        let bounded = cx.layout_bounded(
            WRAPPING_TEXT,
            &s,
            Some(1.0),
            Some(1),
            TextOverflow::Ellipsis,
        );
        assert_eq!(bounded.line_count(), 1);
        assert!(
            bounded.size().width > 0.0,
            "a best-effort bare ellipsis must still paint something"
        );
    }

    #[test]
    fn empty_string_is_not_truncated() {
        let mut cx = test_cx();
        let s = style(16.0);
        let bounded = cx.layout_bounded("", &s, Some(80.0), Some(1), TextOverflow::Ellipsis);
        assert_eq!(bounded.size().width, 0.0);
    }

    #[test]
    fn max_lines_zero_yields_an_empty_layout() {
        let mut cx = test_cx();
        let s = style(16.0);
        let bounded = cx.layout_bounded(
            WRAPPING_TEXT,
            &s,
            Some(80.0),
            Some(0),
            TextOverflow::Ellipsis,
        );
        assert_eq!(bounded.size().width, 0.0);
    }

    // --- Native ellipsis truncation ---

    /// A 500-character unbreakable token: the primary ellipsis case (a URL,
    /// hash, or session id far wider than its box).
    fn long_token() -> String {
        // Mixed-width characters, so a proportional guess can't be exact.
        "aWi".repeat(167)[..500].to_string()
    }

    #[test]
    fn a_long_unbreakable_token_truncates_with_two_shapes_and_no_eviction() {
        // The full text and the ellipsis shape once each; the truncation
        // itself re-breaks the cached shaping and measures nothing.
        let mut cx = test_cx();
        let s = style(16.0);
        let token = long_token();
        let max_width = 80.0;

        let bounded =
            cx.layout_bounded(&token, &s, Some(max_width), Some(1), TextOverflow::Ellipsis);

        assert_eq!(bounded.line_count(), 1);
        let width = bounded.line_info(0).expect("one line").width;
        assert!(
            width <= max_width,
            "the truncated line must still fit the bound: {width} > {max_width}"
        );
        let stats = cx.shape_cache_stats();
        assert_eq!(
            stats.shapes, 2,
            "the token and the ellipsis shape once each"
        );
        assert_eq!(stats.evictions, 0);
        let passes = cx.shape_cx.passes();
        let _ = cx.layout_bounded(&token, &s, Some(60.0), Some(1), TextOverflow::Ellipsis);
        assert_eq!(cx.shape_cx.passes(), passes, "a new width reshapes nothing");
    }

    #[test]
    fn the_truncation_never_evicts_live_cache_entries() {
        // With the LRU full, only the two legitimate inserts (the full text
        // and the ellipsis) may displace live entries.
        let mut cx = test_cx();
        let s = style(16.0);
        for i in 0..DEFAULT_CAPACITY {
            let _ = cx.layout(&format!("live entry {i}"), &s, None);
        }
        let evictions_before = cx.shape_cache_stats().evictions;

        let _ = cx.layout_bounded(
            &long_token(),
            &s,
            Some(60.0),
            Some(1),
            TextOverflow::Ellipsis,
        );

        assert_eq!(
            cx.shape_cache_stats().evictions - evictions_before,
            2,
            "a full cache may only lose the two entries the two legitimate \
             (full + ellipsis) inserts displace"
        );
    }

    /// Asserts `line` capped to one line at `max_width` with an ellipsis keeps
    /// the longest fitting prefix: the kept graphemes plus the ellipsis fit,
    /// and one more grapheme would not. Returns the kept byte range.
    fn assert_longest_fitting_prefix(
        cx: &mut TextContext,
        line: &str,
        s: &TextStyle,
        max_width: f32,
    ) -> std::ops::Range<usize> {
        let natural = cx.layout(line, s, None).size().width as f32;
        let bounded = cx.layout_bounded(line, s, Some(max_width), Some(1), TextOverflow::Ellipsis);
        let info = bounded.line_info(0).expect("one line");
        let what = format!("{max_width}px of {line:?}");
        if natural <= max_width {
            assert_eq!(info.range, 0..line.len(), "{what}: fits untouched");
            return info.range;
        }
        let ellipsis = cx.layout(&ELLIPSIS.to_string(), s, None).size().width as f32;
        let kept = &line[info.range.clone()];
        assert!(
            info.width <= max_width || kept.is_empty(),
            "{what}: {} > {max_width}",
            info.width
        );
        // Measure from the full layout's own shaping (the cut uses its
        // advances, kerning against the dropped neighbour included), so the
        // check holds for any font and wherever the line first wrapped.
        let full = cx.layout(line, s, None);
        let shaped = full.shaped();
        let byte_of = |char_index: u32| {
            shaped
                .chars
                .get(char_index as usize)
                .map_or(line.len(), |c| c.byte_start as usize)
        };
        // The next cut is one more grapheme plus any whitespace before it (a
        // cut never ends on whitespace); none when only whitespace follows.
        let mut kept_advance = 0.0f32;
        let mut next_advance = None;
        let mut gap = 0.0f32;
        for g in &shaped.graphemes {
            if byte_of(g.chars.end) <= info.range.end {
                kept_advance += g.advance;
                continue;
            }
            gap += g.advance;
            if g.whitespace == shape::Whitespace::None {
                next_advance = Some(gap);
                break;
            }
        }
        assert!(
            (info.width - (kept_advance + ellipsis)).abs() < 0.01,
            "{what}: the width is the kept prefix plus the ellipsis"
        );
        if let Some(next) = next_advance {
            assert!(
                kept_advance + next + ellipsis > max_width - 0.01,
                "{what}: one more character would still fit"
            );
        }
        info.range
    }

    #[test]
    fn the_cut_keeps_the_longest_fitting_prefix() {
        // The cut is maximal: the kept atoms plus the ellipsis fit, and one
        // more grapheme would not — including for a line whose glyph widths
        // are nowhere near uniform.
        let s = style(16.0);
        let lines = [
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "iiiiiiiiiiiiiiiiiiiiWWWWWWWWWWWWWWWWWWWW",
            "WWWWWWWWWWWWWWWWWWWWiiiiiiiiiiiiiiiiiiii",
            "https://example.com/a/very/long/path?q=1",
            &long_token(),
        ];
        for line in lines {
            for max_width in [12.0_f32, 40.0, 160.0, 400.0] {
                let mut cx = test_cx();
                assert_longest_fitting_prefix(&mut cx, line, &s, max_width);
            }
        }
    }

    #[test]
    fn a_line_that_wraps_early_is_refilled_before_the_cut() {
        // With the bundled subset (host-independent widths) the line's only
        // wrap opportunity is after `He `: the long word overflows. The cut
        // refills the line from that word rather than ending at `He…`.
        let (mut cx, s) = noto_cx();
        let line = "He Helloooooooooooooooooooooooooooooo";
        let max_width = 150.0;
        let wrapped = cx.layout(line, &s, Some(max_width));
        let first = wrapped.line_info(0).expect("a first line").range;
        assert_eq!(
            first,
            0..3,
            "fixture sanity: the line wraps after `He ` at {max_width}px"
        );
        let kept = assert_longest_fitting_prefix(&mut cx, line, &s, max_width);
        assert!(
            kept.end > first.end,
            "the cut reaches past the wrap point: kept {:?}",
            &line[kept]
        );
        for width in [40.0_f32, 80.0, 220.0] {
            assert_longest_fitting_prefix(&mut cx, line, &s, width);
        }
    }

    #[test]
    fn a_mandatory_break_stops_the_refill() {
        let (mut cx, s) = noto_cx();
        let text = "He Hello\nHelloooooooooooooooooooooooooooo";
        let newline = text.find('\n').expect("a newline");
        let bounded = cx.layout_bounded(text, &s, Some(400.0), Some(1), TextOverflow::Ellipsis);
        assert_eq!(bounded.line_count(), 1);
        assert_eq!(
            bounded.line_info(0).expect("one line").range,
            0..newline,
            "the line ends at its own newline; nothing after it refills it"
        );

        // A wrapped first paragraph refills up to its newline at most.
        let text = "He Helloooooooooooooooooooooooooooooooo\nHello";
        let newline = text.find('\n').expect("a newline");
        let bounded = cx.layout_bounded(text, &s, Some(150.0), Some(1), TextOverflow::Ellipsis);
        let kept = bounded.line_info(0).expect("one line").range;
        assert!(
            kept.end > 3 && kept.end <= newline,
            "the cut stays inside the first paragraph: {kept:?}"
        );
    }

    #[test]
    fn a_truncated_line_shares_the_full_layouts_glyphs() {
        // Nothing reshapes: the kept glyphs are the full line's own, in place.
        let mut cx = test_cx();
        let s = style(16.0);
        let full = cx.layout(WRAPPING_TEXT, &s, Some(60.0));
        let bounded = cx.layout_bounded(
            WRAPPING_TEXT,
            &s,
            Some(60.0),
            Some(2),
            TextOverflow::Ellipsis,
        );
        let full_glyphs = glyphs_of(&full.to_scene_runs(kurbo::Point::ORIGIN));
        let cut = glyphs_of(&bounded.to_scene_runs(kurbo::Point::ORIGIN));
        let ellipsis_glyphs = 1;
        assert!(cut.len() > ellipsis_glyphs);
        let kept = &cut[..cut.len() - ellipsis_glyphs];
        let ids = |glyphs: &[(u32, f32, f32)]| glyphs.iter().map(|g| g.0).collect::<Vec<_>>();
        assert_eq!(
            ids(kept),
            ids(&full_glyphs[..kept.len()]),
            "every kept glyph is the full layout's own, in logical order"
        );
        // Up to the end of the full layout's own last visible line the glyphs
        // sit where it put them; a refilled tail continues that line's pen.
        let last_y = kept.last().map_or(0.0, |g| g.2);
        let in_place = full_glyphs
            .iter()
            .take(kept.len())
            .take_while(|g| g.2 <= last_y + 0.01)
            .count();
        assert_eq!(&kept[..in_place], &full_glyphs[..in_place]);
        assert!(
            kept[in_place.saturating_sub(1)..]
                .windows(2)
                .all(|w| w[0].1 < w[1].1 && (w[0].2 - w[1].2).abs() < 0.01),
            "the refilled glyphs continue the last line: {:?}",
            &kept[in_place.saturating_sub(1)..]
        );
    }

    // --- Trailing-newline discount ---

    #[test]
    fn a_text_terminating_newline_is_not_an_extra_line() {
        // parley opens a zero-width line after a terminal '\n'; counting it as
        // overflow rendered "Hello…" for `text("Hello\n").max_lines(1)`.
        let s = style(16.0);
        for overflow in [TextOverflow::Ellipsis, TextOverflow::Clip] {
            let mut cx = test_cx();
            let plain = cx.layout("Hello", &s, Some(400.0));
            let fitting = plain.line_info(0).expect("one line").width;
            let with_ellipsis = cx
                .layout(&format!("Hello{ELLIPSIS}"), &s, Some(400.0))
                .line_info(0)
                .expect("one line")
                .width;

            let bounded = cx.layout_bounded("Hello\n", &s, Some(400.0), Some(1), overflow);
            let got = bounded.line_info(0).expect("one content line").width;

            assert_eq!(
                got, fitting,
                "{overflow:?}: text that fits must render verbatim despite its \
                 trailing newline"
            );
            assert_ne!(
                got, with_ellipsis,
                "{overflow:?}: no ellipsis may be appended to text that fits"
            );
            assert_eq!(
                bounded.line_count(),
                plain.line_count(),
                "{overflow:?}: parley's phantom trailing line must not survive \
                 into the reported line count"
            );
            assert_eq!(
                bounded.size().height,
                plain.size().height,
                "{overflow:?}: parley's phantom trailing line must not inflate \
                 the reported height — a text ending in one newline, capped to \
                 one line, must paint and measure exactly one line tall"
            );
        }
    }

    #[test]
    fn a_trailing_newline_after_several_lines_is_discounted_too() {
        let s = style(16.0);
        for overflow in [TextOverflow::Ellipsis, TextOverflow::Clip] {
            let mut cx = test_cx();
            let plain = cx.layout("A\nBB", &s, Some(400.0));
            let plain_last_width = plain.line_info(1).expect("two lines").width;
            let bounded = cx.layout_bounded("A\nBB\n", &s, Some(400.0), Some(2), overflow);
            assert_eq!(
                bounded.line_info(1).expect("two content lines").width,
                plain_last_width,
                "{overflow:?}: two content lines plus a terminal newline fit \
                 max_lines(2)"
            );
            assert_eq!(
                bounded.line_count(),
                plain.line_count(),
                "{overflow:?}: the phantom trailing line must not survive into \
                 the reported line count"
            );
            assert_eq!(
                bounded.size().height,
                plain.size().height,
                "{overflow:?}: the phantom trailing line must not inflate the \
                 reported height — two content lines plus a terminal newline, \
                 capped to two lines, must measure exactly two lines tall"
            );
        }
    }

    #[test]
    fn a_real_extra_line_still_truncates_when_the_text_ends_in_a_newline() {
        // The discount must not swallow genuine overflow.
        let mut cx = test_cx();
        let s = style(16.0);
        let clipped = cx.layout_bounded("A\nBB\n", &s, Some(400.0), Some(1), TextOverflow::Clip);
        assert_eq!(
            clipped.line_count(),
            1,
            "the second content line is dropped"
        );
        assert_eq!(
            clipped.size().width,
            cx.layout("A", &s, Some(400.0)).size().width
        );

        let ellipsized =
            cx.layout_bounded("A\nBB\n", &s, Some(400.0), Some(1), TextOverflow::Ellipsis);
        assert_eq!(ellipsized.line_count(), 1);
        assert!(
            ellipsized.size().width > clipped.size().width,
            "Ellipsis must append '…' to the surviving line"
        );
    }

    #[test]
    fn a_blank_line_inside_the_text_is_a_real_line() {
        // Only a *trailing* empty line is phantom: "a\n\nb" genuinely paints a
        // blank second line the caller asked for, so all three count.
        let mut cx = test_cx();
        let s = style(16.0);
        let fits = cx.layout_bounded("a\n\nb", &s, Some(400.0), Some(3), TextOverflow::Ellipsis);
        assert_eq!(
            fits.line_count(),
            3,
            "three content lines (one blank) fit max_lines(3) untouched"
        );

        let capped = cx.layout_bounded("a\n\nb", &s, Some(400.0), Some(2), TextOverflow::Clip);
        assert!(
            capped.line_count() < 3,
            "the blank line occupies one of the two, so 'b' overflows"
        );
    }

    // --- Multi-byte truncation boundaries ---

    #[test]
    fn multi_byte_text_truncates_on_char_boundaries_without_panicking() {
        // Every byte index in the truncation path is parley-derived or
        // char-boundary-derived; a mid-char slice would panic on this input.
        let s = style(16.0);
        let texts = [
            "日本語のテキストです、これは折り返しの確認用の文章です",
            "🙂🎉😀🚀🌍🙂🎉😀🚀🌍🙂🎉😀🚀🌍",
            "Grüße aus München — Übergrößenträger",
        ];
        for text in texts {
            for overflow in [TextOverflow::Ellipsis, TextOverflow::Clip] {
                for max_width in [20.0_f32, 70.0] {
                    let mut cx = test_cx();
                    let bounded = cx.layout_bounded(text, &s, Some(max_width), Some(1), overflow);
                    assert_eq!(bounded.line_count(), 1, "{text:?} at {max_width}px");
                    if overflow == TextOverflow::Ellipsis {
                        let width = bounded.line_info(0).expect("one line").width;
                        assert!(
                            width <= max_width,
                            "{text:?}: truncated width {width} exceeds {max_width}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn ellipsis_truncation_preserves_center_alignment() {
        // Hard-broken lines with generous width headroom, so the centering
        // offset can't be swamped by the truncation search converging on a
        // near-max-width candidate (see `max_lines_two_wrapped_...` above for
        // the width-tight case) — the same robust shape as this file's other
        // alignment tests (`TWO_LINES` at 400px).
        let mut cx = test_cx();
        let text = "A\nBBBBBBBBBB\nCCCCCCCCCC";
        let max_width = 400.0;

        let start = cx.layout_bounded(
            text,
            &aligned_style(TextAlign::Start),
            Some(max_width),
            Some(2),
            TextOverflow::Ellipsis,
        );
        let center = cx.layout_bounded(
            text,
            &aligned_style(TextAlign::Center),
            Some(max_width),
            Some(2),
            TextOverflow::Ellipsis,
        );

        assert_eq!(start.line_count(), 2);
        assert_eq!(center.line_count(), 2);

        let start_x = min_x_per_line(&start);
        let center_x = min_x_per_line(&center);
        assert_eq!(start_x.len(), 2);
        assert_eq!(center_x.len(), 2);

        assert!(
            start_x[1].abs() < 0.5,
            "start-aligned truncated line must hug the left edge: {start_x:?}"
        );
        assert!(
            center_x[1] > start_x[1] + 1.0,
            "center-aligned truncated line must move off the left edge: {center_x:?}"
        );
    }

    // --- .notdef detection (system fonts off so host Han fonts never cover) ---

    struct WarnCapture;
    static WARNS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    impl log::Log for WarnCapture {
        fn enabled(&self, m: &log::Metadata) -> bool {
            m.level() <= log::Level::Warn
        }
        fn log(&self, r: &log::Record) {
            if r.level() == log::Level::Warn {
                WARNS.lock().unwrap().push(r.args().to_string());
            }
        }
        fn flush(&self) {}
    }

    /// Registers `bytes` in a system-less context (an oracle over a backend
    /// with no fonts); returns it and the family.
    fn systemless_cx(bytes: &[u8]) -> (TextContext, String) {
        let mut cx = test_cx();
        cx.oracle = Oracle::new(
            Box::new(oracle::synthetic::SyntheticBackend::new("systemless")),
            RenderCapabilities::default(),
            None,
        );
        let families = cx
            .oracle
            .registry_mut()
            .insert_blob(&Blob::from(bytes.to_vec()));
        (cx, families[0].name.clone())
    }

    fn walk(cx: &mut TextContext, family: &str, text: &str) {
        let style = TextStyle {
            family: crate::FontFamily::named(family),
            ..style(16.0)
        };
        let _ = cx
            .layout(text, &style, None)
            .to_scene_runs(kurbo::Point::ORIGIN);
    }

    fn warns_naming(family: &str) -> Vec<String> {
        let quoted = format!("'{family}");
        WARNS
            .lock()
            .unwrap()
            .iter()
            .filter(|w| w.contains(&quoted) || w.contains(family))
            .cloned()
            .collect()
    }

    // One sequential test on purpose: every leg asserts an exact delta of the
    // process-wide `notdef_glyph_count()`, so splitting them into parallel tests
    // lets one leg's increments land inside another's window.
    #[test]
    fn notdef_counts_and_warns_once() {
        let _ = log::set_logger(&WarnCapture);
        log::set_max_level(log::LevelFilter::Warn);

        // Latin-only subset (" ,Helo\u{e9}"): two Han characters are .notdef.
        let noto = include_bytes!("../../../testing/fonts/NotoSans-Subset.ttf");
        let (mut cx, family) = systemless_cx(noto);
        let before = crate::notdef_glyph_count();
        walk(&mut cx, &family, "He中文 lo");
        assert_eq!(crate::notdef_glyph_count() - before, 2);
        let first = warns_naming(&family);
        assert_eq!(first.len(), 1, "first layout warns: {first:?}");
        assert!(
            first[0].contains("中文"),
            "names affected chars: {}",
            first[0]
        );

        // Counted again, but warned only once per font.
        walk(&mut cx, &family, "He中文 lo");
        assert_eq!(crate::notdef_glyph_count() - before, 4);
        assert_eq!(warns_naming(&family).len(), 1, "warn fires once per font");

        // Whitespace is not .notdef.
        let mid = crate::notdef_glyph_count();
        walk(&mut cx, &family, "He llo");
        assert_eq!(crate::notdef_glyph_count() - mid, 0);

        // Han the face covers is not .notdef.
        let (mut cx, family) = systemless_cx(include_bytes!(
            "../../../testing/fonts/NotoSansJP-Subset.otf"
        ));
        let mid = crate::notdef_glyph_count();
        walk(&mut cx, &family, "日本語");
        assert_eq!(crate::notdef_glyph_count() - mid, 0);
    }
}
