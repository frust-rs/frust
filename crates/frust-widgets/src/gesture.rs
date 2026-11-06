//! The `GestureDetector` widget: a transparent wrapper that recognises a tap and
//! a long-press on its child and forwards raw events through.
//!
//! [`GestureDetector`] wraps an arbitrary child view and fires either an
//! `on_tap` closure (a `Down`→`Up` that stays within [`TOUCH_SLOP`]) or an
//! `on_long_press` closure (a press held past [`LONG_PRESS_MS`] without moving
//! past the slop). Moving past the slop disarms both (it became a drag). The
//! wrapper is transparent — every event is still forwarded to the child, so
//! interactive descendants keep working. It recognises **tap + long-press**
//! and, when a drag callback is attached, **vertical, horizontal and pan
//! drags** (see [Drag recognition](self#drag-recognition)); double-tap is
//! deferred, and lacking an input-kind flag on events the slop is
//! [`TOUCH_SLOP`] uniformly.
//! Two-finger pinch lives in [`crate::pinch`] ([`crate::pinch::pinch_detector`]).
//!
//! # Drag recognition
//!
//! Three single-pointer drag recognizers sit beside tap and long-press, each
//! with `start`/`update`/`end`/`cancel` builders
//! ([`on_vertical_drag_start`](GestureDetectorView::on_vertical_drag_start),
//! [`on_horizontal_drag_start`](GestureDetectorView::on_horizontal_drag_start),
//! [`on_pan_start`](GestureDetectorView::on_pan_start), …). A recognizer takes
//! part in a press when any of its four callbacks is wired; with none wired the
//! detector behaves exactly as a tap/long-press detector.
//!
//! - **Slops.** A primary `Down` arms and captures as for a tap. The vertical
//!   recognizer starts once the travel from the `Down` along **its own axis**
//!   exceeds [`TOUCH_SLOP`] (18 px) — horizontal travel alone never starts it,
//!   and the horizontal recognizer mirrors that. Pan starts once the straight-line
//!   travel exceeds twice that (36 px).
//! - **First to cross wins.** With several recognizers wired, the first whose
//!   slop is crossed starts and locks the gesture; the others stay silent until
//!   the next press. When one `Move` crosses several slops at once, the one with
//!   the largest travel-to-slop ratio wins — the one a straight move from the
//!   `Down` would have crossed first (ties: vertical, horizontal, pan).
//! - **Start point and deltas.** The start reports the position where the slop
//!   was crossed, and the first update's delta is measured from there, so the
//!   slop travel is never replayed. Each update carries that `Move`'s delta; the
//!   axis recognizers zero the cross-axis component and also report the signed
//!   [`primary_delta`](DragUpdateDetails::primary_delta). Positions are local to
//!   the detector.
//! - **End and velocity.** An `Up` after the start fires `end` with the release
//!   velocity in logical px/s, from two [`frust_core::VelocityTracker`]s (x and y)
//!   fed on every `Move` of the press. The event pass carries no clock, so each
//!   sample is stamped with the last paint's raw [`PaintCtx::input_time`] —
//!   the scroll view's approach — never the animation-scaled
//!   [`PaintCtx::frame_time`], so an animation time scale leaves the velocity
//!   unchanged.
//! - **Exclusivity.** Moving past [`TOUCH_SLOP`] disarms tap, long-press and
//!   hold-progress exactly as before, even when no drag slop has been crossed
//!   yet; a drag recognizer can still start later in the same press. A
//!   long-press that already fired keeps the press: no drag starts after it.
//! - **Cancel.** A platform `Cancel` before any drag started stays silent, as
//!   for a tap. A `Cancel` after a drag started owes that recognizer's `cancel`
//!   callback, but the `Cancel` arm itself never reaches application state (a
//!   structural rebuild delivers its `Cancel` over a throwaway state; see
//!   `docs/CODE_STANDARDS.md`). It only records the debt and raises
//!   [`frust_core::mark_pending_result_flush`], and the callback runs from the
//!   next pass that carries real state — the `Housekeeping` broadcast of the
//!   next `RenderRoot::rebuild` (the same rebuild, for a structural cancel), or
//!   an earlier `Down`. A detector torn down mid-drag fires no `cancel`.
//!
//! **Inside a scroll view** the scroll sees each `Move` before its child and
//! claims a drag along its own axis at the same [`TOUCH_SLOP`], sending the
//! child a `Cancel`: wherever the scroll accepts the drag, a same-axis drag
//! recognizer never starts (the press is cancelled silently), while a
//! cross-axis one starts normally.
//! A started cross-axis drag that later travels past the scroll's slop on the
//! scroll axis is taken over too, and receives its `cancel`.
//!
//! # Press state
//!
//! [`on_tap_down`](GestureDetectorView::on_tap_down),
//! [`on_tap_up`](GestureDetectorView::on_tap_up) and
//! [`on_tap_cancel`](GestureDetectorView::on_tap_cancel) make a still press
//! observable (a "hold to see" preview, a pressed highlight). The contract:
//! every `on_tap_down` is followed by **exactly one** `on_tap_up` or
//! `on_tap_cancel` before the next `on_tap_down`.
//!
//! - **Down.** A primary `Down` fires `on_tap_down` at once, with no delay and
//!   even inside a scroll view, carrying the local position. A second `Down`
//!   while a press is still active (a missed `Up`/`Cancel`) first delivers the
//!   `on_tap_cancel` of the old press.
//! - **Up.** A release of an active press fires `on_tap_up` (before `on_tap`).
//!   A press held past [`LONG_PRESS_MS`] still ends in `on_tap_up`.
//! - **Cancel by movement.** A `Move` that passes [`TOUCH_SLOP`] from the
//!   `Down`, or leaves the detector's bounds, fires `on_tap_cancel` at once;
//!   the later `Up` then fires no `on_tap_up`. This is stricter than `on_tap`:
//!   leaving the bounds within the slop cancels the press, yet does not stop a
//!   later `on_tap`. `on_tap`, long-press, hold-progress and drag recognition
//!   keep their own rules.
//! - **Cancel by steal.** A `Cancel` (a scroll view taking the press over, a
//!   system cancel, a structural rebuild over throwaway state) owes the
//!   `on_tap_cancel`, like a drag `cancel`: the `Cancel` arm never reaches
//!   application state, so the callback runs one frame late, from the next
//!   `Housekeeping` broadcast or the next pointer event, whichever comes
//!   first.
//! - **Removal.** A detector torn down mid-press fires nothing, so the
//!   one-follow-up guarantee does not hold across removal.
//!
//! # Long-press firing semantics
//!
//! Timing is measured across **paints**, not events: only the paint pass carries
//! a clock. The hold reads the raw [`PaintCtx::input_time`], so an animation
//! time scale never stretches it. A press records `press_start` on its first
//! paint and marks itself *elapsed* on a later paint once
//! `input_time - press_start >= LONG_PRESS_MS`.
//!
//! **The callback never fires during paint itself** (there is no `&mut
//! State`/`EventCtx` there). Instead, the paint that observes the threshold
//! crossed calls [`frust_core::mark_pending_result_flush`] — the same mechanism
//! [`crate::nav::navigator`] uses to run a queued pop-result callback from a
//! state-free `View::rebuild` pass — and [`PaintCtx::request_frame`]s a follow-up
//! frame. Every shipping shell (desktop/Android/iOS) runs `RenderRoot::rebuild`
//! before `RenderRoot::paint` on every frame it drives, so the very next frame's
//! rebuild drains that mark, dispatches an `InputEvent::Housekeeping` broadcast
//! carrying a real `EventCtx`, and this widget fires `on_long_press` from there —
//! **at the threshold, in wall time**, whether or not the finger ever moves again.
//!
//! A pointer-delivered fire's `EventCtx::request_redraw` rides the shell's own
//! `RenderRoot::event` call back out; a Housekeeping-delivered one reaches the
//! shell only because `RenderRoot::rebuild` propagates the broadcast's
//! `EventOutcome` into `ChangeFlags::PAINT` and the deferred frame request
//! (`docs/CORE_ARCHITECTURE.md`'s event-routing data flow) — so a fire whose
//! consumer mutates nothing the view diff can see still repaints on both a
//! frame-gated mobile loop and a dirty-driven desktop one.
//!
//! Two delivery paths race for the same fire, and whichever reaches the widget
//! first wins — the `Recognizer::Fired` transition makes the other one a no-op:
//!
//! - the **Housekeeping broadcast** above, ordinarily one frame after the
//!   threshold, needing no pointer event at all; or
//! - a **live pointer event** that arrives first — `Up` (a hold that exceeded
//!   the threshold releases as a long-press instead of a tap) or an in-slop
//!   `Move` (so a context menu can still open the instant a held finger jitters,
//!   without waiting out the extra frame).
//!
//! The Housekeeping path runs only when
//! [`on_long_press`](GestureDetectorView::on_long_press) is wired: an
//! `on_hold_progress`-only detector keeps its final observation deferred to the
//! next pointer event, and when both are wired and `on_long_press` fires via
//! Housekeeping the paired final `1.0` (below) rides along in the same pass. The
//! paint timer itself runs for either handler and never for a **tap-only**
//! detector, which therefore never marks `elapsed`: a press held past the threshold
//! still resolves as an ordinary tap on an in-bounds release (the fire-on-up-inside
//! contract is independent of hold duration when there is no long-press to promote
//! it to), and the pending-timer [`PaintCtx::request_frame`] calls are skipped — a
//! small battery win, since a tap-only hold needs no clock. A held-past-threshold
//! resolution falls through to `on_tap` whenever `on_long_press` is absent (e.g.
//! unwired mid-gesture), so a held press never silently swallows its release.
//!
//! Frame liveness while the finger is held still is guaranteed on mobile by the
//! pointer capture keeping the `FrameGate` running; desktop is dirty-driven, so
//! paint calls [`PaintCtx::request_frame`] both while the timer is pending, and
//! once more on the frame that crosses the threshold — that second request is
//! what actually reaches the Housekeeping-flushing rebuild above on a desktop
//! shell with no further input.
//!
//! # Hold-progress observation
//!
//! [`on_hold_progress`](GestureDetectorView::on_hold_progress) reuses the same
//! paint-clock timer as `on_long_press` to let a widget render charge-up feedback
//! (e.g. a charge ring) for a held press. Paint computes progress —
//! `(input_time - press_start) / threshold`, clamped `0.0..=1.0` — every frame
//! while a press is pending, but (the same callback-from-paint caveat as above)
//! can only *record* it; the observation is delivered on the next pointer event
//! that already carries a mutable `EventCtx` (an in-slop `Move`, matching
//! `on_long_press`'s fire-on-move-arrival) — **or, when `on_long_press` is also
//! wired, on the Housekeeping-triggered fire above, whichever reaches the widget
//! first.** A drag past [`TOUCH_SLOP`] or an early release *before* the threshold
//! — i.e. the `Move`/`Up` arm handling that transition, never
//! `PointerPhase::Cancel` (see the Cancel staleness gap below) — delivers a final
//! `0.0`; reaching the threshold delivers a final `1.0`. An
//! `on_hold_progress`-only detector (no `on_long_press` wired) has its final
//! `1.0` deferred to the next pointer event too — the Housekeeping path never
//! runs for it. [`hold_threshold_ms`](GestureDetectorView::hold_threshold_ms)
//! overrides [`LONG_PRESS_MS`] for both `on_long_press` and this timer.
//!
//! ## Cancel staleness gap
//!
//! A platform `Cancel` (gesture steal, e.g. a parent `ScrollView` claiming the
//! drag; or structural teardown, e.g. the child subtree changing shape mid-hold)
//! delivers **no** final observation — the Cancel-never-mutates-state convention
//! (`docs/CODE_STANDARDS.md`) means `on_hold_progress` is never called from that
//! arm. The consumer's last-observed `progress` therefore stays stale (whatever
//! it was mid-hold) until a full new press cycle (`Down`→...→`Up`/threshold)
//! delivers a fresh `0.0`/`1.0` through the normal path — there is no
//! Cancel-delivered reset. **Consumer-side reset idiom:** since a fresh press
//! cycle's *first* observation is always a low value counting up from near `0.0`,
//! a consumer that stored a stale non-zero `progress` can self-correct at the top
//! of its `on_hold_progress` callback by treating an incoming value lower than
//! the last-stored one as the new cycle's baseline rather than carrying the stale
//! high value forward — `demo_charge_ring` in `examples/glyph-catalog`'s
//! `pages/interactions.rs` is a worked instance. Do not add a Cancel-arm callback
//! to close this gap; it would violate that convention.

