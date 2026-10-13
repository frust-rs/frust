//! The scroll surface core: the drag, fling, overscroll, nested-claim and
//! programmatic-scroll machinery every scroll surface shares, written once.
//!
//! [`crate::ScrollView`]'s [`crate::ScrollWidget`] and [`crate::ListView`]'s
//! [`crate::ListViewWidget`] used to carry near-verbatim copies of this — the
//! per-move drag accumulator and its edge pull, the takeover at
//! [`TOUCH_SLOP`] with its innermost-wins claim and live multi-contact veto,
//! the legacy fling and release-settle, the generic ballistic driver, the
//! controller attach/drain/publish plumbing and the one-event-late `on_scroll`
//! rule — kept in step by hand. Now each surface is a thin owner of its own
//! geometry (viewport and content measurement, child origins, list windowing)
//! around one [`ScrollSurfaceCore`], and the logic lives here as
//! [`ScrollSurface`]'s provided methods.
//!
//! # State and logic, split
//!
//! [`ScrollSurfaceCore`] owns every piece of state the machinery needs except
//! the scroll *position* itself. The two surfaces store a past-edge pull
//! differently, and on purpose: a `ScrollWidget` lets its one `offset` run out
//! of range, while a `ListViewWidget` keeps its windowing offset clamped (it is
//! item-index math) and carries the displacement in a separate `overscroll`.
//! Unifying the two would change the floating-point arithmetic each one's
//! drag, settle and ballistic paths perform, so the position stays with its
//! owner and [`ScrollSurface`]'s required methods are the narrow seam the core
//! reads and writes it through — [`ScrollSurface::pixels`],
//! [`ScrollSurface::displacement`], [`ScrollSurface::set_allowed_position`],
//! [`ScrollSurface::decay_displacement`] and friends — each implemented with
//! exactly the operations that owner always used.
//!
//! The behavioral contract is unchanged and documented where it always was:
//! `scroll.rs`'s module docs (*Physics seam*, *Gesture takeover*, *Nested
//! scrolling*, *Multi-contact veto*, *Fling driver*, *Programmatic
//! scrolling*) and `docs/WIDGETS_ARCHITECTURE.md`'s *Scroll Physics*.
//!
//! # Programmatic notifications
//!
//! A controller command or tween moves the position from layout or paint,
//! which carry no [`EventCtx`] to fire `on_scroll` through. The notification is
//! recorded ([`ScrollSurfaceCore::pending_scroll_notify`]) and delivered on the
//! next event; a jump that moved anything, **and every frame a controller
//! tween advances, up to and including the frame it lands on**, also owes a
//! flush ([`ScrollSurfaceCore::scroll_notify_owed`]), so the paint that sees
//! it raises [`frust_core::mark_pending_result_flush`] and the very next
//! frame's `Housekeeping` event delivers it — a programmatic move has no user
//! event of its own to ride. A release fling or settle keeps the plain
//! one-event-late rule: the finger that started it is the event stream it
//! answers to.

use std::cell::Cell;
use std::rc::Rc;

use frust_core::{
    EventCtx, EventResult, FLING_STOP, FrameTime, InputEvent, PaintCtx, PointerEvent, PointerPhase,
    ScrollDelta, TOUCH_SLOP, VelocityTracker, WHEEL_LINE_PX, fling_decay, fling_displacement,
};
use kurbo::{Affine, Point, Size};

use crate::authoring::{ErasedArgCallback, ErasedCallback, presses};
use crate::physics::effect::OverscrollEffect;
use crate::physics::simulation::TweenSimulation;
use crate::physics::{
    MOMENTUM_RETAIN_VELOCITY_THRESHOLD_FACTOR, ScrollMetrics, ScrollPhysics, Simulation,
    default_overscroll_effect, default_physics,
};
use crate::scroll::{
    InnerScrollState, METRICS_FALLBACK_DPR, SETTLE_DECAY, SETTLE_STOP_PX, ScrollInfo,
    ambient_scroll_claim, crossed_refresh_trigger, inner_claim_state, stretch_about_edge,
    with_scroll_claim, with_scroll_veto,
};
use crate::scroll_controller::{
    AnimateTo, ControllerCommand, ScrollBinding, ScrollCommand, ScrollController, ScrollToItem,
};

/// A running [`Simulation`] and the frame clock it is measured from — the
/// state behind the generic ballistic driver ([`ScrollSurface::drive_ballistic`])
/// a release fling, a release spring and a controller tween all run through.
pub(crate) struct BallisticState {
    /// The physics-built curve, in **seconds** from its own start.
    pub(crate) sim: Box<dyn Simulation>,
    /// The frame time the first paint-time pump seeded, or `None` before it —
    /// the same zero-delta seeding convention `last_anim` uses, so the release
    /// itself (which carries no clock) never has to guess a start.
    pub(crate) start: Option<FrameTime>,
}

impl BallisticState {
    /// Seconds elapsed at frame time `now`; `0.0` until the clock is seeded.
    pub(crate) fn elapsed_secs(&self, now: FrameTime) -> f64 {
        match self.start {
            Some(start) => now.saturating_sub(start).as_secs_f64(),
            None => 0.0,
        }
    }
}

/// The axis a core reads pointer travel and wheel deltas along. Local to the
/// core: `frust_core` has no crate-wide scroll-axis type for it to reuse.
/// Only the event-space projection reads it — the owner's geometry decides
/// what the position means on screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ScrollAxis {
    /// Top-to-bottom: pointer `y`, a wheel's vertical delta.
    #[default]
    Vertical,
    /// Leading-to-trailing: pointer `x`, a wheel's horizontal delta.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "no shipped surface scrolls horizontally yet; the core's tests pin it"
        )
    )]
    Horizontal,
}

impl ScrollAxis {
    /// The component of `point` along this axis.
    fn along(self, point: Point) -> f64 {
        match self {
            ScrollAxis::Vertical => point.y,
            ScrollAxis::Horizontal => point.x,
        }
    }

    /// A wheel `delta`'s travel along this axis in logical px (a line is
    /// [`WHEEL_LINE_PX`]).
    fn wheel_delta(self, delta: &ScrollDelta) -> f64 {
        match (self, delta) {
            (ScrollAxis::Vertical, ScrollDelta::Lines(_, y)) => y * WHEEL_LINE_PX,
            (ScrollAxis::Vertical, ScrollDelta::Pixels(_, y)) => *y,
            (ScrollAxis::Horizontal, ScrollDelta::Lines(x, _)) => x * WHEEL_LINE_PX,
            (ScrollAxis::Horizontal, ScrollDelta::Pixels(x, _)) => *x,
        }
    }
}

