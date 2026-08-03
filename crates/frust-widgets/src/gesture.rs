//! The `GestureDetector` widget: a transparent wrapper that
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
//! slop is [`TOUCH_SLOP`] uniformly).
//!
//! # Long-press firing semantics
//!
//! Timing is measured across **paints**, not events: only the paint pass
//! carries a clock ([`PaintCtx::frame_time`]), while events carry none. A press
//! records `press_start` on its first paint and marks itself *elapsed* on a
//! later paint once `frame_time - press_start >= LONG_PRESS_MS`.
//!
//! **The callback still never fires during paint itself** (there is no `&mut
//! State`/`EventCtx` there) — but paint no longer waits for the next *pointer*
//! event to deliver it either. The instant a paint observes the threshold
//! crossed, it calls [`frust_core::mark_pending_result_flush`] — the same
//! mechanism [`crate::nav::navigator`] uses to run a queued pop-result
//! callback from a state-free `View::rebuild` pass — and
//! [`PaintCtx::request_frame`]s a follow-up frame. Every shipping shell
//! (desktop/Android/iOS) runs `RenderRoot::rebuild` before `RenderRoot::paint`
//! on every frame it drives, so the very next frame's rebuild drains that
//! mark, dispatches an `InputEvent::Housekeeping` broadcast carrying a real
//! `EventCtx`, and this widget fires `on_long_press` from there — **at the
//! threshold, in wall time**, whether or not the finger ever moves again. The
//! timer, the slop/cancel rules, and the fire-exactly-once guarantee below are
//! unchanged; what the new vehicle *does* change is where the fire's
//! `EventCtx::request_redraw` goes. A pointer-delivered fire's redraw request
//! rides the shell's own `RenderRoot::event` call back out; a
//! Housekeeping-delivered one is dispatched by `RenderRoot::rebuild` itself, so
//! the repaint reaches the shell only because that method **propagates the
//! broadcast's `EventOutcome`** — folding `needs_redraw` into the rebuild's
//! `ChangeFlags::PAINT` (the mobile frame gate's `has_pending_change_flags`
//! input) and into the deferred frame request the next `paint` surfaces as
//! `needs_frame` (the desktop `ControlFlow::Wait` loop's wake). That fold is
//! part of the flush contract, pinned core-side (`frust-core`'s
//! `RenderRoot::rebuild`, its deferred-callback flush loop and that method's
//! own tests) — a fire whose consumer mutates nothing the view diff can see
//! still repaints on both loop styles because of it.
//!
//! Two delivery paths race for the same fire, and whichever reaches the widget
//! first wins — the `Recognizer::Fired` transition makes the other one a
//! no-op:
//!
//! - the **Housekeeping broadcast** above, ordinarily one frame after the
//!   threshold, needing no pointer event at all; or
//! - a **live pointer event** that arrives first — `Up` (a hold that exceeded
//!   the threshold releases as a long-press instead of a tap) or an in-slop
//!   `Move` (so a context menu can still open the instant a held finger
//!   jitters, without waiting out the extra frame).
//!
//! The Housekeeping path runs only when `on_long_press` is wired (mirroring
//! the paint timer's own gate below): an `on_hold_progress`-only detector
//! keeps its final observation deferred to the next pointer event exactly as
//! before — nothing here moves a hold-progress-only consumer onto the new
//! path. When both are wired and `on_long_press` fires via Housekeeping,
//! `on_hold_progress`'s paired final `1.0` observation (see below) rides along
//! in the same pass, exactly as the `Move`-arrival path has always delivered
//! them together.
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
//! Frame liveness while the finger is held still is guaranteed on mobile by
//! the pointer capture keeping the `FrameGate` running; desktop is
//! dirty-driven, so paint calls [`PaintCtx::request_frame`] both while the
//! timer is pending, and once more on the frame that crosses the threshold —
//! that second request is what actually reaches the Housekeeping-flushing
//! rebuild above on a desktop shell with no further input.
//!
//! # Hold-progress observation
//!
//! [`on_hold_progress`](GestureDetectorView::on_hold_progress) reuses the same
//! paint-clock timer as `on_long_press` to let a widget render charge-up
//! feedback (e.g. a charge ring) for a held press. Paint computes progress —
//! `(frame_time - press_start) / threshold`, clamped `0.0..=1.0` — every frame
//! while a press is pending, but (the same callback-from-paint caveat as
//! `on_long_press` above) can only *record* it; the observation is delivered on
//! the next pointer event that already carries a mutable `EventCtx` (an in-slop
//! `Move`, matching `on_long_press`'s fire-on-move-arrival) — **or, when
//! `on_long_press` is also wired, on the Housekeeping-triggered fire above,
//! whichever reaches the widget first.** A drag past
//! [`TOUCH_SLOP`] or an early release *before* the threshold — i.e. the
//! `Move`/`Up` arm handling that transition, never `PointerPhase::Cancel`
//! (see the Cancel staleness gap below) — delivers a final `0.0`; reaching the
//! threshold delivers a final `1.0`. An `on_hold_progress`-only detector (no
//! `on_long_press` wired) keeps the original deferred-to-next-pointer-event
//! delivery for its final `1.0` too — the Housekeeping path never runs for it.
//! [`hold_threshold_ms`](GestureDetectorView::hold_threshold_ms)
//! overrides [`LONG_PRESS_MS`] for both `on_long_press` and this timer.
//!
//! ## Cancel staleness gap
//!
//! A platform `Cancel` (gesture steal, e.g. a parent `ScrollView` claiming
//! the drag; or structural teardown, e.g. the child subtree changing shape
//! mid-hold) delivers **no** final observation — the Cancel-never-mutates-state
//! convention above means `on_hold_progress` is never called from that arm.
//! The consumer's last-observed `progress` therefore stays stale (whatever it
//! was mid-hold) until a full new press cycle (`Down`→...→`Up`/threshold)
//! delivers a fresh `0.0`/`1.0` through the normal path — there is no
//! Cancel-delivered reset. **Consumer-side reset idiom:** since a fresh press
//! cycle's *first* observation is always a low value counting up from near
//! `0.0`, a consumer that stored a stale non-zero `progress` can self-correct
//! at the top of its `on_hold_progress` callback by detecting a restart (the
//! incoming value is lower than the last-stored one after a gap) and treating
//! it as the new cycle's baseline rather than carrying the stale high value
//! forward — see `demo_charge_ring` in `examples/glyph-catalog`'s
//! `pages/interactions.rs` for a worked instance. Do not add a Cancel-arm
//! callback to close this gap; it would violate the Cancel-never-mutates-state
//! convention (`docs/CODE_STANDARDS.md`).