use std::rc::Rc;

use frust_core::{
    AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, FrameTime,
    InputEvent, LayoutCtx, PaintCtx, PaintScene, PointerPhase, SemanticsCtx, SliverConstraints,
    SliverGeometry, SliverWidget, TOUCH_SLOP, VelocityTracker, View, Widget, any,
};
use kurbo::{Point, Size, Vec2};

use crate::authoring::{
    ErasedArgCallback, ErasedCallback, TypedArgCallback, erase_callback, erase_callback_arg,
    presses,
};

/// The straight-line travel a pan must exceed before it starts: twice
/// [`TOUCH_SLOP`], so an axis drag wired beside it wins a near-straight move.
const PAN_SLOP: f64 = 2.0 * TOUCH_SLOP;

/// What a drag `start` callback receives: where the drag began.
///
/// `#[non_exhaustive]` so fields can be added later; read the public fields.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct DragStartDetails {
    /// The pointer position, local to the detector, at the `Move` that crossed
    /// the recognizer's slop (not the `Down`).
    pub position: Point,
}

/// What a drag `update` callback receives for one `Move`.
///
/// `#[non_exhaustive]` so fields can be added later; read the public fields.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct DragUpdateDetails {
    /// The pointer position, local to the detector.
    pub position: Point,
    /// The movement since the previous update (or since the start point, for
    /// the first update), in logical px. An axis drag zeroes the cross-axis
    /// component.
    pub delta: Vec2,
    /// The signed movement along the drag's axis — `delta.y` for a vertical
    /// drag, `delta.x` for a horizontal one; `None` for a pan.
    pub primary_delta: Option<f64>,
}

/// What a drag `end` callback receives on release.
///
/// `#[non_exhaustive]` so fields can be added later; read the public fields.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct DragEndDetails {
    /// The release velocity in logical px/s, measured on the raw input clock.
    /// An axis drag zeroes the cross-axis component.
    pub velocity: Vec2,
    /// The signed velocity along the drag's axis; `None` for a pan.
    pub primary_velocity: Option<f64>,
}

/// What an `on_tap_down` callback receives: where the press began.
///
/// `#[non_exhaustive]` so fields can be added later; read the public fields.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct TapDownDetails {
    /// The pointer position, local to the detector, at the `Down`.
    pub local_position: Point,
}

/// What an `on_tap_up` callback receives: where the press was released.
///
/// `#[non_exhaustive]` so fields can be added later; read the public fields.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct TapUpDetails {
    /// The pointer position, local to the detector, at the `Up`.
    pub local_position: Point,
}

/// Which drag recognizer a press is tracking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragAxis {
    Vertical,
    Horizontal,
    Pan,
}

impl DragAxis {
    /// Project a 2D quantity onto this recognizer's axis: the zeroed-cross-axis
    /// vector and the signed primary component (`None` for a pan).
    fn project(self, v: Vec2) -> (Vec2, Option<f64>) {
        match self {
            DragAxis::Vertical => (Vec2::new(0.0, v.y), Some(v.y)),
            DragAxis::Horizontal => (Vec2::new(v.x, 0.0), Some(v.x)),
            DragAxis::Pan => (v, None),
        }
    }

    /// How far past its slop `travel` (from the `Down`) has carried this
    /// recognizer, as `travel / slop`; above `1.0` means crossed. Along a
    /// straight move from the `Down`, the larger ratio was crossed first.
    fn slop_ratio(self, travel: Vec2) -> f64 {
        match self {
            DragAxis::Vertical => travel.y.abs() / TOUCH_SLOP,
            DragAxis::Horizontal => travel.x.abs() / TOUCH_SLOP,
            DragAxis::Pan => travel.hypot() / PAN_SLOP,
        }
    }
}

/// One drag recognizer's view-held, typed callbacks (erased on build).
struct DragCallbacks<State: 'static> {
    start: Option<TypedArgCallback<State, DragStartDetails>>,
    update: Option<TypedArgCallback<State, DragUpdateDetails>>,
    end: Option<TypedArgCallback<State, DragEndDetails>>,
    cancel: Option<GestureCallback<State>>,
}

impl<State: 'static> DragCallbacks<State> {
    const fn new() -> Self {
        Self {
            start: None,
            update: None,
            end: None,
            cancel: None,
        }
    }

    fn erase(&self) -> ErasedDragCallbacks {
        ErasedDragCallbacks {
            start: self.start.as_ref().map(erase_callback_arg),
            update: self.update.as_ref().map(erase_callback_arg),
            end: self.end.as_ref().map(erase_callback_arg),
            cancel: self.cancel.as_ref().map(erase_callback),
        }
    }
}

/// One drag recognizer's widget-held, `State`-erased callbacks.
#[derive(Default)]
struct ErasedDragCallbacks {
    start: Option<ErasedArgCallback<DragStartDetails>>,
    update: Option<ErasedArgCallback<DragUpdateDetails>>,
    end: Option<ErasedArgCallback<DragEndDetails>>,
    cancel: Option<ErasedCallback>,
}

impl ErasedDragCallbacks {
    /// Whether this recognizer takes part in a press: any callback wired.
    fn is_wired(&self) -> bool {
        self.start.is_some() || self.update.is_some() || self.end.is_some() || self.cancel.is_some()
    }
}

/// The press duration after which a held press becomes a long-press.
///
/// **Community-approximate**: neither platform publishes this as a stable
/// numeric constant, but the community-converged values agree closely —
/// Android's `ViewConfiguration` long-press timeout defaults to ~400–500ms and
/// iOS's `UILongPressGestureRecognizer.minimumPressDuration` defaults to 0.5s.
/// 500ms sits at that shared upper bound.
///
/// `pub(crate)` because [`crate::textinput`] recognises its own stationary
/// long-press — a text field cannot delegate the gesture to a wrapper, since
/// the recogniser has to reach the editor's own hit test — and a field that
/// held for a different duration than every `GestureDetector` around it would
/// read as an inconsistency rather than a tuning choice. The matching slop is
/// [`frust_core::TOUCH_SLOP`], which both already share.
pub(crate) const LONG_PRESS_MS: f64 = 500.0;

/// The window within which a second press near the first is a double-tap.
///
/// Android publishes this one as a real constant:
/// `ViewConfiguration.getDoubleTapTimeout()` is `DOUBLE_TAP_TIMEOUT = 300`ms
/// (`android.view.ViewConfiguration`). Apple exposes no equivalent number —
/// `UITapGestureRecognizer` recognises a multi-tap internally and publishes no
/// timeout — so the one published value stands for both platforms, paired with
/// the same [`frust_core::TOUCH_SLOP`] the long-press uses.
///
/// Consumed by [`crate::textinput`]'s select-the-word-on-double-tap; the
/// detector in this module still recognises tap + long-press only (double-tap
/// is deferred there — see the [module docs](self)).
pub(crate) const DOUBLE_TAP_MS: f64 = 300.0;

