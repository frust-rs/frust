//! Padding layout container (spec §6.2): insets a single child.
//!
//! [`PaddingView`]/[`PaddingWidget`] deflate the incoming constraints by the
//! [`EdgeInsets`], lay the child out in the reduced space, offset it by the
//! top-left insets, and report a size of `child + insets` (clamped to the
//! original constraints).

use forgekit_core::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, View, Widget, any,
};
use kurbo::{Point, Size};

/// Per-edge inset amounts, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeInsets {
    /// Inset from the left edge.
    pub left: f64,
    /// Inset from the top edge.
    pub top: f64,
    /// Inset from the right edge.
    pub right: f64,
    /// Inset from the bottom edge.
    pub bottom: f64,
}

impl EdgeInsets {
    /// Equal inset on all four edges.
    pub fn all(value: f64) -> Self {
        Self {
            left: value,
            top: value,
            right: value,
            bottom: value,
        }
    }

    /// Symmetric horizontal (`left == right`) and vertical (`top == bottom`) insets.
    pub fn symmetric(horizontal: f64, vertical: f64) -> Self {
        Self {
            left: horizontal,
            top: vertical,
            right: horizontal,
            bottom: vertical,
        }
    }

    /// Total horizontal inset (`left + right`).
    fn horizontal(&self) -> f64 {
        self.left + self.right
    }

    /// Total vertical inset (`top + bottom`).
    fn vertical(&self) -> f64 {
        self.top + self.bottom
    }
}

/// A declarative padding container. See the [module docs](self).
pub struct PaddingView<State: 'static> {
    insets: EdgeInsets,
    child: forgekit_core::AnyView<State>,
}

/// Inset `child` by `insets` on each edge.
#[allow(non_snake_case)]
pub fn Padding<State: 'static, V: View<State>>(insets: EdgeInsets, child: V) -> PaddingView<State> {
    PaddingView {
        insets,
        child: any(child),
    }
}

/// The retained widget for a [`PaddingView`].
pub struct PaddingWidget {
    insets: EdgeInsets,
    child: ChildPod,
}

