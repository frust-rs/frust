//! [`TextContext`]: the heavyweight, `!Sync` owner of parley's font and layout
//! state.
//!
//! Created once and threaded through the app/render loop. It holds
//! a [`parley::FontContext`] (system font sources, loaded via fontique — Core
//! Text on macOS with no registration) and a [`parley::LayoutContext`] scratch
//! buffer reused across layout passes.
//!
//! # The font oracle beside parley
//!
//! It also owns the frust font oracle ([`crate::oracle::Oracle`], built with
//! the platform backend for the target), which will replace fontique's
//! collection as the shaper's font source. Until that switch both are live:
//! parley still shapes through its own `FontContext`, and every app-font and
//! generic-fallback registration goes through the oracle's registry, which
//! mirrors it into parley's collection ([`ParleyMirror`]).
//!
//! # The shaping driver
//!
//! [`shape`] is frust's own shaping path over the oracle: [`TextContext::shape`]
//! returns a width-independent [`shape::Shaped`] through its own cache, beside
//! the parley path [`TextContext::layout`] still takes. Declared here because
//! the context owns its state.
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
//! the *generic* font-family map (fontique's `SystemUi`/`SansSerif`/
//! `Monospace`/`Serif`/`Emoji` slots) rather than a named family — see
//! [`register_generic_fallback`] for why a host needs this seam on `wasm32`.
//!
//! # Han fallback on Apple targets
//!
//! iOS's own Han font (PingFangUI) is hvgl-only, which fontique rejects, and
//! fontique's CoreText fallback hack asks for Heiti, which iOS does not ship,
//! so every Han fallback list is empty and Han shapes to `.notdef`.
//! [`append_han_fallbacks`] fills only the *empty* lists with Hiragino Sans (a
//! partial-coverage stopgap; see LIMITATIONS id `ios-han-fallback-partial`).
//!
//! The oracle's persistent font cache is placed by [`TextContext::builder`];
//! unconfigured contexts honour `FRUST_TEXT_FONT_CACHE` in debug builds.

use std::borrow::Cow;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parley::fontique::{Blob, GenericFamily};
use parley::style::StyleProperty;
use peniko::Brush;

use crate::layout::TextLayout;
use crate::oracle::registry::{FamilySummary, RegistryMirror};
use crate::oracle::{self, CacheLocation, Oracle, RenderCapabilities};
use crate::shape_cache::{DEFAULT_CAPACITY, LayoutCache, ShapeCacheStats, ShapeKey, ShapedCache};
use crate::style::{
    GenericSlot, TextOverflow, TextStyle, to_parley_align, to_parley_family, to_parley_line_height,
    to_parley_style, to_parley_weight,
};

#[path = "shape/mod.rs"]
pub(crate) mod shape;

/// The single character [`TextOverflow::Ellipsis`] appends/substitutes —
/// U+2026 HORIZONTAL ELLIPSIS, one glyph rather than three ASCII periods.
const ELLIPSIS: char = '\u{2026}';

/// The hard cap on how many candidate measurements one
/// [`TextContext::truncate_last_line`] call may perform.
///
/// The seek starts from a width-proportional estimate and normally converges
/// in a handful of measurements; the cap is what makes the *worst* case a
/// constant rather than a function of the line's length — an ellipsized
/// 500-character session id can never cost 500 shaping passes on the UI
/// thread. On exhaustion the longest candidate measured as fitting so far
/// wins (see that fn's docs).
const MAX_TRUNCATION_MEASUREMENTS: usize = 32;

/// A font family registered via [`TextContext::register_fonts`], reported back
/// to the caller so it can resolve widget styles against the exact name(s)
/// fontique assigned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredFamily {
    /// The family name fontique resolved the registered face(s) into (from
    /// the font's own `name` table, or an override).
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
/// process-wide and mirrors it into parley's collection via
/// `fontique::Collection::append_generic_families`.
///
/// # Why this exists
///
/// [`TextContext::new`] builds `parley::FontContext::new()`'s fontique
/// collection with the platform's real system-font backend on every target
/// except `wasm32-unknown-unknown`, where fontique falls back to a dummy
/// backend whose generic-family map is empty: [`crate::FontFamily::SystemUi`]
/// (this crate's default family) and any bare [`GenericSlot`] then resolve to
/// nothing and shape zero glyph runs. This function is the seam a host (a
/// shell) calls once at startup with a bundled face to give a generic slot
/// something to resolve to on that target; a platform whose backend already
/// populates the generic map is unaffected unless it opts in too, since the
/// registered face is only ever *appended* — never a replacement.
///
/// A stack that names a concrete family
/// ([`crate::FontFamily::Named`]/[`crate::FontFamily::NamedWithGeneric`],
/// registered via [`TextContext::register_fonts`]) still resolves before a
/// generic fallback, since a named lookup is always tried first.
///
/// No-op (records nothing) when `generics` is empty. Bytes fontique cannot
/// parse into any face register zero families when eventually applied, so
/// this is a harmless no-op then too — the same "no error surface" contract
/// [`TextContext::sync_app_fonts`] already follows for app fonts.
pub fn register_generic_fallback(data: Vec<u8>, generics: &[GenericSlot]) {
    oracle::registry::record_generic_fallback(data, generics);
}

/// Family names tried, in order, as the Han fallback on Apple targets.
#[cfg(target_vendor = "apple")]
const HAN_FALLBACK_CANDIDATES: &[&str] = &["Hiragino Sans", "Hiragino Kaku Gothic ProN"];