/// The paint-clock hold timer behind a stationary long-press: the press epoch,
/// the threshold it is measured against, and the two reads every hold consumer
/// makes of them.
///
/// Time only enters a widget through paint, and a hold reads the raw
/// [`PaintCtx::input_time`], so the epoch is not the `Down` itself but the
/// **first observation after it** — the first
/// paint that asks [`progress`](Self::progress) or [`fired`](Self::fired)
/// records it, and every later read differences against it (see the [module
/// docs](self#long-press-firing-semantics)). [`cancel`](Self::cancel) forgets
/// the epoch, so the next press re-anchors on its own first paint.
///
/// `Copy` and state-free beyond those two values, so a recogniser keeps one per
/// widget and resets it on every arming `Down` rather than allocating per press.
/// Shared by [`GestureDetectorWidget`] and [`crate::drag`]'s draggable, whose
/// touch long-press initiation must hold for exactly as long as every
/// `GestureDetector` around it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HoldTracker {
    /// The first observed frame time of the current press; `None` before the
    /// first observation (and after [`cancel`](Self::cancel)).
    press_start: Option<FrameTime>,
    /// The hold duration, in ms, after which [`fired`](Self::fired) reports
    /// `true` and [`progress`](Self::progress) reaches `1.0`.
    threshold_ms: f64,
}

impl HoldTracker {
    /// A tracker with no press recorded, firing after `threshold_ms`.
    pub(crate) const fn new(threshold_ms: f64) -> Self {
        Self {
            press_start: None,
            threshold_ms,
        }
    }

    /// Change the hold duration. Takes effect on the next read, so a threshold
    /// changed mid-press is measured against the press's original epoch.
    pub(crate) fn set_threshold_ms(&mut self, threshold_ms: f64) {
        self.threshold_ms = threshold_ms;
    }

    /// Milliseconds held as of `now`, recording `now` as the epoch on the first
    /// observation of a press.
    fn elapsed_ms(&mut self, now: FrameTime) -> f64 {
        let start = *self.press_start.get_or_insert(now);
        now.saturating_sub(start).as_secs_f64() * 1000.0
    }

    /// Hold progress as of `now`: `elapsed / threshold`, clamped `0.0..=1.0`.
    pub(crate) fn progress(&mut self, now: FrameTime) -> f64 {
        (self.elapsed_ms(now) / self.threshold_ms).clamp(0.0, 1.0)
    }

    /// Whether the hold has reached the threshold as of `now`.
    pub(crate) fn fired(&mut self, now: FrameTime) -> bool {
        self.elapsed_ms(now) >= self.threshold_ms
    }

    /// Forget the current press: the next observation records a fresh epoch.
    pub(crate) fn cancel(&mut self) {
        self.press_start = None;
    }
}

/// A view-held, typed gesture callback (erased on build). Shared by `on_tap`
/// and `on_long_press` — both are `Fn(&mut State)`.
type GestureCallback<State> = Rc<dyn Fn(&mut State)>;

/// A declarative gesture wrapper. See the [module docs](self).
pub struct GestureDetectorView<State: 'static> {
    child: AnyView<State>,
    on_tap: Option<GestureCallback<State>>,
    on_long_press: Option<GestureCallback<State>>,
    on_tap_down: Option<TypedArgCallback<State, TapDownDetails>>,
    on_tap_up: Option<TypedArgCallback<State, TapUpDetails>>,
    on_tap_cancel: Option<GestureCallback<State>>,
    on_hold_progress: Option<crate::authoring::TypedArgCallback<State, f64>>,
    hold_threshold_ms: Option<u64>,
    vertical_drag: DragCallbacks<State>,
    horizontal_drag: DragCallbacks<State>,
    pan: DragCallbacks<State>,
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
        on_tap_down: None,
        on_tap_up: None,
        on_tap_cancel: None,
        on_hold_progress: None,
        hold_threshold_ms: None,
        vertical_drag: DragCallbacks::new(),
        horizontal_drag: DragCallbacks::new(),
        pan: DragCallbacks::new(),
    }
}

/// The builders for one drag recognizer's four callbacks: `$field` names the
/// view's [`DragCallbacks`], `$label` the recognizer and `$slop` its start
/// threshold in rustdoc.
macro_rules! drag_builders {
    (
        $field:ident,
        $label:literal,
        $slop:literal,
        $start:ident,
        $update:ident,
        $end:ident,
        $cancel:ident
    ) => {
        #[doc = concat!("Call `f` when a ", $label, " starts: once the press has travelled ")]
        #[doc = concat!($slop, " from the `Down`, before any other wired drag recognizer")]
        #[doc = "crossed its own slop. The details carry the local position where the slop"]
        #[doc = "was crossed; a press that starts a drag never taps, long-presses or reports"]
        #[doc = "hold progress afterwards."]
        pub fn $start<F: Fn(&mut State, DragStartDetails) + 'static>(mut self, f: F) -> Self {
            self.$field.start = Some(Rc::new(f));
            self
        }

        #[doc = concat!("Call `f` on every `Move` of a started ", $label, " with that move's")]
        #[doc = "local position and delta. The first delta is measured from the start point,"]
        #[doc = "so the slop travel is never replayed."]
        pub fn $update<F: Fn(&mut State, DragUpdateDetails) + 'static>(mut self, f: F) -> Self {
            self.$field.update = Some(Rc::new(f));
            self
        }

        #[doc = concat!("Call `f` when a started ", $label, " is released, with the release")]
        #[doc = "velocity in logical px/s, measured on the raw input clock (an animation time"]
        #[doc = "scale does not change it)."]
        pub fn $end<F: Fn(&mut State, DragEndDetails) + 'static>(mut self, f: F) -> Self {
            self.$field.end = Some(Rc::new(f));
            self
        }

        #[doc = concat!("Call `f` when a started ", $label, " is cancelled: a gesture steal")]
        #[doc = "(such as a scroll view taking the press over) or a structural rebuild. It"]
        #[doc = "runs from the next pass that carries application state (normally the next"]
        #[doc = "rebuild), never from the `Cancel` event itself; a `Cancel` before the drag"]
        #[doc = "started calls nothing."]
        pub fn $cancel<F: Fn(&mut State) + 'static>(mut self, f: F) -> Self {
            self.$field.cancel = Some(Rc::new(f));
            self
        }
    };
}

impl<State: 'static> GestureDetectorView<State> {
    drag_builders!(
        vertical_drag,
        "vertical drag",
        "more than 18 px vertically",
        on_vertical_drag_start,
        on_vertical_drag_update,
        on_vertical_drag_end,
        on_vertical_drag_cancel
    );
    drag_builders!(
        horizontal_drag,
        "horizontal drag",
        "more than 18 px horizontally",
        on_horizontal_drag_start,
        on_horizontal_drag_update,
        on_horizontal_drag_end,
        on_horizontal_drag_cancel
    );
    drag_builders!(
        pan,
        "pan",
        "more than 36 px in any direction",
        on_pan_start,
        on_pan_update,
        on_pan_end,
        on_pan_cancel
    );
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

    /// Call `f` the instant a primary press lands on the detector, with the
    /// local position. No delay, even inside a scroll view. Exactly one of
    /// [`on_tap_up`](Self::on_tap_up) or [`on_tap_cancel`](Self::on_tap_cancel)
    /// follows before the next `on_tap_down`; see the [module
    /// docs](self#press-state).
    pub fn on_tap_down<F: Fn(&mut State, TapDownDetails) + 'static>(mut self, f: F) -> Self {
        self.on_tap_down = Some(Rc::new(f));
        self
    }

    /// Call `f` when an active press is released, before
    /// [`on_tap`](Self::on_tap). A press held past the long-press threshold
    /// still ends here. See the [module docs](self#press-state).
    pub fn on_tap_up<F: Fn(&mut State, TapUpDetails) + 'static>(mut self, f: F) -> Self {
        self.on_tap_up = Some(Rc::new(f));
        self
    }

    /// Call `f` when an active press ends without a release: the pointer
    /// passed [`TOUCH_SLOP`] or left the detector's bounds (immediately), or
    /// the press was cancelled by a steal or system cancel (one frame late,
    /// from the next state-bearing pass). Stricter than
    /// [`on_tap`](Self::on_tap): leaving the bounds within the slop cancels
    /// the press but not a later tap. See the [module docs](self#press-state).
    pub fn on_tap_cancel<F: Fn(&mut State) + 'static>(mut self, f: F) -> Self {
        self.on_tap_cancel = Some(Rc::new(f));
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
    /// The widget's [`HoldTracker`] records the press epoch on the first paint
    /// after `Down`; `elapsed` flips once a paint observes the hold exceeding
    /// the threshold — the same
    /// paint also latches a Housekeeping flush when `on_long_press` is wired
    /// (see the [module docs](self#long-press-firing-semantics)). `progress`
    /// is paint's latest `0.0..=1.0` hold-progress observation, delivered to
    /// `on_hold_progress` on the next pointer event or the Housekeeping-triggered
    /// fire, whichever arrives first (see the [module
    /// docs](self#hold-progress-observation)).
    Pressed {
        down_pos: Point,
        elapsed: bool,
        progress: f64,
    },
    /// The press moved past the slop — became a drag; neither recogniser fires.
    /// Reached only when no drag recognizer is wired.
    Dragged,
    /// The press moved past the tap slop (tap, long-press and hold are
    /// disarmed) and a drag recognizer is wired, but none has crossed its own
    /// slop yet. `down_pos` is where every drag slop is measured from.
    DragArmed { down_pos: Point },
    /// A drag recognizer won the press; `last_pos` is the position the next
    /// update's delta is measured from.
    Dragging { axis: DragAxis, last_pos: Point },
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
    on_tap_down: Option<ErasedArgCallback<TapDownDetails>>,
    on_tap_up: Option<ErasedArgCallback<TapUpDetails>>,
    on_tap_cancel: Option<ErasedCallback>,
    /// A press `on_tap_down` was delivered for, with no `on_tap_up` or
    /// `on_tap_cancel` yet.
    press_active: bool,
    /// Where the active press began, for the tap-cancel slop check.
    press_origin: Point,
    /// A `Cancel` ended an active press, whose `on_tap_cancel` is owed to the
    /// next state-bearing pass (see the [module docs](self#press-state)).
    owed_tap_cancel: bool,
    on_hold_progress: Option<crate::authoring::ErasedArgCallback<f64>>,
    /// The press timer, firing at the resolved hold threshold —
    /// [`GestureDetectorView::hold_threshold_ms`] if set, else
    /// [`LONG_PRESS_MS`]. Shared by the long-press timer and the hold-progress
    /// computation; reset on every arming `Down`.
    hold: HoldTracker,
    vertical_drag: ErasedDragCallbacks,
    horizontal_drag: ErasedDragCallbacks,
    pan: ErasedDragCallbacks,
    /// The release-velocity estimators, x and y, cleared on every arming
    /// `Down` and fed on every `Move` while a drag recognizer is wired.
    velocity_x: VelocityTracker,
    velocity_y: VelocityTracker,
    /// The most recent paint's raw clock ([`PaintCtx::input_time`]), the
    /// event-pass timestamp for the velocity samples (the event pass carries
    /// no clock). Stashed only while a drag recognizer is wired and a press is
    /// in flight.
    last_input_time: FrameTime,
    /// A started drag a `Cancel` ended, whose `cancel` callback is owed to the
    /// next state-bearing pass (see the [module docs](self#drag-recognition)).
    owed_cancel: Option<DragAxis>,
}

