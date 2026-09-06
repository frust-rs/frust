//! [`TextLayout`]: a laid-out, line-broken, aligned block of text.
//!
//! Wraps the immutable result of a [`crate::TextContext::layout`] pass and
//! caches its measured size for the widget layout phase.

use std::cell::RefCell;
use std::ops::Range;

use kurbo::{Affine, Point, Size};
use peniko::Brush;

use frust_scene::GlyphRun;

/// A source-text byte range plus rendered width for one line of a
/// [`TextLayout`], used only by [`crate::context::TextContext::layout_bounded`]'s
/// truncation walk. `pub(crate)` — no `parley::layout::Line` leaks past
/// [`TextLayout::line_info`].
pub(crate) struct LineInfo {
    /// The line's text range in the *original* source string passed to
    /// [`crate::TextContext::layout`]/`layout_bounded` — parley's
    /// line-breaker assigns each line a contiguous, non-overlapping span, so
    /// concatenating every line's range in order reconstructs the source.
    pub range: Range<usize>,
    /// The line's rendered advance, excluding trailing whitespace (which
    /// collapses at the line edge and would otherwise make a legitimately
    /// fitting line look like it overflows).
    pub width: f32,
}

/// A finished text layout: positioned glyph runs plus a cached measured size.
///
/// Produced by [`crate::TextContext::layout`]. It borrows nothing from the
/// context, so it can outlive the layout pass and be measured or converted to
/// scene runs on demand.
pub struct TextLayout {
    layout: parley::Layout<Brush>,
    size: Size,
    /// Memoized origin-independent glyph runs, built lazily on the first
    /// [`to_scene_runs`](Self::to_scene_runs) call and reused across every
    /// later call while this layout is retained. The only
    /// origin-dependent field of a [`GlyphRun`] is its `transform`, so a paint
    /// at any origin clones these base runs and re-translates rather than
    /// re-walking the parley layout (font matching / `positioned_glyphs`) every
    /// frame. Retaining the [`TextLayout`] across frames — which the
    /// [`Text`](../../frust_widgets/struct.TextWidget.html) widget now does when
    /// content/style/width are unchanged — is what makes this reuse effective.
    base_runs: RefCell<Option<Vec<GlyphRun>>>,
}

impl TextLayout {
    /// Wraps a finished parley layout, caching its measured size.
    pub(crate) fn new(layout: parley::Layout<Brush>) -> Self {
        let size = Size::new(layout.width() as f64, layout.height() as f64);
        Self {
            layout,
            size,
            base_runs: RefCell::new(None),
        }
    }

    /// The measured size (width x height) of the laid-out block.
    ///
    /// Width is the longest line's advance (bounded by the layout's
    /// `max_width` when one was given); height is the sum of line heights.
    pub fn size(&self) -> Size {
        self.size
    }

    /// Converts the layout into [`frust_scene::GlyphRun`]s placed at `origin`.
    ///
    /// See [`crate::convert`] for the coordinate convention. The origin-invariant
    /// run construction (the parley walk) is memoized on first call and reused
    /// across frames (see [`base_runs`](Self::base_runs)); each call re-applies
    /// only the cheap `origin` translation onto the cached runs' transforms.
    ///
    /// A caller supplying its own `run.brush` after this call (a gradient
    /// foreground, say) must resolve that brush's geometry relative to
    /// `origin` too — never as an already-window-space point. The render
    /// backends apply a `GlyphRun`'s `transform` to its active paint, not
    /// just its glyph outlines (vello's `Scene::draw_glyphs(..).transform(t)`
    /// composes `t` with the brush the same way it composes `t` with the
    /// glyphs; `vello_cpu`'s `set_transform` likewise precedes `set_paint`),
    /// so a brush baked with `origin` already added gets it added a second
    /// time at paint time. Every other [`frust_scene::Command`] variant's
    /// `origin`/position argument does *not* auto-translate its brush this
    /// way (see the `frust-material` gradient-button decoration's own
    /// `PaintScene` doc note) — this contract is specific to
    /// [`frust_scene::Command::GlyphRun`].
    pub fn to_scene_runs(&self, origin: Point) -> Vec<GlyphRun> {
        let mut memo = self.base_runs.borrow_mut();
        let base = memo.get_or_insert_with(|| {
            crate::convert::layout_to_scene_runs(&self.layout, Point::ORIGIN)
        });
        let transform = Affine::translate((origin.x, origin.y));
        base.iter()
            .map(|run| GlyphRun {
                transform,
                ..run.clone()
            })
            .collect()
    }

    /// The number of lines in this layout — `0` only for an entirely empty
    /// layout with no lines at all. `pub(crate)` — the `max_lines`/overflow
    /// truncation walk in [`crate::context`] is the sole caller.
    pub(crate) fn line_count(&self) -> usize {
        self.layout.len()
    }

    /// The source text range and rendered (trailing-whitespace-excluded)
    /// width of line `index`, or `None` if out of bounds. `pub(crate)` — see
    /// [`Self::line_count`].
    pub(crate) fn line_info(&self, index: usize) -> Option<LineInfo> {
        let line = self.layout.get(index)?;
        let metrics = line.metrics();
        Some(LineInfo {
            range: line.text_range(),
            width: (metrics.advance - metrics.trailing_whitespace).max(0.0),
        })
    }
}
