//! The `GestureDetector` widget (spec §6.4): a transparent wrapper that
//! recognises a tap and a long-press on its child and forwards raw events
//! through.
//!
//! [`GestureDetector`] wraps an arbitrary child view and fires either an
//! `on_tap` closure (a `Down`→`Up` that stays within [`TOUCH_SLOP`]) or an
//! `on_long_press` closure (a press held past [`LONG_PRESS_MS`] without moving
//! past the slop). Moving past the slop disarms both (it became a drag). The
//! wrapper is transparent — every event is still forwarded to the child, so
//! interactive descendants keep working. v1 recognises **tap + long-press**;
//! double-tap is still deferred (and, lacking an input-kind flag on events, the
//! slop is [`TOUCH_SLOP`] uniformly — see `research/RESEARCH.md`).
//!
//! # Long-press firing semantics
//!
//! Timing is measured across **paints**, not events: only the paint pass
//! carries a clock ([`PaintCtx::frame_time`]), while events carry none. A press
//! records `press_start` on its first paint and marks itself *elapsed* on a
//! later paint once `frame_time - press_start >= LONG_PRESS_MS`. **The callback
//! never fires during paint** (there is no `&mut State`/`EventCtx` there); paint
//! only *marks* the threshold. `on_long_press` then fires on the next pointer
//! event delivered for this capture:
//!
//! - on **`Up`** — a hold that exceeded the threshold releases as a long-press
//!   instead of a tap ("long-press-on-release"); and
//! - on a **`Move`** (within slop) arriving after the threshold, so a context
//!   menu can open the instant the finger jitters even before release.
//!
//! The paint timer only runs when an [`on_long_press`](GestureDetectorView::on_long_press)
//! handler is wired: a **tap-only** detector never marks `elapsed`, so a press
//! held past the threshold still resolves as an ordinary tap on an in-bounds
//! release (the fire-on-up-inside contract is independent of hold duration when
//! there is no long-press to promote it to). Not running the timer for a
//! tap-only press also skips the pending-timer [`PaintCtx::request_frame`]
//! calls — a small battery win, since a tap-only hold needs no clock. A
//! held-past-threshold resolution falls through to `on_tap` whenever
//! `on_long_press` is absent (e.g. unwired mid-gesture), so a held press never
//! silently swallows its release.
//!
//! Honest limitation: a *true* timer-fires-while-held-perfectly-still gesture
//! would need an event-pass clock the framework doesn't have. What this lands is
//! hold-exceeds-threshold semantics, which feels native for context menus. Frame
//! liveness while the finger is held still is guaranteed on mobile by the pointer
//! capture keeping the `FrameGate` running; desktop is dirty-driven, so paint
//! calls [`PaintCtx::request_frame`] while the timer is pending so it keeps
//! painting with no input until the threshold is reached.

use std::rc::Rc;

use forgekit_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, FrameTime,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, TOUCH_SLOP, View,
    Widget, any,
};
use kurbo::{Point, Size};

/// The press duration after which a held press becomes a long-press.
///
/// **Community-approximate**: neither platform publishes this as a stable
/// numeric constant, but the community-converged values agree closely —
/// Android's `ViewConfiguration` long-press timeout defaults to ~400–500ms and
/// iOS's `UILongPressGestureRecognizer.minimumPressDuration` defaults to 0.5s.
/// 500ms sits at that shared upper bound.
const LONG_PRESS_MS: f64 = 500.0;

/// A view-held, typed gesture callback (erased on build). Shared by `on_tap`
/// and `on_long_press` — both are `Fn(&mut State)`.
type GestureCallback<State> = Rc<dyn Fn(&mut State)>;

/// A declarative gesture wrapper. See the [module docs](self).
pub struct GestureDetectorView<State: 'static> {
    child: AnyView<State>,
    on_tap: Option<GestureCallback<State>>,
    on_long_press: Option<GestureCallback<State>>,
}