impl GestureDetectorWidget {
    /// Whether any drag recognizer takes part in a press.
    fn drags_wired(&self) -> bool {
        self.vertical_drag.is_wired() || self.horizontal_drag.is_wired() || self.pan.is_wired()
    }

    fn drag_callbacks(&mut self, axis: DragAxis) -> &mut ErasedDragCallbacks {
        match axis {
            DragAxis::Vertical => &mut self.vertical_drag,
            DragAxis::Horizontal => &mut self.horizontal_drag,
            DragAxis::Pan => &mut self.pan,
        }
    }

    /// The wired recognizer `travel` (from the `Down`) has carried past its
    /// slop first, if any: the largest travel-to-slop ratio above `1.0`, ties
    /// going to vertical, then horizontal, then pan.
    fn drag_winner(&self, travel: Vec2) -> Option<DragAxis> {
        let mut winner: Option<(DragAxis, f64)> = None;
        for (axis, callbacks) in [
            (DragAxis::Vertical, &self.vertical_drag),
            (DragAxis::Horizontal, &self.horizontal_drag),
            (DragAxis::Pan, &self.pan),
        ] {
            if !callbacks.is_wired() {
                continue;
            }
            let ratio = axis.slop_ratio(travel);
            if ratio > 1.0 && winner.is_none_or(|(_, best)| ratio > best) {
                winner = Some((axis, ratio));
            }
        }
        winner.map(|(axis, _)| axis)
    }

    /// Whether the press is one whose `Move`s feed the velocity trackers.
    fn tracks_velocity(&self) -> bool {
        self.drags_wired()
            && matches!(
                self.state,
                Recognizer::Pressed { .. }
                    | Recognizer::DragArmed { .. }
                    | Recognizer::Dragging { .. }
            )
    }

    /// Record `position` in both velocity trackers at the stashed raw clock.
    fn record_velocity(&mut self, position: Point) {
        let t_ms = self.last_input_time.as_secs_f64() * 1000.0;
        self.velocity_x.record(t_ms, position.x);
        self.velocity_y.record(t_ms, position.y);
    }

    /// Whether any press-state callback is wired.
    fn press_wired(&self) -> bool {
        self.on_tap_down.is_some() || self.on_tap_up.is_some() || self.on_tap_cancel.is_some()
    }

    /// End the active press with its `on_tap_cancel`. Only from passes that
    /// carry real application state.
    fn fire_tap_cancel(&mut self, ctx: &mut EventCtx) {
        self.press_active = false;
        if let Some(cb) = self.on_tap_cancel.as_mut() {
            cb(ctx);
            ctx.request_redraw();
        }
    }

    /// Run an owed `on_tap_cancel`. Called only from passes that carry real
    /// application state — never from the `Cancel` arm.
    fn flush_owed_tap_cancel(&mut self, ctx: &mut EventCtx) {
        if std::mem::take(&mut self.owed_tap_cancel)
            && let Some(cb) = self.on_tap_cancel.as_mut()
        {
            cb(ctx);
            ctx.request_redraw();
        }
    }

    /// Run an owed drag `cancel` callback. Called only from passes that carry
    /// real application state — never from the `Cancel` arm.
    fn flush_owed_cancel(&mut self, ctx: &mut EventCtx) {
        if let Some(axis) = self.owed_cancel.take()
            && let Some(cb) = self.drag_callbacks(axis).cancel.as_mut()
        {
            cb(ctx);
            ctx.request_redraw();
        }
    }
}

