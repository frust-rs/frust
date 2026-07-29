//! SizedBox layout container: forces a specific size on the given
//! axes, passing the others through.
//!
//! [`SizedBoxView`]/[`SizedBoxWidget`] tighten each axis for which a `width`/
//! `height` is set (clamped into the incoming constraints) and leave the other
//! axis untouched. A `SizedBox` may be childless — a fixed-size spacer.

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, SemanticsCtx, View, Widget, any,
};
use kurbo::{Point, Size};

/// A declarative fixed-size box, optionally wrapping a child. See the
/// [module docs](self).
pub struct SizedBoxView<State: 'static> {
    width: Option<f64>,
    height: Option<f64>,
    child: Option<AnyView<State>>,
}

/// A box that forces `width`/`height` where `Some`, passing through where `None`.
///
/// Childless by default (a spacer); attach content with [`SizedBoxView::child`].
#[allow(non_snake_case)]
pub fn SizedBox<State: 'static>(width: Option<f64>, height: Option<f64>) -> SizedBoxView<State> {
    SizedBoxView {
        width,
        height,
        child: None,
    }
}

impl<State: 'static> SizedBoxView<State> {
    /// Attach a child sized by this box.
    pub fn child<V: View<State>>(mut self, child: V) -> Self {
        self.child = Some(any(child));
        self
    }
}

/// The retained widget for a [`SizedBoxView`].
pub struct SizedBoxWidget {
    width: Option<f64>,
    height: Option<f64>,
    child: Option<ChildPod>,
}

