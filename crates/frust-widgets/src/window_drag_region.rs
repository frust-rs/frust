//! A region that turns an unhandled primary press into a window command, for
//! app-drawn title bars and resize strips.
//!
//! Wrap the bar (or an edge strip) in [`window_drag_region`]. A primary press is
//! offered to the child **first**: a control inside the bar keeps its press and
//! the region does nothing. Only a press the child ignores becomes a command,
//! so a disabled button drags the window, as it does in a native bar.
//!
//! - **Move mode** (default): the press pushes [`WindowCommand::BeginMove`]. A
//!   second unhandled press within [`DOUBLE_TAP_MS`] and [`TOUCH_SLOP`] of the
//!   first pushes [`WindowCommand::ToggleMaximized`] instead (a native
//!   title-bar double-click); disable it with
//!   [`double_click_toggles_maximize`](WindowDragRegionView::double_click_toggles_maximize).
//! - **Resize mode** ([`resize`](WindowDragRegionView::resize)): every
//!   unhandled press pushes [`WindowCommand::BeginResize`]; double-press
//!   detection is off. `BeginResize` is a documented no-op on macOS, where
//!   AppKit keeps its own edge resizing on both decoration levels, so an app
//!   hides its strips there and while maximized or fullscreen (read
//!   [`frust_core::WindowState`]).
//!
//! The region never captures the pointer: once the command runs the OS owns
//! the gesture. Every other event (`Move`/`Up`/`Cancel`, secondary and middle
//! presses, keys) goes to the child unchanged.
//!
//! The double-press window rides the paint clock, as the text field's
//! double-tap does, because a pointer event carries no timestamp. A press is
//! stamped at the first paint after it (an idle window's last paint is old),
//! and a press not yet stamped counts as inside the window. Off desktop the
//! commands are no-ops.

use std::marker::PhantomData;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, FrameTime,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, ResizeEdge, SemanticsCtx,
    TOUCH_SLOP, View, Widget, WindowCommand, any, push_window_command,
};
use kurbo::{Point, Size};

use crate::authoring::presses;
use crate::gesture::DOUBLE_TAP_MS;

/// What an unhandled primary press asks of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Move { double_click_toggles: bool },
    Resize(ResizeEdge),
}

/// A declarative window drag region. See the [module docs](self).
pub struct WindowDragRegionView<State: 'static> {
    child: AnyView<State>,
    mode: Mode,
    _state: PhantomData<fn(&mut State)>,
}

/// Wrap `child` so a primary press it ignores moves the window.
pub fn window_drag_region<State: 'static, V: View<State>>(child: V) -> WindowDragRegionView<State> {
    WindowDragRegionView {
        child: any(child),
        mode: Mode::Move {
            double_click_toggles: true,
        },
        _state: PhantomData,
    }
}

impl<State: 'static> WindowDragRegionView<State> {
    /// Whether a second unhandled press toggles maximize (default `true`).
    /// Ignored in resize mode.
    pub fn double_click_toggles_maximize(mut self, on: bool) -> Self {
        if let Mode::Move { .. } = self.mode {
            self.mode = Mode::Move {
                double_click_toggles: on,
            };
        }
        self
    }

    /// Switch to resize mode: every unhandled primary press begins a resize
    /// from `edge`.
    pub fn resize(mut self, edge: ResizeEdge) -> Self {
        self.mode = Mode::Resize(edge);
        self
    }
}

/// The retained widget for a [`WindowDragRegionView`].
pub struct WindowDragRegionWidget {
    child: ChildPod,
    mode: Mode,
    /// The last unhandled press: where it landed and the paint-clock time of
    /// the first paint after it (`None` until that paint).
    last_press: Option<(Point, Option<FrameTime>)>,
    /// The latest paint-pass clock, the only one available to the event pass.
    last_frame_time: FrameTime,
}

impl WindowDragRegionWidget {
    fn elapsed_ms(&self, since: FrameTime) -> f64 {
        self.last_frame_time.saturating_sub(since).as_secs_f64() * 1000.0
    }