impl<State: 'static> View<State> for GestureDetectorView<State> {
    type Element = GestureDetectorWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> GestureDetectorWidget {
        GestureDetectorWidget {
            child: crate::authoring::build_child(&self.child, ctx),
            state: Recognizer::Idle,
            on_tap: self.on_tap.as_ref().map(crate::authoring::erase_callback),
            on_tap_down: self.on_tap_down.as_ref().map(erase_callback_arg),
            on_tap_up: self.on_tap_up.as_ref().map(erase_callback_arg),
            on_tap_cancel: self.on_tap_cancel.as_ref().map(erase_callback),
            press_active: false,
            press_origin: Point::ZERO,
            owed_tap_cancel: false,
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
            hold: HoldTracker::new(
                self.hold_threshold_ms
                    .map(|ms| ms.max(1) as f64)
                    .unwrap_or(LONG_PRESS_MS),
            ),
            vertical_drag: self.vertical_drag.erase(),
            horizontal_drag: self.horizontal_drag.erase(),
            pan: self.pan.erase(),
            velocity_x: VelocityTracker::new(),
            velocity_y: VelocityTracker::new(),
            last_input_time: FrameTime::ZERO,
            owed_cancel: None,
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
        element.on_tap_down = self.on_tap_down.as_ref().map(erase_callback_arg);
        element.on_tap_up = self.on_tap_up.as_ref().map(erase_callback_arg);
        element.on_tap_cancel = self.on_tap_cancel.as_ref().map(erase_callback);
        element.on_hold_progress = self
            .on_hold_progress
            .as_ref()
            .map(crate::authoring::erase_callback_arg);
        // Same NaN guard as `build` above — see `hold_threshold_ms`'s doc comment.
        element.hold.set_threshold_ms(
            self.hold_threshold_ms
                .map(|ms| ms.max(1) as f64)
                .unwrap_or(LONG_PRESS_MS),
        );
        element.vertical_drag = self.vertical_drag.erase();
        element.horizontal_drag = self.horizontal_drag.erase();
        element.pan = self.pan.erase();
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
                elapsed, progress, ..
            } = &mut self.state
        {
            let now = ctx.input_time();
            *progress = self.hold.progress(now);
            if !*elapsed {
                if self.hold.fired(now) {
                    *elapsed = true;
                    // Threshold-time firing (see the module docs' "Long-press
                    // firing semantics"): paint can't call `on_long_press`
                    // itself (no `EventCtx` here), so latch the same
                    // deferred-callback flush `nav::navigator` uses for a
                    // queued pop-result, and request one more frame so a
                    // dirty-driven desktop shell actually reaches the
                    // `RenderRoot::rebuild` that drains it — every shell runs
                    // rebuild (with a real `EventCtx`-bearing `Housekeeping`
                    // dispatch) before its own paint, so the very next frame
                    // fires this at the threshold, in wall time, with no
                    // pointer event required. Gated on `on_long_press` being
                    // wired: an `on_hold_progress`-only detector has its
                    // delivery deferred to the next pointer event instead.
                    if self.on_long_press.is_some() {
                        frust_core::mark_pending_result_flush();
                        ctx.request_frame();
                    }
                } else {
                    ctx.request_frame();
                }
            }
        }
        // Stash the raw clock for the drag velocity samples: the event pass has
        // none, and the animation-scaled `frame_time` would stretch the
        // velocity under a time scale.
        if self.tracks_velocity() {
            self.last_input_time = ctx.input_time();
        }
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if let InputEvent::Pointer(p) = event {
            // A `Cancel` that ended a started drag owes its `cancel` callback;
            // any other pointer phase carries real state, so pay it first.
            if p.phase != PointerPhase::Cancel {
                self.flush_owed_cancel(ctx);
                self.flush_owed_tap_cancel(ctx);
            }
            match p.phase {
                PointerPhase::Down => {
                    // Only a primary press arms a recognizer. A secondary press
                    // is a context gesture — it starts neither a tap nor a
                    // long-press and opens no capture — but it still reaches the
                    // child, whose own context handling (if any) owns it.
                    if !presses(p) {
                        return self.child.event_child(ctx, event);
                    }
                    // A press that never saw its `Up`/`Cancel` ends before the
                    // next one starts.
                    if self.press_active {
                        self.fire_tap_cancel(ctx);
                    }
                    if self.press_wired() {
                        self.press_active = true;
                        self.press_origin = p.position;
                        if let Some(cb) = self.on_tap_down.as_mut() {
                            cb(
                                ctx,
                                TapDownDetails {
                                    local_position: p.position,
                                },
                            );
                            ctx.request_redraw();
                        }
                    }
                    self.state = Recognizer::Pressed {
                        down_pos: p.position,
                        elapsed: false,
                        progress: 0.0,
                    };
                    self.hold.cancel();
                    self.velocity_x.clear();
                    self.velocity_y.clear();
                    ctx.capture_pointer();
                    // Prompt a paint so the long-press/progress timer starts promptly.
                    ctx.request_redraw();
                    self.child.event_child(ctx, event);
                    return EventResult::Handled;
                }
                PointerPhase::Move => {
                    // The press ends the moment the pointer passes the slop or
                    // leaves the bounds; `on_tap` and the other recognizers
                    // below keep their own rules.
                    if self.press_active {
                        let size = ctx.size();
                        let outside = p.position.x < 0.0
                            || p.position.y < 0.0
                            || p.position.x > size.width
                            || p.position.y > size.height;
                        if outside || (p.position - self.press_origin).hypot() > TOUCH_SLOP {
                            self.fire_tap_cancel(ctx);
                        }
                    }
                    // Feed the release-velocity trackers on every `Move` of a
                    // press a drag recognizer takes part in.
                    let tracking = self.tracks_velocity();
                    if tracking {
                        self.record_velocity(p.position);
                    }
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
                            // Cancel, so it's allowed to touch state. With a drag
                            // recognizer wired the press stays armed for it (its
                            // own slop is checked below, on this same `Move`).
                            self.state = if tracking {
                                Recognizer::DragArmed { down_pos }
                            } else {
                                Recognizer::Dragged
                            };
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
                    match self.state {
                        Recognizer::DragArmed { down_pos } => {
                            if let Some(axis) = self.drag_winner(p.position - down_pos) {
                                // The start point is where the slop was crossed,
                                // and the first update measures from here, so the
                                // slop travel is never replayed.
                                self.state = Recognizer::Dragging {
                                    axis,
                                    last_pos: p.position,
                                };
                                self.child.event_child(ctx, event);
                                if let Some(cb) = self.drag_callbacks(axis).start.as_mut() {
                                    cb(
                                        ctx,
                                        DragStartDetails {
                                            position: p.position,
                                        },
                                    );
                                }
                                ctx.request_redraw();
                                return EventResult::Handled;
                            }
                        }
                        Recognizer::Dragging { axis, last_pos } => {
                            self.state = Recognizer::Dragging {
                                axis,
                                last_pos: p.position,
                            };
                            let (delta, primary_delta) = axis.project(p.position - last_pos);
                            self.child.event_child(ctx, event);
                            if let Some(cb) = self.drag_callbacks(axis).update.as_mut() {
                                cb(
                                    ctx,
                                    DragUpdateDetails {
                                        position: p.position,
                                        delta,
                                        primary_delta,
                                    },
                                );
                            }
                            ctx.request_redraw();
                            return EventResult::Handled;
                        }
                        _ => {}
                    }
                    if tracking {
                        // Keep painting through the press so the next sample's
                        // stashed raw clock is fresh, even on a dirty-driven
                        // desktop shell.
                        ctx.request_redraw();
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
                    // A started drag ends here; nothing else fires for it.
                    let drag_end = match self.state {
                        Recognizer::Dragging { axis, .. } => Some(axis),
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
                    if std::mem::take(&mut self.press_active)
                        && let Some(cb) = self.on_tap_up.as_mut()
                    {
                        cb(
                            ctx,
                            TapUpDetails {
                                local_position: p.position,
                            },
                        );
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
                    if let Some(axis) = drag_end {
                        let velocity =
                            Vec2::new(self.velocity_x.velocity(), self.velocity_y.velocity());
                        let (velocity, primary_velocity) = axis.project(velocity);
                        if let Some(cb) = self.drag_callbacks(axis).end.as_mut() {
                            cb(
                                ctx,
                                DragEndDetails {
                                    velocity,
                                    primary_velocity,
                                },
                            );
                        }
                        ctx.request_redraw();
                    }
                    return EventResult::Handled;
                }
                PointerPhase::Cancel => {
                    // Cancel only clears internal flags — never touches app state
                    // (no callback), per the interaction-semantics contract. A
                    // started drag's `cancel` is owed instead: latch the
                    // deferred-callback flush so the next rebuild's Housekeeping
                    // broadcast (real state in scope) pays it.
                    if let Recognizer::Dragging { axis, .. } = self.state
                        && self.drag_callbacks(axis).cancel.is_some()
                    {
                        self.owed_cancel = Some(axis);
                        frust_core::mark_pending_result_flush();
                        ctx.request_redraw();
                    }
                    if std::mem::take(&mut self.press_active) {
                        self.owed_tap_cancel = true;
                        frust_core::mark_pending_result_flush();
                        ctx.request_redraw();
                    }
                    self.state = Recognizer::Idle;
                    self.child.event_child(ctx, event);
                    self.child.set_active(false);
                    return EventResult::Handled;
                }
            }
        }
        if event.is_broadcast() {
            // An owed drag `cancel` (see the Cancel arm) rides the first
            // broadcast after it: the Housekeeping pass carries real state.
            self.flush_owed_cancel(ctx);
            self.flush_owed_tap_cancel(ctx);
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

    /// A detector is a sliver exactly when its child is: the transparent layout
    /// runs through the child pod, so the sliver face is the detector itself
    /// and the pod stays sized for this wrapper's own paint and hit test (the
    /// [`ComponentWidget`](frust_core::ComponentWidget) pattern).
    fn as_sliver(&mut self) -> Option<&mut dyn SliverWidget> {
        if self.child.is_sliver() {
            Some(self)
        } else {
            None
        }
    }

    // Paints nothing of its own: the child is the content, and a press a
    // handler takes is already `Handled`.
    fn hit_test_self(&self) -> bool {
        false
    }

    crate::authoring::visit_children!(child);
}

impl SliverWidget for GestureDetectorWidget {
    /// Pass-through, like [`Widget::layout`]: the child sits at the origin and
    /// receives `sc` unchanged, under the child pod's layout skip. The box
    /// layout does nothing beyond the child layout and origin, so the sliver
    /// layout preserves it exactly; every gesture and event path is untouched.
    fn layout_sliver(&mut self, ctx: &mut LayoutCtx, sc: &SliverConstraints) -> SliverGeometry {
        self.child.set_origin(Point::ZERO);
        // A viewport reaches this only through `ChildPod::layout_sliver_child`,
        // which asks `as_sliver` first, so `None` here means a caller invoked
        // `layout_sliver` on a wrapper whose `as_sliver` answered `None`: a
        // contract breach that fails in debug builds; release builds fall back
        // to an empty sliver.
        self.child
            .layout_sliver_child(ctx, sc)
            .unwrap_or_else(|| {
                debug_assert!(
                    false,
                    "GestureDetectorWidget::layout_sliver called on a wrapper whose as_sliver answered None"
                );
                SliverGeometry::ZERO
            })
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
    fn long_press_fires_on_wall_time_under_slow_motion() {
        // At quarter speed 510 ms of raw time is ~128 ms of animation time: a
        // hold read off the animation clock would still be pending.
        frust_core::anim::with_thread_animation_time_scale(0.25, || {
            let mut root = root();
            let mut state = TapState::default();
            let mut sink = NullScene;
            root.event(&mut state, &ev(PointerPhase::Down, 10.0, 10.0));
            root.paint(&mut sink, ft(1000.0));
            root.paint(&mut sink, ft(1490.0));
            root.rebuild(&mut long_press_logic, &mut state);
            assert_eq!(state.long_presses, 0, "490 ms raw is under the threshold");
            root.paint(&mut sink, ft(1510.0));
            root.rebuild(&mut long_press_logic, &mut state);
            assert_eq!(state.long_presses, 1, "500 ms of raw hold fires");
        });
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

    // --- Tap-only held press --------------------------------------------------
    //
    // The event-only `only_on_tap_still_behaves_as_before` above never paints, so
    // its clock never crosses the threshold. These drive the real paint path with
    // an advancing clock (like the both-handlers tests above) but wire ONLY
    // `on_tap`, pinning that a tap-only detector never marks `elapsed` — marking
    // it would resolve the press as a would-be long-press with no handler and
    // fire NOTHING, swallowing the release.

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

    // --- HoldTracker -------------------------------------------------------------

    #[test]
    fn a_hold_tracker_anchors_on_its_first_observation() {
        let mut hold = HoldTracker::new(500.0);
        // The first read records the epoch, whatever the clock says.
        assert_eq!(hold.progress(ft(1_000.0)), 0.0);
        assert!(!hold.fired(ft(1_000.0)));
        assert_eq!(hold.progress(ft(1_250.0)), 0.5);
        assert!(!hold.fired(ft(1_499.0)));
        assert!(hold.fired(ft(1_500.0)), "fires at the threshold, inclusive");
        assert_eq!(hold.progress(ft(9_000.0)), 1.0, "progress clamps at 1.0");
    }

    #[test]
    fn cancelling_a_hold_tracker_re_anchors_the_next_press() {
        let mut hold = HoldTracker::new(500.0);
        assert!(!hold.fired(ft(0.0)));
        assert!(hold.fired(ft(600.0)));
        hold.cancel();
        assert!(!hold.fired(ft(700.0)), "a fresh epoch, not the old one");
        assert!(hold.fired(ft(1_200.0)));
    }

    #[test]
    fn a_threshold_change_mid_hold_keeps_the_epoch() {
        let mut hold = HoldTracker::new(500.0);
        assert!(!hold.fired(ft(0.0)));
        hold.set_threshold_ms(200.0);
        assert!(hold.fired(ft(250.0)));
    }

    // --- Drag recognition ------------------------------------------------------
    //
    // The velocity tests drive the real paint path through `RenderRoot`, the
    // only place a clock is threaded in: each paint stashes the raw input clock
    // that stamps the following `Move`.

    #[derive(Default)]
    struct DragLog {
        taps: u32,
        starts: Vec<(&'static str, DragStartDetails)>,
        updates: Vec<(&'static str, DragUpdateDetails)>,
        ends: Vec<(&'static str, DragEndDetails)>,
        cancels: Vec<&'static str>,
    }

    type DragView = GestureDetectorView<DragLog>;

    fn tap_detector() -> DragView {
        GestureDetector::<DragLog, _>(Blank).on_tap(|s: &mut DragLog| s.taps += 1)
    }

    fn wire_vertical(view: DragView) -> DragView {
        view.on_vertical_drag_start(|s: &mut DragLog, d| s.starts.push(("v", d)))
            .on_vertical_drag_update(|s: &mut DragLog, d| s.updates.push(("v", d)))
            .on_vertical_drag_end(|s: &mut DragLog, d| s.ends.push(("v", d)))
            .on_vertical_drag_cancel(|s: &mut DragLog| s.cancels.push("v"))
    }

    fn wire_horizontal(view: DragView) -> DragView {
        view.on_horizontal_drag_start(|s: &mut DragLog, d| s.starts.push(("h", d)))
            .on_horizontal_drag_update(|s: &mut DragLog, d| s.updates.push(("h", d)))
            .on_horizontal_drag_end(|s: &mut DragLog, d| s.ends.push(("h", d)))
            .on_horizontal_drag_cancel(|s: &mut DragLog| s.cancels.push("h"))
    }

    fn wire_pan(view: DragView) -> DragView {
        view.on_pan_start(|s: &mut DragLog, d| s.starts.push(("pan", d)))
            .on_pan_update(|s: &mut DragLog, d| s.updates.push(("pan", d)))
            .on_pan_end(|s: &mut DragLog, d| s.ends.push(("pan", d)))
            .on_pan_cancel(|s: &mut DragLog| s.cancels.push("pan"))
    }

    fn vertical_logic(_: &mut DragLog) -> DragView {
        wire_vertical(tap_detector())
    }

    fn both_axes_logic(_: &mut DragLog) -> DragView {
        wire_horizontal(wire_vertical(tap_detector()))
    }

    fn pan_logic(_: &mut DragLog) -> DragView {
        wire_pan(tap_detector())
    }

    fn drag_root(logic: fn(&mut DragLog) -> DragView) -> RenderRoot<DragLog, DragView> {
        let mut root = RenderRoot::new();
        let mut log = DragLog::default();
        root.rebuild(&mut { logic }, &mut log);
        root.layout(Size::new(100.0, 100.0));
        root
    }

    fn assert_close(actual: f64, expected: f64, what: &str) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "{what}: expected {expected}, got {actual}"
        );
    }

    #[test]
    fn vertical_drag_starts_only_past_its_own_axis_slop() {
        let mut root = drag_root(vertical_logic);
        let mut log = DragLog::default();
        root.event(&mut log, &ev(PointerPhase::Down, 50.0, 40.0));
        // 30 px of horizontal travel alone never starts a vertical drag.
        root.event(&mut log, &ev(PointerPhase::Move, 80.0, 40.0));
        assert!(log.starts.is_empty(), "horizontal travel does not start it");
        // Exactly the slop is not past it.
        root.event(&mut log, &ev(PointerPhase::Move, 80.0, 58.0));
        assert!(log.starts.is_empty(), "18 px is not past the slop");
        root.event(&mut log, &ev(PointerPhase::Move, 80.0, 59.0));
        assert_eq!(log.starts.len(), 1, "19 px of vertical travel starts it");
        assert_eq!(log.starts[0].0, "v");
        assert_eq!(
            log.starts[0].1.position,
            Point::new(80.0, 59.0),
            "the start point is where the slop was crossed"
        );
        assert!(log.updates.is_empty(), "the crossing move is not an update");
    }

    #[test]
    fn vertical_drag_update_deltas_sum_to_the_travel_after_the_start_point() {
        let mut root = drag_root(vertical_logic);
        let mut log = DragLog::default();
        root.event(&mut log, &ev(PointerPhase::Down, 50.0, 10.0));
        root.event(&mut log, &ev(PointerPhase::Move, 50.0, 30.0)); // starts at y = 30
        root.event(&mut log, &ev(PointerPhase::Move, 52.0, 40.0));
        root.event(&mut log, &ev(PointerPhase::Move, 47.0, 55.0));
        root.event(&mut log, &ev(PointerPhase::Move, 49.0, 70.0));
        assert_eq!(log.starts.len(), 1);
        assert_eq!(log.updates.len(), 3);
        let primary: f64 = log
            .updates
            .iter()
            .map(|(_, d)| d.primary_delta.unwrap())
            .sum();
        let dy: f64 = log.updates.iter().map(|(_, d)| d.delta.y).sum();
        assert_close(
            primary,
            40.0,
            "primary deltas sum to the travel past the start",
        );
        assert_close(dy, 40.0, "delta.y sums to the same travel");
        assert!(
            log.updates.iter().all(|(_, d)| d.delta.x == 0.0),
            "a vertical drag zeroes the cross-axis delta"
        );
        assert_eq!(log.updates[2].1.position, Point::new(49.0, 70.0));
    }

    /// Down at the top, then ten moves at 8 px per 16 ms of raw clock
    /// (500 px/s), each preceded by the paint that stamps it, then `Up`.
    fn scripted_constant_speed_drag(root: &mut RenderRoot<DragLog, DragView>, log: &mut DragLog) {
        let mut sink = NullScene;
        root.event(log, &ev(PointerPhase::Down, 50.0, 0.0));
        root.paint(&mut sink, ft(1_000.0));
        for i in 1..=10 {
            root.paint(&mut sink, ft(1_000.0 + 16.0 * f64::from(i)));
            root.event(log, &ev(PointerPhase::Move, 50.0, 8.0 * f64::from(i)));
        }
        root.event(log, &ev(PointerPhase::Up, 50.0, 80.0));
    }

    #[test]
    fn vertical_drag_end_reports_the_scripted_release_velocity() {
        let mut root = drag_root(vertical_logic);
        let mut log = DragLog::default();
        scripted_constant_speed_drag(&mut root, &mut log);
        assert_eq!(log.ends.len(), 1, "the release ends the drag");
        let end = log.ends[0].1;
        assert!(
            (end.velocity.y - 500.0).abs() < 1.0,
            "velocity.y matches the scripted 500 px/s, got {}",
            end.velocity.y
        );
        assert_eq!(end.velocity.x, 0.0, "a vertical drag zeroes the cross axis");
        let primary = end
            .primary_velocity
            .expect("an axis drag reports a primary velocity");
        assert!(
            (primary - 500.0).abs() < 1.0,
            "primary velocity, got {primary}"
        );
        assert_eq!(log.taps, 0);
    }

    #[test]
    fn drag_velocity_uses_the_raw_input_clock_under_an_animation_time_scale() {
        // At quarter speed the animation clock advances 4 ms per 16 ms of raw
        // time: a velocity read off it would come out at 2000 px/s.
        frust_core::anim::with_thread_animation_time_scale(0.25, || {
            let mut root = drag_root(vertical_logic);
            let mut log = DragLog::default();
            scripted_constant_speed_drag(&mut root, &mut log);
            let primary = log.ends[0].1.primary_velocity.unwrap();
            assert!(
                (primary - 500.0).abs() < 1.0,
                "the time scale leaves the velocity at 500 px/s, got {primary}"
            );
        });
    }

    #[test]
    fn pan_starts_only_past_twice_the_touch_slop() {
        let mut root = drag_root(pan_logic);
        let mut log = DragLog::default();
        root.event(&mut log, &ev(PointerPhase::Down, 10.0, 10.0));
        // hypot(25, 25) ≈ 35.4: past the tap slop, short of the pan slop.
        root.event(&mut log, &ev(PointerPhase::Move, 35.0, 35.0));
        assert!(log.starts.is_empty(), "a pan needs 36 px");
        // hypot(26, 26) ≈ 36.8.
        root.event(&mut log, &ev(PointerPhase::Move, 36.0, 36.0));
        assert_eq!(log.starts.len(), 1, "past 36 px the pan starts");
        assert_eq!(log.starts[0].0, "pan");
        root.event(&mut log, &ev(PointerPhase::Move, 40.0, 33.0));
        let update = log.updates[0].1;
        assert_eq!(update.delta, Vec2::new(4.0, -3.0), "a pan keeps both axes");
        assert_eq!(update.primary_delta, None, "a pan has no primary axis");
        root.event(&mut log, &ev(PointerPhase::Up, 40.0, 33.0));
        assert_eq!(log.ends[0].1.primary_velocity, None);
        assert_eq!(log.taps, 0, "the press past the tap slop is no tap");
    }

    #[test]
    fn with_both_axes_wired_the_first_axis_to_cross_wins() {
        let mut root = drag_root(both_axes_logic);
        let mut log = DragLog::default();
        root.event(&mut log, &ev(PointerPhase::Down, 50.0, 50.0));
        root.event(&mut log, &ev(PointerPhase::Move, 60.0, 69.0)); // vertical crosses first
        // A later horizontal sweep stays a vertical drag: the winner locks.
        root.event(&mut log, &ev(PointerPhase::Move, 95.0, 70.0));
        root.event(&mut log, &ev(PointerPhase::Up, 95.0, 70.0));
        assert_eq!(log.starts.len(), 1);
        assert_eq!(log.starts[0].0, "v");
        assert_eq!(log.updates.len(), 1);
        assert_eq!(log.updates[0].0, "v");
        assert_eq!(log.updates[0].1.delta, Vec2::new(0.0, 1.0));
        assert_eq!(log.ends.len(), 1);
        assert_eq!(log.ends[0].0, "v");

        // Both slops crossed by one move: the larger travel-to-slop ratio wins.
        let mut log = DragLog::default();
        root.event(&mut log, &ev(PointerPhase::Down, 50.0, 50.0));
        root.event(&mut log, &ev(PointerPhase::Move, 75.0, 70.0));
        assert_eq!(log.starts.len(), 1);
        assert_eq!(log.starts[0].0, "h", "25 px across beats 20 px down");
    }

    #[test]
    fn a_started_drag_suppresses_the_tap() {
        let mut root = drag_root(vertical_logic);
        let mut log = DragLog::default();
        root.event(&mut log, &ev(PointerPhase::Down, 50.0, 10.0));
        root.event(&mut log, &ev(PointerPhase::Move, 50.0, 40.0));
        root.event(&mut log, &ev(PointerPhase::Up, 50.0, 40.0));
        assert_eq!(log.taps, 0, "a drag never also taps");
        assert_eq!(log.ends.len(), 1);

        // A press that stays inside the slop is still a tap with drags wired.
        let mut log = DragLog::default();
        root.event(&mut log, &ev(PointerPhase::Down, 50.0, 10.0));
        root.event(&mut log, &ev(PointerPhase::Move, 52.0, 20.0));
        root.event(&mut log, &ev(PointerPhase::Up, 52.0, 20.0));
        assert_eq!(log.taps, 1);
        assert!(log.starts.is_empty() && log.ends.is_empty());
    }

    #[test]
    fn cancel_after_the_start_fires_cancel_and_before_it_fires_nothing() {
        let mut root = drag_root(vertical_logic);
        let mut log = DragLog::default();
        root.event(&mut log, &ev(PointerPhase::Down, 50.0, 10.0));
        root.event(&mut log, &ev(PointerPhase::Move, 50.0, 40.0));
        root.event(&mut log, &ev(PointerPhase::Cancel, 50.0, 40.0));
        assert!(
            log.cancels.is_empty(),
            "the Cancel arm itself touches no state"
        );
        // The next rebuild's Housekeeping broadcast pays the owed cancel.
        root.rebuild(&mut vertical_logic, &mut log);
        assert_eq!(log.cancels, vec!["v"], "a cancelled drag fires its cancel");
        assert!(log.ends.is_empty(), "a cancelled drag never ends");
        root.rebuild(&mut vertical_logic, &mut log);
        assert_eq!(log.cancels.len(), 1, "the cancel fires once");

        // Before the start — inside the tap slop, then past it but short of the
        // vertical slop — a Cancel stays silent.
        for (x, y) in [(55.0, 15.0), (80.0, 15.0)] {
            let mut log = DragLog::default();
            root.event(&mut log, &ev(PointerPhase::Down, 50.0, 10.0));
            root.event(&mut log, &ev(PointerPhase::Move, x, y));
            root.event(&mut log, &ev(PointerPhase::Cancel, x, y));
            root.rebuild(&mut vertical_logic, &mut log);
            assert!(log.cancels.is_empty(), "no drag started, no cancel");
            assert!(log.starts.is_empty());
            assert_eq!(log.taps, 0);
        }
    }

    #[test]
    fn a_cancel_over_throwaway_state_defers_the_drag_cancel_to_housekeeping() {
        // A structural rebuild cancels a captured child over `()` state; the
        // Cancel arm must not reach for application state there.
        let view = vertical_logic(&mut DragLog::default());
        let mut counter = 0u64;
        let mut w: GestureDetectorWidget =
            View::<DragLog>::build(&view, &mut BuildCtx::new(&mut counter));
        let mut log = DragLog::default();
        let send = |w: &mut GestureDetectorWidget, log: &mut DragLog, event: &InputEvent| {
            let state_any: &mut dyn Any = log;
            let mut ctx = EventCtx::new(state_any, Point::ZERO, Size::new(100.0, 100.0));
            w.event(&mut ctx, event);
        };
        send(&mut w, &mut log, &ev(PointerPhase::Down, 50.0, 10.0));
        send(&mut w, &mut log, &ev(PointerPhase::Move, 50.0, 40.0));
        assert_eq!(log.starts.len(), 1);
        let mut unit = ();
        let mut ctx = EventCtx::new(&mut unit, Point::ZERO, Size::new(100.0, 100.0));
        w.event(&mut ctx, &ev(PointerPhase::Cancel, 50.0, 40.0));
        assert!(
            frust_core::take_pending_result_flush(),
            "the owed cancel latches the deferred-callback flush"
        );
        send(&mut w, &mut log, &InputEvent::Housekeeping);
        assert_eq!(log.cancels, vec!["v"], "Housekeeping pays the owed cancel");
    }

    #[test]
    fn a_detector_without_drag_callbacks_keeps_its_unarmed_drag_state() {
        // No drag recognizer wired: past the slop the press is plain `Dragged`,
        // tracks no velocity and stashes no clock.
        let mut w = widget(true);
        let mut state = TapState::default();
        dispatch(&mut w, &mut state, &ev(PointerPhase::Down, 10.0, 10.0));
        assert!(!w.tracks_velocity());
        dispatch(&mut w, &mut state, &ev(PointerPhase::Move, 10.0, 60.0));
        assert!(matches!(w.state, Recognizer::Dragged));
        assert!(w.velocity_y.is_empty());
    }

    /// A fixed 100×400 child, tall enough to scroll in a 100×100 viewport.
    struct Tall;
    struct TallWidget;
    impl<State: 'static> View<State> for Tall {
        type Element = TallWidget;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> TallWidget {
            TallWidget
        }
        fn rebuild(
            &self,
            _prev: &Self,
            _element: &mut TallWidget,
            _ctx: &mut BuildCtx<'_>,
        ) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }
    impl Widget for TallWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(100.0, 400.0))
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    type ScrolledDrag = crate::scroll::ScrollView<DragLog>;

    fn scrolled_root(logic: fn(&mut DragLog) -> ScrolledDrag) -> RenderRoot<DragLog, ScrolledDrag> {
        let mut root = RenderRoot::new();
        let mut log = DragLog::default();
        root.rebuild(&mut { logic }, &mut log);
        root.layout(Size::new(100.0, 100.0));
        root
    }

    #[test]
    fn inside_a_vertical_scroll_view_a_cross_axis_drag_starts_and_a_same_axis_one_does_not() {
        fn horizontal_in_scroll(_: &mut DragLog) -> ScrolledDrag {
            crate::scroll::scroll_view(wire_horizontal(GestureDetector::<DragLog, _>(Tall)))
        }
        fn vertical_in_scroll(_: &mut DragLog) -> ScrolledDrag {
            crate::scroll::scroll_view(wire_vertical(GestureDetector::<DragLog, _>(Tall)))
        }

        let mut root = scrolled_root(horizontal_in_scroll);
        let mut log = DragLog::default();
        root.event(&mut log, &ev(PointerPhase::Down, 20.0, 50.0));
        root.event(&mut log, &ev(PointerPhase::Move, 50.0, 52.0));
        root.event(&mut log, &ev(PointerPhase::Move, 60.0, 53.0));
        root.event(&mut log, &ev(PointerPhase::Up, 60.0, 53.0));
        assert_eq!(
            log.starts.len(),
            1,
            "a cross-axis drag starts inside the scroll"
        );
        assert_eq!(log.starts[0].0, "h");
        assert_eq!(log.updates[0].1.primary_delta, Some(10.0));
        assert_eq!(log.ends.len(), 1);

        let mut root = scrolled_root(vertical_in_scroll);
        let mut log = DragLog::default();
        root.event(&mut log, &ev(PointerPhase::Down, 50.0, 80.0));
        root.event(&mut log, &ev(PointerPhase::Move, 50.0, 50.0));
        root.event(&mut log, &ev(PointerPhase::Move, 50.0, 30.0));
        root.event(&mut log, &ev(PointerPhase::Up, 50.0, 30.0));
        root.rebuild(&mut vertical_in_scroll, &mut log);
        assert!(
            log.starts.is_empty(),
            "the scroll claims the same-axis drag first"
        );
        assert!(
            log.cancels.is_empty(),
            "a press cancelled before its start is silent"
        );
        assert!(log.ends.is_empty());
    }

    // --- Press state -----------------------------------------------------------

    #[derive(Default)]
    struct PressLog {
        events: Vec<String>,
    }

    type PressView = GestureDetectorView<PressLog>;

    fn press_detector<V: View<PressLog>>(child: V) -> PressView {
        GestureDetector::<PressLog, _>(child)
            .on_tap_down(|s: &mut PressLog, d| {
                s.events.push(format!(
                    "down {} {}",
                    d.local_position.x as i32, d.local_position.y as i32
                ))
            })
            .on_tap_up(|s: &mut PressLog, d| {
                s.events.push(format!(
                    "up {} {}",
                    d.local_position.x as i32, d.local_position.y as i32
                ))
            })
            .on_tap_cancel(|s: &mut PressLog| s.events.push("cancel".into()))
            .on_tap(|s: &mut PressLog| s.events.push("tap".into()))
            .on_long_press(|s: &mut PressLog| s.events.push("long".into()))
    }

    fn press_logic(_: &mut PressLog) -> PressView {
        press_detector(Blank)
    }

    fn press_root() -> RenderRoot<PressLog, PressView> {
        let mut root = RenderRoot::new();
        let mut log = PressLog::default();
        root.rebuild(&mut { press_logic }, &mut log);
        root.layout(Size::new(100.0, 100.0));
        root
    }

    fn press_widget() -> GestureDetectorWidget {
        let view = press_logic(&mut PressLog::default());
        let mut counter = 0u64;
        View::<PressLog>::build(&view, &mut BuildCtx::new(&mut counter))
    }

    fn send_sized(
        w: &mut GestureDetectorWidget,
        log: &mut PressLog,
        size: Size,
        event: &InputEvent,
    ) {
        let state_any: &mut dyn Any = log;
        let mut ctx = EventCtx::new(state_any, Point::ZERO, size);
        w.event(&mut ctx, event);
    }

    fn send(w: &mut GestureDetectorWidget, log: &mut PressLog, event: &InputEvent) {
        send_sized(w, log, Size::new(100.0, 100.0), event);
    }

    #[test]
    fn a_still_press_delivers_down_then_up_with_nothing_between() {
        let mut root = press_root();
        let mut log = PressLog::default();
        root.event(&mut log, &ev(PointerPhase::Down, 10.0, 20.0));
        root.rebuild(&mut { press_logic }, &mut log);
        root.rebuild(&mut { press_logic }, &mut log);
        assert_eq!(log.events, vec!["down 10 20"], "down fires immediately");
        root.event(&mut log, &ev(PointerPhase::Up, 10.0, 20.0));
        assert_eq!(log.events, vec!["down 10 20", "up 10 20", "tap"]);
    }

    #[test]
    fn a_move_past_the_slop_cancels_at_once_and_the_up_delivers_no_tap_up() {
        let mut root = press_root();
        let mut log = PressLog::default();
        root.event(&mut log, &ev(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut log, &ev(PointerPhase::Move, 10.0, 29.0));
        assert_eq!(log.events, vec!["down 10 10", "cancel"]);
        root.event(&mut log, &ev(PointerPhase::Up, 10.0, 29.0));
        assert_eq!(log.events, vec!["down 10 10", "cancel"], "no up, no tap");
    }

    #[test]
    fn leaving_the_bounds_within_the_slop_cancels_the_press_but_not_a_later_tap() {
        let mut w = press_widget();
        let mut log = PressLog::default();
        let small = Size::new(20.0, 20.0);
        send_sized(&mut w, &mut log, small, &ev(PointerPhase::Down, 10.0, 10.0));
        // 11 px from the Down: inside the slop, outside the 20 px bounds.
        send_sized(&mut w, &mut log, small, &ev(PointerPhase::Move, 21.0, 10.0));
        assert_eq!(log.events, vec!["down 10 10", "cancel"]);
        send_sized(&mut w, &mut log, small, &ev(PointerPhase::Up, 12.0, 10.0));
        assert_eq!(
            log.events,
            vec!["down 10 10", "cancel", "tap"],
            "the tap rule is untouched"
        );
    }

    #[test]
    fn a_scroll_steal_cancels_the_press_after_the_next_housekeeping() {
        fn logic(_: &mut PressLog) -> crate::scroll::ScrollView<PressLog> {
            crate::scroll::scroll_view(press_detector(Tall))
        }
        let mut root = RenderRoot::new();
        let mut log = PressLog::default();
        root.rebuild(&mut { logic }, &mut log);
        root.layout(Size::new(100.0, 100.0));
        root.event(&mut log, &ev(PointerPhase::Down, 50.0, 80.0));
        root.event(&mut log, &ev(PointerPhase::Move, 50.0, 50.0));
        root.event(&mut log, &ev(PointerPhase::Move, 50.0, 30.0));
        root.event(&mut log, &ev(PointerPhase::Up, 50.0, 30.0));
        root.rebuild(&mut { logic }, &mut log);
        assert_eq!(log.events.first().map(String::as_str), Some("down 50 80"));
        assert_eq!(
            log.events.iter().filter(|e| *e == "cancel").count(),
            1,
            "{:?}",
            log.events
        );
        assert!(
            !log.events.iter().any(|e| e.starts_with("up") || e == "tap"),
            "a stolen press never releases: {:?}",
            log.events
        );
    }

    #[test]
    fn a_system_cancel_delivers_the_cancel_via_housekeeping() {
        let mut root = press_root();
        let mut log = PressLog::default();
        root.event(&mut log, &ev(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut log, &ev(PointerPhase::Cancel, 10.0, 10.0));
        assert_eq!(
            log.events,
            vec!["down 10 10"],
            "the Cancel arm is state-free"
        );
        root.rebuild(&mut { press_logic }, &mut log);
        assert_eq!(log.events, vec!["down 10 10", "cancel"]);
        root.rebuild(&mut { press_logic }, &mut log);
        assert_eq!(log.events.len(), 2, "the cancel fires once");
    }

    #[test]
    fn a_cancel_over_throwaway_state_defers_the_tap_cancel_to_housekeeping() {
        let mut w = press_widget();
        let mut log = PressLog::default();
        send(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, 10.0));
        let mut unit = ();
        let mut ctx = EventCtx::new(&mut unit, Point::ZERO, Size::new(100.0, 100.0));
        w.event(&mut ctx, &ev(PointerPhase::Cancel, 10.0, 10.0));
        assert!(
            frust_core::take_pending_result_flush(),
            "the owed cancel latches the deferred-callback flush"
        );
        assert_eq!(log.events, vec!["down 10 10"]);
        send(&mut w, &mut log, &InputEvent::Housekeeping);
        assert_eq!(log.events, vec!["down 10 10", "cancel"]);
    }

    #[test]
    fn a_second_down_while_pressed_delivers_cancel_then_down() {
        let mut w = press_widget();
        let mut log = PressLog::default();
        send(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, 10.0));
        send(&mut w, &mut log, &ev(PointerPhase::Down, 30.0, 30.0));
        assert_eq!(log.events, vec!["down 10 10", "cancel", "down 30 30"]);
        // An owed cancel is paid before the next down, too.
        let mut w = press_widget();
        let mut log = PressLog::default();
        send(&mut w, &mut log, &ev(PointerPhase::Down, 10.0, 10.0));
        send(&mut w, &mut log, &ev(PointerPhase::Cancel, 10.0, 10.0));
        send(&mut w, &mut log, &ev(PointerPhase::Down, 30.0, 30.0));
        assert_eq!(log.events, vec!["down 10 10", "cancel", "down 30 30"]);
    }

    #[test]
    fn a_long_hold_ends_in_tap_up_after_the_long_press_and_a_tap_orders_up_first() {
        let mut root = press_root();
        let mut log = PressLog::default();
        let mut sink = NullScene;
        root.event(&mut log, &ev(PointerPhase::Down, 10.0, 10.0));
        root.paint(&mut sink, ft(0.0));
        root.paint(&mut sink, ft(600.0));
        root.event(&mut log, &ev(PointerPhase::Up, 10.0, 10.0));
        assert!(
            log.events.contains(&"long".to_string()) && log.events.contains(&"up 10 10".into()),
            "{:?}",
            log.events
        );
        assert!(!log.events.contains(&"tap".to_string()), "{:?}", log.events);
        assert_eq!(log.events.last().map(String::as_str), Some("long"));

        let mut log = PressLog::default();
        root.event(&mut log, &ev(PointerPhase::Down, 10.0, 10.0));
        root.event(&mut log, &ev(PointerPhase::Up, 10.0, 10.0));
        assert_eq!(log.events, vec!["down 10 10", "up 10 10", "tap"]);
    }

    #[test]
    fn every_tap_down_is_followed_by_exactly_one_up_or_cancel() {
        // A deterministic generator over the six event kinds.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..200 {
            let mut w = press_widget();
            let mut log = PressLog::default();
            for _ in 0..40 {
                let event = match next() % 6 {
                    0 => ev(PointerPhase::Down, 50.0, 50.0),
                    1 => ev(PointerPhase::Move, 52.0, 52.0),
                    2 => ev(PointerPhase::Move, 150.0, 50.0),
                    3 => ev(PointerPhase::Up, 50.0, 50.0),
                    4 => ev(PointerPhase::Cancel, 50.0, 50.0),
                    _ => InputEvent::Housekeeping,
                };
                send(&mut w, &mut log, &event);
            }
            // End any press still held, then let the owed cancel flush.
            send(&mut w, &mut log, &ev(PointerPhase::Cancel, 50.0, 50.0));
            send(&mut w, &mut log, &InputEvent::Housekeeping);
            let mut open = false;
            for e in &log.events {
                if e.starts_with("down") {
                    assert!(!open, "down while open: {:?}", log.events);
                    open = true;
                } else if e == "cancel" || e.starts_with("up") {
                    assert!(open, "up/cancel without a down: {:?}", log.events);
                    open = false;
                }
            }
            assert!(!open, "a down was left open: {:?}", log.events);
        }
    }
}