impl<State: 'static> View<State> for SizedBoxView<State> {
    type Element = SizedBoxWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SizedBoxWidget {
        SizedBoxWidget {
            width: self.width,
            height: self.height,
            child: self
                .child
                .as_ref()
                .map(|view| crate::build_child(view, ctx)),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SizedBoxWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let mut flags = ChangeFlags::NONE;
        if prev.width != self.width || prev.height != self.height {
            element.width = self.width;
            element.height = self.height;
            flags |= ChangeFlags::LAYOUT;
        }
        match (&prev.child, &self.child, &mut element.child) {
            (Some(prev_view), Some(next_view), Some(pod)) => {
                flags |= crate::rebuild_child(prev_view, next_view, pod, ctx);
            }
            (None, Some(next_view), _) => {
                element.child = Some(crate::build_child(next_view, ctx));
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            (Some(prev_view), None, Some(pod)) => {
                crate::teardown_child(prev_view, pod, ctx);
                element.child = None;
                flags |= ChangeFlags::LAYOUT | ChangeFlags::PAINT;
            }
            _ => {}
        }
        flags
    }

    fn teardown(&self, element: &mut SizedBoxWidget, ctx: &mut BuildCtx<'_>) {
        if let (Some(view), Some(pod)) = (&self.child, &mut element.child) {
            crate::teardown_child(view, pod, ctx);
        }
    }
}

impl Widget for SizedBoxWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        // Resolve each requested axis, clamped into the incoming constraints.
        let target_w = self.width.map(|w| w.clamp(bc.min().width, bc.max().width));
        let target_h = self
            .height
            .map(|h| h.clamp(bc.min().height, bc.max().height));

        // Tighten the requested axes; pass the rest of the constraint through.
        let child_bc = BoxConstraints::new(
            Size::new(
                target_w.unwrap_or(bc.min().width),
                target_h.unwrap_or(bc.min().height),
            ),
            Size::new(
                target_w.unwrap_or(bc.max().width),
                target_h.unwrap_or(bc.max().height),
            ),
        );

        let size = if let Some(pod) = &mut self.child {
            let child_size = pod.layout_child(ctx, &child_bc);
            pod.set_origin(Point::ZERO);
            // A tightened axis wins; an untightened axis takes the child's choice.
            Size::new(
                target_w.unwrap_or(child_size.width),
                target_h.unwrap_or(child_size.height),
            )
        } else {
            // Childless spacer: tightened axes, else collapse to the minimum.
            Size::new(
                target_w.unwrap_or(bc.min().width),
                target_h.unwrap_or(bc.min().height),
            )
        };
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        if let Some(pod) = &mut self.child {
            pod.paint_child(ctx, scene);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        match &mut self.child {
            Some(pod) => crate::route_event_single(pod, ctx, event),
            None => EventResult::Ignored,
        }
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Transparent fixed-size wrapper: forward to the child if any (a
        // childless spacer contributes nothing).
        if let Some(pod) = &self.child {
            pod.semantics_child(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::leaf;
    use frust_core::{BuildCtx, PointerButton, PointerEvent, PointerPhase};
    use std::any::Any;

    fn build<S: 'static>(view: &SizedBoxView<S>) -> SizedBoxWidget {
        let mut counter = 0u64;
        view.build(&mut BuildCtx::new(&mut counter))
    }

    #[test]
    fn tightens_both_axes_over_child_intrinsic() {
        // Both axes forced to 40x40, overriding the child's 200x10 intrinsic.
        let view: SizedBoxView<()> = SizedBox(Some(40.0), Some(40.0)).child(leaf(200.0, 10.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size, Size::new(40.0, 40.0));
        assert_eq!(w.child.as_ref().unwrap().size(), Size::new(40.0, 40.0));
    }

    #[test]
    fn passes_through_the_unspecified_axis() {
        // Only width is forced (to 40); height passes through to the child's 30.
        let view: SizedBoxView<()> = SizedBox(Some(40.0), None).child(leaf(200.0, 30.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size, Size::new(40.0, 30.0));
    }

    #[test]
    fn childless_box_is_a_fixed_size_spacer() {
        let view: SizedBoxView<()> = SizedBox(Some(16.0), Some(24.0));
        let mut w = build(&view);
        let mut lctx = LayoutCtx::new();
        let size = w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        assert_eq!(size, Size::new(16.0, 24.0));
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

    /// Build a `SizedBox(Button)` with a forced width wider than the child
    /// needs, laid out with a real text context.
    fn button_sized_box() -> SizedBoxWidget {
        let view: SizedBoxView<Counter> =
            SizedBox(Some(300.0), None)
                .child(crate::button::<Counter, _>("go", |s: &mut Counter| {
                    s.presses += 1
                }));
        let mut w = build(&view);
        let mut text_ctx = frust_text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut text_ctx);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));
        w
    }

    fn dispatch<S: 'static>(
        w: &mut SizedBoxWidget,
        state: &mut S,
        event: &InputEvent,
    ) -> EventResult {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(500.0, 500.0));
        w.event(&mut ctx, event)
    }

    #[test]
    fn captured_button_receives_move_and_up_outside_its_bounds() {
        let mut w = button_sized_box();
        let mut state = Counter::default();
        let size = w.child.as_ref().unwrap().size();
        assert!(size.width > 0.0 && size.height > 0.0);

        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                &pointer_ev(PointerPhase::Down, 2.0, 2.0)
            ),
            EventResult::Handled
        );
        assert!(
            w.child.as_ref().unwrap().is_active(),
            "down captures the pointer"
        );

        // Move well past the child's bounds — the capture must still route it
        // there.
        let outside = Point::new(size.width + 100.0, size.height + 100.0);
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
        assert!(
            !w.child.as_ref().unwrap().is_active(),
            "capture releases on Up"
        );
    }

    #[test]
    fn captured_button_fires_on_move_back_inside_then_up() {
        let mut w = button_sized_box();
        let mut state = Counter::default();
        let size = w.child.as_ref().unwrap().size();

        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Down, 2.0, 2.0),
        );

        let outside = Point::new(size.width + 100.0, 2.0);
        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Move, outside.x, outside.y),
        );

        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Move, 4.0, 4.0),
        );
        dispatch(&mut w, &mut state, &pointer_ev(PointerPhase::Up, 4.0, 4.0));
        assert_eq!(state.presses, 1, "up back inside must fire on_press");
    }

    #[test]
    fn active_clears_on_up_so_a_later_down_elsewhere_is_not_routed() {
        let mut w = button_sized_box();
        let mut state = Counter::default();

        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Down, 2.0, 2.0),
        );
        dispatch(&mut w, &mut state, &pointer_ev(PointerPhase::Up, 2.0, 2.0));
        assert!(
            !w.child.as_ref().unwrap().is_active(),
            "capture releases on Up"
        );

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

    #[test]
    fn childless_box_ignores_events_without_panicking() {
        let view: SizedBoxView<()> = SizedBox(Some(16.0), Some(24.0));
        let mut w = build(&view);
        let mut state = ();
        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                &pointer_ev(PointerPhase::Down, 5.0, 5.0)
            ),
            EventResult::Ignored
        );
        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                &pointer_ev(PointerPhase::Move, 5.0, 5.0)
            ),
            EventResult::Ignored
        );
        assert_eq!(
            dispatch(&mut w, &mut state, &pointer_ev(PointerPhase::Up, 5.0, 5.0)),
            EventResult::Ignored
        );
    }

    // -- Type-swap capture clearing (re-review round 1: `rebuild_child` must
    // clear a stale capture on an AnyView type swap, matching
    // `rebuild_children`'s semantics for `Flex`/`Stack`) --

    #[test]
    fn type_swap_at_captured_child_clears_active_and_stops_routing() {
        let mut counter = 0u64;
        let prev: SizedBoxView<Counter> =
            SizedBox(Some(300.0), None)
                .child(crate::button::<Counter, _>("go", |s: &mut Counter| {
                    s.presses += 1
                }));
        let mut w = prev.build(&mut BuildCtx::new(&mut counter));
        let mut text_ctx = frust_text::TextContext::new();
        let mut lctx = LayoutCtx::with_text_context(&mut text_ctx);
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(500.0, 500.0)));

        let mut state = Counter::default();
        dispatch(
            &mut w,
            &mut state,
            &pointer_ev(PointerPhase::Down, 2.0, 2.0),
        );
        assert!(
            w.child.as_ref().unwrap().is_active(),
            "down captures the pointer"
        );

        // Rebuild, swapping the child's concrete type (Button -> Checkbox).
        let swapped: SizedBoxView<Counter> =
            SizedBox(Some(300.0), None).child(crate::checkbox::<Counter, _>(
                false,
                "swapped",
                |_s: &mut Counter, _v: bool| {},
            ));
        swapped.rebuild(&prev, &mut w, &mut BuildCtx::new(&mut counter));
        assert!(
            !w.child.as_ref().unwrap().is_active(),
            "the type swap must clear the stale capture"
        );

        // A Down well outside the child's bounds must now be Ignored — not
        // routed to the fresh widget via a stale `active` flag.
        assert_eq!(
            dispatch(
                &mut w,
                &mut state,
                &pointer_ev(PointerPhase::Down, 400.0, 400.0)
            ),
            EventResult::Ignored,
            "a swap must clear active so an out-of-bounds Down is not delivered"
        );
    }
}
