//! The `GestureDetector` widget (spec §6.4): a transparent wrapper that
//! recognises a tap on its child and forwards raw events through.
//!
//! [`GestureDetector`] wraps an arbitrary child view and, when given an
//! `on_tap` closure, fires it on a `Down`→`Up` that stays within [`TOUCH_SLOP`]
//! (a tap). Moving past the slop disarms the tap (it became a drag). The wrapper
//! is transparent — every event is still forwarded to the child, so interactive
//! descendants keep working. v1 recognises tap only; long-press/double-tap are
//! deferred (and, lacking an input-kind flag on events, the slop is
//! [`TOUCH_SLOP`] uniformly — see `research/RESEARCH.md`).

use std::rc::Rc;

use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent,
    LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, TOUCH_SLOP, View, Widget, any,
};
use kurbo::{Point, Size};

/// A view-held, typed tap callback (erased on build).
type OnTap<State> = Rc<dyn Fn(&mut State)>;

/// A declarative gesture wrapper. See the [module docs](self).
pub struct GestureDetectorView<State: 'static> {
    child: AnyView<State>,
    on_tap: Option<OnTap<State>>,
}

/// Wrap `child` in a gesture detector (no recognisers until one is attached,
/// e.g. with [`GestureDetectorView::on_tap`]).
#[allow(non_snake_case)]
pub fn GestureDetector<State: 'static, V: View<State>>(child: V) -> GestureDetectorView<State> {
    GestureDetectorView {
        child: any(child),
        on_tap: None,
    }
}

impl<State: 'static> GestureDetectorView<State> {
    /// Fire `on_tap` when the child is tapped (a press+release within
    /// [`TOUCH_SLOP`]).
    pub fn on_tap<F: Fn(&mut State) + 'static>(mut self, on_tap: F) -> Self {
        self.on_tap = Some(Rc::new(on_tap));
        self
    }
}

/// The retained widget for a [`GestureDetectorView`].
pub struct GestureDetectorWidget {
    child: ChildPod,
    armed: bool,
    down_pos: Point,
    on_tap: Option<crate::ErasedCallback>,
}

impl<State: 'static> View<State> for GestureDetectorView<State> {
    type Element = GestureDetectorWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> GestureDetectorWidget {
        GestureDetectorWidget {
            child: crate::build_child(&self.child, ctx),
            armed: false,
            down_pos: Point::ZERO,
            on_tap: self.on_tap.as_ref().map(crate::erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut GestureDetectorWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_tap = self.on_tap.as_ref().map(crate::erase_callback);
        crate::rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut GestureDetectorWidget, ctx: &mut BuildCtx<'_>) {
        crate::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for GestureDetectorWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Pointer(p) = event {
            match p.phase {
                PointerPhase::Down => {
                    self.armed = true;
                    self.down_pos = p.position;
                    ctx.capture_pointer();
                    self.child.event_child(ctx, event);
                    return EventResult::Handled;
                }
                PointerPhase::Move => {
                    if self.armed && (p.position - self.down_pos).hypot() > TOUCH_SLOP {
                        self.armed = false; // became a drag
                    }
                    self.child.event_child(ctx, event);
                    return EventResult::Handled;
                }
                PointerPhase::Up => {
                    let fired = self.armed;
                    self.armed = false;
                    self.child.event_child(ctx, event);
                    self.child.set_active(false);
                    if fired && let Some(cb) = self.on_tap.as_mut() {
                        cb(ctx);
                        ctx.request_redraw();
                    }
                    return EventResult::Handled;
                }
                PointerPhase::Cancel => {
                    self.armed = false;
                    self.child.event_child(ctx, event);
                    self.child.set_active(false);
                    return EventResult::Handled;
                }
            }
        }
        // Non-pointer (scroll) events pass straight through to the child.
        self.child.event_child(ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        // Transparent tap-recognizer wrapper: forward to the single child.
        self.child.semantics_child(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    #[derive(Default)]
    struct TapState {
        taps: u32,
    }

    /// A trivial full-bleed child that ignores events.
    struct Blank;
    struct BlankWidget;
    impl View<TapState> for Blank {
        type Element = BlankWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> BlankWidget {
            BlankWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut BlankWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for BlankWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    fn widget(with_tap: bool) -> GestureDetectorWidget {
        let mut view = GestureDetector::<TapState, _>(Blank);
        if with_tap {
            view = view.on_tap(|s: &mut TapState| s.taps += 1);
        }
        let mut counter = 0u64;
        View::<TapState>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(forgekit_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: forgekit_core::PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut GestureDetectorWidget, state: &mut TapState, event: &InputEvent) {
        let state_any: &mut dyn Any = state;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, 100.0));
        w.event(&mut ctx, event);
    }

    #[test]
    fn press_release_within_slop_is_a_tap() {
        let mut w = widget(true);
        let mut state = TapState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 15.0, 12.0));
        assert_eq!(state.taps, 1);
    }

    #[test]
    fn drag_past_slop_disarms_the_tap() {
        let mut w = widget(true);
        let mut state = TapState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        // 30 px down: well past TOUCH_SLOP (18).
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 10.0, 40.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 10.0, 40.0));
        assert_eq!(state.taps, 0);
    }

    #[test]
    fn cancel_disarms_the_tap() {
        let mut w = widget(true);
        let mut state = TapState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Cancel, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(state.taps, 0);
    }

    #[test]
    fn no_tap_callback_is_a_benign_noop() {
        let mut w = widget(false);
        let mut state = TapState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(state.taps, 0);
    }
}
