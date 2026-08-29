//! The engine's draw record and its depth counter.
//!
//! A compiled frame is a flat list of [`EngineDraw`]s, each naming a paint, a
//! depth, and the half-open range of strips the compiler generated for it. The
//! strips themselves live in one shared
//! [`StripStorage`](vello_common::strip_generator::StripStorage) so a whole
//! frame uploads as a single instance buffer; a draw only borrows its slice of
//! it.
//!
//! [`EngineDraw`] implements [`Drawable`] so the recorder can fold each draw's
//! bounding box into the enclosing layer's. Per that trait's contract the
//! caller slices the strips before the call, so `bbox` measures exactly the
//! strips it is handed rather than re-applying `strip_range`.

use core::ops::Range;

use vello_common::geometry::RectU16;
use vello_common::paint::Paint;
use vello_common::record::Drawable;
use vello_common::strip::Strip;
use vello_common::util::strip_bbox;

/// One recorded draw: a paint, a depth, and the strips it covers.
///
/// `paint` is a `vello_common` [`Paint`] — either an inline solid colour or an
/// index into the frame's encoded-paint table. Carrying the vello type here
/// (rather than a `peniko::Brush`) is what keeps the GPU-side conversion a
/// plain field read, and is the seam the paint-encoding pass writes into.
#[derive(Debug, Clone)]
pub struct EngineDraw {
    /// Paint applied to every strip in `strip_range`.
    pub paint: Paint,
    /// Painter's-algorithm depth, assigned by [`DepthCounter`].
    ///
    /// The back-most draw of a frame is `0` and each subsequent draw is one
    /// greater, matching the GPU strip instance's `depth_index` contract: a
    /// larger value is nearer the viewer and wins the depth test.
    pub depth: u32,
    /// Half-open range selecting this draw's strips from the frame's shared
    /// strip storage.
    pub strip_range: Range<usize>,
}

impl EngineDraw {
    /// A draw covering `strip_range` with `paint` at `depth`.
    pub fn new(paint: Paint, depth: u32, strip_range: Range<usize>) -> Self {
        Self {
            paint,
            depth,
            strip_range,
        }
    }
}

impl Drawable for EngineDraw {
    fn bbox(&self, strips: &[Strip]) -> Option<RectU16> {
        strip_bbox(strips)
    }
}

/// Hands out the monotonically increasing depths a frame's draws are stamped
/// with.
///
/// Draws are emitted in paint order, so the counter runs from `0` (back-most)
/// upward and never repeats a value inside one frame. It saturates rather than
/// wrapping: a frame with more than `u32::MAX` draws would otherwise reuse a
/// depth and let a later draw lose the depth test to an earlier one, and the
/// compiler has no error to report on the frame path for it.
#[derive(Debug, Default, Clone, Copy)]
pub struct DepthCounter {
    next: u32,
}

impl DepthCounter {
    /// A counter whose first handed-out depth is `0`.
    pub fn new() -> Self {
        Self::default()
    }

    /// The next depth, advancing the counter.
    pub fn advance(&mut self) -> u32 {
        let depth = self.next;
        self.next = self.next.saturating_add(1);
        depth
    }

    /// How many depths have been handed out.
    pub fn count(&self) -> u32 {
        self.next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vello_common::color::palette::css::RED;

    #[test]
    fn depths_start_at_zero_and_increase() {
        let mut counter = DepthCounter::new();
        assert_eq!(counter.advance(), 0);
        assert_eq!(counter.advance(), 1);
        assert_eq!(counter.advance(), 2);
        assert_eq!(counter.count(), 3);
    }

    #[test]
    fn depth_saturates_instead_of_wrapping() {
        let mut counter = DepthCounter { next: u32::MAX };
        assert_eq!(counter.advance(), u32::MAX);
        assert_eq!(counter.advance(), u32::MAX);
    }

    #[test]
    fn a_draw_with_no_strips_has_no_bbox() {
        let draw = EngineDraw::new(Paint::from(RED), 0, 0..0);
        assert!(draw.bbox(&[]).is_none());
    }
}