use std::rc::Rc;

use frust_core::{
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
    on_hold_progress: Option<crate::authoring::TypedArgCallback<State, f64>>,
    hold_threshold_ms: Option<u64>,
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
        on_hold_progress: None,
        hold_threshold_ms: None,
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

    /// Observe press-hold progress (`0.0..=1.0` against
    /// [`hold_threshold_ms`](Self::hold_threshold_ms), default [`LONG_PRESS_MS`])
    /// while the child is held — the primitive a charge-up ring or similar hold
    /// feedback widget renders from. See the [module docs](self#hold-progress-observation)
    /// for the delivery/firing semantics: paint-clock timed, delivered on the
    /// next pointer event, with a final `0.0` on an early lift/drag-past-slop
    /// or a final `1.0` on reaching the threshold.
    ///
    /// **Cancel staleness gap**: a platform `PointerPhase::Cancel` (gesture
    /// steal or structural teardown) delivers **no** final observation — the
    /// last value this callback saw stays stale until the next full press
    /// cycle. See the [module docs](self#cancel-staleness-gap) for the
    /// consumer-side reset idiom that self-corrects on the next touch-down
    /// rather than waiting for a full press-release.
    pub fn on_hold_progress<F: Fn(&mut State, f64) + 'static>(
        mut self,
        on_hold_progress: F,
    ) -> Self {
        self.on_hold_progress = Some(Rc::new(on_hold_progress));
        self
    }

    /// Override the hold threshold (milliseconds), used by both
    /// [`on_long_press`](Self::on_long_press) and
    /// [`on_hold_progress`](Self::on_hold_progress); defaults to [`LONG_PRESS_MS`].
    ///
    /// Resolved with a floor of 1ms (`ms.max(1)`) at both `build`/`rebuild` —
    /// `hold_threshold_ms(0)` never divides progress by zero (`f64::clamp`
    /// passes a `NaN` numerator/denominator-zero result straight through
    /// rather than clamping it away), so it instead yields a defined, finite
    /// progress that reaches `1.0` on the very next paint.
    pub fn hold_threshold_ms(mut self, ms: u64) -> Self {
        self.hold_threshold_ms = Some(ms);
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
    /// flips once a paint observes the hold exceeding the threshold — the same
    /// paint also latches a Housekeeping flush when `on_long_press` is wired
    /// (see the [module docs](self#long-press-firing-semantics)). `progress`
    /// is paint's latest `0.0..=1.0` hold-progress observation, delivered to
    /// `on_hold_progress` on the next pointer event or the Housekeeping-triggered
    /// fire, whichever arrives first (see the [module
    /// docs](self#hold-progress-observation)).
    Pressed {
        down_pos: Point,
        press_start: Option<FrameTime>,
        elapsed: bool,
        progress: f64,
    },
    /// The press moved past the slop — became a drag; neither recogniser fires.
    Dragged,
    /// A long-press already fired for this press — via the threshold-time
    /// Housekeeping broadcast (the common case; see the [module
    /// docs](self#long-press-firing-semantics)) or a post-threshold `Move`/`Up`
    /// arrival that won the race instead; we hold capture until `Up`/`Cancel`
    /// but fire nothing further.
    Fired,
}