/// Every piece of scroll-surface state except the position itself (see the
/// [module docs](self)): the installed physics and overscroll effect, the
/// drag accumulator and edge pull, the per-gesture takeover/claim/veto state,
/// post-release motion, the frame clocks, the observation callbacks and the
/// controller binding.
///
/// `pub` only so an owner's test-build `Deref` can name it — the module is
/// private, so nothing outside this crate can reach the type, and every field
/// and method is crate-visible at most.
pub struct ScrollSurfaceCore {
    /// The axis pointer travel and wheel deltas are read along.
    axis: ScrollAxis,
    /// The installed scroll physics — [`crate::physics::default_physics`]'s
    /// platform-adaptive choice unless the view names one. `Rc`, not `Box`:
    /// every [`ScrollPhysics`] method takes `&self`, so a shared, immutable
    /// handle is both cheap to (re)install on rebuild (a `Rc::clone`, where
    /// `Box<dyn ScrollPhysics>` isn't `Clone`) and sufficient. Survives a
    /// rebuild whose view carries no `.physics(...)` call untouched (see
    /// [`ScrollSurfaceCore::install`]).
    pub(crate) physics: Rc<dyn ScrollPhysics>,
    /// How past-edge pull is visualized —
    /// [`crate::physics::default_overscroll_effect`]'s platform pairing unless
    /// the view names one. Read at paint alone, never by layout or the physics.
    pub(crate) effect: OverscrollEffect,
    /// The signed pull past an edge, in [`ScrollInfo::overscroll`]'s sense:
    /// **negative past the leading edge**, positive past the trailing one,
    /// `0.0` in range. The displacement the physics *allowed* plus whatever
    /// [`ScrollPhysics::apply_boundary_conditions`] *rejected* while the
    /// position was pinned at the edge — so a clamping physics, whose position
    /// never leaves range, still reports how hard the finger is pulling. Under
    /// a physics that rejects nothing (`Bouncing`,
    /// [`RubberBand`](crate::RubberBand)) it is exactly the displacement.
    ///
    /// Re-derived from [`ScrollSurfaceCore::drag_position`] on every drag move
    /// (never summed across moves, which would strand a rejected pull the
    /// finger has since eased back), re-seeded from the live displacement on
    /// `Down`, re-derived on every ballistic pump, decayed by the
    /// release-settle, and zeroed by the wheel/`Cancel` hard clamps and every
    /// controller command. A ballistic that ends with a pull outstanding hands
    /// it to the settle ([`ScrollSurface::settle_ballistic_residual`]).
    pub(crate) edge_pull: f64,
    /// The physics-mapped drag position accumulated during an active scroll
    /// drag, **before** boundary rejection, in the owner's
    /// [`ScrollSurface::pixels`] space: seeded from it at takeover (so a regrab
    /// mid-bounce continues from what is on screen), advanced by each move's
    /// mapped delta ([`ScrollSurface::apply_drag_offset`]), and re-seeded by
    /// every controller command. It runs off past the edge under a clamping
    /// physics — which is exactly what lets a pinned surface still report how
    /// hard the finger is pulling.
    pub(crate) drag_position: f64,
    /// Whether we have taken the gesture over as a scroll drag (past the slop).
    pub(crate) scrolling: bool,
    /// Whether a release-settle is easing an overscrolled surface back to its
    /// clamped edge (driven at paint via [`ScrollSurface::advance_settle`]).
    pub(crate) settling: bool,
    /// A generic ballistic simulation — a physics-built release fling/spring
    /// or a controller [`ScrollController::animate_to`] tween — or `None`.
    /// Always `None` after a release under [`RubberBand`](crate::RubberBand),
    /// which keeps the legacy [`ScrollSurfaceCore::fling`]/settle path.
    pub(crate) ballistic: Option<BallisticState>,
    /// Velocity (px/s of offset) of the motion a new `Down` interrupted, fed to
    /// [`ScrollPhysics::carried_momentum`] at the next fling start. `Down` is
    /// its only writer and always writes it (`0.0` on a resting surface), so it
    /// can never carry a stale value into a later gesture.
    pub(crate) carried_velocity: f64,
    /// Active legacy fling velocity, in px/s of *offset* (opposite the
    /// finger), or `None` when not flinging.
    pub(crate) fling: Option<f64>,
    /// Whether a `Down` has armed an active gesture (distinct from
    /// `scrolling`, which only becomes true past the slop). The slop/takeover
    /// math runs only while it is true, so a hover `Move` is never mistaken
    /// for a drag and can never take the gesture from a child.
    pub(crate) down_active: bool,
    /// What the nearest nested scroll surface claimed it could do with this
    /// gesture, read back out of the claim cell this surface pushed around the
    /// `Down`'s forward — the whole input to the innermost-wins decision at
    /// the takeover site (`scroll.rs`'s *Nested scrolling*). Reset on `Down`
    /// (before that forward) and on `Up`/`Cancel`.
    ///
    /// **Accepted staleness**: a `Down`-time snapshot. An inner revealed or
    /// re-pinned *during* the gesture never re-registers, and the decision
    /// taken from it is never revisited — the same limitation the navigator's
    /// edge-swipe claim accepts (one arbitration point per gesture).
    pub(crate) inner_at_down: InnerScrollState,
    /// Whether this gesture was handed to the nested surface at the takeover
    /// site — **sticky** for the rest of the gesture, so a mid-gesture
    /// direction reversal cannot steal the drag back. Cleared with
    /// [`ScrollSurfaceCore::inner_at_down`].
    pub(crate) deferring: bool,
    /// The live multi-contact veto a nested recognizer (`pinch_detector`,
    /// [`crate::pan_zoom()`]'s child-owned-press branch) raises for the rest of
    /// this gesture while it tracks more than the claimant's own contact
    /// (`scroll.rs`'s *Multi-contact veto*). Checked on every `Move` at the
    /// takeover site. Replaced with a fresh, unset cell on every `Down` (and
    /// again on `Up`/`Cancel`) so a stale recognizer handle from a previous
    /// gesture can never veto this one.
    pub(crate) live_veto: Rc<Cell<bool>>,
    /// Where this gesture's `Down` landed — the slop is measured from here.
    pub(crate) down_start: Point,
    /// The pointer position the previous drag `Move` was measured to.
    pub(crate) last_drag: Point,
    /// The release-velocity estimator, fed along [`ScrollSurfaceCore::axis`].
    pub(crate) tracker: VelocityTracker,
    /// The most recent paint's frame time, reused as the event-pass timestamp
    /// for velocity tracking (the event pass carries no clock of its own).
    pub(crate) last_frame_time: FrameTime,
    /// Last animation frame time for the paint-time pump; `None` seeds the
    /// clock (zero-delta) on the first paint after a release.
    pub(crate) last_anim: Option<FrameTime>,
    /// The scroll observation callback (`None` if the view set none).
    pub(crate) on_scroll: Option<ErasedArgCallback<ScrollInfo>>,
    /// A scroll notification produced by a pass with no [`EventCtx`] (the
    /// paint-time pump, a controller command); delivered to `on_scroll` on the
    /// next event and cleared by a `Cancel` without firing.
    pub(crate) pending_scroll_notify: bool,
    /// The pull-to-refresh release callback (`None` if the view set none).
    pub(crate) on_refresh_release: Option<ErasedCallback>,
    /// This surface's claim on an attached [`ScrollController`], or `None`.
    /// Dropping it (with the widget, or on a rebuild that names no handle)
    /// detaches the handle when this surface still holds it.
    pub(crate) controller: Option<ScrollBinding>,
    /// Whether a layout has run, so the extent a controller command clamps
    /// against (and the snapshot it publishes) is a measured one rather than
    /// the pre-layout zero.
    pub(crate) laid_out: bool,
    /// Whether a programmatic move since the last paint owes `on_scroll` a
    /// `Housekeeping` flush (the [module docs](self)' *Programmatic
    /// notifications*). Consumed by [`ScrollSurface::paint_motion`].
    pub(crate) scroll_notify_owed: bool,
    /// Whether [`ScrollSurfaceCore::ballistic`] is running a controller
    /// [`ScrollController::animate_to`] tween rather than a release
    /// fling/spring — this surface's own mirror of the attached
    /// [`ScrollController::is_animating`] flag, read back at the one tear-down
    /// site ([`ScrollSurfaceCore::stop_ballistic`]) so the controller is told
    /// the instant it actually ends.
    pub(crate) controller_animating: bool,
}

impl Default for ScrollSurfaceCore {
    fn default() -> Self {
        Self {
            axis: ScrollAxis::default(),
            physics: default_physics(),
            effect: default_overscroll_effect(),
            edge_pull: 0.0,
            drag_position: 0.0,
            scrolling: false,
            settling: false,
            ballistic: None,
            carried_velocity: 0.0,
            fling: None,
            down_active: false,
            inner_at_down: InnerScrollState::default(),
            deferring: false,
            live_veto: Rc::new(Cell::new(false)),
            down_start: Point::ZERO,
            last_drag: Point::ZERO,
            tracker: VelocityTracker::new(),
            last_frame_time: FrameTime::ZERO,
            last_anim: None,
            on_scroll: None,
            pending_scroll_notify: false,
            on_refresh_release: None,
            controller: None,
            laid_out: false,
            scroll_notify_owed: false,
            controller_animating: false,
        }
    }
}

