//! `ClearRect`: the platform-view hole punch, hoisted to the frame root and
//! lowered to destination-out coverage.
//!
//! `frust_scene`'s `ClearRect` erases an axis-aligned rectangle to full
//! transparency, colour and alpha both, so an OS view hosted behind a
//! translucent surface shows through a slot the app's own opaque backdrop
//! would otherwise seal. It is a real destination-clearing composite, not a
//! skipped paint.
//!
//! ## Hoisting
//!
//! A punch confined to the group it was recorded in would only erase that
//! group's own accumulated content — an app-root backdrop painted outside a
//! scroll view's clip survives the group composite and seals the hole again.
//! So the punch is hoisted past every enclosing clip, layer and snapshot
//! bracket to the frame root, bounded by the intersection of those brackets'
//! device-space bounds so a partially scrolled slot still clips to its
//! viewport ([`punch_rect`]). At the root there is nothing left to hoist past,
//! and the intersection is the punch itself.
//!
//! ## Destination-out, not a clear
//!
//! The lowered form is coverage drawn with `dst' = dst · (1 − src.a)`, which
//! erases colour and alpha exactly where the source covers and leaves an
//! uncovered pixel bit-identical. Deliberately not a clear op: a clear applies
//! at whole-tile granularity, which bleeds the punch up to a tile past a
//! tile-unaligned rectangle edge. Destination-out weights the erase by the
//! source's own coverage instead, so a pixel-aligned edge lands pixel-exact
//! however it falls inside a tile, and a fully covered pixel reads exactly
//! `(0, 0, 0, 0)` on a target that carries alpha.
//!
//! ## The wiring contract
//!
//! This module produces the lowered form and stops there — a
//! [`ClearPunch`](ClearPunch) per surviving punch, naming the strips that
//! carry its coverage, the device bounds they cover, and the painter-order
//! depth it was hoisted from. Issuing them is the renderer's, and is specified
//! here so the pass that grows it has one place to read:
//!
//! 1. **One pass, after the root round.** The punches are issued together,
//!    into the frame's own target, once every draw of the root round has been
//!    recorded. They never touch an intermediate page: a punch inside an
//!    isolated layer was hoisted out of it.
//! 2. **Fixed-function destination-out, over the strip program.** A punch is a
//!    strip run like any other, so it goes through the same strip program and
//!    instance layout; only the blend state differs — `src_factor: Zero`,
//!    `dst_factor: OneMinusSrcAlpha`, `operation: Add`, for the colour *and*
//!    the alpha component. That computes `dst · (1 − src.a)`, which is the
//!    `COMPOSE_DEST_OUT` arm of `shaders/blend.wgsl` evaluated for a
//!    premultiplied source, without a shader-side composite. It has to be
//!    fixed-function here: the blend module reads its backdrop with
//!    `textureLoad`, and the frame's own target is not readable inside the
//!    pass that writes it. `shaders/blend.wgsl` remains the route for a
//!    composite *between* intermediate textures.
//! 3. **Depth-tested against opaque coverage, and against nothing else.** A
//!    punch carries the depth it was hoisted from, and the ordinary `LessEqual`
//!    test against the frame's depth attachment is what keeps content painted
//!    over the slot from being erased along with what is under it. That test
//!    can only order the punch against paint that *wrote* the depth
//!    attachment, and one pass writes it: the root round's opaque pass, fed the
//!    fully covered spans of opaque draws. So the contract is narrower than the
//!    recorded paint order restored — it is:
//!
//!    - opaque root-round coverage recorded *before* the clear is erased;
//!    - opaque root-round coverage recorded *after* it survives, because it
//!      wrote a nearer depth the punch fails against;
//!    - everything else recorded after it is erased anyway. A layer's composite,
//!      an anti-aliased edge, any span of a translucent paint, the punch of
//!      another `ClearRect` — none of them write depth, so the depth under them
//!      is whatever the opaque pass left, and the punch passes over it.
//!
//!    A translucent widget drawn over a platform-view slot is therefore punched
//!    through, not composited over the hole. Without a depth attachment at all
//!    the pass erases everything under the punch rectangle, the same
//!    correctness/ordering trade the frame's two draw passes already make when
//!    they collapse into one.
//! 4. **Skipped whole on a target that disregards alpha.** Destination-out
//!    darkens colour as well as erasing alpha, so on an opaque presentation —
//!    where the erased alpha is disregarded — issuing the pass would leave a
//!    black rectangle where the display list says nothing should change. The
//!    punches are kept out of [`CompiledFrame::draws`](super::CompiledFrame)
//!    for exactly this reason: dropping the pass restores the frame exactly,
//!    which is the no-op the display list mandates there.

use core::ops::Range;

use kurbo::{Affine, Rect};
use vello_common::geometry::RectU16;

