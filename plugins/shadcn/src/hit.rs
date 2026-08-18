//! The catalog's two pointer admission tests.
//!
//! Every interactive component in this crate answers the same pair of questions
//! in its `event` arms — "did this pointer land on me?" ([`inside`], against
//! widget-local coordinates and the size `EventCtx` reports) and "may this
//! button start a press?" ([`presses`]) — so the catalog has exactly one answer
//! to each, boundaries included.

use frust::authoring::{Point, PointerButton, PointerEvent, Size};

/// Whether widget-local `pos` lies inside a `size`-shaped box.
///
/// The interval is **half-open** on both axes (`0 <= x < width`,
/// `0 <= y < height`), matching `kurbo::Rect::contains`: the top-left edge is
/// inside, the bottom-right edge is not, so two adjacent boxes sharing a seam
/// never both claim the same pointer.
pub(crate) fn inside(pos: Point, size: Size) -> bool {
    pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.width && pos.y < size.height
}

/// Whether `p` carries a button that may begin a press.
///
/// A press/activation machine — a `pressed` visual, a pointer capture, an
/// up-inside callback — starts on the **primary** button alone: the left mouse
/// button, or any touch/pen contact (which every shell reports as
/// [`PointerButton::Primary`] too). A secondary press is a context gesture:
/// `context_menu`'s trigger owns it, and no other component in this catalog
/// does anything with it. That matches the browser the catalog ports from — a
/// right-click never activates a control — and it is the invariant
/// `RenderRoot`'s capture bookkeeping rests on, since capture release there is
/// phase-only and never button-checked.
///
/// Move/hover arms deliberately do **not** consult this: hover chrome and
/// cursor shapes are position-driven, and a move carries no meaningful button.
pub(crate) fn presses(p: &PointerEvent) -> bool {
    p.button == PointerButton::Primary
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::authoring::Rect;

    #[test]
    fn the_interior_hits_and_anything_outside_misses() {
        let size = Size::new(40.0, 20.0);
        assert!(inside(Point::new(0.0, 0.0), size));
        assert!(inside(Point::new(20.0, 10.0), size));
        assert!(inside(Point::new(39.9, 19.9), size));

        assert!(!inside(Point::new(-0.1, 10.0), size));
        assert!(!inside(Point::new(20.0, -0.1), size));
        assert!(!inside(Point::new(40.1, 10.0), size));
        assert!(!inside(Point::new(20.0, 20.1), size));
    }

    #[test]
    fn the_box_is_half_open_so_a_shared_seam_belongs_to_one_side() {
        let size = Size::new(40.0, 20.0);
        // The far edges are *outside*: a neighbour starting at x = 40 owns them.
        assert!(!inside(Point::new(40.0, 10.0), size));
        assert!(!inside(Point::new(20.0, 20.0), size));
        // ...and the near edges are inside.
        assert!(inside(Point::new(0.0, 10.0), size));
        assert!(inside(Point::new(20.0, 0.0), size));
    }

    #[test]
    fn it_agrees_with_the_kurbo_rect_it_replaces() {
        let size = Size::new(40.0, 20.0);
        let rect = Rect::from_origin_size(Point::ORIGIN, size);
        for x in [-1.0, 0.0, 0.5, 20.0, 39.5, 40.0, 41.0] {
            for y in [-1.0, 0.0, 0.5, 10.0, 19.5, 20.0, 21.0] {
                let p = Point::new(x, y);
                assert_eq!(inside(p, size), rect.contains(p), "at {p:?}");
            }
        }
    }

    #[test]
    fn a_zero_sized_box_can_never_be_hit() {
        assert!(!inside(Point::ORIGIN, Size::ZERO));
    }

    fn at(button: PointerButton) -> PointerEvent {
        PointerEvent {
            phase: frust::authoring::PointerPhase::Down,
            position: Point::ORIGIN,
            button,
        }
    }

    #[test]
    fn only_the_primary_button_presses() {
        assert!(presses(&at(PointerButton::Primary)));
        assert!(!presses(&at(PointerButton::Secondary)));
        assert!(!presses(&at(PointerButton::Middle)));
    }
}
