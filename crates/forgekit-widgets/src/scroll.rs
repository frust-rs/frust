//! The `ScrollView` widget (spec §6.4): a vertical scroll surface with drag,
//! fling, and wheel support and clipped, offset content.
//!
//! [`scroll_view`] wraps a child that is laid out with unbounded height; the
//! view itself takes the incoming constraints and paints the child offset by
//! `-scroll_offset` inside a clip. Offsets are clamped to `[0, content −
//! viewport]` — no overscroll.
//!
//! # Gesture takeover
//!
//! ScrollView captures the pointer on `Down` and forwards events to the child
//! so descendant widgets stay interactive. It *observes* `Move` deltas before
//! forwarding: once the accumulated drag passes [`TOUCH_SLOP`] it enters
//! scrolling mode — it sends the child a synthetic `Cancel` (disarming any
//! armed descendant tap/press), stops forwarding, and consumes the drag itself.
//! This is how a scroll can be *taken* from a child after the slop, matching
//! masonry (`research/RESEARCH.md`).
//!
//! # Fling driver (v1)
//!
//! On release with sufficient velocity a fling begins, integrated
//! frame-by-frame with [`ScrollWidget::tick`] (pure, unit-tested). [`paint`]
//! pumps the fling from a monotonic clock so it animates for free on the
//! continuous-loop mobile shells, and calls [`PaintCtx::request_frame`] while the
//! fling is still in flight so the desktop shell (event-driven
//! `ControlFlow::Wait`) keeps scheduling frames via `window.request_redraw()`;
//! the signal stops once the fling reaches rest.

use std::time::Instant;

use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, FLING_STOP,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerButton, PointerEvent, PointerPhase,
    ScrollDelta, TOUCH_SLOP, VelocityTracker, View, WHEEL_LINE_PX, Widget, any, fling_decay,
    fling_displacement,
};
use kurbo::{Point, Size};

/// A declarative vertical scroll surface. See the [module docs](self).
pub struct ScrollView<State: 'static> {
    child: AnyView<State>,
}

impl<State: 'static> ScrollView<State> {
    /// Wrap `child` in a vertical scroll view.
    pub fn new<V: View<State>>(child: V) -> Self {
        Self { child: any(child) }
    }
}

/// Wrap `child` in a vertical [`ScrollView`] — the free-function spelling of
/// [`ScrollView::new`].
pub fn scroll_view<State: 'static, V: View<State>>(child: V) -> ScrollView<State> {
    ScrollView::new(child)
}

/// The retained widget for a [`ScrollView`].
pub struct ScrollWidget {
    child: ChildPod,
    /// Current scroll offset in `[0, max_offset]` (px scrolled down).
    offset: f64,
    /// Resolved viewport size (this widget's own size).
    viewport: Size,
    /// The child's (content) size after an unbounded-height layout.
    content: Size,
    /// Whether we have taken the gesture over as a scroll drag.
    scrolling: bool,
    /// Whether a `Down` has armed an active gesture (distinct from `scrolling`,
    /// which only becomes true after the drag passes the slop). Set on `Down`
    /// and cleared on `Up`/`Cancel`; the slop/takeover math runs only while it
    /// is true, so a hover `Move` (dispatched on every cursor motion) is never
    /// mistaken for a drag and can never take the gesture from a child.
    down_active: bool,
    down_start: Point,
    last_drag: Point,
    tracker: VelocityTracker,
    /// Active fling velocity, in px/s of *offset* (opposite the finger), or
    /// `None` when not flinging.
    fling: Option<f64>,
    clock: Instant,
    /// Last animation timestamp (ms) for the paint-time fling pump.
    last_anim: Option<f64>,
}

impl ScrollWidget {
    fn new(child: ChildPod) -> Self {
        Self {
            child,
            offset: 0.0,
            viewport: Size::ZERO,
            content: Size::ZERO,
            scrolling: false,
            down_active: false,
            down_start: Point::ZERO,
            last_drag: Point::ZERO,
            tracker: VelocityTracker::new(),
            fling: None,
            clock: Instant::now(),
            last_anim: None,
        }
    }

    /// The current scroll offset.
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// The maximum scroll offset (`content − viewport`, never negative).
    pub fn max_offset(&self) -> f64 {
        (self.content.height - self.viewport.height).max(0.0)
    }

    /// Whether a fling animation is in flight.
    pub fn is_flinging(&self) -> bool {
        self.fling.is_some()
    }