/// Wrap `child` in a gesture detector (no recognisers until one is attached,
/// e.g. with [`GestureDetectorView::on_tap`] or
/// [`GestureDetectorView::on_long_press`]).
#[allow(non_snake_case)]
pub fn GestureDetector<State: 'static, V: View<State>>(child: V) -> GestureDetectorView<State> {
    GestureDetectorView {
        child: any(child),
        on_tap: None,
        on_long_press: None,
    }
}

impl<State: 'static> GestureDetectorView<State> {
    /// Fire `on_tap` when the child is tapped (a press+release within
    /// [`TOUCH_SLOP`] and shorter than [`LONG_PRESS_MS`]).
    pub fn on_tap<F: Fn(&mut State) + 'static>(mut self, on_tap: F) -> Self {
        self.on_tap = Some(Rc::new(on_tap));
        self
    }

    /// Fire `on_long_press` when the child is pressed and held past
    /// [`LONG_PRESS_MS`] without moving beyond [`TOUCH_SLOP`]. Composes with
    /// [`on_tap`](Self::on_tap): a given press fires exactly one of the two,
    /// never both (see the [module docs](self) for the firing semantics).
    pub fn on_long_press<F: Fn(&mut State) + 'static>(mut self, on_long_press: F) -> Self {
        self.on_long_press = Some(Rc::new(on_long_press));
        self
    }
}

/// The gesture recogniser's state machine. `Copy` so an event arm can inspect it
/// by value without holding a borrow of the widget while it reassigns.
#[derive(Clone, Copy)]
enum Recognizer {
    /// No press in flight.
    Idle,
    /// A press is down and still within the slop (tap + long-press both viable).
    /// `press_start` is recorded on the first paint after `Down`; `elapsed`
    /// flips once a paint observes the hold exceeding [`LONG_PRESS_MS`].
    Pressed {
        down_pos: Point,
        press_start: Option<FrameTime>,
        elapsed: bool,
    },
    /// The press moved past the slop — became a drag; neither recogniser fires.
    Dragged,
    /// A long-press already fired for this press (on a post-threshold `Move`);
    /// we hold capture until `Up`/`Cancel` but fire nothing further.
    Fired,
}

/// The retained widget for a [`GestureDetectorView`].
pub struct GestureDetectorWidget {
    child: ChildPod,
    state: Recognizer,
    on_tap: Option<crate::ErasedCallback>,
    on_long_press: Option<crate::ErasedCallback>,
}

