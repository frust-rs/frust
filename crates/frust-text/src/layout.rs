//! [`TextLayout`]: a laid-out, line-broken, aligned block of text.
//!
//! Wraps the immutable result of a [`crate::TextContext::layout`] pass and
//! caches its measured size for the widget layout phase.

use std::cell::RefCell;

use kurbo::{Affine, Point, Size};
use peniko::Brush;

use frust_scene::GlyphRun;

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
}