    fn now_ms(&self) -> f64 {
        self.clock.elapsed().as_secs_f64() * 1000.0
    }

    fn set_offset(&mut self, value: f64) {
        self.offset = value.clamp(0.0, self.max_offset());
    }

    fn sync_child_origin(&mut self) {
        self.child.set_origin(Point::new(0.0, -self.offset));
    }

    /// Advance an in-flight fling by `dt_ms`, returning whether it is still
    /// animating. Pure and deterministic — the paint-time pump and the tests
    /// both drive it.
    pub fn tick(&mut self, dt_ms: f64) -> bool {
        let Some(v) = self.fling else {
            return false;
        };
        self.set_offset(self.offset + fling_displacement(v, dt_ms));
        self.sync_child_origin();
        let next_v = fling_decay(v, dt_ms);
        let at_bound = self.offset <= 0.0 || self.offset >= self.max_offset();
        if next_v.abs() < FLING_STOP || at_bound {
            self.fling = None;
            false
        } else {
            self.fling = Some(next_v);
            true
        }
    }

    /// Advance the fling by the wall-clock delta since the last paint, and signal
    /// [`PaintCtx::request_frame`] while it is still running so the shell keeps
    /// scheduling frames (the desktop `ControlFlow::Wait` loop would otherwise
    /// idle). Stops signalling once [`ScrollWidget::tick`] brings the fling to
    /// rest (`|velocity|` below [`FLING_STOP`], or a scroll bound reached).
    fn pump_fling(&mut self, ctx: &mut PaintCtx) {
        if self.fling.is_none() {
            self.last_anim = None;
            return;
        }
        let now = self.now_ms();
        let dt = match self.last_anim {
            Some(t) => now - t,
            None => 0.0,
        };
        self.last_anim = Some(now);
        if dt > 0.0 {
            self.tick(dt);
        }
        // `tick` clears `self.fling` once the fling reaches rest; while it is
        // still set, ask the shell for another frame to continue animating.
        if self.fling.is_some() {
            ctx.request_frame();
        }
    }

    fn send_child_cancel(&mut self, ctx: &mut EventCtx, pos: Point) {
        let cancel = InputEvent::Pointer(PointerEvent {
            phase: PointerPhase::Cancel,
            position: pos,
            button: PointerButton::Primary,
        });
        self.child.event_child(ctx, &cancel);
    }

    /// The event body, parameterised on an explicit timestamp so velocity math
    /// is deterministic in tests; [`Widget::event`] supplies the real clock.
    fn event_at(&mut self, ctx: &mut EventCtx, event: &InputEvent, t_ms: f64) -> EventResult {
        match event {
            InputEvent::Scroll { delta, .. } => {
                let dy = match delta {
                    ScrollDelta::Lines(_, y) => y * WHEEL_LINE_PX,
                    ScrollDelta::Pixels(_, y) => *y,
                };
                self.fling = None;
                self.set_offset(self.offset + dy);
                self.sync_child_origin();
                ctx.request_redraw();
                EventResult::Handled
            }
            InputEvent::Pointer(p) => match p.phase {
                PointerPhase::Down => {
                    self.scrolling = false;
                    self.down_active = true;
                    self.fling = None;
                    self.last_anim = None;
                    self.down_start = p.position;
                    self.last_drag = p.position;
                    self.tracker.clear();
                    self.tracker.record(t_ms, p.position.y);
                    ctx.capture_pointer();
                    self.child.event_child(ctx, event);
                    EventResult::Handled
                }
                PointerPhase::Move => {
                    // Without an armed `Down`, this is a hover move (the desktop
                    // shell dispatches `Move` on every cursor motion): never run
                    // the slop/takeover math against a stale `down_start`, just
                    // forward it to the child.
                    if !self.down_active {
                        return self.child.event_child(ctx, event);
                    }
                    self.tracker.record(t_ms, p.position.y);
                    if self.scrolling {
                        let dy = p.position.y - self.last_drag.y;
                        self.last_drag = p.position;
                        self.set_offset(self.offset - dy);
                        self.sync_child_origin();
                        ctx.request_redraw();
                    } else if (p.position.y - self.down_start.y).abs() > TOUCH_SLOP {
                        // Take the gesture over: cancel the child, stop forwarding.
                        self.scrolling = true;
                        self.last_drag = p.position;
                        self.send_child_cancel(ctx, p.position);
                        self.child.set_active(false);
                        ctx.request_redraw();
                    } else {
                        self.child.event_child(ctx, event);
                    }
                    EventResult::Handled
                }
                PointerPhase::Up => {
                    if self.scrolling {
                        let finger_v = self.tracker.velocity();
                        if finger_v.abs() > FLING_STOP {
                            // Offset moves opposite the finger.
                            self.fling = Some(-finger_v);
                            self.last_anim = None;
                        }
                    } else {
                        self.child.event_child(ctx, event);
                    }
                    self.child.set_active(false);
                    self.scrolling = false;
                    self.down_active = false;
                    ctx.request_redraw();
                    EventResult::Handled
                }
                PointerPhase::Cancel => {
                    self.child.event_child(ctx, event);
                    self.child.set_active(false);
                    self.scrolling = false;
                    self.down_active = false;
                    ctx.request_redraw();
                    EventResult::Handled
                }
            },
        }
    }
}

