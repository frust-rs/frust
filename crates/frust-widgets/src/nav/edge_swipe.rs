//! The interactive edge-swipe back gesture: its tuning constants, the
//! [`EdgeSwipe`] gesture state, and the
//! [`NavigatorWidget`] driver methods that
//! gate arming, begin the interactive pop, drive it from the pointer stream and
//! settle it.
//!
//! The decisive `Move` **steal** site itself stays in
//! [`navigator`](super::navigator)'s `event_at` router, alongside the rest of
//! the navigator's event routing.

use std::collections::HashMap;

use frust_core::{EventCtx, EventResult, InputEvent, PointerPhase, VelocityTracker};
use kurbo::Point;

use super::navigator::{ActiveTransition, NavigatorWidget};
use super::options::BackPolicy;
use super::transition::{PageTransition, TransitionDriver, make_driver, settle_driver};

// --- Edge-swipe tuning constants (see per-constant approximation notes) ------

/// Left-edge activation zone width for the interactive pop-swipe, in logical px.
///
/// **Community-approximate**: UIKit's
/// `interactivePopGestureRecognizer` edge zone is not a published constant;
/// ~20dp is the value the community-reverse-engineered reimplementations
/// converge on. A `Down` at `x <= EDGE_SWIPE_ZONE_DP` (with a poppable stack)
/// arms the gesture.
pub(super) const EDGE_SWIPE_ZONE_DP: f64 = 20.0;

/// Progress past which a *released* edge-swipe completes the pop; at or below
/// it, the pop cancels and the page springs back.
///
/// The standard halfway commit point — Flutter's `CupertinoPageRoute` uses the
/// same 0.5 threshold for its interactive back gesture.
pub(super) const EDGE_SWIPE_COMMIT_PROGRESS: f64 = 0.5;

/// Release x-velocity (logical px/s) past which an edge-swipe completes the pop
/// regardless of how far it dragged (a fast flick from near the edge still
/// pops).
///
/// **Community-approximate**: matches Flutter's Cupertino commit velocity
/// (~1300 pt/s); UIKit's exact interactive-pop fling threshold is private.
const EDGE_SWIPE_FLING_VELOCITY: f64 = 1300.0;

/// The interactive edge-swipe gesture state. Mirrors
/// [`ScrollWidget`](crate::ScrollWidget)'s arm/steal model: `armed` on a
/// left-edge `Down`, promoted to `active` (an interactive pop in flight) once a
/// decisive horizontal drag steals the gesture from the page.
pub(super) struct EdgeSwipe {
    /// A left-edge `Down` on a poppable stack armed the gesture, but the slop has
    /// not yet been crossed. Disarmed by a vertical/leftward drag or `Up`.
    pub(super) armed: bool,
    /// The gesture stole from the page and is driving a held pop transition; the
    /// navigator owns the pointer stream until release.
    pub(super) active: bool,
    /// The `Down` position the drag delta is measured from.
    pub(super) down_start: Point,
    /// Trailing-window x-velocity tracker for the release fling decision.
    pub(super) tracker: VelocityTracker,
    /// **R-B3-inner.** Whether a navigator reached by forwarding the most
    /// recent `Down` through [`route_top`](NavigatorWidget::route_top) also
    /// armed (directly or, through the same propagation, at a third nesting
    /// level). Recorded at `Down` (via [`with_swipe_claim`]/
    /// [`ambient_swipe_claim`]) and consulted at the `Move` steal site: if set,
    /// this navigator defers — an inner navigator on the same `Down` is
    /// upstream of nobody, so it is the one that gets to steal. Cleared with
    /// the rest of this state on `Up`/`Cancel` and on any structural op that
    /// disarms.
    pub(super) inner_claimed: bool,
}

impl EdgeSwipe {
    pub(super) fn new() -> Self {
        Self {
            armed: false,
            active: false,
            down_start: Point::ZERO,
            tracker: VelocityTracker::new(),
            inner_claimed: false,
        }
    }
}

