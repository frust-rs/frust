//! [`TextLayout`]: a laid-out, line-broken, aligned block of text.
//!
//! Wraps the immutable result of a [`crate::TextContext::layout`] pass and
//! caches its measured size for the widget layout phase.

use kurbo::{Point, Size};
use peniko::Brush;

use forgekit_scene::GlyphRun;

/// A finished text layout: positioned glyph runs plus a cached measured size.
///
/// Produced by [`crate::TextContext::layout`]. It borrows nothing from the
/// context, so it can outlive the layout pass and be measured or converted to
/// scene runs on demand.
pub struct TextLayout {
    layout: parley::Layout<Brush>,
    size: Size,
}

impl TextLayout {
    /// Wraps a finished parley layout, caching its measured size.
    pub(crate) fn new(layout: parley::Layout<Brush>) -> Self {
        let size = Size::new(layout.width() as f64, layout.height() as f64);
        Self { layout, size }
    }

    /// The measured size (width x height) of the laid-out block.
    ///
    /// Width is the longest line's advance (bounded by the layout's
    /// `max_width` when one was given); height is the sum of line heights.
    pub fn size(&self) -> Size {
        self.size
    }

    /// Converts the layout into [`forgekit_scene::GlyphRun`]s placed at `origin`.
    ///
    /// See [`crate::convert`] for the coordinate convention.
    pub fn to_scene_runs(&self, origin: Point) -> Vec<GlyphRun> {
        crate::convert::layout_to_scene_runs(&self.layout, origin)
    }
}