    fn is_double_press(&self, pos: Point) -> bool {
        self.last_press.is_some_and(|(prev, at)| {
            (pos - prev).hypot() <= TOUCH_SLOP
                && at.is_none_or(|at| self.elapsed_ms(at) <= DOUBLE_TAP_MS)
        })
    }
}

impl<State: 'static> View<State> for WindowDragRegionView<State> {
    type Element = WindowDragRegionWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> WindowDragRegionWidget {
        WindowDragRegionWidget {
            child: crate::authoring::build_child(&self.child, ctx),
            mode: self.mode,
            last_press: None,
            last_frame_time: FrameTime::default(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut WindowDragRegionWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if element.mode != self.mode {
            element.last_press = None;
        }
        element.mode = self.mode;
        crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut WindowDragRegionWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for WindowDragRegionWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.last_frame_time = ctx.frame_time();
        // Keep the clock moving while a press is inside its window, so an idle
        // window cannot make a much later press look like a double-press.
        if let Some((_, at)) = self.last_press {
            let at = at.unwrap_or(self.last_frame_time);
            if let Some((_, stamp)) = &mut self.last_press {
                *stamp = Some(at);
            }
            if self.elapsed_ms(at) > DOUBLE_TAP_MS {
                self.last_press = None;
            } else {
                ctx.request_frame();
            }
        }
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            return self.child.event_child(ctx, event);
        };
        if p.phase != PointerPhase::Down || !presses(p) {
            return self.child.event_child(ctx, event);
        }
        if self.child.event_child(ctx, event) == EventResult::Handled {
            return EventResult::Handled;
        }
        match self.mode {
            Mode::Resize(edge) => push_window_command(WindowCommand::BeginResize(edge)),
            Mode::Move {
                double_click_toggles,
            } => {
                if double_click_toggles && self.is_double_press(p.position) {
                    self.last_press = None;
                    push_window_command(WindowCommand::ToggleMaximized);
                } else {
                    self.last_press = Some((p.position, None));
                    push_window_command(WindowCommand::BeginMove);
                    // Start the paint loop that ages the record out.
                    ctx.request_redraw();
                }
            }
        }
        EventResult::Handled
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    crate::authoring::visit_children!(child);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::button::button;
    use frust_core::{PointerButton, PointerEvent, RenderRoot, take_window_commands};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Default)]
    struct Presses(u32);

    /// A child answering every event with a configurable result and logging
    /// the phases it was offered.
    #[derive(Clone)]
    struct Probe {
        result: Rc<Cell<EventResult>>,
        seen: Rc<RefCell<Vec<PointerPhase>>>,
    }
    struct ProbeWidget(Probe);
    impl<State: 'static> View<State> for Probe {
        type Element = ProbeWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> ProbeWidget {
            ProbeWidget(self.clone())
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut ProbeWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for ProbeWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn event(&mut self, _ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = event {
                self.0.seen.borrow_mut().push(p.phase);
            }
            self.0.result.get()
        }
    }

    struct NullScene;
    impl PaintScene for NullScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: peniko::Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
    }

    fn ft(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    fn ev_btn(phase: PointerPhase, button: PointerButton, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(x, y),
            button,
        })
    }

    fn ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        ev_btn(phase, PointerButton::Primary, x, y)
    }

    fn probe(result: EventResult) -> Probe {
        Probe {
            result: Rc::new(Cell::new(result)),
            seen: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// Run `body` with a laid-out root over `view`, the command queue drained
    /// and the process-global lock held.
    fn with_root(
        view: impl Fn() -> WindowDragRegionView<Presses>,
        body: impl FnOnce(&mut RenderRoot<Presses, WindowDragRegionView<Presses>>, &mut Presses),
    ) {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        take_window_commands();
        let mut logic = |_: &mut Presses| view();
        let mut root = RenderRoot::new();
        let mut state = Presses::default();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(100.0, 100.0));
        body(&mut root, &mut state);
    }

    /// Press twice at `x2`; the press-triggered paint stamps at 0, then one at `t2` ms.
    fn two_presses(
        root: &mut RenderRoot<Presses, WindowDragRegionView<Presses>>,
        state: &mut Presses,
        t2: f64,
        x2: f64,
    ) -> Vec<WindowCommand> {
        let mut sink = NullScene;
        root.paint(&mut sink, ft(0.0));
        root.event(state, &ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0));
        root.paint(&mut sink, ft(t2));
        root.event(state, &ev(PointerPhase::Down, x2, 10.0));
        take_window_commands()
    }

    fn press_at(root: &mut RenderRoot<Presses, WindowDragRegionView<Presses>>, s: &mut Presses) {
        root.event(s, &ev(PointerPhase::Down, 10.0, 10.0));
    }

    #[test]
    fn a_double_click_survives_an_idle_window() {
        let p = probe(EventResult::Ignored);
        for second_paint in [None, Some(5200.0)] {
            with_root(
                || window_drag_region(p.clone()),
                |root, state| {
                    let mut sink = NullScene;
                    root.paint(&mut sink, ft(0.0));
                    press_at(root, state);
                    root.paint(&mut sink, ft(5000.0));
                    if let Some(t) = second_paint {
                        root.paint(&mut sink, ft(t));
                    }
                    press_at(root, state);
                    assert_eq!(
                        take_window_commands(),
                        vec![WindowCommand::BeginMove, WindowCommand::ToggleMaximized]
                    );
                },
            );
        }
    }

    #[test]
    fn two_presses_with_no_paint_between_toggle_maximize() {
        let p = probe(EventResult::Ignored);
        with_root(
            || window_drag_region(p.clone()),
            |root, state| {
                press_at(root, state);
                press_at(root, state);
                assert_eq!(
                    take_window_commands(),
                    vec![WindowCommand::BeginMove, WindowCommand::ToggleMaximized]
                );
            },
        );
    }

    #[test]
    fn an_aged_press_is_cleared_and_stops_requesting_frames() {
        let p = probe(EventResult::Ignored);
        with_root(
            || window_drag_region(p.clone()),
            |root, state| {
                let mut sink = NullScene;
                root.paint(&mut sink, ft(0.0));
                press_at(root, state);
                assert!(root.paint(&mut sink, ft(5000.0)).needs_frame);
                assert!(!root.paint(&mut sink, ft(5400.0)).needs_frame);
                press_at(root, state);
                assert_eq!(
                    take_window_commands(),
                    vec![WindowCommand::BeginMove, WindowCommand::BeginMove]
                );
            },
        );
    }

    #[test]
    fn a_mode_change_on_rebuild_clears_the_record() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        take_window_commands();
        let p = probe(EventResult::Ignored);
        let resize = Cell::new(false);
        let mut logic = |_: &mut Presses| {
            let v = window_drag_region(p.clone());
            if resize.get() {
                v.resize(ResizeEdge::SouthEast)
            } else {
                v
            }
        };
        let mut root = RenderRoot::new();
        let mut state = Presses::default();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(100.0, 100.0));
        press_at(&mut root, &mut state);
        resize.set(true);
        root.rebuild(&mut logic, &mut state);
        press_at(&mut root, &mut state);
        resize.set(false);
        root.rebuild(&mut logic, &mut state);
        press_at(&mut root, &mut state);
        assert_eq!(
            take_window_commands(),
            vec![
                WindowCommand::BeginMove,
                WindowCommand::BeginResize(ResizeEdge::SouthEast),
                WindowCommand::BeginMove,
            ]
        );
    }

    #[test]
    fn a_press_the_child_handles_pushes_nothing_and_stays_handled() {
        let p = probe(EventResult::Handled);
        with_root(
            || window_drag_region(p.clone()),
            |root, state| {
                let out = root.event(state, &ev(PointerPhase::Down, 10.0, 10.0));
                assert!(out.handled);
                assert!(take_window_commands().is_empty());
            },
        );
    }

    #[test]
    fn a_press_the_child_ignores_begins_a_move() {
        let p = probe(EventResult::Ignored);
        with_root(
            || window_drag_region(p.clone()),
            |root, state| {
                let out = root.event(state, &ev(PointerPhase::Down, 10.0, 10.0));
                assert!(out.handled);
                assert_eq!(take_window_commands(), vec![WindowCommand::BeginMove]);
            },
        );
    }

    #[test]
    fn a_second_press_within_the_window_toggles_maximize() {
        let p = probe(EventResult::Ignored);
        with_root(
            || window_drag_region(p.clone()),
            |root, state| {
                assert_eq!(
                    two_presses(root, state, 200.0, 10.0),
                    vec![WindowCommand::BeginMove, WindowCommand::ToggleMaximized]
                );
            },
        );
    }

    #[test]
    fn a_second_press_after_the_window_begins_another_move() {
        let p = probe(EventResult::Ignored);
        with_root(
            || window_drag_region(p.clone()),
            |root, state| {
                assert_eq!(
                    two_presses(root, state, 400.0, 10.0),
                    vec![WindowCommand::BeginMove, WindowCommand::BeginMove]
                );
            },
        );
    }

    #[test]
    fn a_second_press_beyond_the_slop_begins_another_move() {
        let p = probe(EventResult::Ignored);
        with_root(
            || window_drag_region(p.clone()),
            |root, state| {
                let x2 = 10.0 + 2.0 * TOUCH_SLOP;
                assert_eq!(
                    two_presses(root, state, 200.0, x2),
                    vec![WindowCommand::BeginMove, WindowCommand::BeginMove]
                );
            },
        );
    }

    #[test]
    fn disabling_double_click_never_toggles_maximize() {
        let p = probe(EventResult::Ignored);
        with_root(
            || window_drag_region(p.clone()).double_click_toggles_maximize(false),
            |root, state| {
                assert_eq!(
                    two_presses(root, state, 200.0, 10.0),
                    vec![WindowCommand::BeginMove, WindowCommand::BeginMove]
                );
            },
        );
    }

    #[test]
    fn resize_mode_begins_a_resize_on_every_press_and_never_toggles() {
        let p = probe(EventResult::Ignored);
        with_root(
            || window_drag_region(p.clone()).resize(ResizeEdge::SouthEast),
            |root, state| {
                let edge = WindowCommand::BeginResize(ResizeEdge::SouthEast);
                assert_eq!(two_presses(root, state, 200.0, 10.0), vec![edge, edge]);
            },
        );
    }

    #[test]
    fn a_secondary_press_is_forwarded_and_pushes_nothing() {
        let p = probe(EventResult::Ignored);
        with_root(
            || window_drag_region(p.clone()),
            |root, state| {
                let e = ev_btn(PointerPhase::Down, PointerButton::Secondary, 10.0, 10.0);
                root.event(state, &e);
                assert!(take_window_commands().is_empty());
                assert_eq!(*p.seen.borrow(), vec![PointerPhase::Down]);
            },
        );
    }

    #[test]
    fn move_and_up_are_forwarded_to_the_child() {
        let p = probe(EventResult::Ignored);
        with_root(
            || window_drag_region(p.clone()),
            |root, state| {
                root.event(state, &ev(PointerPhase::Move, 10.0, 10.0));
                root.event(state, &ev(PointerPhase::Up, 10.0, 10.0));
                assert!(take_window_commands().is_empty());
                assert_eq!(*p.seen.borrow(), vec![PointerPhase::Move, PointerPhase::Up]);
            },
        );
    }

    #[test]
    fn a_button_inside_the_region_keeps_its_press() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        take_window_commands();
        let mut logic =
            |_: &mut Presses| window_drag_region(button("Close", |s: &mut Presses| s.0 += 1));
        let mut root = RenderRoot::new();
        let mut state = Presses::default();
        root.rebuild(&mut logic, &mut state);
        let mut tcx = frust_text::TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn std::any::Any);
        root.event(&mut state, &ev(PointerPhase::Down, 5.0, 5.0));
        root.event(&mut state, &ev(PointerPhase::Up, 5.0, 5.0));
        assert_eq!(state.0, 1);
        assert!(take_window_commands().is_empty());
    }
}