/// The retained widget for a [`GestureDetectorView`].
pub struct GestureDetectorWidget {
    child: ChildPod,
    state: Recognizer,
    on_tap: Option<crate::authoring::ErasedCallback>,
    on_long_press: Option<crate::authoring::ErasedCallback>,
    on_hold_progress: Option<crate::authoring::ErasedArgCallback<f64>>,
    /// Resolved hold threshold in ms — [`GestureDetectorView::hold_threshold_ms`]
    /// if set, else [`LONG_PRESS_MS`]. Shared by the long-press timer and the
    /// hold-progress computation.
    threshold_ms: f64,
}

impl<State: 'static> View<State> for GestureDetectorView<State> {
    type Element = GestureDetectorWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> GestureDetectorWidget {
        GestureDetectorWidget {
            child: crate::authoring::build_child(&self.child, ctx),
            state: Recognizer::Idle,
            on_tap: self.on_tap.as_ref().map(crate::authoring::erase_callback),
            on_long_press: self
                .on_long_press
                .as_ref()
                .map(crate::authoring::erase_callback),
            on_hold_progress: self
                .on_hold_progress
                .as_ref()
                .map(crate::authoring::erase_callback_arg),
            // `.max(1)` guards against a 0ms override: `f64::clamp` passes
            // NaN through unchanged, so an unguarded `0.0 / 0.0` divisor in
            // paint's progress computation would poison every subsequent
            // observation. See `hold_threshold_ms`'s doc comment.
            threshold_ms: self
                .hold_threshold_ms
                .map(|ms| ms.max(1) as f64)
                .unwrap_or(LONG_PRESS_MS),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut GestureDetectorWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        element.on_tap = self.on_tap.as_ref().map(crate::authoring::erase_callback);
        element.on_long_press = self
            .on_long_press
            .as_ref()
            .map(crate::authoring::erase_callback);
        element.on_hold_progress = self
            .on_hold_progress
            .as_ref()
            .map(crate::authoring::erase_callback_arg);
        // Same NaN guard as `build` above — see `hold_threshold_ms`'s doc comment.
        element.threshold_ms = self
            .hold_threshold_ms
            .map(|ms| ms.max(1) as f64)
            .unwrap_or(LONG_PRESS_MS);
        crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut GestureDetectorWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for GestureDetectorWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        // Measure the long-press/hold-progress across paints (the only pass
        // with a clock). Record the press epoch on the first paint, mark
        // `elapsed` once the hold exceeds the threshold, refresh the
        // `0.0..=1.0` progress observation every paint, and — while the timer
        // is still pending — request another frame so the dirty-driven desktop
        // shell keeps painting with no input (mobile keeps the FrameGate alive
        // via pointer capture).
        //
        // Only run the timer when an `on_long_press`/`on_hold_progress` handler
        // is wired: a tap-only detector has no long-press to mark and no
        // progress to observe, so it neither flips `elapsed` (which would
        // otherwise swallow the release, since a held press resolves as a
        // would-be long-press that fires nothing) nor request_frame's
        // pointlessly during the hold — a small battery win.
        if (self.on_long_press.is_some() || self.on_hold_progress.is_some())
            && let Recognizer::Pressed {
                press_start,
                elapsed,
                progress,
                ..
            } = &mut self.state
        {
            let start = *press_start.get_or_insert(ctx.frame_time());
            let elapsed_ms = ctx.frame_time().saturating_sub(start).as_secs_f64() * 1000.0;
            *progress = (elapsed_ms / self.threshold_ms).clamp(0.0, 1.0);
            if !*elapsed {
                if elapsed_ms >= self.threshold_ms {
                    *elapsed = true;
                    // Threshold-time firing (see the module docs' "Long-press
                    // firing semantics"): paint still can't call `on_long_press`
                    // itself (no `EventCtx` here), but it no longer waits for
                    // the next pointer event either. Latch the same
                    // deferred-callback flush `nav::navigator` uses for a
                    // queued pop-result, and request one more frame so a
                    // dirty-driven desktop shell actually reaches the
                    // `RenderRoot::rebuild` that drains it — every shell runs
                    // rebuild (with a real `EventCtx`-bearing `Housekeeping`
                    // dispatch) before its own paint, so the very next frame
                    // fires this at the threshold, in wall time, with no
                    // pointer event required. Gated on `on_long_press` being
                    // wired: an `on_hold_progress`-only detector keeps its
                    // original deferred-to-next-pointer-event delivery.
                    if self.on_long_press.is_some() {
                        frust_core::mark_pending_result_flush();
                        ctx.request_frame();
                    }
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
                        progress: 0.0,
                    };
                    ctx.capture_pointer();
                    // Prompt a paint so the long-press/progress timer starts promptly.
                    ctx.request_redraw();
                    self.child.event_child(ctx, event);
                    return EventResult::Handled;
                }
                PointerPhase::Move => {
                    if let Recognizer::Pressed {
                        down_pos,
                        elapsed,
                        progress,
                        ..
                    } = self.state
                    {
                        if (p.position - down_pos).hypot() > TOUCH_SLOP {
                            // Became a drag — disarms both. A mid-hold progress
                            // observation resets to exactly 0.0 (never a stale
                            // >0 value left behind); this is the Move arm, not
                            // Cancel, so it's allowed to touch state.
                            self.state = Recognizer::Dragged;
                            if let Some(cb) = self.on_hold_progress.as_mut() {
                                cb(ctx, 0.0);
                                ctx.request_redraw();
                            }
                        } else {
                            // Deliver the latest paint-observed progress: paint has
                            // no EventCtx, so this in-slop Move is the deferred-fire
                            // opportunity (mirrors on_long_press's fire-on-move-arrival
                            // below — this also delivers the final 1.0 once `elapsed`).
                            if let Some(cb) = self.on_hold_progress.as_mut() {
                                cb(ctx, progress);
                                ctx.request_redraw();
                            }
                            if elapsed {
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
                    // A final hold-progress observation, only if the press was
                    // still `Pressed` at Up (a `Fired`/`Dragged` state already
                    // delivered its final call from the Move arm above): 1.0 if
                    // the threshold was reached, else a clean 0.0 (early lift).
                    let final_progress = match self.state {
                        Recognizer::Pressed { elapsed: true, .. } => Some(1.0),
                        Recognizer::Pressed { elapsed: false, .. } => Some(0.0),
                        _ => None,
                    };
                    self.state = Recognizer::Idle;
                    self.child.event_child(ctx, event);
                    self.child.set_active(false);
                    if let Some(progress) = final_progress
                        && let Some(cb) = self.on_hold_progress.as_mut()
                    {
                        cb(ctx, progress);
                        ctx.request_redraw();
                    }
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
        if event.is_broadcast() {
            // `InputEvent::Housekeeping` today: the delivery vehicle for a
            // threshold-time long-press latched during an earlier paint (see
            // the module docs' "Long-press firing semantics") — the soonest
            // pass carrying a real `EventCtx` when no pointer event arrived
            // first. Gated on `on_long_press` being wired, mirroring paint's
            // own gate: an `on_hold_progress`-only press never reaches this
            // arm, so it keeps its original fire-on-move/up-arrival delivery
            // untouched.
            if self.on_long_press.is_some()
                && let Recognizer::Pressed {
                    elapsed: true,
                    progress,
                    ..
                } = self.state
            {
                // Whichever of this broadcast or a live pointer event reaches
                // the widget first wins the fire; the other finds `state` no
                // longer `Pressed` and is a no-op (fire-exactly-once).
                self.state = Recognizer::Fired;
                if let Some(cb) = self.on_long_press.as_mut() {
                    cb(ctx);
                    ctx.request_redraw();
                }
                // `on_hold_progress`'s paired final observation (already
                // pinned at 1.0 by `elapsed`) rides along in the same pass,
                // exactly as the Move-arrival path always delivered them
                // together.
                if let Some(cb) = self.on_hold_progress.as_mut() {
                    cb(ctx, progress);
                    ctx.request_redraw();
                }
            }
            // A broadcast is never consumed and always forwarded to the
            // child unconditionally, reporting Ignored regardless of what it
            // returns (`docs/CODE_STANDARDS.md`'s interaction-semantics
            // convention).
            self.child.event_child(ctx, event);
            return EventResult::Ignored;
        }
        // Non-broadcast, non-pointer events (scroll/key/IME) pass straight
        // through to the child, reporting whatever it returns.
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
    use frust_core::RenderRoot;
    use std::any::Any;

    #[derive(Default)]
    struct TapState {
        taps: u32,
        long_presses: u32,
    }

    /// A trivial full-bleed child that ignores events. Generic over `State` so
    /// it's reusable across both `TapState` and `HoldState` test fixtures.
    struct Blank;
    struct BlankWidget;
    impl<State: 'static> View<State> for Blank {
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
        InputEvent::Pointer(frust_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust_core::PointerButton::Primary,
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

    // --- Long-press ------------------------------------------------------------
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

    /// The same view `root()` builds, standalone — for a test that needs to
    /// call `RenderRoot::rebuild` a second time against the same retained
    /// widget (simulating a real shell's next per-frame rebuild).
    fn long_press_logic(_: &mut TapState) -> GestureDetectorView<TapState> {
        GestureDetector::<TapState, _>(Blank)
            .on_tap(|s: &mut TapState| s.taps += 1)
            .on_long_press(|s: &mut TapState| s.long_presses += 1)
    }

    #[test]
    fn stationary_hold_fires_on_long_press_at_the_threshold_with_no_pointer_event() {
        // The regression this task fixes: before it, a perfectly stationary
        // finger got nothing until `Up` — paint only *marked* `elapsed`, and
        // the callback fired on the next pointer event (`Move`/`Up`). Now
        // paint latches a Housekeeping flush the instant it crosses the
        // threshold, and the very next `RenderRoot::rebuild` (which every
        // shell runs before its own paint) drains it and fires — with zero
        // pointer events anywhere in this test.
        let mut root = root();
        let mut state = TapState::default();
        let mut sink = NullScene;
        root.event(&mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0)); // records press_start = 0
        let crossing = root.paint(&mut sink, ft(600.0)); // 600ms >= 500ms → elapsed marked, flush latched
        assert!(
            crossing.needs_frame,
            "the threshold-crossing paint requests a follow-up frame so a \
             dirty-driven desktop shell actually reaches the rebuild that fires"
        );
        // Paint alone never fires — there is no `EventCtx` in that pass.
        assert_eq!(state.long_presses, 0, "paint only latches; it never fires");

        // The follow-up frame: every shell runs `rebuild` before `paint`,
        // and `rebuild` is what drains the flush and dispatches the
        // `Housekeeping` broadcast this widget fires from.
        let flags = root.rebuild(&mut long_press_logic, &mut state);
        assert_eq!(
            state.long_presses, 1,
            "a stationary hold fires on_long_press at the threshold, in \
             wall time, with no pointer event"
        );
        assert_eq!(state.taps, 0);

        // End-to-end wake: this consumer mutates only a counter the view never
        // reads, so the re-diff inside `rebuild` sees nothing — the repaint the
        // fire asked for exists only because `RenderRoot::rebuild` propagates
        // the Housekeeping dispatch's `EventOutcome` (see the module docs'
        // "Long-press firing semantics" and `frust-core`'s flush loop).
        assert!(
            flags.needs_paint(),
            "the fire's `request_redraw` folds into the rebuild's flags, which \
             is what the mobile frame gate reads"
        );
        let after = root.paint(&mut sink, ft(620.0));
        assert!(
            after.needs_frame,
            "and surfaces as `needs_frame` for the desktop `Wait` loop — the \
             widget itself stops requesting frames once it has fired"
        );
    }

    #[test]
    fn housekeeping_fire_wins_the_race_and_a_later_move_or_up_is_inert() {
        // If the Housekeeping broadcast reaches the widget before any
        // further pointer event does, it fires there — and a `Move`/`Up`
        // arriving afterwards finds `state` already `Fired`, so neither may
        // double-fire (fire-exactly-once, regardless of which delivery path
        // won the race).
        let mut root = root();
        let mut state = TapState::default();
        let mut sink = NullScene;
        root.event(&mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0));
        root.paint(&mut sink, ft(600.0)); // elapsed marked, flush latched
        root.rebuild(&mut long_press_logic, &mut state); // Housekeeping fires here
        assert_eq!(state.long_presses, 1);

        root.event(&mut state, &ev(PointerPhase::Move, 11.0, 11.0));
        assert_eq!(
            state.long_presses, 1,
            "a post-fire Move must not double-fire"
        );
        assert_eq!(state.taps, 0);

        root.event(&mut state, &ev(PointerPhase::Up, 11.0, 11.0));
        assert_eq!(state.long_presses, 1, "a post-fire Up must not double-fire");
        assert_eq!(
            state.taps, 0,
            "a post-fire Up must not fall through to on_tap"
        );
    }

    #[test]
    fn drag_before_threshold_then_rebuild_never_fires_via_housekeeping() {
        // A slop-cancel before the threshold must stay disarmed even across
        // a later rebuild — the flush is only latched at the
        // threshold-crossing paint, which a pre-threshold drag never
        // reaches.
        let mut root = root();
        let mut state = TapState::default();
        let mut sink = NullScene;
        root.event(&mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0));
        root.paint(&mut sink, ft(100.0)); // short of the 500ms threshold
        root.event(&mut state, &ev(PointerPhase::Move, 10.0, 40.0)); // past slop -> Dragged
        root.event(&mut state, &ev(PointerPhase::Up, 10.0, 40.0));
        root.rebuild(&mut long_press_logic, &mut state);
        assert_eq!(
            state.long_presses, 0,
            "a pre-threshold drag never fires via Housekeeping"
        );
        assert_eq!(state.taps, 0, "a dragged press is not a tap either");
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
    fn quick_tap_then_rebuild_never_retroactively_long_presses() {
        // A quick release before the threshold resolves as a tap; a later
        // rebuild (which would dispatch Housekeeping if anything were
        // pending) must not retroactively promote it to a long-press.
        let mut root = root();
        let mut state = TapState::default();
        let mut sink = NullScene;
        root.event(&mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0));
        root.paint(&mut sink, ft(100.0)); // short of the threshold
        root.event(&mut state, &ev(PointerPhase::Up, 12.0, 12.0));
        root.rebuild(&mut long_press_logic, &mut state);
        assert_eq!(state.taps, 1, "the tap already resolved on Up");
        assert_eq!(
            state.long_presses, 0,
            "a subsequent rebuild must not retroactively long-press a released tap"
        );
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

    // --- Tap-only held-press regression --------------------------------------
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

    // --- Hold-progress observation ---------------------------------------------
    //
    // `on_hold_progress` rides the same paint-clock timer as `on_long_press`:
    // paint records the latest 0.0..=1.0 observation, and it's delivered on the
    // next pointer event carrying a mutable `EventCtx` — an in-slop `Move` (the
    // same fire-on-move-arrival opportunity `on_long_press` uses) or `Up`.

    #[derive(Default)]
    struct HoldState {
        taps: u32,
        long_presses: u32,
        progress: Vec<f64>,
    }

    fn hold_ev(phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(frust_core::PointerEvent {
            phase,
            position: Point::new(x, y),
            button: frust_core::PointerButton::Primary,
        })
    }

    /// A `RenderRoot` over a `GestureDetector` wired with all three callbacks,
    /// at the default [`LONG_PRESS_MS`] threshold.
    fn progress_root() -> RenderRoot<HoldState, GestureDetectorView<HoldState>> {
        fn logic(_: &mut HoldState) -> GestureDetectorView<HoldState> {
            GestureDetector::<HoldState, _>(Blank)
                .on_tap(|s: &mut HoldState| s.taps += 1)
                .on_long_press(|s: &mut HoldState| s.long_presses += 1)
                .on_hold_progress(|s: &mut HoldState, p| s.progress.push(p))
        }
        let mut root = RenderRoot::new();
        let mut state = HoldState::default();
        root.rebuild(&mut logic, &mut state);
        root.layout(Size::new(100.0, 100.0));
        root
    }

    /// Like [`progress_root`], but with an overridden `hold_threshold_ms`.
    fn progress_root_with_threshold(
        ms: u64,
    ) -> RenderRoot<HoldState, GestureDetectorView<HoldState>> {
        fn logic(state: &mut HoldState, ms: u64) -> GestureDetectorView<HoldState> {
            let _ = state;
            GestureDetector::<HoldState, _>(Blank)
                .on_long_press(|s: &mut HoldState| s.long_presses += 1)
                .on_hold_progress(|s: &mut HoldState, p| s.progress.push(p))
                .hold_threshold_ms(ms)
        }
        let mut root = RenderRoot::new();
        let mut state = HoldState::default();
        root.rebuild(&mut |s: &mut HoldState| logic(s, ms), &mut state);
        root.layout(Size::new(100.0, 100.0));
        root
    }

    #[test]
    fn hold_progress_increases_monotonically_and_long_press_still_fires() {
        // Criterion 1: progress observations increase monotonically 0->1;
        // on_long_press still fires per its existing contract.
        let mut root = progress_root();
        let mut state = HoldState::default();
        let mut sink = NullScene;
        root.event(&mut state, &hold_ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0));
        root.event(&mut state, &hold_ev(PointerPhase::Move, 10.0, 10.0)); // in-slop jitter
        root.paint(&mut sink, ft(150.0));
        root.event(&mut state, &hold_ev(PointerPhase::Move, 11.0, 10.0));
        root.paint(&mut sink, ft(300.0));
        root.event(&mut state, &hold_ev(PointerPhase::Move, 10.0, 11.0));
        root.paint(&mut sink, ft(500.0)); // elapsed marked (500ms threshold)
        root.event(&mut state, &hold_ev(PointerPhase::Move, 11.0, 11.0)); // fires long-press

        assert_eq!(
            state.progress,
            vec![0.0, 0.3, 0.6, 1.0],
            "progress observations increase monotonically 0..1"
        );
        assert_eq!(state.long_presses, 1, "threshold reached: long-press fires");
        assert_eq!(state.taps, 0);

        // A following Up must not double-fire anything.
        root.event(&mut state, &hold_ev(PointerPhase::Up, 11.0, 11.0));
        assert_eq!(state.long_presses, 1);
        assert_eq!(
            state.progress.len(),
            4,
            "Up after a fired long-press adds no observation"
        );
    }

    /// The same view [`progress_root`] builds, standalone — for a test that
    /// needs to call `RenderRoot::rebuild` a second time against the same
    /// retained widget (simulating a real shell's next per-frame rebuild).
    fn progress_logic(_: &mut HoldState) -> GestureDetectorView<HoldState> {
        GestureDetector::<HoldState, _>(Blank)
            .on_tap(|s: &mut HoldState| s.taps += 1)
            .on_long_press(|s: &mut HoldState| s.long_presses += 1)
            .on_hold_progress(|s: &mut HoldState, p| s.progress.push(p))
    }

    #[test]
    fn housekeeping_fire_delivers_the_paired_final_progress_and_a_later_drag_is_inert() {
        // A perfectly stationary hold with both `on_long_press` and
        // `on_hold_progress` wired: the Housekeeping-triggered fire delivers
        // both in the same pass (matching the paired delivery the
        // Move-arrival path has always given), and — since `state` is
        // `Fired`, not `Pressed`, afterwards — a subsequent Move past the
        // slop must not be mistaken for the Pressed→Dragged transition
        // (which would otherwise re-deliver a spurious final `0.0`); it just
        // forwards to the child, e.g. so an underlying scrollable can keep
        // tracking the drag.
        let mut root = progress_root();
        let mut state = HoldState::default();
        let mut sink = NullScene;
        root.event(&mut state, &hold_ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0));
        root.paint(&mut sink, ft(500.0)); // elapsed marked, flush latched
        root.rebuild(&mut progress_logic, &mut state); // Housekeeping fires here
        assert_eq!(state.long_presses, 1);
        assert_eq!(
            state.progress,
            vec![1.0],
            "the paired final progress observation rides the same fire"
        );

        root.event(&mut state, &hold_ev(PointerPhase::Move, 10.0, 40.0)); // past TOUCH_SLOP
        assert_eq!(state.long_presses, 1, "no double-fire on a post-fire drag");
        assert_eq!(
            state.progress,
            vec![1.0],
            "a post-fire Move delivers no further progress observation"
        );
    }

    #[test]
    fn early_lift_delivers_a_final_zero_observation_and_no_long_press() {
        // Criterion 2: early lift at ~50% -> a final 0.0 observation; no long-press.
        let mut root = progress_root();
        let mut state = HoldState::default();
        let mut sink = NullScene;
        root.event(&mut state, &hold_ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0));
        root.paint(&mut sink, ft(250.0)); // ~50% of the 500ms threshold, not elapsed
        root.event(&mut state, &hold_ev(PointerPhase::Up, 11.0, 11.0));

        assert_eq!(
            state.progress.last().copied(),
            Some(0.0),
            "an early lift's final observation resets to exactly 0.0"
        );
        assert_eq!(state.long_presses, 0, "early lift never long-presses");
    }

    #[test]
    fn move_past_slop_mid_hold_delivers_a_final_zero_and_no_long_press() {
        // Criterion 3: move past slop mid-hold -> cancel + 0.0; no long-press fire.
        let mut root = progress_root();
        let mut state = HoldState::default();
        let mut sink = NullScene;
        root.event(&mut state, &hold_ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0));
        root.paint(&mut sink, ft(300.0)); // 60% held, not elapsed
        // 30px move: well past TOUCH_SLOP (18) -> becomes a drag.
        root.event(&mut state, &hold_ev(PointerPhase::Move, 10.0, 40.0));
        root.event(&mut state, &hold_ev(PointerPhase::Up, 10.0, 40.0));

        assert_eq!(
            state.progress.last().copied(),
            Some(0.0),
            "a drag past slop delivers a final 0.0 observation"
        );
        assert_eq!(state.long_presses, 0);
        assert_eq!(state.taps, 0);
    }

    #[test]
    fn hold_threshold_ms_override_is_respected_by_both_callbacks() {
        // Criterion 4: hold_threshold_ms(700) respected by both callbacks.
        let mut root = progress_root_with_threshold(700);
        let mut state = HoldState::default();
        let mut sink = NullScene;
        root.event(&mut state, &hold_ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0));
        root.paint(&mut sink, ft(600.0)); // past the default 500ms, short of 700ms
        root.event(&mut state, &hold_ev(PointerPhase::Move, 10.0, 10.0));
        assert_eq!(
            state.long_presses, 0,
            "the 700ms override must not fire long-press at 600ms"
        );
        assert!(
            (state.progress.last().copied().unwrap() - 600.0 / 700.0).abs() < 1e-9,
            "progress reflects the overridden 700ms threshold, not the 500ms default"
        );

        root.paint(&mut sink, ft(700.0)); // now past the overridden threshold
        root.event(&mut state, &hold_ev(PointerPhase::Move, 11.0, 10.0));
        assert_eq!(
            state.long_presses, 1,
            "the 700ms override still fires long-press once reached"
        );
        assert_eq!(state.progress.last().copied(), Some(1.0));
    }

    #[test]
    fn no_hold_progress_handler_is_a_benign_noop() {
        // Criterion 5: on_tap/on_long_press-only users unaffected.
        let mut w = widget(true);
        let mut state = TapState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 11.0, 11.0));
        dispatch(&mut w, &mut state, &ev(PointerPhase::Up, 12.0, 12.0));
        assert_eq!(state.taps, 1);
        assert_eq!(state.long_presses, 0);
    }

    // --- Cancel staleness gap + zero-threshold NaN guard ---------------------

    #[test]
    fn cancel_leaves_progress_stale_until_fresh_down_cycle_delivers_clean_value() {
        // Regression for the Cancel staleness gap (see gesture.rs module
        // docs): a platform Cancel delivers NO final observation, so the
        // consumer's last-observed progress stays whatever it was mid-hold.
        // A fresh Down-cycle's first observation must start low again, never
        // resuming from the stale value Cancel left behind — the reset idiom
        // works at the widget contract level.
        let mut root = progress_root();
        let mut state = HoldState::default();
        let mut sink = NullScene;

        // Cycle 1: hold to ~30% progress, then Cancel (gesture steal).
        root.event(&mut state, &hold_ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0));
        root.event(&mut state, &hold_ev(PointerPhase::Move, 10.0, 10.0)); // delivers 0.0
        root.paint(&mut sink, ft(150.0)); // 30% of the 500ms default threshold
        root.event(&mut state, &hold_ev(PointerPhase::Move, 11.0, 10.0)); // delivers 0.3
        let before_cancel_len = state.progress.len();
        let stale_value = *state.progress.last().unwrap();
        assert!(
            stale_value > 0.0,
            "sanity: mid-hold progress is non-zero before Cancel"
        );

        root.event(&mut state, &hold_ev(PointerPhase::Cancel, 11.0, 10.0));
        assert_eq!(
            state.progress.len(),
            before_cancel_len,
            "Cancel delivers no observation — the Cancel-never-mutates-state convention"
        );

        // Cycle 2: a fresh Down cycle. The first delivered observation must
        // be a low, fresh value — never the stale value Cancel left behind.
        root.event(&mut state, &hold_ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(150.0)); // press_start re-anchors here; elapsed_ms = 0
        root.event(&mut state, &hold_ev(PointerPhase::Move, 10.0, 10.0));
        let fresh_value = *state.progress.last().unwrap();
        assert!(
            fresh_value < stale_value,
            "a fresh Down-cycle's first observation starts low, self-correcting the stale \
             ring (fresh={fresh_value}, stale={stale_value})"
        );
        assert_eq!(
            fresh_value, 0.0,
            "a fresh press-start yields exactly 0.0 progress"
        );
    }

    #[test]
    fn zero_threshold_guard_yields_finite_progress_never_nan() {
        // Regression: `hold_threshold_ms(0)` must resolve to a floor of 1ms
        // (`ms.max(1)`), never dividing progress by zero — `f64::clamp`
        // passes a NaN numerator/zero-denominator result straight through
        // rather than clamping it away, which would otherwise poison every
        // subsequent observation.
        let mut root = progress_root_with_threshold(0);
        let mut state = HoldState::default();
        let mut sink = NullScene;

        root.event(&mut state, &hold_ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0));
        root.event(&mut state, &hold_ev(PointerPhase::Move, 10.0, 10.0)); // first observation
        let p0 = state.progress[0];
        assert!(!p0.is_nan(), "zero threshold must never yield NaN progress");
        assert!(
            (0.0..=1.0).contains(&p0),
            "progress stays in the defined [0.0, 1.0] range"
        );

        // The 1ms floor is reached almost immediately: elapsed marks, and
        // the long-press fires on the very next deliverable event, per the
        // existing fire-on-move-arrival convention.
        root.paint(&mut sink, ft(1.0));
        root.event(&mut state, &hold_ev(PointerPhase::Move, 11.0, 10.0));
        assert_eq!(
            state.long_presses, 1,
            "long-press fires almost immediately at the 1ms floor"
        );
    }
}