impl<State: 'static> NavigatorWidget<State> {
    /// Steal the gesture from the top page into an interactive pop.
    ///
    /// Mirrors the [`NavOp::Pop`] animated-pop structure but built entirely in the
    /// event pass (no `BuildCtx`): the top page's in-flight capture is
    /// synthetically cancelled ([`cancel_top`](Self::cancel_top) — the covered
    /// widget's press machine must not fire on a later `Up`), then the page is
    /// popped into a **held** pop transition the drag drives via
    /// [`set_transition_progress`](Self::set_transition_progress). Building/tearing
    /// pods is deferred: the stashed page's teardown (or restore) happens at
    /// finalize, which runs at rebuild with a `BuildCtx` in scope.
    ///
    /// The pusher's result callback is intentionally *not* queued here — a swipe
    /// may still cancel; it fires only if the pop later completes (see
    /// [`finalize_transition`](Self::finalize_transition)).
    pub(super) fn begin_interactive_pop(&mut self, initial_progress: f64) {
        debug_assert!(self.pages.len() > 1, "steal requires a poppable stack");
        debug_assert!(
            self.transition.is_none(),
            "steal requires no active transition"
        );
        // The outgoing page's own focus link, read before `cancel_top` drops it
        // (see `top_pod_focused`).
        let outgoing_focused = self.top_pod_focused();
        self.cancel_top();
        let popped = self.pages.pop().expect("depth > 1 checked before steal");
        // Out of `self.pages`, so out of `publish_reach`'s sight — same
        // stashed-page rule `start_transition` applies (a cancelled swipe's
        // finalize restores it, followed by an explicit `publish_state`).
        popped.reach.set(false);
        let from_depth = self.pages.len() + 1;
        let spec = popped.transition;
        // The swipe animates the popped page's own preset; a page pushed without an
        // animated transition still swipes with the iOS-push geometry (a swipe is
        // inherently an iOS-style interaction). The timing only supplies the
        // fallback settle spring — the drag itself holds progress.
        let preset = if spec.is_animated() {
            spec.preset
        } else {
            PageTransition::IosPush
        };
        let (_driver, settle_spring) = make_driver(spec.timing);
        let held = initial_progress.clamp(0.0, 1.0);
        self.transition = Some(ActiveTransition {
            driver: TransitionDriver::Held { value: held },
            preset,
            is_pop: true,
            stashed: Some(popped),
            settle_spring,
            settled: false,
            interactive: true,
            restore_on_finalize: false,
            hero_leaving: HashMap::new(),
            hero_entering: HashMap::new(),
            // The swipe drives a held progress, not a theme-timed driver, so
            // there is no deferred timing to resolve at paint.
            pending_spec: None,
        });
        // Publication point: an interactive pop starts at whatever progress the
        // drag has already reached, and is flagged `interactive` until release.
        // Unlike `start_transition` this runs in the EVENT pass, so a build-time
        // observer sees the edge on the next frame's build (the paint-time reader
        // still sees it this frame).
        self.publish_transition_start(from_depth, held, true, true);
        // Gate the queued IME clear on the popped page's own focus link
        // (`top_pod_focused`). **Pod-flag-only — the sanctioned fallback**: this
        // is the one producer built entirely in the EVENT pass (see this
        // method's doc), so no `BuildCtx` exists here to AND the live chain onto.
        // `EventCtx::has_focus()` is deliberately *not* substituted: it carries
        // this navigator's own pod flag as its parent recorded it, one link — not
        // the root-down chain `BuildCtx::has_focus` composes — and reusing the
        // name for a weaker value is how the two get confused later.
        //
        // Residual, bounded: a *stale* page-pod flag (set, but under an ancestor
        // link some container already cleared) still clears here — an
        // unconditional clear, but for this navigator alone. It cannot reach an
        // unrelated subtree's session, because a stale flag on a page pod means
        // focus was previously inside THIS navigator. In practice the window is
        // narrower still: the `Down` that arms an edge swipe is itself a
        // blur-on-outside-tap for anything it does not land in, so by the time a
        // steal happens the root has usually already released the session and
        // `RenderRoot::paint`'s `focus_active` guard makes the publish inert.
        if outgoing_focused {
            self.needs_ime_clear = true;
        }
    }

    /// Settle a released interactive pop toward completion (`1.0`) or cancellation
    /// (`0.0`), springing from the held progress with initial `progress_velocity`
    /// (progress units/s). A cancel flags the transition for
    /// [restore](Self::finalize_transition) rather than teardown.
    fn settle_interactive(&mut self, complete: bool, progress_velocity: f64) {
        let Some(t) = self.transition.as_mut() else {
            return;
        };
        let from = t.driver.value();
        let target = if complete { 1.0 } else { 0.0 };
        t.driver = settle_driver(t.settle_spring, from, progress_velocity, target);
        t.settled = false;
        t.restore_on_finalize = !complete;
        // The edge-swipe's own release path, publishing exactly what the public
        // `settle_transition` seam does: a spring, not a finger, drives it now.
        self.publish_transition_progress(from, Some(false));
    }

    /// Drive an in-progress interactive edge-swipe from a pointer event:
    /// `Move` maps drag-x to held progress; `Up` settles by progress/velocity;
    /// `Cancel` (system gesture steal) cancels the pop with no state mutation.
    ///
    /// This runs *before* the mid-transition input block in
    /// [`event_at`](Self::event_at) — the swipe owns the pointer stream and must
    /// keep receiving moves/releases even though a (held) transition is present.
    pub(super) fn drive_edge_swipe(
        &mut self,
        ctx: &mut EventCtx<'_>,
        event: &InputEvent,
        t_ms: f64,
    ) -> EventResult {
        let InputEvent::Pointer(p) = event else {
            // Only pointer events drive a swipe; ignore anything else while active.
            return EventResult::Ignored;
        };
        let width = ctx.size().width.max(1.0);
        match p.phase {
            PointerPhase::Move => {
                self.edge.tracker.record(t_ms, p.position.x);
                let dx = p.position.x - self.edge.down_start.x;
                let progress = (dx / width).clamp(0.0, 1.0);
                self.set_transition_progress(progress);
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Up => {
                let finger_v = self.edge.tracker.velocity();
                let progress = self
                    .transition
                    .as_ref()
                    .map(|t| t.driver.value())
                    .unwrap_or(0.0);
                // Complete if dragged past the commit point, or flicked rightward
                // fast enough — the low-progress high-velocity case.
                let complete =
                    progress > EDGE_SWIPE_COMMIT_PROGRESS || finger_v > EDGE_SWIPE_FLING_VELOCITY;
                // The spring drives *progress*, so convert the px/s finger velocity
                // into progress/s by the drag axis length.
                self.settle_interactive(complete, finger_v / width);
                self.edge.active = false;
                self.edge.armed = false;
                self.edge.inner_claimed = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            PointerPhase::Cancel => {
                // System cancel mid-drag = cancel path: spring back and restore the
                // page. No state mutation here (the `()` Cancel tripwire contract).
                self.settle_interactive(false, 0.0);
                self.edge.active = false;
                self.edge.armed = false;
                self.edge.inner_claimed = false;
                ctx.request_redraw();
                EventResult::Handled
            }
            // A stray extra Down during a drive: swallow it (the navigator owns the
            // stream), don't re-enter arming.
            PointerPhase::Down => EventResult::Handled,
        }
    }

    /// Whether the CURRENT top page honours the edge-swipe gesture at all,
    /// independent of the geometric arm test (edge zone / slop / direction).
    ///
    /// Two gates, both page-scoped: (1) [`BackPolicy`] — a
    /// [`DismissAnimated`](BackPolicy::DismissAnimated) page (a dismiss
    /// *question*, not a normal pop) or a [`Veto`](BackPolicy::Veto) page must
    /// never be scrub-popped by the gesture, so a non-[`Pop`](BackPolicy::Pop)
    /// top refuses to arm outright — arm-refusal, not a dismiss-signal bump
    /// (see the design rationale this mirrors: a swipe *scrubs* the popped
    /// page's own transition in reverse, and a `DismissAnimated` page does not
    /// leave on back, so bumping its signal instead would drag it most of the
    /// way off-screen and snap back while a dialog appears — a lying
    /// affordance; refusing to arm instead leaves the whole pointer stream
    /// with the page, so an edge-anchored gesture inside it keeps working).
    /// (2) [`PushOptions::pop_swipe`] — this page's own override, if any, wins
    /// over the navigator's resolved default (`pop_swipe_enabled`).
    ///
    /// Consulted both at `Down` (the initial arm) and re-consulted at the
    /// `Move` steal site (a push landed between `Down` and the decisive `Move`
    /// can put a page this call no longer honours on top). `false` on an
    /// empty stack (defensive; the navigator always keeps one page).
    pub(super) fn swipe_armable(&self) -> bool {
        match self.pages.last() {
            Some(top) => {
                top.back == BackPolicy::Pop && top.pop_swipe.unwrap_or(self.pop_swipe_enabled)
            }
            None => false,
        }
    }
}