impl<State: 'static> View<State> for ScrollView<State> {
    type Element = ScrollWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> ScrollWidget {
        ScrollWidget::new(crate::build_child(&self.child, ctx))
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut ScrollWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        crate::rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut ScrollWidget, ctx: &mut BuildCtx<'_>) {
        crate::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for ScrollWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let vw = if bc.max().width.is_finite() {
            bc.max().width
        } else {
            0.0
        };
        // The child is laid out at the viewport width with unbounded height.
        let child_bc = BoxConstraints::new(Size::new(vw, 0.0), Size::new(vw, f64::INFINITY));
        self.content = self.child.layout_child(ctx, &child_bc);
        let vh = if bc.max().height.is_finite() {
            bc.max().height
        } else {
            self.content.height
        };
        self.viewport = Size::new(vw, vh);
        self.set_offset(self.offset); // re-clamp against new content/viewport
        self.sync_child_origin();
        bc.constrain(self.viewport)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.pump_fling(ctx);
        scene.push_clip(ctx.origin(), ctx.size());
        self.sync_child_origin();
        self.child.paint_child(ctx, scene);
        scene.pop_clip();
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        let t = self.now_ms();
        self.event_at(ctx, event, t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::leaf;
    use std::any::Any;

    /// Build and lay out a scroll widget over `()` state with a `content_h`-tall
    /// leaf child inside a `vw`×`vh` viewport.
    fn laid_out(vw: f64, vh: f64, content_h: f64) -> ScrollWidget {
        let view: ScrollView<()> = scroll_view(leaf(vw, content_h));
        let mut counter = 0u64;
        let mut w = View::<()>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut lctx = LayoutCtx::new();
        w.layout(&mut lctx, &BoxConstraints::loose(Size::new(vw, vh)));
        w
    }

    fn scroll(y: f64, lines: bool, amount: f64) -> InputEvent {
        let delta = if lines {
            ScrollDelta::Lines(0.0, amount)
        } else {
            ScrollDelta::Pixels(0.0, amount)
        };
        InputEvent::Scroll {
            position: Point::new(10.0, y),
            delta,
        }
    }

    fn ev(phase: PointerPhase, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            phase,
            position: Point::new(10.0, y),
            button: PointerButton::Primary,
        })
    }

    fn dispatch(w: &mut ScrollWidget, event: &InputEvent, t_ms: f64) {
        let mut unit = ();
        let state_any: &mut dyn Any = &mut unit;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, w.viewport);
        w.event_at(&mut ctx, event, t_ms);
    }

    #[test]
    fn max_offset_is_content_minus_viewport() {
        let w = laid_out(200.0, 100.0, 1000.0);
        assert_eq!(w.max_offset(), 900.0);
        assert_eq!(w.offset(), 0.0);
    }

    #[test]
    fn wheel_scrolls_and_clamps_with_no_overscroll() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        // 3 lines * 40 px = 120.
        dispatch(&mut w, &scroll(50.0, true, 3.0), 0.0);
        assert_eq!(w.offset(), 120.0);
        // A huge line delta clamps to max_offset (no overscroll past the end).
        dispatch(&mut w, &scroll(50.0, true, 100.0), 0.0);
        assert_eq!(w.offset(), 900.0);
        // Scrolling back past the top clamps to 0.
        dispatch(&mut w, &scroll(50.0, false, -5000.0), 0.0);
        assert_eq!(w.offset(), 0.0);
    }

    #[test]
    fn drag_past_slop_scrolls_the_offset() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        // First move crosses the slop → takeover (no scroll on this move).
        dispatch(&mut w, &ev(PointerPhase::Move, 70.0), 16.0);
        assert_eq!(w.offset(), 0.0);
        assert!(w.scrolling);
        // Next move drags the finger up 30 px → content scrolls down 30 px.
        dispatch(&mut w, &ev(PointerPhase::Move, 40.0), 32.0);
        assert_eq!(w.offset(), 30.0);
    }

    #[test]
    fn takeover_sends_the_child_a_cancel() {
        // A child that records the pointer phases it receives.
        #[derive(Default)]
        struct Rec {
            downs: u32,
            cancels: u32,
        }
        struct Probe;
        struct ProbeW;
        impl View<Rec> for Probe {
            type Element = ProbeW;
            fn build(&self, _c: &mut BuildCtx<'_>) -> ProbeW {
                ProbeW
            }
            fn rebuild(&self, _p: &Self, _e: &mut ProbeW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
                ChangeFlags::NONE
            }
        }
        impl Widget for ProbeW {
            fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                bc.constrain(Size::new(200.0, 1000.0))
            }
            fn paint(&mut self, _c: &mut PaintCtx, _s: &mut dyn PaintScene) {}
            fn event(&mut self, ctx: &mut EventCtx, e: &InputEvent) -> EventResult {
                if let InputEvent::Pointer(p) = e {
                    let rec = ctx.state_mut::<Rec>();
                    match p.phase {
                        PointerPhase::Down => rec.downs += 1,
                        PointerPhase::Cancel => rec.cancels += 1,
                        _ => {}
                    }
                }
                EventResult::Handled
            }
        }

        let view: ScrollView<Rec> = scroll_view(Probe);
        let mut counter = 0u64;
        let mut w = View::<Rec>::build(&view, &mut BuildCtx::new(&mut counter));
        w.viewport = Size::new(200.0, 100.0);

        let mut state = Rec::default();
        let run = |w: &mut ScrollWidget, state: &mut Rec, e: &InputEvent, t: f64| {
            let sa: &mut dyn Any = state;
            let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(200.0, 100.0));
            w.event_at(&mut ctx, e, t);
        };
        run(&mut w, &mut state, &ev(PointerPhase::Down, 100.0), 0.0);
        // Drag 30 px past the slop → takeover fires a Cancel at the child.
        run(&mut w, &mut state, &ev(PointerPhase::Move, 70.0), 16.0);
        assert_eq!(state.downs, 1);
        assert_eq!(state.cancels, 1);
        // Subsequent scrolling moves are not forwarded to the child.
        run(&mut w, &mut state, &ev(PointerPhase::Move, 50.0), 32.0);
        assert_eq!(state.cancels, 1);
    }

    /// A recording child probe, shared by the takeover/hover tests.
    #[derive(Default)]
    struct Rec {
        downs: u32,
        cancels: u32,
        moves: u32,
    }
    struct Probe;
    struct ProbeW;
    impl View<Rec> for Probe {
        type Element = ProbeW;
        fn build(&self, _c: &mut BuildCtx<'_>) -> ProbeW {
            ProbeW
        }
        fn rebuild(&self, _p: &Self, _e: &mut ProbeW, _c: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for ProbeW {
        fn layout(&mut self, _c: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(200.0, 1000.0))
        }
        fn paint(&mut self, _c: &mut PaintCtx, _s: &mut dyn PaintScene) {}
        fn event(&mut self, ctx: &mut EventCtx, e: &InputEvent) -> EventResult {
            if let InputEvent::Pointer(p) = e {
                let rec = ctx.state_mut::<Rec>();
                match p.phase {
                    PointerPhase::Down => rec.downs += 1,
                    PointerPhase::Cancel => rec.cancels += 1,
                    PointerPhase::Move => rec.moves += 1,
                    _ => {}
                }
            }
            EventResult::Ignored
        }
    }

    fn probe_scroll() -> ScrollWidget {
        let view: ScrollView<Rec> = scroll_view(Probe);
        let mut counter = 0u64;
        let mut w = View::<Rec>::build(&view, &mut BuildCtx::new(&mut counter));
        w.viewport = Size::new(200.0, 100.0);
        w
    }

    fn run_rec(w: &mut ScrollWidget, state: &mut Rec, e: &InputEvent, t: f64) -> bool {
        let sa: &mut dyn Any = state;
        let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(200.0, 100.0));
        w.event_at(&mut ctx, e, t);
        ctx.needs_redraw()
    }

    #[test]
    fn hover_move_without_down_never_scrolls_or_cancels_child() {
        let mut w = probe_scroll();
        let mut state = Rec::default();
        // A cursor drifting over the list with no prior Down: no takeover, no
        // Cancel to the child, no offset change, no self redraw request.
        let redraw = run_rec(&mut w, &mut state, &ev(PointerPhase::Move, 40.0), 16.0);
        assert!(!w.scrolling, "hover must not enter scrolling");
        assert!(!w.down_active);
        assert_eq!(w.offset(), 0.0, "hover must not move the offset");
        assert_eq!(state.cancels, 0, "hover must not cancel the child");
        assert!(!redraw, "hover must not request a redraw");
        // The hover move is forwarded to the child (which ignores it).
        assert_eq!(state.moves, 1);
    }

    #[test]
    fn cancel_clears_down_active() {
        let mut w = probe_scroll();
        let mut state = Rec::default();
        run_rec(&mut w, &mut state, &ev(PointerPhase::Down, 100.0), 0.0);
        assert!(w.down_active);
        run_rec(&mut w, &mut state, &ev(PointerPhase::Cancel, 100.0), 16.0);
        assert!(!w.down_active, "Cancel disarms the gesture");
        assert!(!w.scrolling);
        // A subsequent hover Move must not run the takeover math.
        run_rec(&mut w, &mut state, &ev(PointerPhase::Move, 20.0), 32.0);
        assert!(!w.scrolling, "hover after Cancel must not take over");
        assert_eq!(w.offset(), 0.0);
    }

    #[test]
    fn fling_after_release_decays_and_clamps() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 75.0), 16.0); // takeover
        dispatch(&mut w, &ev(PointerPhase::Move, 50.0), 32.0); // scroll, builds velocity
        dispatch(&mut w, &ev(PointerPhase::Up, 50.0), 32.0);
        assert!(w.is_flinging(), "release with velocity starts a fling");
        // Integrate to completion.
        let mut steps = 0;
        while w.tick(16.0) {
            steps += 1;
            assert!(steps < 100_000, "fling failed to terminate");
        }
        assert!(!w.is_flinging());
        assert!(w.offset() >= 0.0 && w.offset() <= w.max_offset());
    }

    #[test]
    fn pump_fling_signals_needs_frame_until_at_rest() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        // Drive a release-with-velocity to start a fling (deterministic seam).
        dispatch(&mut w, &ev(PointerPhase::Down, 100.0), 0.0);
        dispatch(&mut w, &ev(PointerPhase::Move, 75.0), 16.0); // takeover
        dispatch(&mut w, &ev(PointerPhase::Move, 50.0), 32.0); // build velocity
        dispatch(&mut w, &ev(PointerPhase::Up, 50.0), 32.0);
        assert!(w.is_flinging(), "release with velocity starts a fling");

        // A paint-time pump while flinging asks for another frame.
        let mut ctx = PaintCtx::new(Point::ZERO, w.viewport);
        w.pump_fling(&mut ctx);
        assert!(
            ctx.needs_frame(),
            "an in-flight fling requests continuation"
        );
        assert!(w.is_flinging());

        // Integrate the fling to rest via the deterministic tick seam.
        while w.tick(16.0) {}
        assert!(!w.is_flinging());

        // At rest, the pump no longer signals — the shell can idle again.
        let mut ctx_rest = PaintCtx::new(Point::ZERO, w.viewport);
        w.pump_fling(&mut ctx_rest);
        assert!(
            !ctx_rest.needs_frame(),
            "a fling at rest stops requesting frames"
        );
    }

    #[test]
    fn tick_without_a_fling_is_a_noop() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        assert!(!w.tick(16.0));
        assert_eq!(w.offset(), 0.0);
    }

    #[test]
    fn offset_reclamps_when_content_shrinks() {
        let mut w = laid_out(200.0, 100.0, 1000.0);
        dispatch(&mut w, &scroll(50.0, false, 800.0), 0.0);
        assert_eq!(w.offset(), 800.0);
        // Content shrinks to just above the viewport → max_offset drops to 50,
        // and a re-clamp (what layout does) pulls the stale offset back in range.
        w.content = Size::new(200.0, 150.0);
        w.set_offset(w.offset);
        assert_eq!(w.max_offset(), 50.0);
        assert_eq!(w.offset(), 50.0);
    }
}