impl<State: 'static> View<State> for GestureDetectorView<State> {
    type Element = GestureDetectorWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> GestureDetectorWidget {
        GestureDetectorWidget {
            child: crate::build_child(&self.child, ctx),
            state: Recognizer::Idle,
            on_tap: self.on_tap.as_ref().map(crate::erase_callback),
            on_long_press: self.on_long_press.as_ref().map(crate::erase_callback),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut GestureDetectorWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_tap = self.on_tap.as_ref().map(crate::erase_callback);
        element.on_long_press = self.on_long_press.as_ref().map(crate::erase_callback);
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
        // Measure the long-press across paints (the only pass with a clock).
        // Record the press epoch on the first paint, mark `elapsed` once the
        // hold exceeds the threshold, and — while the timer is still pending —
        // request another frame so the dirty-driven desktop shell keeps painting
        // with no input (mobile keeps the FrameGate alive via pointer capture).
        //
        // Only run the timer when an `on_long_press` handler is wired: a
        // tap-only detector has no long-press to mark, so it neither flips
        // `elapsed` (which would otherwise swallow the release, since a held
        // press resolves as a would-be long-press that fires nothing) nor
        // request_frame's pointlessly during the hold — a small battery win.
        if self.on_long_press.is_some()
            && let Recognizer::Pressed {
                press_start,
                elapsed,
                ..
            } = &mut self.state
        {
            let start = *press_start.get_or_insert(ctx.frame_time());
            if !*elapsed {
                if ctx.frame_time().saturating_sub(start).as_secs_f64() * 1000.0 >= LONG_PRESS_MS {
                    *elapsed = true; // paint only marks; the callback fires on the next event
                } else {
                    ctx.request_frame();
                }
            }
        }
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Pointer(p) = event {
            match p.phase {
                PointerPhase::Down => {
                    self.state = Recognizer::Pressed {
                        down_pos: p.position,
                        press_start: None,
                        elapsed: false,
                    };
                    ctx.capture_pointer();
                    // Prompt a paint so the long-press timer starts promptly.
                    ctx.request_redraw();
                    self.child.event_child(ctx, event);
                    return EventResult::Handled;
                }
                PointerPhase::Move => {
                    if let Recognizer::Pressed {
                        down_pos, elapsed, ..
                    } = self.state
                    {
                        if (p.position - down_pos).hypot() > TOUCH_SLOP {
                            self.state = Recognizer::Dragged; // became a drag; disarms both
                        } else if elapsed {
                            // Threshold already passed and a pointer event arrived:
                            // fire the long-press now (fire-on-move-arrival). If the
                            // handler was unwired mid-gesture (rebuild between the
                            // marking paint and this Move), fall through to on_tap
                            // so the press still resolves rather than silently dying.
                            self.state = Recognizer::Fired;
                            self.child.event_child(ctx, event);
                            if let Some(cb) = self.on_long_press.as_mut() {
                                cb(ctx);
                                ctx.request_redraw();
                            } else if let Some(cb) = self.on_tap.as_mut() {
                                cb(ctx);
                                ctx.request_redraw();
                            }
                            return EventResult::Handled;
                        }
                    }
                    self.child.event_child(ctx, event);
                    return EventResult::Handled;
                }
                PointerPhase::Up => {
                    // Resolve at most one recogniser: a held-past-threshold press
                    // releases as a long-press, otherwise a within-slop press is
                    // a tap. Never both.
                    let fire_long = matches!(self.state, Recognizer::Pressed { elapsed: true, .. });
                    let fire_tap = matches!(self.state, Recognizer::Pressed { elapsed: false, .. });
                    self.state = Recognizer::Idle;
                    self.child.event_child(ctx, event);
                    self.child.set_active(false);
                    // A held-past-threshold press prefers on_long_press, but falls
                    // through to on_tap when no long-press handler is wired — a
                    // tap-only detector must still fire the tap on an in-bounds
                    // release (the paint timer never marks `elapsed` for it, but
                    // stay robust to a handler unwired mid-gesture).
                    if fire_long && let Some(cb) = self.on_long_press.as_mut() {
                        cb(ctx);
                        ctx.request_redraw();
                    } else if (fire_tap || fire_long)
                        && let Some(cb) = self.on_tap.as_mut()
                    {
                        cb(ctx);
                        ctx.request_redraw();
                    }
                    return EventResult::Handled;
                }
                PointerPhase::Cancel => {
                    // Cancel only clears internal flags — never touches app state
                    // (no callback), per the interaction-semantics contract.
                    self.state = Recognizer::Idle;
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
        // Transparent gesture-recognizer wrapper: forward to the single child.
        self.child.semantics_child(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forgekit_core::RenderRoot;
    use std::any::Any;

    #[derive(Default)]
    struct TapState {
        taps: u32,
        long_presses: u32,
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

    // --- Long-press (task 03) ------------------------------------------------
    //
    // The long-press timer is measured across paints, so these tests drive the
    // real paint path through `RenderRoot` (the only place a `FrameTime` is
    // threaded in) with an advancing injected frame clock, exactly like the
    // scroll/textinput clock tests.

    /// A no-op paint sink for the clock-driven tests.
    struct NullScene;
    impl PaintScene for NullScene {
        fn fill_rect(&mut self, _o: Point, _s: Size, _c: peniko::Color) {}
        fn draw_text(&mut self, _o: Point, _t: &str) {}
    }

    /// A frame time `ms` milliseconds from an arbitrary origin.
    fn ft(ms: f64) -> FrameTime {
        FrameTime::from_nanos((ms * 1_000_000.0) as u64)
    }

    /// A `RenderRoot` over a `GestureDetector` wired with both callbacks.
    fn root() -> RenderRoot<TapState, GestureDetectorView<TapState>> {
        fn logic(_: &mut TapState) -> GestureDetectorView<TapState> {
            GestureDetector::<TapState, _>(Blank)
                .on_tap(|s: &mut TapState| s.taps += 1)
                .on_long_press(|s: &mut TapState| s.long_presses += 1)
        }
        let mut root = RenderRoot::new();
        let mut state = TapState::default();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root
    }

    #[test]
    fn hold_past_threshold_then_up_fires_long_press_only() {
        let mut root = root();
        let mut state = TapState::default();
        let mut sink = NullScene;
        root.event(&mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0)); // records press_start = 0
        root.paint(&mut sink, ft(600.0)); // 600ms >= 500ms → elapsed marked
        root.event(&mut state, &ev(PointerPhase::Up, 11.0, 11.0));
        assert_eq!(state.long_presses, 1, "held press releases as a long-press");
        assert_eq!(state.taps, 0, "a long-press must not also fire on_tap");
    }

    #[test]
    fn quick_tap_before_threshold_fires_tap_only() {
        let mut root = root();
        let mut state = TapState::default();
        let mut sink = NullScene;
        root.event(&mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0));
        root.paint(&mut sink, ft(100.0)); // 100ms < 500ms → not elapsed
        root.event(&mut state, &ev(PointerPhase::Up, 12.0, 12.0));
        assert_eq!(state.taps, 1, "a quick release is a tap");
        assert_eq!(state.long_presses, 0, "under threshold must not long-press");
    }

    #[test]
    fn move_after_threshold_fires_long_press_before_release() {
        let mut root = root();
        let mut state = TapState::default();
        let mut sink = NullScene;
        root.event(&mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0));
        root.paint(&mut sink, ft(600.0)); // elapsed
        // A tiny jitter (within slop) after the threshold fires immediately.
        root.event(&mut state, &ev(PointerPhase::Move, 12.0, 11.0));
        assert_eq!(state.long_presses, 1, "fires on the post-threshold move");
        // A following Up must not double-fire (long or tap).
        root.event(&mut state, &ev(PointerPhase::Up, 12.0, 11.0));
        assert_eq!(
            state.long_presses, 1,
            "Up after a fired long-press is inert"
        );
        assert_eq!(state.taps, 0);
    }

    #[test]
    fn hold_then_drag_then_up_fires_neither() {
        let mut root = root();
        let mut state = TapState::default();
        let mut sink = NullScene;
        root.event(&mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0));
        root.paint(&mut sink, ft(600.0)); // elapsed
        // Drag past the slop wins over the elapsed timer.
        root.event(&mut state, &ev(PointerPhase::Move, 10.0, 40.0));
        root.event(&mut state, &ev(PointerPhase::Up, 10.0, 40.0));
        assert_eq!(state.long_presses, 0, "a drag disarms the long-press");
        assert_eq!(state.taps, 0, "a drag disarms the tap");
    }

    #[test]
    fn cancel_mid_hold_fires_neither_and_leaves_state_untouched() {
        let mut root = root();
        let mut state = TapState::default();
        let mut sink = NullScene;
        root.event(&mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0));
        root.paint(&mut sink, ft(600.0)); // elapsed
        root.event(&mut state, &ev(PointerPhase::Cancel, 10.0, 10.0));
        root.event(&mut state, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(state.long_presses, 0, "Cancel fires no long-press");
        assert_eq!(state.taps, 0, "Cancel fires no tap");
    }

    #[test]
    fn paint_requests_frames_while_pressed_and_stops_after_up() {
        let mut root = root();
        let mut state = TapState::default();
        let mut sink = NullScene;
        root.event(&mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        // The timer is pending: paint asks the shell to keep painting.
        assert!(
            root.paint(&mut sink, ft(0.0)).needs_frame,
            "a pending long-press timer requests continuation frames"
        );
        root.event(&mut state, &ev(PointerPhase::Up, 10.0, 10.0));
        // Released: no timer, no continuation frame.
        assert!(
            !root.paint(&mut sink, ft(16.0)).needs_frame,
            "the timer stops requesting frames after Up"
        );
    }

    #[test]
    fn only_on_tap_still_behaves_as_before() {
        // Regression: a detector with no long-press wired taps exactly as v1.
        let mut w = widget(true);
        let mut state = TapState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 12.0, 12.0));
        assert_eq!(state.taps, 1);
        assert_eq!(state.long_presses, 0);
    }

    // --- Tap-only held-press regression (review A1 / fix F1) -----------------
    //
    // The event-only `only_on_tap_still_behaves_as_before` above never paints,
    // so it never advanced the clock past the threshold and missed the bug: a
    // tap-only detector held past LONG_PRESS_MS used to mark `elapsed` in paint,
    // resolve as a would-be long-press with no handler, and fire NOTHING. These
    // drive the real paint path with an advancing clock (like the both-handlers
    // tests above) but wire ONLY `on_tap`.

    /// A `RenderRoot` over a `GestureDetector` wired with ONLY `on_tap`.
    fn tap_only_root() -> RenderRoot<TapState, GestureDetectorView<TapState>> {
        fn logic(_: &mut TapState) -> GestureDetectorView<TapState> {
            GestureDetector::<TapState, _>(Blank).on_tap(|s: &mut TapState| s.taps += 1)
        }
        let mut root = RenderRoot::new();
        let mut state = TapState::default();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root
    }

    #[test]
    fn tap_only_hold_past_threshold_then_up_still_fires_tap() {
        // (a) hold past the threshold, release in-bounds → on_tap fires.
        let mut root = tap_only_root();
        let mut state = TapState::default();
        let mut sink = NullScene;
        root.event(&mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0)); // records press_start = 0
        root.paint(&mut sink, ft(600.0)); // 600ms >= 500ms, but no long-press wired
        root.event(&mut state, &ev(PointerPhase::Up, 11.0, 11.0));
        assert_eq!(
            state.taps, 1,
            "a held press with no on_long_press still taps on release"
        );
        assert_eq!(state.long_presses, 0);
    }

    #[test]
    fn tap_only_hold_then_within_slop_move_taps_on_release_not_on_move() {
        // (b) hold past the threshold, a within-slop Move arrives after elapsed →
        // nothing at the Move; on_tap still fires on release.
        let mut root = tap_only_root();
        let mut state = TapState::default();
        let mut sink = NullScene;
        root.event(&mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0));
        root.paint(&mut sink, ft(600.0));
        root.event(&mut state, &ev(PointerPhase::Move, 12.0, 11.0));
        assert_eq!(
            state.taps, 0,
            "a tap-only detector fires nothing on the move"
        );
        assert_eq!(state.long_presses, 0);
        root.event(&mut state, &ev(PointerPhase::Up, 12.0, 11.0));
        assert_eq!(state.taps, 1, "the tap fires on the in-bounds release");
        assert_eq!(state.long_presses, 0);
    }

    #[test]
    fn tap_only_press_requests_no_continuation_frames() {
        // The battery win: a tap-only detector runs no long-press timer, so a
        // pending press must not keep the dirty-driven shell painting.
        let mut root = tap_only_root();
        let mut state = TapState::default();
        let mut sink = NullScene;
        root.event(&mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        assert!(
            !root.paint(&mut sink, ft(0.0)).needs_frame,
            "a tap-only press has no timer, so it requests no continuation frames"
        );
    }

    #[test]
    fn both_handlers_long_press_behavior_unchanged() {
        // (c) with both handlers wired, a held-past-threshold release is still a
        // long-press and never also a tap — the fix must not regress this.
        let mut root = root();
        let mut state = TapState::default();
        let mut sink = NullScene;
        root.event(&mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0));
        root.paint(&mut sink, ft(600.0));
        root.event(&mut state, &ev(PointerPhase::Up, 11.0, 11.0));
        assert_eq!(state.long_presses, 1, "both wired: hold still long-presses");
        assert_eq!(state.taps, 0, "both wired: a long-press never also taps");
    }
}