impl ScrollSurfaceCore {
    /// Carry a view's physics and overscroll effect down on build/rebuild: a
    /// `.physics(...)`-carrying view reinstalls it every time (a trait object
    /// isn't comparable, so this never diffs), a view with no opinion
    /// (`None`) leaves whatever is installed alone, and the effect — plain
    /// view-owned data — is always carried down.
    pub(crate) fn install(
        &mut self,
        physics: Option<&Rc<dyn ScrollPhysics>>,
        effect: OverscrollEffect,
    ) {
        if let Some(physics) = physics {
            self.physics = Rc::clone(physics);
        }
        self.effect = effect;
    }

    /// Whether post-release motion is in flight — the legacy fling, or a
    /// [`Simulation`] the generic driver is running (never both).
    pub(crate) fn is_flinging(&self) -> bool {
        self.fling.is_some() || self.ballistic.is_some()
    }

    /// Whether anything the paint pump drives is live: a fling, a settle or a
    /// simulation.
    fn is_moving(&self) -> bool {
        self.fling.is_some() || self.settling || self.ballistic.is_some()
    }

    /// The velocity (px/s of offset) of whatever post-release motion is live
    /// right now — the legacy fling's own, or a running simulation's at the
    /// last painted frame — and `0.0` at rest.
    fn live_velocity(&self) -> f64 {
        if let Some(v) = self.fling {
            return v;
        }
        match self.ballistic.as_ref() {
            Some(state) => state.sim.dx(state.elapsed_secs(self.last_frame_time)),
            None => 0.0,
        }
    }

    /// A new fling's starting velocity: the `release` velocity plus whatever
    /// [`ScrollPhysics::carried_momentum`] carries over from the motion this
    /// gesture's `Down` interrupted ([`RubberBand`](crate::RubberBand)
    /// carries `0.0`, leaving `release` untouched).
    ///
    /// **Momentum is carried only onto a release that plainly continues the
    /// interrupted motion**: same sign, and faster than
    /// [`MOMENTUM_RETAIN_VELOCITY_THRESHOLD_FACTOR`] of the physics' own
    /// **mapped** share of the interrupted velocity —
    /// `physics.carried_momentum(carried)`, the exact value the release is
    /// about to add, not the raw interrupted speed. Flutter's two
    /// `ScrollDragController.end` guards compare against `carriedMomentum` the
    /// same way; `Bouncing`'s fitted power curve sits below the raw speed under
    /// ~1563 px/s and above it beyond, so gating on the mapped value changes
    /// where the threshold actually sits. Both guards are load-bearing: the
    /// carried term is comparable in magnitude to an ordinary release, so
    /// adding it to a flick back the other way cancels the finger's own
    /// velocity out or reverses it outright.
    ///
    /// **Accepted gap**: Flutter drops the carried velocity a third way, when
    /// the finger held still before letting go (`_maybeLoseMomentum`); that
    /// guard is not ported. A press that stalls live motion, pauses, then
    /// releases slowly in the same direction still carries momentum here.
    ///
    /// The single carry site for both release branches — the generic
    /// ballistic driver ([`ScrollSurface::release_simulation`]) and the legacy
    /// fling — so neither can grow a rule of its own.
    fn fling_start_velocity(&self, release: f64) -> f64 {
        let mapped = self.physics.carried_momentum(self.carried_velocity);
        let continues_it = release.signum() == mapped.signum()
            && release.abs() > MOMENTUM_RETAIN_VELOCITY_THRESHOLD_FACTOR * mapped.abs();
        if continues_it {
            release + mapped
        } else {
            release
        }
    }

    /// The last painted frame time as milliseconds — the event-pass timestamp
    /// source for velocity tracking (see [`ScrollSurfaceCore::last_frame_time`]).
    pub(crate) fn event_time_ms(&self) -> f64 {
        self.last_frame_time.as_secs_f64() * 1000.0
    }

    /// End any live [`ScrollSurfaceCore::ballistic`] simulation — a release
    /// fling/spring, or a controller [`ScrollController::animate_to`] tween —
    /// telling the attached controller the animation stopped if it was one.
    ///
    /// The single tear-down path for every place user input or a command
    /// interrupts motion (`Down`, wheel, `Cancel`, a jump or animate, and the
    /// driver's own completion), **and every start goes through it too**: the
    /// `Up` release tears every motion down before it starts a simulation,
    /// settle or fling of its own, and a controller tween is only ever
    /// installed after the same tear-down — so a fling, a settle and a
    /// simulation are never live at once, and
    /// [`ScrollController::is_animating`] cannot outlive the tween it reports
    /// *while this surface stays current on the handle*. A tween whose surface
    /// is dropped, taken over, or rebound to a different handle mid-flight
    /// never reaches this — that case is reconciled on the handle's own side
    /// ([`ScrollController::bind`] and `impl Drop for ScrollBinding`), since
    /// the tween keeps running regardless of which binding, if any, is still
    /// current to report it.
    pub(crate) fn stop_ballistic(&mut self) {
        self.ballistic = None;
        if std::mem::take(&mut self.controller_animating)
            && let Some(binding) = self.controller.as_ref()
        {
            binding.set_animating(false);
        }
    }

    /// Tear every post-release motion down — the fling, the settle and (through
    /// [`ScrollSurfaceCore::stop_ballistic`]) any simulation.
    fn halt_motion(&mut self) {
        self.fling = None;
        self.settling = false;
        self.stop_ballistic();
    }

    /// Clear every per-gesture flag a `Down` arms: the drag/armed flags, the
    /// `Down`-time claim snapshot, the sticky defer decision, and a fresh,
    /// unset multi-contact veto cell (never the previous one, which a
    /// since-torn-down recognizer may still hold a clone of).
    fn reset_gesture(&mut self) {
        self.scrolling = false;
        self.down_active = false;
        self.inner_at_down = InnerScrollState::default();
        self.deferring = false;
        self.live_veto = Rc::new(Cell::new(false));
    }

    /// End a live drag because a controller command just took the position
    /// over — Flutter's `animateTo`/`jumpTo` semantics, where a programmatic
    /// command replaces the user's drag rather than running alongside it.
    /// Resets exactly the per-gesture state the `Down` arm resets, so the rest
    /// of the gesture's `Move`s take the unarmed child/hover path and its `Up`
    /// the non-scrolling forward branch — the finger can no longer write the
    /// offset and the release starts no fling or settle.
    ///
    /// Only an *established* drag ends here. A gesture armed but still inside
    /// [`TOUCH_SLOP`] is left as it is: a pre-slop finger writes no offset, and
    /// the child under it holds an uncancelled `Down` whose slop takeover (and
    /// the child `Cancel` it sends) must still happen — Flutter's `jumpTo`
    /// during a hold behaves the same way. A no-op when nothing is scrolling.
    ///
    /// Pointer capture is deliberately left alone: the drain that calls this
    /// runs in layout/paint (or a list's rebuild), which carry no
    /// [`EventCtx`], and the shell clears capture on the physical
    /// `Up`/`Cancel` whatever this flag says. The caller re-seeds
    /// [`ScrollSurfaceCore::drag_position`] from the position it applies.
    fn end_live_drag(&mut self) {
        if !self.scrolling {
            return;
        }
        self.reset_gesture();
    }

    /// Reconcile the attached [`ScrollController`] with the one a view names
    /// (the owner's `controller(..)` build/rebuild semantics), returning
    /// whether the binding changed. A rebuild with the same handle keeps the
    /// binding — and leaves it with another surface that has since taken it
    /// over, re-binding only once the handle is unattached again. Every fresh
    /// [`ScrollController::bind`] resets the handle's `is_animating` to
    /// `false`; a tween that is genuinely still running
    /// (`controller_animating`) belongs to this surface, not the handle, so it
    /// is republished onto the new binding right after binding rather than
    /// stopped.
    pub(crate) fn attach_controller(&mut self, requested: Option<&ScrollController>) -> bool {
        match (requested, self.controller.as_ref()) {
            (None, None) => false,
            (None, Some(_)) => {
                self.controller = None;
                true
            }
            (Some(handle), Some(binding)) if binding.controller().same(handle) => {
                if binding.is_current() || handle.is_attached() {
                    false
                } else {
                    let binding = handle.bind();
                    binding.set_animating(self.controller_animating);
                    self.controller = Some(binding);
                    true
                }
            }
            (Some(handle), _) => {
                let binding = handle.bind();
                binding.set_animating(self.controller_animating);
                self.controller = Some(binding);
                true
            }
        }
    }

