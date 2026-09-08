//! [`TextContext`]: the heavyweight, `!Sync` owner of parley's font and layout
//! state.
//!
//! Created once and threaded through the app/render loop. It holds
//! a [`parley::FontContext`] (system font sources, loaded via fontique — Core
//! Text on macOS with no registration) and a [`parley::LayoutContext`] scratch
//! buffer reused across layout passes.
//!
//! # App fonts across independent contexts
//!
//! A shell owns *the* context widgets shape through at layout time, but a
//! widget that must shape outside the layout pass legitimately owns a private
//! one (`frust_widgets::textinput`). [`APP_FONTS`] is the process-wide record
//! that keeps those two in agreement about registered app fonts — see its docs
//! for the layering rationale.
//!
//! # Generic-family fallback registry
//!
//! [`GENERIC_FALLBACKS`] is the process-wide counterpart for the *generic*
//! font-family map (fontique's `SystemUi`/`SansSerif`/`Monospace`/`Serif`/
//! `Emoji` slots) rather than a named family — see [`register_generic_fallback`]
//! for why a host needs this seam on `wasm32`.

use std::sync::Mutex;

use parley::fontique::{Blob, GenericFamily};
use parley::style::StyleProperty;
use peniko::Brush;

use crate::layout::TextLayout;
use crate::shape_cache::{DEFAULT_CAPACITY, ShapeCache, ShapeCacheStats, ShapeKey};
use crate::style::{
    GenericSlot, TextOverflow, TextStyle, to_parley_align, to_parley_family, to_parley_line_height,
    to_parley_style, to_parley_weight,
};

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

/// The process-wide record of every font blob a [`TextContext::register_fonts`]
/// call accepted, in registration order and **never drained**.
///
/// # Why this lives here
///
/// App fonts enter the framework through `frust::register_app_fonts`, whose
/// pending-byte slot (`frust_shell_common::font_registry`) each shell drains —
/// destructively — into the one shell-owned `TextContext`. That context is the
/// one `LayoutCtx::text_context` threads to widgets during layout, so every
/// widget shaping through it (`Text`) already sees app fonts. The gap is the
/// widget that legitimately owns a *private* context: `TextInput` applies edits
/// synchronously during the event pass, where no context is threaded (see
/// `frust_widgets::textinput`'s module docs), and a private context built after
/// the shell drained the pending slot would otherwise carry no app font at all.
///
/// `frust-widgets` cannot read the shell's slot — `frust-shell-common` sits
/// *above* it (`docs/ARCHITECTURE.md`'s layer dependencies) — so the durable
/// record lives at the one layer the shell drain and every widget both already
/// depend on. Registering into any context records the blob here; every other
/// context picks it up through [`TextContext::sync_app_fonts`], which
/// [`TextContext::new`] calls for free.
///
/// Append-only by design: the payloads must survive to seed contexts created
/// *later*, which is exactly what a drain-once slot cannot do. Blobs are
/// `Arc`-backed, so seeding a fresh context costs a refcount bump plus
/// fontique's own parse, never a copy of the font bytes.
static APP_FONTS: Mutex<Vec<Blob<u8>>> = Mutex::new(Vec::new());

/// One [`register_generic_fallback`] call's payload: the raw font bytes plus
/// the generic slot(s) it should be appended to.
struct PendingGenericFallback {
    blob: Blob<u8>,
    generics: Vec<GenericSlot>,
}

/// The process-wide record of every [`register_generic_fallback`] call
/// accepted so far, in registration order and **never drained** — the
/// generic-family counterpart of [`APP_FONTS`], applied by
/// [`TextContext::sync_app_fonts`] (and so, for free, by [`TextContext::new`])
/// to every context's `fontique::Collection` generic-family map rather than
/// its named-family table.
static GENERIC_FALLBACKS: Mutex<Vec<PendingGenericFallback>> = Mutex::new(Vec::new());

/// Registers `data` (raw font bytes) as a fallback face for each generic
/// family slot in `generics`, applied to every [`TextContext`] — existing,
/// on its next [`TextContext::sync_app_fonts`], and future, seeded for free
/// by [`TextContext::new`] — via `fontique::Collection::append_generic_families`.
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
    if generics.is_empty() {
        return;
    }
    let mut slot = GENERIC_FALLBACKS.lock().unwrap_or_else(|e| e.into_inner());
    slot.push(PendingGenericFallback {
        blob: Blob::from(data),
        generics: generics.to_vec(),
    });
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