/// One hoisted clear, ready to be issued as destination-out coverage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearPunch {
    /// Half-open range selecting this punch's coverage from the frame's shared
    /// strip storage.
    ///
    /// The strips are generated after the frame's draws and referenced by no
    /// draw, so a renderer that drops the punch pass reads the frame exactly
    /// as if the clear had never been recorded.
    pub strip_range: Range<usize>,
    /// The device-space rectangle the punch covers, clamped to the grid the
    /// strip pipeline addresses.
    ///
    /// The coverage in `strip_range` is authoritative for *which* pixels are
    /// erased; this is the pass's own extent, for a scissor or a bounds check.
    pub bounds: RectU16,
    /// The painter-order depth the punch was hoisted from.
    ///
    /// A depth of its own rather than the depth of a neighbouring draw, so
    /// every draw recorded after the clear sits strictly in front of it. Which
    /// of those draws the depth test then actually saves is narrower than that
    /// ordering suggests — only depth-writing opaque coverage — for the reason
    /// the module header's wiring contract gives.
    pub depth: u32,
}

/// A punch the walk has hoisted, held until the frame's draws are done.
///
/// The rectangle is already in device space — the hoist happens where the
/// clear was recorded, because that is the only place the brackets it is
/// confined by are still open — while the coverage for it is generated at the
/// end of the frame, where it belongs in the strip storage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StagedPunch {
    /// The device-space rectangle to cover.
    pub device: Rect,
    /// The painter-order depth the punch was hoisted from.
    pub depth: u32,
}

/// The device-space rectangle a clear punches, confined to every bracket open
/// around it, or `None` when nothing of it survives.
///
/// `transform` is the clear's own composed transform and `groups` the
/// device-space bounds of the open brackets, in any order. Intersecting rather
/// than clipping is what makes the hoist safe: the punch leaves its brackets
/// behind, so the only thing it can keep from them is the region they admitted
/// it to.
#[must_use]
pub fn punch_rect(
    rect: Rect,
    transform: Affine,
    groups: impl Iterator<Item = Rect>,
) -> Option<Rect> {
    let mut punch = transform.transform_rect_bbox(rect);
    for bounds in groups {
        punch = punch.intersect(bounds);
    }

    (punch.width() > 0.0 && punch.height() > 0.0).then_some(punch)
}

/// A device-space rectangle on the `u16` grid the strip pipeline addresses.
///
/// Clamped rather than refused, so a punch reaching far outside the viewport
/// bounds the pass at the grid's edge instead of wrapping. Kept here rather
/// than shared with the clip stack's own clamp: that one converts a rectangle
/// already proven pixel-aligned, and this one converts an arbitrary punch.
#[must_use]
pub fn device_bounds(rect: Rect) -> RectU16 {
    let coordinate = |value: f64| value.clamp(0.0, f64::from(u16::MAX)) as u16;
    let x0 = coordinate(rect.x0);
    let y0 = coordinate(rect.y0);
    RectU16::new(
        x0,
        y0,
        coordinate(rect.x1).max(x0),
        coordinate(rect.y1).max(y0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTER: Rect = Rect::new(0.0, 0.0, 40.0, 40.0);
    const INNER: Rect = Rect::new(10.0, 10.0, 30.0, 30.0);

    #[test]
    fn a_punch_at_the_root_is_its_own_transformed_rectangle() {
        let punch = punch_rect(
            Rect::new(2.0, 4.0, 6.0, 8.0),
            Affine::translate((10.0, 20.0)),
            core::iter::empty(),
        );
        assert_eq!(punch, Some(Rect::new(12.0, 24.0, 16.0, 28.0)));
    }

    #[test]
    fn a_punch_is_confined_to_every_bracket_it_is_hoisted_past() {
        let punch = punch_rect(
            Rect::new(0.0, 0.0, 40.0, 40.0),
            Affine::IDENTITY,
            [OUTER, INNER].into_iter(),
        );
        assert_eq!(punch, Some(INNER));
    }

    #[test]
    fn a_punch_no_open_bracket_admits_survives_nowhere() {
        let punch = punch_rect(
            Rect::new(34.0, 34.0, 40.0, 40.0),
            Affine::IDENTITY,
            [INNER].into_iter(),
        );
        assert_eq!(punch, None);
    }

    #[test]
    fn a_degenerate_punch_survives_nowhere() {
        let empty = punch_rect(
            Rect::new(4.0, 4.0, 4.0, 12.0),
            Affine::IDENTITY,
            core::iter::empty(),
        );
        assert_eq!(empty, None);
    }

    #[test]
    fn an_enormous_punch_clamps_rather_than_wrapping() {
        assert_eq!(
            device_bounds(Rect::new(-1e30, -1e30, 1e30, 1e30)),
            RectU16::new(0, 0, u16::MAX, u16::MAX)
        );
        assert!(device_bounds(Rect::new(-1e30, -1e30, -1e29, -1e29)).is_empty());
    }
}