/// The Han-capable fallback keys: script `Hani` with no locale and each CJK
/// locale fontique tracks, plus `Hant`/`Hans` for scripts that distinguish them.
const HAN_FALLBACK_KEYS: &[([u8; 4], Option<&str>)] = &[
    (*b"Hani", None),
    (*b"Hani", Some("zh-Hans")),
    (*b"Hani", Some("zh-CN")),
    (*b"Hani", Some("zh-Hant")),
    (*b"Hani", Some("zh-TW")),
    (*b"Hani", Some("zh-HK")),
    (*b"Hani", Some("ja")),
    (*b"Hant", None),
    (*b"Hans", None),
];

/// Appends the first installed family of `candidates` to every Han fallback
/// key whose list is currently empty, returning how many keys were filled.
///
/// A non-empty list is never touched (macOS's Heiti, or a parsable PingFang,
/// keeps its answer). When no candidate is installed this changes nothing and
/// logs at debug level. Target-independent so host tests exercise it.
#[cfg_attr(not(target_vendor = "apple"), allow(dead_code))]
pub(crate) fn append_han_fallbacks(
    collection: &mut parley::fontique::Collection,
    candidates: &[&str],
) -> usize {
    use parley::fontique::{FallbackKey, Language, Script};
    let family = candidates
        .iter()
        .find_map(|name| collection.family_by_name(name))
        .map(|info| info.id());
    let Some(family) = family else {
        log::debug!("frust-text: no Han fallback family installed (tried {candidates:?})");
        return 0;
    };
    let mut filled = 0;
    for &(script, locale) in HAN_FALLBACK_KEYS {
        let script = Script::from_bytes(script);
        let locale = locale.and_then(|l| Language::parse(l).ok());
        let key = FallbackKey::new(script, locale.as_ref());
        if collection.fallback_families(key).next().is_none()
            && collection.append_fallbacks(key, std::iter::once(family))
        {
            filled += 1;
        }
    }
    filled
}

/// Converts a Frust [`GenericSlot`] into parley's `GenericFamily` — the
/// [`register_generic_fallback`] seam's own copy of
/// `crate::style::generic_slot_to_parley` (private to that module), so this
/// module still speaks only [`GenericSlot`] outward.
fn generic_slot_to_parley(slot: GenericSlot) -> GenericFamily {
    match slot {
        GenericSlot::Monospace => GenericFamily::Monospace,
        GenericSlot::SansSerif => GenericFamily::SansSerif,
        GenericSlot::Serif => GenericFamily::Serif,
        GenericSlot::SystemUi => GenericFamily::SystemUi,
        GenericSlot::Emoji => GenericFamily::Emoji,
    }
}

/// Mirrors the oracle registry's registrations into parley's fontique
/// collection, which still does the shaping until the oracle feeds the shaper.
pub(crate) struct ParleyMirror<'a>(pub(crate) &'a mut parley::FontContext);

impl RegistryMirror for ParleyMirror<'_> {
    fn app_font(&mut self, blob: &Blob<u8>) -> Vec<FamilySummary> {
        let collection = &mut self.0.collection;
        let registered = collection.register_fonts(blob.clone(), None);
        registered
            .into_iter()
            .map(|(family_id, faces)| FamilySummary {
                name: collection
                    .family_name(family_id)
                    .unwrap_or_default()
                    .to_string(),
                face_count: faces.len(),
            })
            .collect()
    }

    fn generic_fallback(&mut self, blob: &Blob<u8>, generics: &[GenericSlot]) -> bool {
        let collection = &mut self.0.collection;
        let mut applied = false;
        for (family_id, _faces) in collection.register_fonts(blob.clone(), None) {
            applied = true;
            for &generic in generics {
                collection.append_generic_families(
                    generic_slot_to_parley(generic),
                    std::iter::once(family_id),
                );
            }
        }
        applied
    }
}

/// Owns parley's font matching and layout scratch state.
///
/// This is deliberately not `Clone`/`Sync`: it is expensive per-instance state
/// meant to be constructed once and borrowed mutably for each layout pass. The
/// generic brush parameter is fixed to [`peniko::Brush`] so glyph runs carry the
/// same paint vocabulary as [`frust_scene`].
pub struct TextContext {
    font_ctx: parley::FontContext,
    layout_ctx: parley::LayoutContext<Brush>,
    /// Width-independent shape cache: a bounded LRU of
    /// shaped layouts keyed by (text, style), so a width change re-runs
    /// line-breaking only and repeated content shapes once. See
    /// [`crate::shape_cache`].
    shape_cache: LayoutCache,
    /// The shaping driver's analyzer/shaper state ([`shape::ShapeContext`]).
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "the line layout over `Shaped` is the driver's first production caller"
        )
    )]
    shape_cx: shape::ShapeContext,
    /// The shaping driver's cache: one [`shape::Shaped`] per (text, style).
    shaped_cache: ShapedCache,
    /// The frust font oracle: platform backend, fallback cache, and this
    /// context's app-font registry (with its watermarks into the process-wide
    /// app-font and generic-fallback records).
    oracle: Oracle,
    /// How many [`Self::measure_uncached`] passes this context has run — the
    /// observable hook the truncation walk's measurement bound is asserted
    /// against (an uncached measure is invisible to [`ShapeCacheStats`], which
    /// is the whole point of it).
    #[cfg(test)]
    measurements: usize,
}