/// Applies `entries[*watermark..]` to `font_ctx`'s generic-family map,
/// registering each entry's face and appending it to every generic slot the
/// entry requested, then advances `*watermark` to `entries.len()`. A no-op,
/// returning `false`, when `*watermark` already equals `entries.len()`.
///
/// Pure over its arguments — no process-wide state — so [`TextContext`]'s
/// [`TextContext::sync_app_fonts`] can drive it against the shared
/// [`GENERIC_FALLBACKS`] record for production use, while a test drives it
/// against a locally built `entries`/`watermark` pair to exercise the exact
/// registration/append logic without leaking a face into the process-wide
/// record — [`register_generic_fallback`] never drains, so a test call
/// through the real seam would otherwise remain registered for, and change
/// the font resolution of, every other `TextContext` built later in the same
/// test binary process.
fn apply_generic_fallbacks(
    font_ctx: &mut parley::FontContext,
    entries: &[PendingGenericFallback],
    watermark: &mut usize,
) -> bool {
    if *watermark == entries.len() {
        return false;
    }
    let mut applied = false;
    for entry in &entries[*watermark..] {
        let registered = font_ctx.collection.register_fonts(entry.blob.clone(), None);
        for (family_id, _faces) in registered {
            applied = true;
            for &generic in &entry.generics {
                font_ctx.collection.append_generic_families(
                    generic_slot_to_parley(generic),
                    std::iter::once(family_id),
                );
            }
        }
    }
    *watermark = entries.len();
    applied
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
    shape_cache: ShapeCache,
    /// How many of [`APP_FONTS`]' blobs this context has already registered —
    /// its watermark into that append-only record. Bumped by
    /// [`Self::sync_app_fonts`] and [`Self::register_fonts`].
    app_fonts_applied: usize,
    /// How many of [`GENERIC_FALLBACKS`]' entries this context has already
    /// applied — its watermark into that append-only record, mirroring
    /// `app_fonts_applied`. Bumped by [`Self::sync_app_fonts`] only; unlike an
    /// app font, a generic fallback has no direct `register_*` method of its
    /// own on `TextContext`.
    generic_fallbacks_applied: usize,
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

impl TextContext {
    /// Builds a context with system fonts available, seeded with every app font
    /// registered so far.
    ///
    /// [`parley::FontContext::new`] populates the fontique source collection from
    /// the platform (Core Text on macOS); no manual font registration is
    /// required for the default [`crate::FontFamily::SystemUi`] to resolve.
    ///
    /// The seeding step ([`Self::sync_app_fonts`]) is what makes a *private*
    /// context (a `TextInput`'s) shape with the same app fonts the shell-owned
    /// context does, however late it is constructed — see [`APP_FONTS`]. It
    /// also applies every pending [`register_generic_fallback`] entry, so a
    /// generic-family fallback a shell registered before this call is already
    /// live in the returned context — see [`GENERIC_FALLBACKS`].
    pub fn new() -> Self {
        let mut cx = Self {
            font_ctx: parley::FontContext::new(),
            layout_ctx: parley::LayoutContext::new(),
            shape_cache: ShapeCache::new(DEFAULT_CAPACITY),
            app_fonts_applied: 0,
            generic_fallbacks_applied: 0,
            #[cfg(test)]
            measurements: 0,
        };
        cx.sync_app_fonts();
        cx
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
    /// [`crate::shape_cache::ShapeCache::get`] — see that fn's docs.
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
        // re-break path in `ShapeCache::get`) must apply `style.align` — see
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

    /// Registers font faces from raw bytes (TTF/OTF, or a TTC/OTC
    /// collection) so they resolve by family name via
    /// [`crate::FontFamily::named`]/[`crate::FontFamily::stack`].
    ///
    /// Wraps fontique's [`parley::fontique::Collection::register_fonts`]. A
    /// registered family **shadows** a same-named system family (fontique
    /// 0.11 semantics: the registered map is checked before the system map),
    /// so bundling a family already present on the platform (e.g. "Roboto")
    /// deterministically wins over the platform's own copy.
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
    /// An accepted payload is also recorded process-wide ([`APP_FONTS`]), so
    /// every `TextContext` constructed later — and every existing one that
    /// calls [`Self::sync_app_fonts`] — resolves the same family. That is what
    /// carries an app font from a shell's drain into a widget-owned private
    /// context.
    pub fn register_fonts(&mut self, data: Vec<u8>) -> Result<Vec<RegisteredFamily>, FontError> {
        let blob = Blob::from(data);

        // Held across the registration below so the watermark this sets cannot
        // skip a blob another context records concurrently.
        let mut slot = APP_FONTS.lock().unwrap_or_else(|e| e.into_inner());
        // Catch up first: this context may be behind the record (another
        // context registered since it was built), and the watermark below
        // would otherwise declare those blobs applied without applying them.
        for missed in &slot[self.app_fonts_applied..] {
            let _ = self
                .font_ctx
                .collection
                .register_fonts(missed.clone(), None);
        }
        self.app_fonts_applied = slot.len();

        let registered = self.font_ctx.collection.register_fonts(blob.clone(), None);
        if registered.is_empty() {
            return Err(FontError::NoFacesFound);
        }
        slot.push(blob);
        self.app_fonts_applied = slot.len();
        drop(slot);

        let families = registered
            .into_iter()
            .map(|(family_id, faces)| RegisteredFamily {
                name: self
                    .font_ctx
                    .collection
                    .family_name(family_id)
                    .unwrap_or_default()
                    .to_string(),
                face_count: faces.len(),
            })
            .collect();

        self.clear_shape_cache();
        Ok(families)
    }

    /// Registers every app font this context is missing (see [`APP_FONTS`])
    /// and applies every [`register_generic_fallback`] entry it is missing
    /// (see [`GENERIC_FALLBACKS`]), returning whether either actually
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
        let mut applied = self.sync_generic_fallbacks();

        let slot = APP_FONTS.lock().unwrap_or_else(|e| e.into_inner());
        if self.app_fonts_applied != slot.len() {
            for blob in &slot[self.app_fonts_applied..] {
                if !self
                    .font_ctx
                    .collection
                    .register_fonts(blob.clone(), None)
                    .is_empty()
                {
                    applied = true;
                }
            }
            self.app_fonts_applied = slot.len();
        }
        drop(slot);

        if applied {
            self.clear_shape_cache();
        }
        applied
    }

    /// Applies every [`GENERIC_FALLBACKS`] entry this context is missing,
    /// appending each face's registered family(ies) to every generic slot the
    /// entry requested. Idempotent: an entry already applied to this context
    /// (tracked by [`Self::generic_fallbacks_applied`], a watermark into that
    /// append-only record, exactly like [`Self::app_fonts_applied`]) is never
    /// re-registered, so a repeated [`Self::sync_app_fonts`] call never
    /// double-appends the same family into a generic slot's fallback list.
    ///
    /// Returns whether at least one face actually registered — the same
    /// signal [`Self::sync_app_fonts`] folds together with the app-font half
    /// to decide whether to clear the shape cache.
    ///
    /// Thin wrapper over [`apply_generic_fallbacks`], the pure logic this
    /// drives against the process-wide [`GENERIC_FALLBACKS`] record — see
    /// that function's docs for why the split exists.
    fn sync_generic_fallbacks(&mut self) -> bool {
        let slot = GENERIC_FALLBACKS.lock().unwrap_or_else(|e| e.into_inner());
        apply_generic_fallbacks(
            &mut self.font_ctx,
            &slot,
            &mut self.generic_fallbacks_applied,
        )
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
        self.shape_cache = ShapeCache::new(DEFAULT_CAPACITY);
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
        let mut older = TextContext::new();

        // The shell's drain.
        let mut shell = TextContext::new();
        shell
            .register_fonts(TUFFY.to_vec())
            .expect("valid TTF bytes must register");

        // A context built after it needs no explicit sync.
        let mut later = TextContext::new();
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
        use parley::fontique::{Collection, CollectionOptions, GenericFamily};

        // Entries built locally, driven straight through `apply_generic_fallbacks`
        // rather than the real `register_generic_fallback` seam — that seam
        // writes to the process-wide `GENERIC_FALLBACKS` record, which is
        // never drained, so a real call here would keep applying this face to
        // every other test's (real-system-font) `TextContext` built later in
        // this same test binary process and could change which face they
        // resolve. See `apply_generic_fallbacks`'s docs.
        let entries = vec![PendingGenericFallback {
            blob: Blob::from(TUFFY.to_vec()),
            generics: vec![GenericSlot::SystemUi, GenericSlot::SansSerif],
        }];
        let mut watermark = 0;

        // A systemless collection: fontique's own stand-in for the wasm32
        // dummy backend's empty generic-family map.
        let mut font_ctx = parley::FontContext {
            collection: Collection::new(CollectionOptions {
                system_fonts: false,
                ..Default::default()
            }),
            source_cache: Default::default(),
        };

        let applied = apply_generic_fallbacks(&mut font_ctx, &entries, &mut watermark);
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

        // The defect under test: SystemUi-styled text on an otherwise-empty
        // system collection must still shape into glyph runs.
        let mut cx = TextContext {
            font_ctx,
            layout_ctx: parley::LayoutContext::new(),
            shape_cache: ShapeCache::new(DEFAULT_CAPACITY),
            app_fonts_applied: 0,
            generic_fallbacks_applied: watermark,
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
        let applied_again = apply_generic_fallbacks(&mut cx.font_ctx, &entries, &mut watermark);
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
        // writes to the process-wide, never-drained `GENERIC_FALLBACKS` record,
        // so a call naming a real slot here would keep applying for the rest of
        // this test binary's run — see `apply_generic_fallbacks`'s docs and the
        // isolated-collection test above, which exercises the real
        // registration/append behavior without that leak.
        let before = GENERIC_FALLBACKS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len();
        register_generic_fallback(vec![1, 2, 3], &[]);
        let after = GENERIC_FALLBACKS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len();
        assert_eq!(
            before, after,
            "no generic slots requested — nothing should be queued"
        );
    }

    #[test]
    fn same_text_two_widths_shapes_once() {
        // The core claim: shaping is width-independent. Laying out
        // the same text+style at two different widths shapes exactly once; the
        // width change re-runs line-breaking only, and a repeated width is a
        // full reuse.
        let mut cx = TextContext::new();
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

    #[test]
    fn text_change_forces_a_fresh_shape() {
        let mut cx = TextContext::new();
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
        let mut cx = TextContext::new();
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

        let mut cx = TextContext::new();
        for _round in 0..3 {
            for t in &texts {
                for s in &styles {
                    for w in &widths {
                        let cached = cx.layout(t, s, *w).size();
                        // A pristine context shapes this exact combination fresh.
                        let mut reference = TextContext::new();
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
        let mut cx = TextContext::new();
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
        let mut cx = TextContext::new();
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
        // The resize regression the adversarial pass flagged: `ShapeCache::get`
        // had its own hardcoded `Alignment::Start` on the re-break path, so a
        // resized layout would silently revert to `Start` even though the
        // initial (from-scratch) layout in this fn correctly centered.
        let mut cx = TextContext::new();
        let centered = aligned_style(TextAlign::Center);

        // First pass: shapes and breaks from scratch at 400px.
        let _ = cx.layout(TWO_LINES, &centered, Some(400.0));

        // Second pass at a different width: a shape-cache hit that re-breaks
        // (not a fresh shape) — exactly the path `ShapeCache::get` owns.
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
        let mut cx = TextContext::new();
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
        let mut cx = TextContext::new();
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
        let mut cx = TextContext::new();
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
        let mut cx = TextContext::new();
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
        let mut cx = TextContext::new();
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
        let mut cx = TextContext::new();
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
        let mut cx = TextContext::new();
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
        let mut cx = TextContext::new();
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
        let mut cx = TextContext::new();
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
        let mut cx = TextContext::new();
        let s = style(16.0);
        let bounded = cx.layout_bounded("", &s, Some(80.0), Some(1), TextOverflow::Ellipsis);
        assert_eq!(bounded.size().width, 0.0);
    }

    #[test]
    fn max_lines_zero_yields_an_empty_layout() {
        let mut cx = TextContext::new();
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
        let mut cx = TextContext::new();
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
        let mut cx = TextContext::new();
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
        let mut cx = TextContext::new();
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
                let mut cx = TextContext::new();
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
        let mut cx = TextContext::new();
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
            let mut cx = TextContext::new();
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
            let mut cx = TextContext::new();
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
        let mut cx = TextContext::new();
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
        let mut cx = TextContext::new();
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
                    let mut cx = TextContext::new();
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
        let mut cx = TextContext::new();
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
}
