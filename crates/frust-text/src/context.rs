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

use std::sync::Mutex;

use parley::fontique::Blob;
use parley::style::StyleProperty;
use peniko::Brush;

use crate::layout::TextLayout;
use crate::shape_cache::{DEFAULT_CAPACITY, ShapeCache, ShapeCacheStats, ShapeKey};
use crate::style::{
    TextStyle, to_parley_align, to_parley_family, to_parley_line_height, to_parley_style,
    to_parley_weight,
};

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
    /// context does, however late it is constructed — see [`APP_FONTS`].
    pub fn new() -> Self {
        let mut cx = Self {
            font_ctx: parley::FontContext::new(),
            layout_ctx: parley::LayoutContext::new(),
            shape_cache: ShapeCache::new(DEFAULT_CAPACITY),
            app_fonts_applied: 0,
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

        // SystemUi resolves to the platform UI font (e.g. San Francisco on
        // macOS) with no registration; see the task's verified-parley notes.
        builder.push_default(to_parley_family(&style.family));
        builder.push_default(StyleProperty::FontSize(style.size));
        builder.push_default(StyleProperty::FontWeight(to_parley_weight(style.weight)));
        builder.push_default(StyleProperty::FontStyle(to_parley_style(style.style)));
        builder.push_default(StyleProperty::LetterSpacing(style.letter_spacing));
        builder.push_default(StyleProperty::LineHeight(to_parley_line_height(
            style.line_height,
        )));
        builder.push_default(StyleProperty::Brush(Brush::Solid(style.color)));

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

    /// Registers every app font this context is missing (see [`APP_FONTS`]),
    /// returning whether at least one face actually registered.
    ///
    /// Cheap when there is nothing to do — one lock and a length compare, no
    /// allocation and no font parsing — so a widget owning a private context
    /// can call it once per layout pass to pick up a font registered *after*
    /// the context was built (the shells' per-frame late drain).
    ///
    /// A `true` return carries the same caller-visible relayout contract as
    /// [`Self::register_fonts`]: this context's shape cache is cleared, but a
    /// layout retained *outside* it (a [`crate::TextEditor`]'s parley layout)
    /// must be re-shaped by its owner.
    pub fn sync_app_fonts(&mut self) -> bool {
        let slot = APP_FONTS.lock().unwrap_or_else(|e| e.into_inner());
        if self.app_fonts_applied == slot.len() {
            return false;
        }
        let mut applied = false;
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
        drop(slot);

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
}