    /// Whether the attached handle holds commands this surface has yet to
    /// drain (and this surface still holds it).
    pub(crate) fn has_pending_commands(&self) -> bool {
        self.controller
            .as_ref()
            .is_some_and(ScrollBinding::has_pending)
    }

    /// The paint-side affine [`OverscrollEffect::Stretch`] wraps the
    /// viewport's content in for the current [`ScrollSurfaceCore::edge_pull`]
    /// (see `scroll.rs`'s [`stretch_about_edge`]), or `None` under every other
    /// effect or with nothing to stretch. Paint-only, by contract: no layout
    /// pass reads the pull.
    pub(crate) fn stretch_transform(&self, origin: Point, size: Size) -> Option<Affine> {
        match self.effect {
            OverscrollEffect::Stretch => stretch_about_edge(origin, size, self.edge_pull),
            OverscrollEffect::Translate | OverscrollEffect::None => None,
        }
    }
}

/// A scroll surface built on a [`ScrollSurfaceCore`]: the owner supplies its
/// position and geometry through the required methods, and inherits the
/// whole drag/fling/overscroll/claim/controller machinery as the provided
/// ones (see the [module docs](self)).
///
/// The **pixels** the core reasons in are the owner's raw scroll position,
/// past-edge displacement included — `0.0` is the leading edge,
/// [`ScrollSurface::max_scroll_extent`] the trailing one.
pub(crate) trait ScrollSurface {
    // --- Required: the owner's state and geometry. ---

    /// The shared core.
    fn core(&self) -> &ScrollSurfaceCore;

    /// The shared core, mutably.
    fn core_mut(&mut self) -> &mut ScrollSurfaceCore;

    /// The largest in-range position (`content − viewport`, never negative),
    /// read fresh on every call.
    fn max_scroll_extent(&self) -> f64;

    /// The viewport's extent along the scroll axis.
    fn viewport_extent(&self) -> f64;

    /// The position the hard-clamped paths — wheel and the legacy fling —
    /// advance from.
    fn base_offset(&self) -> f64;

    /// Hard-clamp the base position to `value` within `[0,
    /// max_scroll_extent]`, leaving any separately stored displacement alone.
    fn set_base_offset(&mut self, value: f64);

    /// The position the physics reasons about: the base position plus any
    /// live past-edge displacement.
    fn pixels(&self) -> f64;

    /// The signed distance the surface currently sits past an edge (negative
    /// past the leading edge, `0.0` in range) — what
    /// [`ScrollInfo::overscroll`] reports, and the allowed half of
    /// [`ScrollSurfaceCore::edge_pull`].
    fn displacement(&self) -> f64;

    /// Store `allowed` — a position the physics accepted, possibly past an
    /// edge — as the surface's position.
    fn set_allowed_position(&mut self, allowed: f64);

    /// Ease the displacement toward the edge, keeping `retained` of it.
    fn decay_displacement(&mut self, retained: f64);

    /// Drop any displacement, putting the surface exactly at its clamped edge.
    fn settle_to_edge(&mut self);

    /// Re-place the content for the current position (paint geometry only).
    fn sync_origins(&mut self);

    /// The current position as a [`ScrollInfo`] snapshot.
    fn scroll_info(&self) -> ScrollInfo;

