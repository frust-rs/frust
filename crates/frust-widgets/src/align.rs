//! Align layout container: positions a single child within the
//! space the align itself is given.
//!
//! [`AlignView`]/[`AlignWidget`] loosen the incoming constraints for the child
//! (so it takes its natural size), grow to fill the available (max) space when it
//! is bounded, and position the child within that box per an [`Alignment`].
//!
//! # Flutter-parity contract
//!
//! Sizing matches Flutter's `RenderPositionedBox`/`RenderAligningShiftedBox`
//! (`packages/flutter/lib/src/rendering/shifted_box.dart`, retrieved
//! 2026-07-19): with no width/height *factor* (Frust does not expose one —
//! do not add it), the align sizes **per axis** to `constraints.biggest` on a
//! **bounded** axis (fill) and to the **child's** extent on an **unbounded**
//! axis (shrink-wrap). This is decided independently on each axis, so a
//! bounded-width / unbounded-height constraint fills horizontally and
//! shrink-wraps vertically. The child is then positioned in the free space per
//! the [`Alignment`] fractions.
//!
//! The practical consequence (the huddle avatar bug shape): centering a small
//! child over a fixed box — `Stack`/`SizedBox` bounding the align, then
//! `Align(CENTER, child)` — centers only because the incoming constraint is
//! *bounded* (the bounding parent supplies the max). Under an *unbounded*
//! constraint the align shrink-wraps to the child and there is no free space to
//! center within, so the child lands at the origin. Give the align a bounded
//! box (e.g. wrap in [`crate::SizedBox`]) when centered content over a larger
//! region is the intent — matching Flutter, where an unbounded `Align` is
//! likewise a no-op for positioning.

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget, any,
};
use kurbo::{Point, Size};

/// A relative alignment within a box: each axis runs `-1.0` (start) through
/// `0.0` (center) to `1.0` (end).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Alignment {
    /// Horizontal position: `-1.0` = left, `0.0` = center, `1.0` = right.
    pub x: f64,
    /// Vertical position: `-1.0` = top, `0.0` = center, `1.0` = bottom.
    pub y: f64,
}

impl Alignment {
    /// Center of the box.
    pub const CENTER: Self = Self { x: 0.0, y: 0.0 };
    /// Top-left corner.
    pub const TOP_LEFT: Self = Self { x: -1.0, y: -1.0 };
    /// Top-right corner.
    pub const TOP_RIGHT: Self = Self { x: 1.0, y: -1.0 };
    /// Bottom-left corner.
    pub const BOTTOM_LEFT: Self = Self { x: -1.0, y: 1.0 };
    /// Bottom-right corner.
    pub const BOTTOM_RIGHT: Self = Self { x: 1.0, y: 1.0 };

    /// A custom alignment with `x`/`y` each in `-1.0..=1.0`.
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Map an alignment component in `-1.0..=1.0` to a `0.0..=1.0` fraction of
    /// the free space (`-1 → 0`, `0 → 0.5`, `1 → 1`).
    fn fraction(component: f64) -> f64 {
        (component + 1.0) / 2.0
    }
}

/// A declarative alignment container. See the [module docs](self).
pub struct AlignView<State: 'static> {
    alignment: Alignment,
    child: AnyView<State>,
}

/// Position `child` within the align's box per `alignment`.
#[allow(non_snake_case)]
pub fn Align<State: 'static, V: View<State>>(alignment: Alignment, child: V) -> AlignView<State> {
    AlignView {
        alignment,
        child: any(child),
    }
}

/// The retained widget for an [`AlignView`].
pub struct AlignWidget {
    alignment: Alignment,
    child: ChildPod,
}