/// Pushes `style` onto a fresh parley builder as its default run properties.
///
/// Shared by the caching [`TextContext::layout`] and the throwaway
/// [`TextContext::measure_uncached`] so the two can never drift into shaping
/// the same string against different fonts.
fn push_style_defaults(builder: &mut parley::RangedBuilder<'_, Brush>, style: &TextStyle) {
    // SystemUi resolves to the platform UI font (e.g. San Francisco on
    // macOS) with no registration.
    builder.push_default(to_parley_family(&style.family));
    builder.push_default(StyleProperty::FontSize(style.size));
    builder.push_default(StyleProperty::FontWeight(to_parley_weight(style.weight)));
    builder.push_default(StyleProperty::FontStyle(to_parley_style(style.style)));
    builder.push_default(StyleProperty::FontWidth(parley::FontWidth::from_ratio(
        style.width,
    )));
    if !style.variations.is_empty() {
        let variations: Vec<parley::FontVariation> = style
            .variations
            .iter()
            .map(|&(tag, value)| parley::FontVariation::new(parley::setting::Tag::new(&tag), value))
            .collect();
        builder.push_default(StyleProperty::FontVariations(parley::FontVariations::List(
            Cow::Owned(variations),
        )));
    }
    if let Some(locale) = style.locale.as_deref() {
        builder.push_default(StyleProperty::Locale(
            parley::style::Language::parse(locale).ok(),
        ));
    }
    builder.push_default(StyleProperty::LetterSpacing(style.letter_spacing));
    builder.push_default(StyleProperty::LineHeight(to_parley_line_height(
        style.line_height,
    )));
    builder.push_default(StyleProperty::Brush(Brush::Solid(style.color)));
}

/// The number of lines a layout actually puts content on, discounting the
/// empty line parley emits after a text-terminating `'\n'`.
///
/// parley attributes a hard break's newline to the line it terminates and then
/// opens a further, zero-width line for the caret position after it — so
/// `"Hello\n"` reports two lines while painting one. Counting that phantom
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

/// The truncation candidate for the prefix of `line_text` ending at byte
/// `end`: that prefix plus [`ELLIPSIS`].
///
/// `end` is always one of `line_text`'s own `char_indices` boundaries, so the
/// slice is in bounds by construction; `get` keeps an out-of-bounds/mid-char
/// index a graceful degradation to the bare ellipsis rather than a panic.
fn ellipsized(line_text: &str, end: usize) -> String {
    format!("{}{ELLIPSIS}", line_text.get(..end).unwrap_or_default())
}

/// `span` scaled by `target / measured`, floored and clamped to `0..=span`.
///
/// The truncation seek's start estimate: `span` characters spanning `measured`
/// pixels put roughly `span * target / measured` of them inside `target`. A
/// degenerate `measured` (zero, negative, or non-finite — an unmeasurable
/// line) falls back to the whole `span`, which merely starts the linear walk
/// where an exhaustive longest-prefix-first scan would have.
fn proportional_estimate(span: usize, target: f32, measured: f32) -> usize {
    if measured <= 0.0 || !measured.is_finite() || !target.is_finite() {
        return span;
    }
    if target <= 0.0 {
        return 0;
    }
    let span_f = span as f64;
    let estimate = (span_f * f64::from(target) / f64::from(measured)).floor();
    if estimate <= 0.0 {
        0
    } else if estimate >= span_f {
        span
    } else {
        estimate as usize
    }
}