    /// Deliver a pointer `event` to the content, returning the content's
    /// answer.
    fn forward_pointer(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult;

    /// The drag just crossed the slop and this surface takes it over: cancel
    /// the content's armed `Down` at `position` and release its capture.
    fn take_over_from_content(&mut self, ctx: &mut EventCtx, position: Point);

    // --- Hooks an owner may override (all default to doing nothing). ---

    /// User input (a primary `Down`, wheel, or `Cancel`) is taking the
    /// position: end anything owner-side that a user gesture supersedes.
    fn on_user_input(&mut self) {}

    /// Input just moved the position (a drag `Move` or a wheel step) and
    /// `on_scroll` has been told; fire any owner-side position callback.
    fn after_input_scroll(&mut self, _ctx: &mut EventCtx) {}

    /// The paint pump just advanced the position by one frame (`tween`: the
    /// motion was a controller tween); record any owner-side notification it
    /// owes.
    fn after_motion_frame(&mut self, _tween: bool) {}

    /// A controller jump just moved the position.
    fn after_jump(&mut self) {}

    /// A pointer `Cancel` just snapped the surface back into range: drop any
    /// owner-side notification owed, without firing it.
    fn on_cancel(&mut self) {}

    /// The gesture ended (`Up`/`Cancel`): clear the content's active state.
    fn release_content(&mut self) {}

    /// Hard-clamp to `target` for a controller jump, displacement and all.
    fn jump_position(&mut self, target: f64) {
        self.settle_to_edge();
        self.set_base_offset(target);
    }

    /// A controller animate is about to start (or collapse to a jump).
    fn prepare_animate(&mut self) {}

    /// The position a controller tween starts from (and the drag accumulator
    /// re-seeds to); an owner with an uncommitted correction commits it here.
    fn tween_origin(&mut self) -> f64 {
        self.base_offset()
    }

    /// Whether this surface resolves [`ScrollController::scroll_to_item`]
    /// itself. A surface that does not drops item commands at drain time with
    /// a debug-build log.
    fn handles_item_commands(&self) -> bool {
        false
    }

    /// An offset command is about to apply.
    fn before_offset_command(&mut self) {}

    /// Apply a [`ScrollController::scroll_to_item`] request (only reached
    /// when [`ScrollSurface::handles_item_commands`]).
    fn apply_item_command(&mut self, _request: ScrollToItem) {}

    // --- Provided: the shared machinery. ---

    /// This surface's extent/position snapshot for the physics, reading
    /// `pixels` from a caller-supplied position.
    fn metrics_at(&self, pixels: f64) -> ScrollMetrics {
        ScrollMetrics {
            pixels,
            min_scroll_extent: 0.0,
            max_scroll_extent: self.max_scroll_extent(),
            viewport_dimension: self.viewport_extent(),
            device_pixel_ratio: METRICS_FALLBACK_DPR,
        }
    }

    /// This surface's extent/position snapshot at its live
    /// [`ScrollSurface::pixels`].
    fn metrics(&self) -> ScrollMetrics {
        self.metrics_at(self.pixels())
    }

    /// Ask the installed physics for post-release motion at the release
    /// velocity its own bounds allow: under
    /// [`ScrollPhysics::min_fling_velocity`] the release is not a fling at all
    /// (the physics may still spring an overscrolled surface back, from rest),
    /// and over [`ScrollPhysics::max_fling_velocity`] it clamps. **Those bounds
    /// govern the generic driver only** — the legacy fling keeps its own pinned
    /// [`FLING_STOP`] threshold and no upper clamp, so a physics that returns
    /// `None` here (as [`RubberBand`](crate::RubberBand) does) cannot change a
    /// single shipped fling.
    fn release_simulation(&self) -> Option<Box<dyn Simulation>> {
        let core = self.core();
        // The offset moves opposite the finger, like every other release path.
        let released = core.fling_start_velocity(-core.tracker.velocity());
        let max = core.physics.max_fling_velocity();
        let velocity = if released.abs() < core.physics.min_fling_velocity() {
            0.0
        } else {
            released.clamp(-max, max)
        };
        core.physics
            .create_ballistic_simulation(&self.metrics(), velocity)
    }

    /// Fire `on_scroll` (if set) with the current [`ScrollInfo`] — from an
    /// event pass, after an input-driven position change.
    fn notify_scroll(&mut self, ctx: &mut EventCtx) {
        let info = self.scroll_info();
        if let Some(cb) = self.core_mut().on_scroll.as_mut() {
            cb(ctx, info);
        }
    }

    /// Deliver a notification recorded by a pass with no [`EventCtx`] on the
    /// next event — one event of latency, the controlled-component convention.
    fn deliver_pending_scroll(&mut self, ctx: &mut EventCtx) {
        if self.core().pending_scroll_notify {
            self.core_mut().pending_scroll_notify = false;
            self.notify_scroll(ctx);
        }
    }

    /// Advance the drag by `delta` px of raw finger travel in offset space
    /// (positive = the content scrolls toward its end), asking the physics what
    /// that delta is worth from where the surface actually sits —
    /// `scroll.rs`'s *Drag convention*.
    ///
    /// Three steps, in order: the physics maps the raw delta at metrics
    /// reporting the **live** position (so a depth-aware curve reads a real
    /// overscroll depth); the mapped delta accumulates into
    /// [`ScrollSurfaceCore::drag_position`]; and the boundary rule decides how
    /// much of that accumulated position the surface may actually hold. What
    /// it rejects never reaches the position but is still reported through
    /// [`ScrollSurfaceCore::edge_pull`], which is what lets a clamping surface
    /// drive pull-to-refresh and a stretch effect.
    fn apply_drag_offset(&mut self, delta: f64) {
        let metrics = self.metrics();
        let core = self.core_mut();
        let mapped = core.physics.apply_physics_to_user_offset(&metrics, delta);
        core.drag_position += mapped;
        let rejected = core
            .physics
            .apply_boundary_conditions(&metrics, core.drag_position);
        let allowed = core.drag_position - rejected;
        self.set_allowed_position(allowed);
        // Read the allowed half back off the stored position rather than
        // reusing the term above, so this is the *same* number `scroll_info`
        // reports and the two can never disagree by a rounding step.
        let pull = self.displacement() + rejected;
        self.core_mut().edge_pull = pull;
    }

    /// Advance a release-settle by `dt_ms`, easing the displacement back to
    /// the clamped edge on [`SETTLE_DECAY`] and returning whether it is still
    /// animating. Pure and deterministic; the paint pump and the tests both
    /// drive it.
    fn advance_settle(&mut self, dt_ms: f64) -> bool {
        if !self.core().settling {
            return false;
        }
        let displacement = self.displacement();
        // `edge_pull`'s boundary-rejected half (always `0.0` under
        // `RubberBand`, where the pull *is* the displacement) has no position
        // to ride back, so it decays on the same curve of its own — otherwise a
        // clamping physics' stretch would snap off at release.
        let rejected = self.core().edge_pull - displacement;
        if displacement.abs() <= SETTLE_STOP_PX && rejected.abs() <= SETTLE_STOP_PX {
            self.settle_to_edge();
            let core = self.core_mut();
            core.settling = false;
            core.edge_pull = 0.0;
            self.sync_origins();
            return false;
        }
        let retained = SETTLE_DECAY.powf(dt_ms);
        self.decay_displacement(retained);
        let pull = self.displacement() + rejected * retained;
        self.core_mut().edge_pull = pull;
        self.sync_origins();
        true
    }

    /// Advance an in-flight legacy fling by `dt_ms`, returning whether it is
    /// still animating. Hard-clamped: it stops at rest (`|velocity|` below
    /// [`FLING_STOP`]) or the instant it reaches a bound. Pure and
    /// deterministic; the paint pump and the tests both drive it.
    fn advance_fling(&mut self, dt_ms: f64) -> bool {
        let Some(v) = self.core().fling else {
            return false;
        };
        self.set_base_offset(self.base_offset() + fling_displacement(v, dt_ms));
        self.sync_origins();
        let next_v = fling_decay(v, dt_ms);
        let offset = self.base_offset();
        let at_bound = offset <= 0.0 || offset >= self.max_scroll_extent();
        if next_v.abs() < FLING_STOP || at_bound {
            self.core_mut().fling = None;
            false
        } else {
            self.core_mut().fling = Some(next_v);
            true
        }
    }

    /// Whether the running simulation can no longer move anything on screen:
    /// the position it proposed sits outside the range, the physics rejected
    /// **all** of that excess (so the position is already pinned at the edge),
    /// and the curve is still travelling further out. Nothing it reports after
    /// that can reach the position, so the driver ends it rather than asking
    /// the shell for the rest of the spline's worth of frames.
    ///
    /// **Deliberately conservative.** A *partial* rejection and an inward
    /// velocity each keep the simulation running, because either can still
    /// bring the position back in range; only the fully-rejected-and-outward
    /// case is one-way for every curve in [`crate::physics::simulation`]. The
    /// comparison against the raw excess is exact rather than tolerant for the
    /// same reason. On a list, `max_scroll_extent` may be a converging
    /// estimate, so a fling stopped against an under-estimated end stays
    /// stopped — the position was already pinned there for as long as the
    /// estimate held.
    fn ballistic_is_pinned_outward(&self, proposed: f64, rejected: f64, velocity: f64) -> bool {
        let excess = proposed - proposed.clamp(0.0, self.max_scroll_extent());
        excess != 0.0 && rejected == excess && velocity * excess > 0.0
    }

    /// Hand a residual [`ScrollSurfaceCore::edge_pull`] left behind by a
    /// finished ballistic to the release-settle, so it decays on
    /// [`SETTLE_DECAY`] exactly as a drag release's pull does. A simulation
    /// that ends in range leaves nothing to settle: the guard is
    /// [`SETTLE_STOP_PX`], the distance the settle itself calls settled, so a
    /// spring's sub-pixel float residue never arms an animation that would
    /// stop on its first tick.
    fn settle_ballistic_residual(&mut self) {
        let core = self.core_mut();
        if core.edge_pull.abs() > SETTLE_STOP_PX {
            core.settling = true;
        }
    }

    /// Advance the running [`Simulation`] to frame time `now`: the position it
    /// reports, minus whatever [`ScrollPhysics::apply_boundary_conditions`]
    /// rejects of it. Subtracting the rejection is what keeps the driver honest
    /// for **any** physics — a clamping one can never paint an out-of-range
    /// position even if its simulation overshoots, while a bouncing one
    /// (rejecting nothing) is free to run past the edge and back.
    ///
    /// Clears the simulation once it reports itself done — or once it is
    /// [pinned outward](ScrollSurface::ballistic_is_pinned_outward) — and hands
    /// any pull the rejection left behind to the settle.
    fn drive_ballistic(&mut self, now: FrameTime) {
        let Some((proposed, velocity, done)) = self.core().ballistic.as_ref().map(|state| {
            let t = state.elapsed_secs(now);
            (state.sim.x(t), state.sim.dx(t), state.sim.is_done(t))
        }) else {
            return;
        };
        let rejected = self
            .core()
            .physics
            .apply_boundary_conditions(&self.metrics(), proposed);
        self.set_allowed_position(proposed - rejected);
        let pull = self.displacement() + rejected;
        self.core_mut().edge_pull = pull;
        self.sync_origins();
        if done || self.ballistic_is_pinned_outward(proposed, rejected, velocity) {
            self.core_mut().stop_ballistic();
            self.settle_ballistic_residual();
        }
    }

    /// Advance the ballistic simulation, the legacy fling, *or* the
    /// release-settle by the delta since the last paint, and signal
    /// [`PaintCtx::request_frame`] while any of them is still running (the
    /// desktop `ControlFlow::Wait` loop would otherwise idle). The first paint
    /// after a start seeds both clocks with a zero-delta frame.
    ///
    /// Because this path carries no [`EventCtx`], a frame that moved the
    /// position records a pending `on_scroll` for the next event — and, when
    /// the motion is a controller tween, owes the flush that delivers it on
    /// the very next frame (the [module docs](self)' *Programmatic
    /// notifications*), including the frame the tween lands on.
    ///
    /// The three are mutually exclusive at any one instant — a release picks
    /// one after tearing every one down through
    /// [`ScrollSurfaceCore::stop_ballistic`], and a controller command ends the
    /// drag a release would come from — though a simulation ending against an
    /// edge hands the pull it left behind to the settle for the frames after
    /// it. Under [`RubberBand`](crate::RubberBand) the simulation arm is never
    /// taken at all.
    fn pump_fling(&mut self, ctx: &mut PaintCtx) {
        if !self.core().is_moving() {
            self.core_mut().last_anim = None;
            return;
        }
        let now = ctx.frame_time();
        let core = self.core_mut();
        let dt = match core.last_anim {
            Some(t) => now.saturating_sub(t).as_secs_f64() * 1000.0,
            None => 0.0,
        };
        core.last_anim = Some(now);
        // A simulation measures time from its own start, so the first pump
        // after the release seeds it — the same zero-delta seeding frame
        // `last_anim` takes, so neither clock ever jumps on frame one.
        if let Some(state) = core.ballistic.as_mut() {
            state.start.get_or_insert(now);
        }
        if dt > 0.0 {
            // Read before advancing: the frame a tween lands on tears it down.
            let tween = core.controller_animating;
            if core.ballistic.is_some() {
                self.drive_ballistic(now);
            } else if core.fling.is_some() {
                self.advance_fling(dt);
            } else {
                self.advance_settle(dt);
            }
            let core = self.core_mut();
            core.pending_scroll_notify = true;
            if tween {
                core.scroll_notify_owed = true;
            }
            self.after_motion_frame(tween);
        }
        if self.core().is_moving() {
            ctx.request_frame();
        }
    }

    /// Apply every command queued on the attached [`ScrollController`], in
    /// recording order — once this surface has laid out (a command queued
    /// earlier waits for the first measured extent) and only while it still
    /// holds the handle. Returns whether any command was drained.
    fn drain_controller(&mut self, reduce_motion: bool) -> bool {
        if !self.core().laid_out {
            return false;
        }
        let commands: Vec<ControllerCommand> = match self.core().controller.as_ref() {
            Some(binding) if self.handles_item_commands() => binding.take_all_commands(),
            Some(binding) => binding
                .take_commands()
                .into_iter()
                .map(ControllerCommand::Offset)
                .collect(),
            None => return false,
        };
        let drained = !commands.is_empty();
        for command in commands {
            match command {
                ControllerCommand::Offset(command) => {
                    self.before_offset_command();
                    match command {
                        ScrollCommand::JumpTo(target) => self.apply_jump(target),
                        ScrollCommand::AnimateTo(target, options) => {
                            self.apply_animate_to(target, options, reduce_motion);
                        }
                    }
                }
                ControllerCommand::ScrollToItem(request) => self.apply_item_command(request),
            }
        }
        drained
    }

    /// Move straight to `target`, clamped to `[0, max_scroll_extent]`, ending
    /// any fling, release-settle or ballistic simulation (and the pull it
    /// rode) in flight. A live drag ends here
    /// ([`ScrollSurfaceCore::end_live_drag`]). `NaN` is ignored. A jump that
    /// stopped motion or changed the [`ScrollInfo`] owes `on_scroll` a
    /// notification and the flush that delivers it.
    fn apply_jump(&mut self, target: f64) {
        if target.is_nan() {
            return;
        }
        self.core_mut().end_live_drag();
        let before = self.scroll_info();
        let core = self.core_mut();
        let was_moving = core.is_flinging() || core.settling;
        core.halt_motion();
        core.edge_pull = 0.0;
        core.last_anim = None;
        self.jump_position(target);
        let landed = self.base_offset();
        self.core_mut().drag_position = landed;
        self.sync_origins();
        if was_moving || self.scroll_info() != before {
            let core = self.core_mut();
            core.pending_scroll_notify = true;
            core.scroll_notify_owed = true;
        }
        self.after_jump();
    }

    /// Start (or replace) a controller-driven tween to `target`, clamped to
    /// `[0, max_scroll_extent]`, ending any fling, release-settle or ballistic
    /// simulation in flight first — the same replace semantics a jump has.
    /// Drives the tween through a [`TweenSimulation`] in
    /// [`ScrollSurfaceCore::ballistic`], the exact field and pump
    /// ([`ScrollSurface::drive_ballistic`]) a release fling runs through, so
    /// paint cadence, `request_frame` and the installed physics' boundary
    /// handling are shared rather than duplicated. `reduce_motion` (or a
    /// non-positive/non-finite `duration_ms`) collapses this to
    /// [`ScrollSurface::apply_jump`]. A live drag ends here, so the tween is
    /// the only writer of the position until it finishes or user input
    /// interrupts it. `NaN` is ignored.
    fn apply_animate_to(&mut self, target: f64, options: AnimateTo, reduce_motion: bool) {
        if target.is_nan() {
            return;
        }
        let core = self.core_mut();
        core.end_live_drag();
        core.halt_motion();
        self.prepare_animate();
        let core = self.core_mut();
        core.edge_pull = 0.0;
        core.last_anim = None;
        if reduce_motion || !options.duration_ms.is_finite() || options.duration_ms <= 0.0 {
            self.apply_jump(target);
            return;
        }
        let origin = self.tween_origin();
        let clamped = target.clamp(0.0, self.max_scroll_extent());
        let sim =
            TweenSimulation::new(origin, clamped, options.duration_ms / 1000.0, options.curve);
        let core = self.core_mut();
        core.drag_position = origin;
        core.ballistic = Some(BallisticState {
            sim: Box::new(sim),
            start: None,
        });
        core.controller_animating = true;
        if let Some(binding) = core.controller.as_ref() {
            binding.set_animating(true);
        }
    }

    /// End a controller tween in flight where it stands, leaving user-driven
    /// motion (a fling, a settle, a live drag) alone — what a controller
    /// command that resolves to no movement still owes the commands it
    /// supersedes.
    fn supersede_controller_tween(&mut self) {
        if self.core().controller_animating {
            self.core_mut().stop_ballistic();
        }
    }

    /// Publish the current position to the attached [`ScrollController`] (a
    /// no-op before the first layout, or once another surface took it over).
    fn publish_to_controller(&self) {
        let core = self.core();
        if !core.laid_out {
            return;
        }
        if let Some(binding) = core.controller.as_ref() {
            binding.publish(self.scroll_info(), self.viewport_extent());
        }
    }

    /// The motion half of a paint pass, run before the owner paints anything:
    /// record the frame clock, apply any command recorded since layout (so a
    /// jump ends a fling before it advances), pump post-release motion,
    /// publish, raise the owed `on_scroll` flush, and ask for the frame that
    /// applies a command recorded during this paint (a listener reacting to
    /// the publish).
    fn paint_motion(&mut self, ctx: &mut PaintCtx, reduce_motion: bool) {
        self.core_mut().last_frame_time = ctx.frame_time();
        self.drain_controller(reduce_motion);
        self.pump_fling(ctx);
        self.publish_to_controller();
        let core = self.core_mut();
        if std::mem::take(&mut core.scroll_notify_owed) && core.on_scroll.is_some() {
            // Paint has no `&mut State`: owe the notification to the next
            // frame's Housekeeping flush (the `pan_zoom` precedent).
            frust_core::mark_pending_result_flush();
            ctx.request_frame();
        }
        if core.has_pending_commands() {
            ctx.request_frame();
        }
    }

    /// A wheel step: a physics-independent hard clamp (no overscroll, no
    /// physics consulted — the clamp is a property of the input device, not of
    /// the installed feel), ending any motion in flight.
    fn handle_wheel(&mut self, ctx: &mut EventCtx, delta: &ScrollDelta) -> EventResult {
        let travel = self.core().axis.wheel_delta(delta);
        self.core_mut().halt_motion();
        self.on_user_input();
        self.core_mut().edge_pull = 0.0;
        let target = self.base_offset() + travel;
        self.settle_to_edge();
        self.set_base_offset(target);
        self.sync_origins();
        self.notify_scroll(ctx);
        self.after_input_scroll(ctx);
        ctx.request_redraw();
        EventResult::Handled
    }

    /// The pointer gesture machinery: a primary `Down` arms a gesture (and
    /// runs the innermost-wins claim around its forward), a `Move` past
    /// [`TOUCH_SLOP`] takes the drag over unless a nested surface claimed it or
    /// the live veto is up, an established drag feeds
    /// [`ScrollSurface::apply_drag_offset`], and `Up` picks the release motion
    /// (firing pull-to-refresh past [`crate::scroll::REFRESH_TRIGGER_PX`]).
    /// `Cancel` snaps back into range and fires nothing.
    fn handle_pointer(
        &mut self,
        ctx: &mut EventCtx,
        event: &InputEvent,
        p: &PointerEvent,
        t_ms: f64,
    ) -> EventResult {
        match p.phase {
            PointerPhase::Down => self.pointer_down(ctx, event, p, t_ms),
            PointerPhase::Move => self.pointer_move(ctx, event, p, t_ms),
            PointerPhase::Up => self.pointer_up(ctx, event),
            PointerPhase::Cancel => self.pointer_cancel(ctx, event),
        }
    }

    /// The `Down` arm of [`ScrollSurface::handle_pointer`].
    fn pointer_down(
        &mut self,
        ctx: &mut EventCtx,
        event: &InputEvent,
        p: &PointerEvent,
        t_ms: f64,
    ) -> EventResult {
        // Only a primary press arms a drag. A secondary press is a context
        // gesture: it still reaches the content (a context-menu consumer must
        // see it), but opens no capture and can never start a scroll.
        if !presses(p) {
            return self.forward_pointer(ctx, event);
        }
        let core = self.core_mut();
        core.reset_gesture();
        core.down_active = true;
        // Remember what this press interrupted before killing it — the next
        // fling asks the physics how much of it to carry forward (`0.0` under
        // `RubberBand`, i.e. start cold).
        core.carried_velocity = core.live_velocity();
        core.halt_motion();
        self.on_user_input();
        // A `Down` deliberately leaves a mid-bounce displacement on screen
        // (the regrab continues from it), so the pull is re-seeded from that
        // displacement rather than zeroed — "reset" here means "carries
        // nothing stale from the previous gesture".
        let pull = self.displacement();
        let core = self.core_mut();
        core.edge_pull = pull;
        core.last_anim = None;
        core.down_start = p.position;
        core.last_drag = p.position;
        core.tracker.clear();
        core.tracker.record(t_ms, core.axis.along(p.position));
        ctx.capture_pointer();
        // Innermost-wins arbitration, both halves in dispatch order. First
        // report THIS surface into whatever cell is ambient — the nearest
        // *enclosing* scrollable's, if any — while that is still the cell on
        // top; only then push this surface's own cell for the forward below,
        // so a nested scrollable's write lands here and never in the
        // grandparent's.
        if let Some(host) = ambient_scroll_claim() {
            host.set(inner_claim_state(
                self.core().physics.as_ref(),
                &self.metrics(),
            ));
        }
        let claim = Rc::new(Cell::new(InnerScrollState::default()));
        let veto = Rc::clone(&self.core().live_veto);
        with_scroll_claim(&claim, || {
            with_scroll_veto(&veto, || self.forward_pointer(ctx, event))
        });
        self.core_mut().inner_at_down = claim.get();
        EventResult::Handled
    }

    /// The `Move` arm of [`ScrollSurface::handle_pointer`].
    fn pointer_move(
        &mut self,
        ctx: &mut EventCtx,
        event: &InputEvent,
        p: &PointerEvent,
        t_ms: f64,
    ) -> EventResult {
        // Without an armed `Down`, this is a hover move (the desktop shell
        // dispatches `Move` on every cursor motion): never run the
        // slop/takeover math against a stale `down_start`, just forward it.
        if !self.core().down_active {
            return self.forward_pointer(ctx, event);
        }
        let core = self.core_mut();
        let along = core.axis.along(p.position);
        core.tracker.record(t_ms, along);
        if core.scrolling {
            let delta = along - core.axis.along(core.last_drag);
            core.last_drag = p.position;
            // Hand the physics this move's raw delta (the offset moves
            // opposite the finger) and let it decide what reaches the
            // position past an edge.
            self.apply_drag_offset(-delta);
            self.sync_origins();
            self.notify_scroll(ctx);
            self.after_input_scroll(ctx);
            ctx.request_redraw();
            return EventResult::Handled;
        }
        let travel = along - core.axis.along(core.down_start);
        let armed = !core.deferring && !core.live_veto.get() && travel.abs() > TOUCH_SLOP;
        let accepts = armed && {
            let metrics = self.metrics();
            self.core().physics.should_accept_user_offset(&metrics)
        };
        if !accepts {
            self.forward_pointer(ctx, event);
            return EventResult::Handled;
        }
        if self.core().inner_at_down.defers(travel) {
            // Innermost wins: a nested scrollable registered on this gesture's
            // `Down` and can consume this direction, so take nothing over — no
            // `Cancel`, no capture handover — and keep forwarding. The
            // decision is sticky (`deferring` gates this whole branch), and the
            // inner's own slop machinery cancels its own content from here.
            self.core_mut().deferring = true;
            self.forward_pointer(ctx, event);
        } else {
            // Take the gesture over: cancel the content, stop forwarding —
            // unless the physics refuses drags outright, in which case the
            // move kept flowing to the content above. `Bouncing`/`RubberBand`
            // accept unconditionally (even content that fits rubber-bands), so
            // that gate is inert on the bouncing-family default.
            let seed = self.pixels();
            let core = self.core_mut();
            core.scrolling = true;
            core.settling = false;
            core.last_drag = p.position;
            // Seed the drag accumulator from the live position — including a
            // mid-bounce displacement, so a regrab continues from what is on
            // screen.
            core.drag_position = seed;
            self.take_over_from_content(ctx, p.position);
            ctx.request_redraw();
        }
        EventResult::Handled
    }

    /// The `Up` arm of [`ScrollSurface::handle_pointer`].
    fn pointer_up(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        if self.core().scrolling {
            let core = self.core_mut();
            // Pull-to-refresh: released past the leading trigger fires the app
            // hook (an Up, so mutating state is allowed). Measured on
            // `edge_pull`, so a clamping physics — which never lets the
            // position leave range — can still trigger it; under `RubberBand`
            // this is bit-for-bit the overscroll.
            if crossed_refresh_trigger(core.edge_pull)
                && let Some(cb) = core.on_refresh_release.as_mut()
            {
                cb(ctx);
            }
            // Whatever the release starts, it starts alone: tear every motion
            // down first, through the single tear-down path, so no fling,
            // settle or simulation (a controller tween included) survives
            // alongside the one picked below.
            core.halt_motion();
            core.last_anim = None;
            // Ask the physics for post-release motion first: one that hands
            // back a simulation owns the release outright, and one that does
            // not (`RubberBand`) falls through to the legacy settle/fling.
            if let Some(sim) = self.release_simulation() {
                self.core_mut().ballistic = Some(BallisticState { sim, start: None });
            } else if self.core().edge_pull != 0.0 {
                // Released while overscrolled: settle back to the edge, never
                // fling out of range.
                self.core_mut().settling = true;
            } else {
                // The legacy path keeps its own FLING_STOP threshold (the
                // trait's min/max fling bounds govern the generic driver only)
                // and takes carried momentum, which is `0.0` under
                // `RubberBand`.
                let core = self.core_mut();
                let finger_v = core.tracker.velocity();
                if finger_v.abs() > FLING_STOP {
                    // Offset moves opposite the finger.
                    core.fling = Some(core.fling_start_velocity(-finger_v));
                }
            }
            self.notify_scroll(ctx);
        } else {
            self.forward_pointer(ctx, event);
        }
        self.release_content();
        self.core_mut().reset_gesture();
        ctx.request_redraw();
        EventResult::Handled
    }

    /// The `Cancel` arm of [`ScrollSurface::handle_pointer`]: never mutates
    /// app state and never fires a callback — any pending notification is
    /// dropped and an overscrolled surface snaps back into range (no settle,
    /// no `on_scroll`/`on_refresh_release`), including any live simulation and
    /// the pull it was riding. A legacy fling in flight keeps running.
    fn pointer_cancel(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        self.forward_pointer(ctx, event);
        self.release_content();
        let core = self.core_mut();
        core.reset_gesture();
        core.pending_scroll_notify = false;
        core.settling = false;
        core.stop_ballistic();
        core.edge_pull = 0.0;
        self.on_cancel();
        self.on_user_input();
        self.settle_to_edge();
        self.sync_origins();
        ctx.request_redraw();
        EventResult::Handled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    use frust_core::{PointerButton, PointerEvent};
    use kurbo::Size;

    use crate::physics::rubber_band::RubberBand;

    /// A minimal owner: one clamped offset plus a separate displacement (the
    /// list's storage shape), over `content` px in a `viewport`-px window,
    /// with no content to forward to.
    struct Probe {
        core: ScrollSurfaceCore,
        offset: f64,
        overscroll: f64,
        content: f64,
        viewport: f64,
    }

    impl Probe {
        fn new(axis: ScrollAxis) -> Self {
            let mut core = ScrollSurfaceCore {
                axis,
                ..ScrollSurfaceCore::default()
            };
            core.physics = Rc::new(RubberBand::new());
            Self {
                core,
                offset: 0.0,
                overscroll: 0.0,
                content: 1000.0,
                viewport: 200.0,
            }
        }
    }

    impl ScrollSurface for Probe {
        fn core(&self) -> &ScrollSurfaceCore {
            &self.core
        }
        fn core_mut(&mut self) -> &mut ScrollSurfaceCore {
            &mut self.core
        }
        fn max_scroll_extent(&self) -> f64 {
            (self.content - self.viewport).max(0.0)
        }
        fn viewport_extent(&self) -> f64 {
            self.viewport
        }
        fn base_offset(&self) -> f64 {
            self.offset
        }
        fn set_base_offset(&mut self, value: f64) {
            self.offset = value.clamp(0.0, self.max_scroll_extent());
        }
        fn pixels(&self) -> f64 {
            self.offset + self.overscroll
        }
        fn displacement(&self) -> f64 {
            self.overscroll
        }
        fn set_allowed_position(&mut self, allowed: f64) {
            self.offset = allowed.clamp(0.0, self.max_scroll_extent());
            self.overscroll = allowed - self.offset;
        }
        fn decay_displacement(&mut self, retained: f64) {
            self.overscroll *= retained;
        }
        fn settle_to_edge(&mut self) {
            self.overscroll = 0.0;
        }
        fn sync_origins(&mut self) {}
        fn scroll_info(&self) -> ScrollInfo {
            ScrollInfo {
                offset: self.offset,
                max_offset: self.max_scroll_extent(),
                overscroll: self.overscroll,
            }
        }
        fn forward_pointer(&mut self, _ctx: &mut EventCtx, _event: &InputEvent) -> EventResult {
            EventResult::Ignored
        }
        fn take_over_from_content(&mut self, _ctx: &mut EventCtx, _position: Point) {}
    }

    fn pointer(phase: PointerPhase, at: Point) -> (InputEvent, PointerEvent) {
        let p = PointerEvent {
            phase,
            position: at,
            button: PointerButton::Primary,
        };
        (InputEvent::Pointer(p), p)
    }

    fn send(probe: &mut Probe, phase: PointerPhase, at: Point, t_ms: f64) {
        let mut unit = ();
        let sa: &mut dyn Any = &mut unit;
        let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(200.0, 200.0));
        let (event, p) = pointer(phase, at);
        probe.handle_pointer(&mut ctx, &event, &p, t_ms);
    }

    fn wheel(probe: &mut Probe, delta: ScrollDelta) {
        let mut unit = ();
        let sa: &mut dyn Any = &mut unit;
        let mut ctx = EventCtx::new(sa, Point::ZERO, Size::new(200.0, 200.0));
        probe.handle_wheel(&mut ctx, &delta);
    }

    #[test]
    fn a_vertical_core_reads_pointer_y_and_ignores_x() {
        let mut probe = Probe::new(ScrollAxis::Vertical);
        send(
            &mut probe,
            PointerPhase::Down,
            Point::new(100.0, 100.0),
            0.0,
        );
        // Sideways travel well past the slop never takes the drag over.
        send(
            &mut probe,
            PointerPhase::Move,
            Point::new(10.0, 100.0),
            16.0,
        );
        assert!(!probe.core.scrolling);
        send(&mut probe, PointerPhase::Move, Point::new(10.0, 60.0), 32.0);
        assert!(probe.core.scrolling, "vertical travel past the slop does");
        send(&mut probe, PointerPhase::Move, Point::new(10.0, 40.0), 48.0);
        assert_eq!(probe.offset, 20.0, "the finger moved up 20 px");
    }

    #[test]
    fn a_horizontal_core_reads_pointer_x_and_the_horizontal_wheel_delta() {
        let mut probe = Probe::new(ScrollAxis::Horizontal);
        send(
            &mut probe,
            PointerPhase::Down,
            Point::new(100.0, 100.0),
            0.0,
        );
        send(
            &mut probe,
            PointerPhase::Move,
            Point::new(100.0, 10.0),
            16.0,
        );
        assert!(!probe.core.scrolling, "vertical travel is off-axis");
        send(&mut probe, PointerPhase::Move, Point::new(60.0, 10.0), 32.0);
        assert!(probe.core.scrolling);
        send(&mut probe, PointerPhase::Move, Point::new(30.0, 10.0), 48.0);
        assert_eq!(probe.offset, 30.0);
        send(
            &mut probe,
            PointerPhase::Cancel,
            Point::new(30.0, 10.0),
            64.0,
        );

        wheel(&mut probe, ScrollDelta::Pixels(25.0, 400.0));
        assert_eq!(probe.offset, 55.0, "only the horizontal delta scrolls");
        wheel(&mut probe, ScrollDelta::Lines(1.0, -3.0));
        assert_eq!(probe.offset, 55.0 + WHEEL_LINE_PX);
    }

    #[test]
    fn a_tween_frame_owes_the_on_scroll_flush_and_a_fling_frame_does_not() {
        let mut probe = Probe::new(ScrollAxis::Vertical);
        probe.core.laid_out = true;
        probe.apply_animate_to(
            300.0,
            AnimateTo {
                duration_ms: 100.0,
                curve: frust_core::Curve::Linear,
            },
            false,
        );
        let at = |ms: u64| {
            PaintCtx::for_test(
                Point::ZERO,
                Size::new(200.0, 200.0),
                FrameTime::from_nanos(ms * 1_000_000),
            )
        };
        probe.pump_fling(&mut at(0));
        assert!(
            !probe.core.scroll_notify_owed,
            "the seeding frame moves nothing"
        );
        probe.pump_fling(&mut at(50));
        assert!(std::mem::take(&mut probe.core.scroll_notify_owed));
        probe.pump_fling(&mut at(200));
        assert_eq!(probe.offset, 300.0);
        assert!(probe.core.ballistic.is_none(), "the tween landed");
        assert!(
            std::mem::take(&mut probe.core.scroll_notify_owed),
            "the landing frame owes it too"
        );

        probe.core.fling = Some(2000.0);
        probe.core.last_anim = None;
        probe.pump_fling(&mut at(300));
        probe.pump_fling(&mut at(316));
        assert!(probe.core.pending_scroll_notify);
        assert!(
            !probe.core.scroll_notify_owed,
            "a fling waits for the next event"
        );
    }
}