impl<State: 'static> View<State> for AlignView<State> {
    type Element = AlignWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> AlignWidget {
        AlignWidget {
            alignment: self.alignment,
            child: crate::build_child(&self.child, ctx),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut AlignWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.alignment != self.alignment {
            element.alignment = self.alignment;
            flags |= ChangeFlags::LAYOUT;
        }
        flags |= crate::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        flags
    }

    fn teardown(&self, element: &mut AlignWidget, ctx: &mut BuildCtx<'_>) {
        crate::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for AlignWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // The child takes its natural size under loosened constraints.
        let child_size = self.child.layout_child(ctx, &bc.loosen());
        // Flutter parity (shifted_box.dart's RenderPositionedBox, retrieved
        // 2026-07-19; see module docs): fill the available space on each bounded
        // axis, shrink-wrap an unbounded one to the child — decided per axis.
        let width = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            child_size.width
        };
        let height = if bc.max().height.is_finite() {
            bc.max().height
        } else {
            child_size.height
        };
        let size = bc.constrain(Size::new(width, height));
        // Position the child within the free space per the alignment fractions.
        let x = (size.width - child_size.width) * Alignment::fraction(self.alignment.x);
        let y = (size.height - child_size.height) * Alignment::fraction(self.alignment.y);
        self.child.set_origin(Point::new(x, y));
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Transparent positioning wrapper: forward to the single child.
        self.child.semantics_child(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{RecordingScene, leaf, leaf_any};
    use frust_core::{BuildCtx, PointerButton, PointerEvent, PointerPhase};
    use std::any::Any;

    fn build<S: 'static>(view: &AlignView<S>) -> AlignWidget {
        let mut counter = 0u64;
        view.build(&mut BuildCtx::new(&mut counter))
    }

    fn placed(alignment: Alignment) -> (Size, Point) {
        // 20x20 child inside a 100x100 align box.
        let view: AlignView<()> = Align(alignment, leaf(20.0, 20.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::tight(Size::new(100.0, 100.0)));
        (size, w.child.origin())
    }

    #[test]
    fn fills_box_and_centers_child() {
        let (size, origin) = placed(Alignment::CENTER);
        assert_eq!(size, Size::new(100.0, 100.0));
        // (100 - 20) * 0.5 = 40 on each axis.
        assert_eq!(origin, Point::new(40.0, 40.0));
    }

    #[test]
    fn positions_child_at_corners() {
        assert_eq!(placed(Alignment::TOP_LEFT).1, Point::new(0.0, 0.0));
        assert_eq!(placed(Alignment::TOP_RIGHT).1, Point::new(80.0, 0.0));
        assert_eq!(placed(Alignment::BOTTOM_LEFT).1, Point::new(0.0, 80.0));
        assert_eq!(placed(Alignment::BOTTOM_RIGHT).1, Point::new(80.0, 80.0));
    }

    // -- Flutter-parity sizing (shifted_box.dart's RenderPositionedBox): fill a
    // bounded axis, shrink-wrap an unbounded one, per axis. --

    /// Lay a 20x20 child inside an align under arbitrary `bc` and report the
    /// align's chosen size and the child's origin within it.
    fn placed_bc(alignment: Alignment, bc: &BoxConstraints) -> (Size, Point) {
        let view: AlignView<()> = Align(alignment, leaf(20.0, 20.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, bc);
        (size, w.child.origin())
    }

    #[test]
    fn bounded_loose_fills_and_centers_child() {
        // A *loose* (min 0) but bounded constraint must fill to the max on both
        // axes and center the child — the case that bit the huddle avatars.
        let (size, origin) = placed_bc(
            Alignment::CENTER,
            &BoxConstraints::loose(Size::new(100.0, 100.0)),
        );
        assert_eq!(size, Size::new(100.0, 100.0), "bounded axes fill to max");
        assert_eq!(origin, Point::new(40.0, 40.0), "(100-20)*0.5 on each axis");
    }

    #[test]
    fn unbounded_shrink_wraps_to_child() {
        // Both axes unbounded: the align shrink-wraps to the child, leaving no
        // free space, so the child sits at the origin regardless of alignment.
        let bc = BoxConstraints::loose(Size::new(f64::INFINITY, f64::INFINITY));
        let (size, origin) = placed_bc(Alignment::CENTER, &bc);
        assert_eq!(
            size,
            Size::new(20.0, 20.0),
            "unbounded axes shrink to child"
        );
        assert_eq!(origin, Point::ZERO, "no free space to align within");
    }

    #[test]
    fn mixed_axis_fills_bounded_shrinks_unbounded() {
        // Bounded width, unbounded height: fill horizontally (100), shrink-wrap
        // vertically (20). The decision is independent per axis.
        let bc = BoxConstraints::new(Size::ZERO, Size::new(100.0, f64::INFINITY));
        let (size, origin) = placed_bc(Alignment::CENTER, &bc);
        assert_eq!(size, Size::new(100.0, 20.0));
        // x centered over the 100px width; y has no slack to center within.
        assert_eq!(origin, Point::new(40.0, 0.0));
    }

    #[test]
    fn tight_passes_through() {
        // A tight constraint forces the align's size exactly, both axes bounded.
        let (size, origin) = placed_bc(
            Alignment::CENTER,
            &BoxConstraints::tight(Size::new(60.0, 60.0)),
        );
        assert_eq!(size, Size::new(60.0, 60.0));
        assert_eq!(origin, Point::new(20.0, 20.0), "(60-20)*0.5");
    }

    #[test]
    fn stack_align_center_positions_small_child_over_larger_sibling() {
        // Regression for the huddle avatar bug shape: a Stack bounded to the
        // "circle" size (40x40, as a SizedBox parent would supply) overlays a
        // 40x40 sibling with `Align(CENTER, <20x20 child>)`. Because the Stack
        // hands its children a *bounded* loose constraint, the align fills 40x40
        // and centers the child at (10,10) — not shrink-wrapped to the origin,
        // which is what pinned the avatar initials to the corner.
        let view: crate::StackView<()> = crate::Stack(vec![
            leaf_any(40.0, 40.0),
            any(Align(Alignment::CENTER, leaf(20.0, 20.0))),
        ]);
        let mut counter = 0u64;
        let mut w = view.build(&mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(40.0, 40.0)));
        assert_eq!(size, Size::new(40.0, 40.0));

        // Paint into a recording scene: each leaf fills a rect at its absolute
        // origin, so the child's centered placement is observable end-to-end
        // through the real Stack -> Align -> child origin threading.
        let mut scene = RecordingScene::default();
        let mut pctx = PaintCtx::new(Point::ZERO, size);
        w.paint(&mut pctx, &mut scene);

        assert!(
            scene
                .rects
                .contains(&(Point::new(10.0, 10.0), Size::new(20.0, 20.0))),
            "the 20x20 child must center at (10,10) over the 40x40 box, not sit \
             at the origin; recorded rects: {:?}",
            scene.rects,
        );
    }

    // -- Capture routing (a captured child must keep receiving
    // events regardless of hit geometry, not just while the point is still
    // over it) --

    #[derive(Default)]
    struct Counter {
        presses: u32,
    }

    fn pointer_ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
        })
    }

    /// Build a `Align(CENTER, Button)` inside a generous box, so the child is
    /// surrounded by free space it does not occupy.
    fn button_align() -> AlignWidget {
        let view: AlignView<Counter> = Align(
            Alignment::CENTER,
            crate::button::<Counter, _>("go", |s: &mut Counter| s.presses += 1),
        );
        let mut w = build(&view);
        let mut text_ctx = frust_text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut text_ctx);
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(500.0, 500.0)));
        w
    }

    fn dispatch<S: 'static>(w: &mut AlignWidget, state: &mut S, event: &InputEvent) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(500.0, 500.0));
        w.event(&mut ctx, event)
    }

    #[test]
    fn captured_button_receives_move_and_up_outside_its_bounds() {
        let mut w = button_align();
        let mut state = Counter::default();
        let origin = w.child.origin();
        assert!(
            origin.x > 0.0 && origin.y > 0.0,
            "the centered button should have free space around it"
        );

        let inside = Point::new(origin.x + 2.0, origin.y + 2.0);
        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                &pointer_ev(PointerPhase::Down, inside.x, inside.y)
            ),
            EventResult::Handled
        );
        assert!(w.child.is_active(), "down captures the pointer");

        // Move to the corner of the 500x500 box — well outside the centered
        // child — the capture must still route it there.
        let outside = Point::new(1.0, 1.0);
        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                &pointer_ev(PointerPhase::Move, outside.x, outside.y)
            ),
            EventResult::Handled,
            "a captured Move outside the child's bounds must still route to it"
        );

        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                &pointer_ev(PointerPhase::Up, outside.x, outside.y)
            ),
            EventResult::Handled,
            "the capturing Up must still route to the child even though it's outside"
        );
        assert_eq!(
            state.presses, 0,
            "up outside the button must not fire on_press"
        );
        assert!(!w.child.is_active(), "capture releases on Up");
    }

    #[test]
    fn captured_button_fires_on_move_back_inside_then_up() {
        let mut w = button_align();
        let mut state = Counter::default();
        let origin = w.child.origin();

        let inside = Point::new(origin.x + 2.0, origin.y + 2.0);
        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Down, inside.x, inside.y),
        );

        let outside = Point::new(1.0, 1.0);
        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Move, outside.x, outside.y),
        );

        let back_inside = Point::new(origin.x + 4.0, origin.y + 4.0);
        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Move, back_inside.x, back_inside.y),
        );

        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Up, back_inside.x, back_inside.y),
        );
        assert_eq!(state.presses, 1, "up back inside must fire on_press");
    }

    #[test]
    fn active_clears_on_up_so_a_later_down_elsewhere_is_not_routed() {
        let mut w = button_align();
        let mut state = Counter::default();
        let origin = w.child.origin();

        let inside = Point::new(origin.x + 2.0, origin.y + 2.0);
        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Down, inside.x, inside.y),
        );
        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Up, inside.x, inside.y),
        );
        assert!(!w.child.is_active(), "capture releases on Up");

        // A Down far away, outside the child's bounds, must now be ignored —
        // not routed to the (no-longer-active) child.
        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                &pointer_ev(PointerPhase::Down, 1.0, 1.0)
            ),
            EventResult::Ignored
        );
    }

    // -- Type-swap capture clearing (`rebuild_child` must
    // clear a stale capture on an AnyView type swap, matching
    // `rebuild_children`'s semantics for `Flex`/`Stack`) --

    #[test]
    fn type_swap_at_captured_child_clears_active_and_stops_routing() {
        let mut counter = 0u64;
        let prev: AlignView<Counter> = Align(
            Alignment::CENTER,
            crate::button::<Counter, _>("go", |s: &mut Counter| s.presses += 1),
        );
        let mut w = prev.build(&mut BuildCtx::new(&mut counter));
        let mut text_ctx = frust_text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut text_ctx);
        w.layout(&mut lctx, &BoxConstraints::tight(Size::new(500.0, 500.0)));

        let mut state = Counter::default();
        let origin = w.child.origin();
        let inside = Point::new(origin.x + 2.0, origin.y + 2.0);
        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Down, inside.x, inside.y),
        );
        assert!(w.child.is_active(), "down captures the pointer");

        // Rebuild, swapping the child's concrete type (Button -> Checkbox).
        let swapped: AlignView<Counter> = Align(
            Alignment::CENTER,
            crate::checkbox::<Counter, _>(false, "swapped", |_s: &mut Counter, _v: bool| {}),
        );
        swapped.rebuild(&prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(
            !w.child.is_active(),
            "the type swap must clear the stale capture"
        );

        // A Down outside the child's bounds must now be Ignored — not routed
        // to the fresh widget via a stale `active` flag.
        let outside = Point::new(1.0, 1.0);
        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                &pointer_ev(PointerPhase::Down, outside.x, outside.y)
            ),
            EventResult::Ignored,
            "a swap must clear active so an out-of-bounds Down is not delivered"
        );
    }
}