/// Where [`TextContext::new_with_timings`] spent its time (startup evidence).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NewTimings {
    /// The font system's start-up: the oracle's construction (its persistent
    /// fallback cache is read later, at the first miss) plus — while parley
    /// still shapes — `parley::FontContext::new`'s platform font-source scan.
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
    /// [`parley::FontContext::new`] populates the fontique source collection from
    /// the platform (Core Text on macOS); no manual font registration is
    /// required for the default [`crate::FontFamily::SystemUi`] to resolve.
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
        let font_started = Instant::now();
        #[allow(unused_mut)]
        let mut font_ctx = parley::FontContext::new();
        #[cfg(target_vendor = "apple")]
        append_han_fallbacks(&mut font_ctx.collection, HAN_FALLBACK_CANDIDATES);
        let parley_scan = font_started.elapsed();
        let oracle_started = Instant::now();
        let oracle = Oracle::with_cache_location(
            oracle::platform_backend(),
            capabilities,
            cache_location(font_cache),
        );
        let font_system = parley_scan + oracle_started.elapsed();
        log::trace!("frust-text: font system {font_system:?}");
        let mut cx = Self {
            font_ctx,
            layout_ctx: parley::LayoutContext::new(),
            shape_cache: LayoutCache::new(DEFAULT_CAPACITY),
            shape_cx: shape::ShapeContext::default(),
            shaped_cache: ShapedCache::new(DEFAULT_CAPACITY),
            oracle,
            #[cfg(test)]
            measurements: 0,
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
    /// with no glyph runs and a near-zero size.
    ///
    /// `style.align` positions every line within the layout's width (a
    /// no-op distinction from [`crate::TextAlign::Start`] until `max_width`
    /// is bounded, since an unbounded line's width already equals its
    /// content). Applies here **and** on the width-change re-break path in
    /// [`crate::shape_cache::LayoutCache::get`] — see that fn's docs.
    pub fn layout(&mut self, text: &str, style: &TextStyle, max_width: Option<f32>) -> TextLayout {
        let key = ShapeKey::new(text, style);

        // Shape-cache fast path: reuse the shaped layout, re-running
        // line-breaking only on a width change (never re-shaping). Shaping is
        // width-independent, so `max_width` is not part of the key.
        if let Some(layout) = self.shape_cache.get(&key, max_width) {
            return TextLayout::new(layout);
        }

        // Miss: shape from scratch, then cache the shaped/broken result.
        //
        // scale = 1.0: lay out in logical pixels; the render tier applies the
        // device scale factor. quantize = true snaps advances for crisp glyphs.
        let mut builder = self
            .layout_ctx
            .ranged_builder(&mut self.font_ctx, text, 1.0, true);
        push_style_defaults(&mut builder, style);

        let mut layout = builder.build(text);
        layout.break_all_lines(max_width);
        // Both hardcoded-alignment sites (this one and the width-change
        // re-break path in `LayoutCache::get`) must apply `style.align` — see
        // `to_parley_align`'s docs.
        layout.align(
            to_parley_align(style.align),
            parley::layout::AlignmentOptions::default(),
        );

        self.shape_cache.insert(key, layout.clone(), max_width);
        TextLayout::new(layout)
    }

    /// Measures `text` as one unwrapped line, **without touching the shape
    /// cache** — no lookup, and crucially no insert.
    ///
    /// The truncation walk measures throwaway candidate strings that will
    /// never be laid out again; routing them through [`Self::layout`] would
    /// insert every one of them into the bounded LRU and evict that many live
    /// entries app-wide — a whole-cache flush for a long line, on top of the
    /// shaping cost. Shaping here is the same parley build [`Self::layout`]
    /// performs minus the cache insert and the alignment pass (alignment moves
    /// lines within a bounded width; it cannot change an unbounded line's
    /// advance, which is all this returns).
    fn measure_uncached(&mut self, text: &str, style: &TextStyle) -> f32 {
        #[cfg(test)]
        {
            self.measurements += 1;
        }
        let mut builder = self
            .layout_ctx
            .ranged_builder(&mut self.font_ctx, text, 1.0, true);
        push_style_defaults(&mut builder, style);
        let mut layout = builder.build(text);
        layout.break_all_lines(None);
        layout.width()
    }

    /// How many uncached measurements ([`Self::measure_uncached`]) this
    /// context has performed since it was built.
    #[cfg(test)]
    fn measurement_count(&self) -> usize {
        self.measurements
    }

    /// Lays out `text` exactly like [`Self::layout`], then caps it to
    /// `max_lines` (when `Some`), applying `overflow` to whatever is cut.
    ///
    /// `max_lines = None` delegates straight to [`Self::layout`] — the
    /// zero-cost, behavior-unchanged path every existing caller keeps taking.
    /// parley 0.11 has no native `max_lines`/ellipsis support, so a bounded
    /// call does two *cached* shaping passes on the truncating path: once to
    /// measure the full text, once more (in [`Self::layout`], so still
    /// shape-cache-backed) to shape the truncated result — post-shaping
    /// measure-and-truncate, not a parley feature. The ellipsis walk in
    /// between adds only bounded, cache-invisible measurements
    /// ([`Self::measure_uncached`]).
    ///
    /// # Algorithm
    ///
    /// A layout overflows `max_lines` in one of two ways parley itself
    /// exposes no direct query for, so both are checked explicitly against
    /// the full (untruncated) layout:
    /// - **extra lines**: line-breaking produced more than `max_lines`
    ///   *content* lines (wrapping, or `max_lines` hard `\n` breaks in the
    ///   source) — see [`visible_line_count`] for why the raw line count is
    ///   not that number when the text ends in `\n`.
    /// - **an unbreakable overrun**: exactly `max_lines` lines came out, but
    ///   the last visible one is itself wider than `max_width` — a run with
    ///   no break opportunity (one long unspaced word) that parley lets
    ///   overflow rather than force-break.
    ///
    /// Neither condition holds → the text already fits (this is also why an
    /// exact-fit line, width `== max_width`, never gets truncated: the
    /// comparison is a strict `>`); both overflow modes then return the full
    /// layout as-is, *unless* `full` itself still carries the raw phantom
    /// line parley opens after a terminal `'\n'` — in which case it's
    /// reshaped with the trailing newline(s) stripped first, so a fitting
    /// layout's reported line count and height always match what paints
    /// (see [`visible_line_count`]).
    ///
    /// On overflow, [`TextOverflow::Clip`] only ever drops whole trailing
    /// lines — the source text is cut at the end of line `max_lines - 1`'s
    /// span and reshaped; an unbreakable-overrun-only case (no extra lines
    /// to drop) is left untouched, since Clip never character-trims.
    /// [`TextOverflow::Ellipsis`] does the same line drop, then further
    /// truncates the last visible line's own text via
    /// [`Self::truncate_last_line`] and appends [`ELLIPSIS`], before
    /// reshaping the whole (earlier lines + truncated last line) string —
    /// which is also why earlier lines reliably survive verbatim: parley's
    /// line-breaker is greedy/left-to-right, so shortening what follows a
    /// line never changes how that line itself broke.
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
        let last_visible = max_lines - 1;
        let visible_lines = visible_line_count(&full, text.len());
        let has_extra_lines = visible_lines > max_lines;
        let Some(last_line) = full.line_info(last_visible) else {
            // Fewer than `max_lines` lines exist at all: nothing overflowed.
            return full;
        };
        let width_overflows = matches!(max_width, Some(w) if last_line.width > w);

        if !has_extra_lines && !width_overflows {
            if full.line_count() == visible_lines {
                // No phantom trailing line to discount — `full`'s raw line
                // count already matches what's painted.
                return full;
            }
            // The text fits, but `full` is parley's raw, undiscounted
            // layout: it still counts the phantom line opened after a
            // terminal `'\n'` in both its line count and its height (the
            // sum of every raw line's height, `visible_line_count`'s docs)
            // — so returning it unchanged reports a taller block than what
            // actually paints. Reshape with the trailing newline(s)
            // stripped — the same trim-then-reshape the `Clip` arm below
            // performs — so the returned layout's line count and height
            // match the visible content exactly.
            let Some(cut) = text.get(..last_line.range.end) else {
                return full;
            };
            return self.layout(cut.trim_end(), style, max_width);
        }

        match overflow {
            TextOverflow::Clip => {
                if !has_extra_lines {
                    // Only an unbreakable-run width overrun, no extra lines
                    // to drop — Clip never character-trims a line.
                    return full;
                }
                // `trim_end`: a line's own text range can include the very
                // whitespace/`\n` that ends it (parley attributes a hard
                // break's newline to the line it terminates), so a naive cut
                // could leave a trailing `\n` in the reshaped text — which
                // parley reads as *another* hard break, silently growing the
                // line count back past `max_lines`.
                //
                // `get` (not `[..]`): the index is parley-derived, and a
                // truncated layout is a far better failure mode for a caller
                // than a panic if it ever stops landing on a char boundary of
                // *this* string — see the fn docs' slicing contract.
                let Some(cut) = text.get(..last_line.range.end) else {
                    return full;
                };
                self.layout(cut.trim_end(), style, max_width)
            }
            TextOverflow::Ellipsis => {
                // See the `Clip` arm above for the `get` and `trim_end`
                // rationale (a trailing `\n` must not survive into a reshaped
                // fragment).
                let (Some(before), Some(line_text)) = (
                    text.get(..last_line.range.start),
                    text.get(last_line.range.start..last_line.range.end),
                ) else {
                    return full;
                };
                let line_text = line_text.trim_end();
                let truncated_last = match max_width {
                    Some(w) => self.truncate_last_line(line_text, style, w, last_line.width),
                    // No width to truncate against — keep the whole line,
                    // just mark it cut.
                    None => format!("{line_text}{ELLIPSIS}"),
                };
                let final_text = format!("{before}{truncated_last}");
                self.layout(&final_text, style, max_width)
            }
        }
    }

    /// The [`TextOverflow::Ellipsis`] truncation walk: returns a prefix of
    /// `line_text` (cut on a char boundary) plus `'…'`, measured alone as a
    /// single unwrapped line to be no wider than `max_width`. Falls back to a
    /// bare `'…'` if not even that fits (never returns an empty string with no
    /// overflow marker at all). `line_width` is the line's already-measured
    /// natural advance — the seek's scale reference, not a bound.
    ///
    /// # Bounded seek
    ///
    /// The walk *starts near the answer* rather than at one end: the ellipsis
    /// is measured once, and the first candidate is the width-proportional
    /// character estimate `chars * (max_width - ellipsis) / line_width`. One
    /// proportional re-estimate from that candidate's own measured prefix
    /// width follows (which lands a line mixing very narrow and very wide
    /// glyphs within a few characters of the answer), and only then a linear,
    /// one-character-at-a-time correction in whichever direction the probe
    /// pointed.
    ///
    /// The correction stays *linear* deliberately: kerning/ligature reshaping
    /// around a truncation point is not provably monotonic in every font, so a
    /// binary search could converge one character off in an adversarial font.
    /// This is the muxr `clipped_label` precedent's walk — only its starting
    /// point is estimated instead of being the whole string.
    ///
    /// # Cost bound
    ///
    /// Every measurement goes through [`Self::measure_uncached`], and the walk
    /// performs at most [`MAX_TRUNCATION_MEASUREMENTS`] of them. Both halves
    /// matter: a longest-prefix-first scan of an unbreakable token (a URL,
    /// hash, or session id wider than its box — the primary ellipsis case)
    /// cost one *cached* shaping pass per character, i.e. `O(len²)` work on
    /// the UI thread plus `len` inserts that evicted the shape cache's live
    /// entries app-wide. On cap exhaustion the longest candidate measured as
    /// fitting so far wins (the bare `'…'` in the worst case): the seek never
    /// loops unbounded, and never returns a candidate it did not measure as
    /// fitting.
    fn truncate_last_line(
        &mut self,
        line_text: &str,
        style: &TextStyle,
        max_width: f32,
        line_width: f32,
    ) -> String {
        let mut budget = MAX_TRUNCATION_MEASUREMENTS;

        // The empty-prefix candidate first: it is both the fallback and the
        // headroom every other candidate is measured against.
        let ellipsis_width = self.measure_uncached(&ellipsized(line_text, 0), style);
        budget -= 1;
        if ellipsis_width > max_width {
            // Even a bare ellipsis overflows — best effort, still signal the
            // truncation rather than silently rendering nothing.
            return ELLIPSIS.to_string();
        }
        let available = max_width - ellipsis_width;

        // `ends[i]` is the byte length of the prefix holding the line's first
        // `i` characters; `ends[longest]` is the whole line.
        let mut ends: Vec<usize> = line_text.char_indices().map(|(i, _)| i).collect();
        ends.push(line_text.len());
        let longest = ends.len() - 1;

        let mut cursor = proportional_estimate(longest, available, line_width);
        let mut width = self.measure_uncached(&ellipsized(line_text, ends[cursor]), style);
        budget -= 1;
        // One re-estimate from what the probe actually measured, so a line
        // whose glyph widths are nowhere near uniform still starts the linear
        // walk close to the answer.
        if budget > 0 && cursor > 0 {
            let refined = proportional_estimate(cursor, available, width - ellipsis_width);
            if refined != cursor {
                cursor = refined;
                width = self.measure_uncached(&ellipsized(line_text, ends[cursor]), style);
                budget -= 1;
            }
        }

        // The empty prefix is known to fit (checked above), so a fitting
        // answer always exists no matter where the budget runs out.
        let mut best = 0;
        if width <= max_width {
            best = cursor;
            // Grow a character at a time while candidates keep fitting.
            while best < longest && budget > 0 {
                budget -= 1;
                let next = best + 1;
                if self.measure_uncached(&ellipsized(line_text, ends[next]), style) > max_width {
                    break;
                }
                best = next;
            }
        } else {
            // Shrink a character at a time until one fits.
            while cursor > 0 && budget > 0 {
                budget -= 1;
                cursor -= 1;
                if self.measure_uncached(&ellipsized(line_text, ends[cursor]), style) <= max_width {
                    best = cursor;
                    break;
                }
            }
        }
        ellipsized(line_text, ends[best])
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
    /// same (text, style) is served without analysis or shaping.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "the line layout over `Shaped` is the driver's first production caller"
        )
    )]
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
            reason = "the line layout over `Shaped` is the driver's first production caller"
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
    /// read-fonts), which mirrors the bytes into parley's fontique collection
    /// ([`parley::fontique::Collection::register_fonts`]) while parley still
    /// shapes. A registered family **shadows** a same-named system family in
    /// both (fontique 0.11: the registered map is checked before the system
    /// map; the oracle: app faces before the system, and a provided family is
    /// never asked of the system), so bundling a family already present on the
    /// platform (e.g. "Roboto") deterministically wins over the platform's own
    /// copy. The returned names are fontique's when it registered the bytes.
    ///
    /// Always clears the shape cache on success — a same-named registered
    /// family changes shaping without changing the cache key, so any layout
    /// shaped before this call could otherwise be served stale. Returns
    /// [`FontError::NoFacesFound`] (no panic) for invalid/empty data, an
    /// empty byte slice, or bytes with no faces fontique can parse.
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
            .register(blob, &mut ParleyMirror(&mut self.font_ctx))
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
    /// layout retained *outside* it (a [`crate::TextEditor`]'s parley layout)
    /// must be re-shaped by its owner.
    pub fn sync_app_fonts(&mut self) -> bool {
        let applied = self
            .oracle
            .registry_mut()
            .sync(&mut ParleyMirror(&mut self.font_ctx));
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

    /// Borrows the parley font and layout contexts together, for constructing a
    /// transient [`parley::PlainEditorDriver`] in [`crate::TextEditor::apply`].
    ///
    /// Returned as a tuple so a single mutable borrow of `self` yields both
    /// contexts the driver's borrow-triple needs. `pub(crate)` — parley types
    /// must not leak past the crate boundary (scene-layer purity).
    pub(crate) fn driver_contexts(
        &mut self,
    ) -> (&mut parley::FontContext, &mut parley::LayoutContext<Brush>) {
        (&mut self.font_ctx, &mut self.layout_ctx)
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
    use peniko::Color;
    use std::sync::Mutex;

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

    /// The generic-fallback registry, exercised against a collection built
    /// with `system_fonts: false` — the same empty-generic-family-map shape
    /// `wasm32`'s dummy fontique backend has (see
    /// [`register_generic_fallback`]'s docs) — since a real host's system
    /// collection already has a resolvable `SystemUi`, so it could never
    /// reproduce the defect this seam fixes.
    #[test]
    fn generic_fallback_maps_requested_slots_and_is_idempotent() {
        use crate::oracle::registry::PendingGenericFallback;
        use parley::fontique::{Collection, CollectionOptions, GenericFamily};

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

        // A systemless collection: fontique's own stand-in for the wasm32
        // dummy backend's empty generic-family map.
        let mut font_ctx = parley::FontContext {
            collection: Collection::new(CollectionOptions {
                system_fonts: false,
                ..Default::default()
            }),
            source_cache: Default::default(),
        };
        let mut oracle = Oracle::new(
            Box::new(oracle::synthetic::SyntheticBackend::new("systemless")),
            RenderCapabilities::default(),
            None,
        );

        let applied = oracle
            .registry_mut()
            .apply_generic_entries(&entries, &mut ParleyMirror(&mut font_ctx));
        assert!(
            applied,
            "registering a parseable face must report at least one applied family"
        );

        let system_ui: Vec<_> = font_ctx
            .collection
            .generic_families(GenericFamily::SystemUi)
            .collect();
        assert_eq!(
            system_ui.len(),
            1,
            "SystemUi must resolve to exactly the registered fallback face"
        );
        assert!(
            font_ctx
                .collection
                .generic_families(GenericFamily::Monospace)
                .next()
                .is_none(),
            "Monospace must stay unmapped — only SystemUi/SansSerif were requested"
        );
        // The oracle sees the same face (the synthetic backend answers nothing, so
        // the app generic fallback is the answer).
        let face = oracle
            .default_face(GenericSlot::SystemUi)
            .expect("the oracle resolves SystemUi to the registered fallback");
        assert_eq!(face.blob.data(), TUFFY);
        assert!(oracle.default_face(GenericSlot::Monospace).is_none());

        // The defect under test: SystemUi-styled text on an otherwise-empty
        // system collection must still shape into glyph runs.
        let mut cx = TextContext {
            font_ctx,
            layout_ctx: parley::LayoutContext::new(),
            shape_cache: LayoutCache::new(DEFAULT_CAPACITY),
            shape_cx: shape::ShapeContext::default(),
            shaped_cache: ShapedCache::new(DEFAULT_CAPACITY),
            oracle,
            #[cfg(test)]
            measurements: 0,
        };
        let layout = cx.layout("Hello", &style(20.0), None);
        let runs = layout.to_scene_runs(kurbo::Point::ORIGIN);
        assert!(
            !runs.is_empty(),
            "SystemUi text must shape with the registered fallback face even \
             when the system font collection is empty"
        );

        // Idempotence: a second apply with nothing new pending must not
        // re-append the same family into the generic-family list.
        let applied_again = cx
            .oracle
            .registry_mut()
            .apply_generic_entries(&entries, &mut ParleyMirror(&mut cx.font_ctx));
        assert!(
            !applied_again,
            "nothing new is pending — a repeat apply must be a no-op"
        );
        let system_ui_after: Vec<_> = cx
            .font_ctx
            .collection
            .generic_families(GenericFamily::SystemUi)
            .collect();
        assert_eq!(
            system_ui_after, system_ui,
            "a repeat apply must not double-register the fallback family"
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
            "the parley path's cache is separate"
        );
        cx.clear_shape_cache();
        let _ = cx.shape("Hello, hello", &s);
        assert_eq!(cx.shape_cx.passes(), passes + 1, "cleared → shaped again");
    }

    #[test]
    fn the_driver_and_the_parley_path_pick_the_same_glyphs() {
        let (mut cx, s) = noto_cx();
        // Only characters the subset maps: fallback is where the two differ.
        let text = "Hello, Hello";
        let shaped = cx.shape(text, &s);
        let ours: Vec<u32> = shaped.glyphs.iter().map(|g| g.id).collect();
        let parley: Vec<u32> = cx
            .layout(text, &s, None)
            .to_scene_runs(kurbo::Point::ORIGIN)
            .iter()
            .flat_map(|run| run.glyphs.iter().map(|g| g.id))
            .collect();
        assert_eq!(ours, parley);
        let width: f32 = shaped.runs.iter().map(|r| r.advance).sum();
        let parley_width = cx.layout(text, &s, None).size().width as f32;
        assert!(
            (width - parley_width).abs() < 0.5,
            "{width} vs {parley_width}"
        );
    }

    #[test]
    fn style_variations_reach_the_parley_path_too() {
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

    use crate::style::TextAlign;

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

    // --- Bounded ellipsis seek ---

    /// A 500-character unbreakable token: the primary ellipsis case (a URL,
    /// hash, or session id far wider than its box), and the input the
    /// pre-bound walk shaped once per character.
    fn long_token() -> String {
        // Mixed-width characters, so the walk's proportional estimate can't
        // be trivially exact.
        "aWi".repeat(167)[..500].to_string()
    }

    /// The exhaustive longest-fitting-prefix scan (every char boundary,
    /// longest first) that the bounded seek approximates — the reference
    /// implementation for the seek's quality, kept only here.
    fn exhaustive_truncation(line: &str, s: &TextStyle, max_width: f32) -> String {
        let mut cx = test_cx();
        let mut ends: Vec<usize> = line.char_indices().map(|(i, _)| i).collect();
        ends.push(line.len());
        for &end in ends.iter().rev() {
            let candidate = format!("{}{ELLIPSIS}", &line[..end]);
            if cx.layout(&candidate, s, None).size().width <= f64::from(max_width) {
                return candidate;
            }
        }
        ELLIPSIS.to_string()
    }

    #[test]
    fn a_long_unbreakable_token_truncates_within_the_measurement_cap() {
        // The O(n²)-on-the-UI-thread defect: one full shaping pass per
        // character of the overflowing line. The bound is a constant now, and
        // none of those measurements may reach the shape cache.
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
        assert!(
            cx.measurement_count() <= MAX_TRUNCATION_MEASUREMENTS,
            "the seek must stay inside its cap: {} measurements for a {}-char token",
            cx.measurement_count(),
            token.chars().count()
        );
        let stats = cx.shape_cache_stats();
        assert_eq!(
            stats.shapes, 2,
            "only the full and the final truncated layout may be cached — every \
             candidate measurement is uncached"
        );
        assert_eq!(stats.evictions, 0);
    }

    #[test]
    fn the_truncation_walk_never_evicts_live_cache_entries() {
        // The cache-flush half of the same defect: with the LRU full, a
        // per-candidate insert evicted one live entry per character.
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
             (full + final) inserts displace"
        );
    }

    #[test]
    fn the_bounded_seek_matches_an_exhaustive_longest_prefix_scan() {
        // The bound must not cost accuracy: the seek's cut is the same one the
        // exhaustive scan finds, including for a line whose glyph widths are
        // nowhere near uniform (the case a single proportional estimate would
        // land far from).
        let s = style(16.0);
        let lines = [
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "iiiiiiiiiiiiiiiiiiiiWWWWWWWWWWWWWWWWWWWW",
            "WWWWWWWWWWWWWWWWWWWWiiiiiiiiiiiiiiiiiiii",
            "https://example.com/a/very/long/path?q=1",
            &long_token(),
        ];
        let mut worst = 0;
        for line in lines {
            for max_width in [12.0_f32, 40.0, 160.0, 400.0] {
                let mut cx = test_cx();
                let line_width = cx.layout(line, &s, None).size().width as f32;
                let got = cx.truncate_last_line(line, &s, max_width, line_width);
                let want = exhaustive_truncation(line, &s, max_width);
                assert_eq!(
                    got, want,
                    "bounded seek disagreed with the exhaustive scan for \
                     {max_width}px of {line:?}"
                );
                worst = worst.max(cx.measurement_count());
            }
        }
        // Comfortably inside the cap (12 on the reference host), so the
        // agreement above is real convergence, not a capped coincidence.
        assert!(
            worst <= MAX_TRUNCATION_MEASUREMENTS,
            "worst seek across the fixtures took {worst} measurements"
        );
    }

    #[test]
    fn the_seek_returns_a_fitting_candidate_even_when_the_cap_is_exhausted() {
        // Cap exhaustion is a quality fallback, never a correctness one: the
        // returned candidate is always one measured as fitting. Forced here by
        // a line whose natural width lies about where the fitting prefix ends
        // (a deliberately misleading `line_width`).
        let mut cx = test_cx();
        let s = style(16.0);
        let line = "iiiiiiiiiiiiiiiiiiiiiiiiiiiiiiWWWWWWWWWWWWWWWWWWWWWWWWWWWWWW";
        let max_width = 60.0;

        let got = cx.truncate_last_line(line, &s, max_width, 1.0);

        assert!(
            cx.measurement_count() <= MAX_TRUNCATION_MEASUREMENTS,
            "the cap binds even when the estimate is useless: {}",
            cx.measurement_count()
        );
        assert!(got.ends_with(ELLIPSIS));
        let width = cx.layout(&got, &s, None).size().width;
        assert!(
            width <= f64::from(max_width),
            "a cap-exhausted seek must still return a measured-fitting candidate: \
             {width} > {max_width}"
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
            assert_eq!(
                cx.measurement_count(),
                0,
                "{overflow:?}: a fitting line must not enter the truncation walk"
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
        assert_eq!(
            cx.measurement_count(),
            0,
            "no truncation walk for text that fits"
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

    /// A system-less `TextContext` plus the JP subset face's family name.
    fn bare_cx() -> (TextContext, String, Vec<u8>) {
        let mut cx = test_cx();
        cx.font_ctx.collection =
            parley::fontique::Collection::new(parley::fontique::CollectionOptions {
                shared: false,
                system_fonts: false,
            });
        let bytes = include_bytes!("../../../testing/fonts/NotoSansJP-Subset.otf").to_vec();
        let registered = cx
            .font_ctx
            .collection
            .register_fonts(Blob::from(bytes.clone()), None);
        let id = registered[0].0;
        let name = cx.font_ctx.collection.family_name(id).unwrap().to_owned();
        (cx, name, bytes)
    }

    fn han_key(locale: Option<&str>) -> parley::fontique::FallbackKey {
        let locale = locale.and_then(|l| parley::fontique::Language::parse(l).ok());
        parley::fontique::FallbackKey::new(
            parley::fontique::Script::from_bytes(*b"Hani"),
            locale.as_ref(),
        )
    }

    #[test]
    fn han_fallback_fills_empty_keys_and_han_text_shapes() {
        let (mut cx, name, _) = bare_cx();
        let id = cx.font_ctx.collection.family_id(&name).unwrap();
        assert!(
            cx.font_ctx
                .collection
                .fallback_families(han_key(None))
                .next()
                .is_none()
        );
        let filled = append_han_fallbacks(&mut cx.font_ctx.collection, &["No Such Family", &name]);
        assert!(filled > 0);
        for loc in [
            None,
            Some("zh-CN"),
            Some("zh-TW"),
            Some("zh-HK"),
            Some("ja"),
        ] {
            let got: Vec<_> = cx
                .font_ctx
                .collection
                .fallback_families(han_key(loc))
                .collect();
            assert_eq!(got, vec![id], "locale {loc:?}");
        }
        let layout = cx.layout("日本語", &style(16.0), None);
        let runs = layout.to_scene_runs(kurbo::Point::ORIGIN);
        let glyphs: Vec<_> = runs.iter().flat_map(|r| r.glyphs.iter()).collect();
        assert!(!glyphs.is_empty());
        assert!(
            glyphs.iter().all(|g| g.id != 0),
            "Han must not shape to .notdef"
        );
    }

    #[test]
    fn han_fallback_leaves_non_empty_lists_untouched() {
        let (mut cx, name, bytes) = bare_cx();
        let other = cx.font_ctx.collection.register_fonts(
            Blob::from(bytes),
            Some(parley::fontique::FontInfoOverride {
                family_name: Some("Pre Existing"),
                ..Default::default()
            }),
        )[0]
        .0;
        let key = han_key(Some("ja"));
        cx.font_ctx
            .collection
            .set_fallbacks(key, std::iter::once(other));
        append_han_fallbacks(&mut cx.font_ctx.collection, &[&name]);
        let got: Vec<_> = cx.font_ctx.collection.fallback_families(key).collect();
        assert_eq!(got, vec![other]);
    }

    #[test]
    fn han_fallback_with_no_installed_candidate_changes_nothing() {
        let (mut cx, _, _) = bare_cx();
        assert_eq!(
            append_han_fallbacks(&mut cx.font_ctx.collection, &["Nope", "Also Nope"]),
            0
        );
        assert!(
            cx.font_ctx
                .collection
                .fallback_families(han_key(None))
                .next()
                .is_none()
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

    /// Registers `bytes` in a system-less context; returns it and the family.
    fn systemless_cx(bytes: &[u8]) -> (TextContext, String) {
        let mut cx = test_cx();
        cx.font_ctx.collection =
            parley::fontique::Collection::new(parley::fontique::CollectionOptions {
                shared: false,
                system_fonts: false,
            });
        let id = cx
            .font_ctx
            .collection
            .register_fonts(Blob::from(bytes.to_vec()), None)[0]
            .0;
        let name = cx.font_ctx.collection.family_name(id).unwrap().to_owned();
        (cx, name)
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
