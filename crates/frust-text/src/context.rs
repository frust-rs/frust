//! [`TextContext`]: the heavyweight, `!Sync` owner of parley's font and layout
//! state.
//!
//! Created once and threaded through the app/render loop (spec §10.3). It holds
//! a [`parley::FontContext`] (system font sources, loaded via fontique — Core
//! Text on macOS with no registration) and a [`parley::LayoutContext`] scratch
//! buffer reused across layout passes.

use parley::fontique::Blob;
use parley::style::StyleProperty;
use peniko::Brush;

use crate::layout::TextLayout;
use crate::shape_cache::{DEFAULT_CAPACITY, ShapeCache, ShapeCacheStats, ShapeKey};
use crate::style::{
    TextStyle, to_parley_family, to_parley_line_height, to_parley_style, to_parley_weight,
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

/// Owns parley's font matching and layout scratch state.
///
/// This is deliberately not `Clone`/`Sync`: it is expensive per-instance state
/// meant to be constructed once and borrowed mutably for each layout pass. The
/// generic brush parameter is fixed to [`peniko::Brush`] so glyph runs carry the
/// same paint vocabulary as [`frust_scene`].
pub struct TextContext {
    font_ctx: parley::FontContext,
    layout_ctx: parley::LayoutContext<Brush>,
    /// Width-independent shape cache (spec §10.3, phase 10.B): a bounded LRU of
    /// shaped layouts keyed by (text, style), so a width change re-runs
    /// line-breaking only and repeated content shapes once. See
    /// [`crate::shape_cache`].
    shape_cache: ShapeCache,
}

impl TextContext {
    /// Builds a context with system fonts available.
    ///
    /// [`parley::FontContext::new`] populates the fontique source collection from
    /// the platform (Core Text on macOS); no manual font registration is
    /// required for the default [`crate::FontFamily::SystemUi`] to resolve.
    pub fn new() -> Self {
        Self {
            font_ctx: parley::FontContext::new(),
            layout_ctx: parley::LayoutContext::new(),
            shape_cache: ShapeCache::new(DEFAULT_CAPACITY),
        }
    }

    /// Lays out `text` with `style`, wrapping to `max_width` when supplied.
    ///
    /// `max_width` is in the same logical-pixel units as `style.size` (the
    /// layout scale is fixed at `1.0` here — physical-pixel scaling is applied
    /// downstream via the scene transform). Passing `None` produces a single
    /// unwrapped line per hard break in `text`. An empty `text` yields a layout
    /// with no glyph runs and a near-zero size.
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
        layout.align(
            parley::layout::Alignment::Start,
            parley::layout::AlignmentOptions::default(),
        );

        self.shape_cache.insert(key, layout.clone(), max_width);
        TextLayout::new(layout)
    }

    /// The shape cache's instrumentation counters (shapes performed,
    /// line-break-only relayouts, full hits, evictions).
    ///
    /// The observable hook the phase-10 text-cache tests assert against, and a
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
    pub fn register_fonts(&mut self, data: Vec<u8>) -> Result<Vec<RegisteredFamily>, FontError> {
        let blob = Blob::from(data);
        let registered = self.font_ctx.collection.register_fonts(blob, None);
        if registered.is_empty() {
            return Err(FontError::NoFacesFound);
        }

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

    #[test]
    fn same_text_two_widths_shapes_once() {
        // The core phase-10.B claim: shaping is width-independent. Laying out
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
}