impl<State: 'static> View<State> for PaddingView<State> {
    type Element = PaddingWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> PaddingWidget {
        PaddingWidget {
            insets: self.insets,
            child: crate::build_child(&self.child, ctx),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut PaddingWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.insets != self.insets {
            element.insets = self.insets;
            flags |= ChangeFlags::LAYOUT;
        }
        flags |= crate::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        flags
    }

    fn teardown(&self, element: &mut PaddingWidget, ctx: &mut BuildCtx<'_>) {
        crate::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for PaddingWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let h = self.insets.horizontal();
        let v = self.insets.vertical();
        // Deflate the constraints by the insets (never below zero — insets larger
        // than the available space collapse the child to nothing).
        let child_bc = BoxConstraints::new(
            Size::new(
                (bc.min().width - h).max(0.0),
                (bc.min().height - v).max(0.0),
            ),
            Size::new(
                (bc.max().width - h).max(0.0),
                (bc.max().height - v).max(0.0),
            ),
        );
        let child_size = self.child.layout_child(ctx, &child_bc);
        self.child
            .set_origin(Point::new(self.insets.left, self.insets.top));
        bc.constrain(Size::new(child_size.width + h, child_size.height + v))
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::route_event_single(&mut self.child, ctx, event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::leaf;
    use forgekit_core::{BuildCtx, PointerButton, PointerEvent, PointerPhase};
    use std::any::Any;

    fn build<S: 'static>(view: &PaddingView<S>) -> PaddingWidget {
        let mut counter = 0u64;
        view.build(&mut BuildCtx::new(&mut counter))
    }

    #[test]
    fn adds_insets_around_child() {
        // 40x20 child, insets (l=5, t=10, r=15, b=20) → size 60x50, child at (5,10).
        let view: PaddingView<()> = Padding(
            EdgeInsets {
                left: 5.0,
                top: 10.0,
                right: 15.0,
                bottom: 20.0,
            },
            leaf(40.0, 20.0),
        );
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size, Size::new(60.0, 50.0));
        assert_eq!(w.child.origin(), Point::new(5.0, 10.0));
        assert_eq!(w.child.size(), Size::new(40.0, 20.0));
    }

    #[test]
    fn clamps_when_insets_exceed_max() {
        // Uniform 100px inset but only 50x50 available: the child collapses to
        // zero and the padding's own size clamps back to the 50x50 max.
        let view: PaddingView<()> = Padding(EdgeInsets::all(100.0), leaf(30.0, 30.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(50.0, 50.0)));
        assert_eq!(w.child.size(), Size::ZERO);
        assert_eq!(size, Size::new(50.0, 50.0));
    }

    // -- Capture routing (review R1: a captured child must keep receiving
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

    /// Build a `Padding(Button)`, laid out under generous constraints so the
    /// child has real geometry to hit-test/capture against.
    fn button_padding(insets: EdgeInsets) -> PaddingWidget {
        let view: PaddingView<Counter> = Padding(
            insets,
            crate::button::<Counter, _>("go", |s: &mut Counter| s.presses += 1),
        );
        let mut w = build(&view);
        let mut text_ctx = forgekit_text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut text_ctx);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        w
    }

    fn dispatch<S: 'static>(
        w: &mut PaddingWidget,
        state: &mut S,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(500.0, 500.0));
        w.event(&mut ctx, event)
    }

    #[test]
    fn captured_button_receives_move_and_up_outside_its_bounds() {
        let mut w = button_padding(EdgeInsets::all(10.0));
        let mut state = Counter::default();
        let origin = w.child.origin();
        let size = w.child.size();
        assert!(
            size.width > 0.0 && size.height > 0.0,
            "button should have real geometry from layout"
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

        // Move far outside the child's bounds — must still reach the child
        // because it holds the capture, not because the point is over it.
        let outside = Point::new(
            origin.x + size.width + 100.0,
            origin.y + size.height + 100.0,
        );
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
        let mut w = button_padding(EdgeInsets::all(10.0));
        let mut state = Counter::default();
        let origin = w.child.origin();
        let size = w.child.size();

        let inside = Point::new(origin.x + 2.0, origin.y + 2.0);
        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Down, inside.x, inside.y),
        );

        let outside = Point::new(origin.x + size.width + 100.0, origin.y);
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
    fn captured_slider_move_outside_insets_still_reports_value() {
        // Padding(Slider) with insets: the inset margin is space the padding
        // owns but the child does not — a captured drag that strays into it
        // must still reach the slider.
        #[derive(Default)]
        struct Val {
            changes: u32,
        }

        let view: PaddingView<Val> = Padding(
            EdgeInsets::all(20.0),
            crate::slider::<Val, _>(0.0, |s: &mut Val, _v: f64| s.changes += 1),
        );
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new(); // Slider's layout needs no text context.
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        let origin = w.child.origin();

        let mut state = Val::default();
        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Down, origin.x + 10.0, origin.y + 5.0),
        );
        assert_eq!(state.changes, 1);
        assert!(w.child.is_active());

        // A captured Move landing inside the inset margin (outside the child's
        // origin.x) must still fire on_change.
        let outside = Point::new(5.0, origin.y + 5.0);
        assert!(
            !w.child.contains(outside),
            "sanity: the point is outside the child"
        );
        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                &pointer_ev(PointerPhase::Move, outside.x, outside.y)
            ),
            EventResult::Handled
        );
        assert_eq!(
            state.changes, 2,
            "captured Move outside the child must still fire on_change"
        );
    }

    #[test]
    fn active_clears_on_up_so_a_later_down_elsewhere_is_not_routed() {
        let mut w = button_padding(EdgeInsets::all(10.0));
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
                &pointer_ev(PointerPhase::Down, 400.0, 400.0)
            ),
            EventResult::Ignored
        );
    }
}
