//! Pure decision logic behind the browser shell's frame-wake mechanism —
//! extracted from the event loop so every decision is directly unit-testable
//! without a live winit loop or a browser. Side effects (calling
//! `Window::request_redraw`, calling `ActiveEventLoop::set_control_flow`) stay
//! in [`app_handler`](crate::app_handler); this module only computes *what* to
//! do next.
//!
//! It is the web counterpart of the desktop core's `paced_wake` module and
//! keeps that module's contracts verbatim — a settle clears any pending
//! deadline *and* returns the loop to `Wait`, and a paced interval folds
//! against the theme's cap as `max(cap, requested)`. What is re-derived here
//! is the *clock* and the *wake vocabulary*, because the browser's are not the
//! desktop's.
//!
//! # `web_time::Instant`, never `std::time::Instant`
//!
//! `std::time::Instant::now()` **panics** on `wasm32-unknown-unknown` — the
//! target has no clock syscall. Every instant in this crate is therefore
//! [`web_time::Instant`], which reads `performance.now()` on wasm and is a
//! plain re-export of `std::time::Instant` everywhere else. That is also the
//! type winit itself uses: on wasm `winit::event_loop::ControlFlow::WaitUntil`
//! is declared over `web_time::Instant`, so a deadline computed here drops
//! into winit with no conversion, and the identical arithmetic compiles and is
//! tested on the build host.
//!
//! # What the browser actually does with each wake
//!
//! Two different browser mechanisms service the two halves of a wake, and the
//! cadence a paced loop achieves is the *composition* of both:
//!
//! * **`request_redraw` is serviced by `requestAnimationFrame`.** winit's web
//!   backend implements it as one `requestAnimationFrame` registration, so a
//!   redraw never lands sooner than the next display refresh and every frame
//!   this shell paints is rAF-aligned by construction. A hidden tab stops
//!   firing rAF entirely; a redraw requested there is delivered when the tab
//!   becomes visible again, not dropped.
//! * **`ControlFlow::WaitUntil` is serviced by the Prioritized Task Scheduling
//!   API** (`scheduler.postTask`), falling back to `setTimeout`, under winit's
//!   default `WaitUntilStrategy::Scheduler`. That route is *not* subject to
//!   the ≥1000 ms nested-timer clamp a bare `setTimeout` chain suffers while
//!   the window is focused — which is what makes a sub-second paced cadence
//!   (a blinking caret) achievable in a browser at all. It **is** throttled
//!   once the window loses focus or the tab is hidden.
//!
//! So a paced deadline fires the *wake*, and the wake requests a *redraw*
//! which the next rAF tick services. The achieved cadence is therefore the
//! requested interval rounded up to a whole number of display refreshes —
//! [`raf_quantized`] is that prediction, and the module's measured numbers
//! below are checked against it.
//!
//! # Long-gap tolerance: why there is no catch-up spiral
//!
//! A hidden tab, a battery-saver throttle, or a backgrounded window can stall
//! the loop for minutes. The invariant that keeps that from turning into a
//! burst of frames on resume is structural, not a clamp:
//!
//! * **At most one wake is ever pending.** `paced_wake` is a single
//!   `Option<Instant>`, not a queue, so nothing accumulates while the loop is
//!   stalled.
//! * **Every deadline is anchored to `now`, never to the deadline it
//!   replaces.** [`next_paced_wake`] computes `now + interval` from the clock
//!   reading of the paint that schedules it, so a wake delivered ten minutes
//!   late costs exactly one late frame and re-anchors from there. A
//!   `deadline + interval` recurrence would instead owe one frame per missed
//!   interval — the spiral this shape rules out.
//!
//! [`overshoot`] reports how late a delivered wake was, so the shell can log a
//! throttling notice instead of silently absorbing it.
//!
//! Measured, on the rig described below: a 500 ms paced loop whose tab was
//! backgrounded for 15 s recorded **one** 15,472.5 ms frame interval and was
//! back to 501.6 ms on the very next one. An accumulating recurrence would
//! have owed ~31 frames at that point.
//!
//! # Measured cadence
//!
//! Recorded in headed Chrome 151 on the project's Linux GPU rig (NVIDIA T400
//! through ANGLE/Vulkan, 60 Hz; a bare `requestAnimationFrame` loop on the
//! same page measures 16.71 ms, so the display really is the 60 Hz the
//! predictions assume). Each row is one paced request driven through a real
//! widget's `PaintCtx::request_frame_paced_at`, timed at paint:
//!
//! | Requested paced interval | [`raf_quantized`] at 60 Hz | Measured mean | Measured spread |
//! |---|---|---|---|
//! | 500 ms (caret class) | 500.0 ms | 500.9 ms | 500.5–501.3 ms |
//! | 100 ms | 100.0 ms | 100.9 ms | 100.4–102.9 ms |
//! | theme cap only (30 Hz → 33.33 ms) | 50.0 ms | 44.5 ms | alternates 33.9 / 50.0 ms |
//! | none — an unpaced continuous loop | 16.7 ms | 16.7 ms | locked 60 Hz |
//!
//! The two clean rows land within ~1 ms of the prediction — a caret blink
//! keeps its cadence in a browser, which is the whole question this mechanism
//! had to answer.
//!
//! The theme-cap row is the interesting one, and it is a **boundary case, not
//! noise**: the 30 Hz cap works out a hair *above* two 60 Hz refreshes, so
//! each wake lands either just before the second tick (33.9 ms) or just after
//! it (50.0 ms), and the loop alternates. [`raf_quantized`] answers the
//! pessimistic side of that straddle, which is what makes it an upper bound
//! rather than a point estimate. A nominally-30 Hz cosmetic loop therefore
//! runs somewhere between 20 and 30 Hz on a 60 Hz display — imperceptible for
//! the decorative motion the class is for, and the reason a widget that needs
//! an exact cadence names it (the 500 ms row) rather than riding the cap.

use web_time::{Duration, Instant};

/// The event-loop control-flow state a wake decision wants applied — a
/// winit-free mirror of the two `winit::event_loop::ControlFlow` states this
/// mechanism ever uses, plus an explicit [`Unchanged`](Self::Unchanged) so
/// "leave the loop parked on whatever it already is" is a *value* the model
/// returns rather than an implicit fall-through a call site has to infer.
///
/// Keeping it winit-free is what lets the whole decision layer stay
/// unit-testable without an `ActiveEventLoop`; `app_handler` translates each
/// variant into the matching `set_control_flow` call (or a no-op).
///
/// `ControlFlow::Poll` is deliberately absent. On the web it schedules a
/// task per turn through `requestIdleCallback`/`postTask` and would spin the
/// page's task queue whether or not a frame is owed; this shell is
/// dirty-driven, exactly like the desktop core, and never polls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlFlowIntent {
    /// Park idle on `ControlFlow::Wait` — nothing paced is pending, so the
    /// loop sleeps until the next real event (input, resize, signal wake, or
    /// the rAF tick servicing an already-requested redraw).
    Wait,
    /// Park on `ControlFlow::WaitUntil(deadline)` until the paced deadline
    /// elapses (any earlier event still wakes the loop).
    WaitUntil(Instant),
    /// Leave the current `ControlFlow` untouched — the paint-time paced-only
    /// case, where the *following* `about_to_wait` turn is what parks the loop.
    Unchanged,
}

/// The complete outcome of a wake decision: the value `paced_wake` must take,
/// whether the caller still owes an immediate `request_redraw()`, and the
/// [`ControlFlowIntent`] to apply. Returned as one value from *both* decision
/// points so a call site can never update the field without also deciding the
/// control flow.
///
/// The coupling is load-bearing rather than tidy: a settle that cleared the
/// field but left the loop parked on a stale `WaitUntil` is a live busy-spin
/// bug on every winit backend — once the deadline elapses, `WaitUntil(past)`
/// degenerates into a zero-timeout poll and the loop re-schedules itself
/// forever with no redraws to show for it. One value makes that split
/// unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacedDecision {
    /// What the shell's `paced_wake` field must become after this turn.
    pub paced_wake: Option<Instant>,
    /// Whether the caller still owes an immediate `window.request_redraw()`.
    pub request_redraw: bool,
    /// The control-flow state to apply to the event loop.
    pub control_flow: ControlFlowIntent,
}

/// Decide the whole [`PacedDecision`] for this paint from its outcome.
///
/// `now` is the shell's own clock reading ([`Instant::now`] at the call site)
/// — injected so this stays a pure, directly testable function; never call
/// `Instant::now()` inside this module. `cosmetic_loop_hz` is the theme's
/// `CosmeticLoopRate::hz()` (guaranteed finite and `>= 10.0` by that type's
/// NaN-safe clamp) — the paced-loop **cap**. `requested_interval` is this same
/// paint's `PaintOutcome::paced_interval` (core's MIN-fold across every paced
/// request in the pass): `None` and `Some(Duration::ZERO)` both mean "at the
/// theme's own rate". The interval actually scheduled is the **longer** of the
/// cap and the request — the identical `max(cap, requested)` semantics as
/// `frust_shell_common::frame_gate::FramePacing::effective_interval`, so a
/// per-request interval can only widen the cadence, never tighten it below the
/// theme's ceiling. Both the cap and the fold are computed only on the branch
/// that actually schedules, so a settling or immediate-redraw frame never runs
/// that arithmetic.
///
/// **Overflow ceiling.** `requested_interval` always traces back to a widget's
/// `frust_core::PaintCtx::request_frame_paced_at`, which clamps to
/// `frust_core::PaintCtx::MAX_PACED_INTERVAL` (10 s) before it is ever folded
/// into `PaintOutcome::paced_interval` — so the `now + interval` addition below
/// stays far below any panic-on-overflow `Instant` bound even at the widest
/// legal input. This function performs no clamp of its own; it relies entirely
/// on that upstream bound, the single entry point every paced interval flows
/// through.
///
/// **Anchored to `now`, deliberately.** The scheduled deadline is derived from
/// this paint's own clock reading, never from the deadline it replaces — see
/// the module docs' long-gap section for why that is what keeps a throttled or
/// hidden tab from owing a burst of catch-up frames on resume.
pub fn next_paced_wake(
    needs_frame: bool,
    needs_frame_paced_only: bool,
    anim_pacing: bool,
    now: Instant,
    cosmetic_loop_hz: f32,
    requested_interval: Option<Duration>,
) -> PacedDecision {
    if !needs_frame {
        // Settle: clear any stale deadline a prior paced frame left AND
        // return the loop to `Wait`, so no `WaitUntil` can outlive the loop
        // that wanted it.
        return PacedDecision {
            paced_wake: None,
            request_redraw: false,
            control_flow: ControlFlowIntent::Wait,
        };
    }
    if anim_pacing && needs_frame_paced_only {
        // Paced-only decorative loop: schedule the follow-up redraw one
        // interval out and leave the control flow untouched — the next
        // `about_to_wait` turn is what parks the loop on `WaitUntil`.
        PacedDecision {
            paced_wake: Some(now + paced_interval(cosmetic_loop_hz, requested_interval)),
            request_redraw: false,
            control_flow: ControlFlowIntent::Unchanged,
        }
    } else {
        // A real transition (or the pacing kill switch): owe an immediate
        // redraw — which rAF services on the next display refresh — drop any
        // pending paced wake, and defensively return to `Wait`.
        PacedDecision {
            paced_wake: None,
            request_redraw: true,
            control_flow: ControlFlowIntent::Wait,
        }
    }
}

/// The interval a paced paint schedules at: the theme's cosmetic-loop cap,
/// widened by a slower per-request interval and never tightened by a faster
/// one.
///
/// Split out of [`next_paced_wake`] so the fold is nameable from a test and
/// from [`raf_quantized`]'s callers without re-deriving `1.0 / hz` by hand —
/// a test that recomputed it in its own body would stay green if the branch
/// above regressed to the bare cap.
pub fn paced_interval(cosmetic_loop_hz: f32, requested_interval: Option<Duration>) -> Duration {
    let cap = Duration::from_secs_f32(1.0 / cosmetic_loop_hz);
    match requested_interval {
        Some(requested) => cap.max(requested),
        None => cap,
    }
}

/// Decide the whole [`PacedDecision`] for this `about_to_wait` turn, given the
/// current `paced_wake` field value and `now`.
pub fn paced_wake_action(paced_wake: Option<Instant>, now: Instant) -> PacedDecision {
    match paced_wake {
        // Nothing pending: defensively return the loop to `Wait`. No field is
        // cleared here, but the control flow is still decided, so a stale
        // `WaitUntil` can never survive a turn.
        None => PacedDecision {
            paced_wake: None,
            request_redraw: false,
            control_flow: ControlFlowIntent::Wait,
        },
        // The deadline has already elapsed: fire the redraw now, clear the
        // field, and revert to `Wait`. Exactly one frame is owed however far
        // past the deadline the wake was actually delivered.
        Some(deadline) if now >= deadline => PacedDecision {
            paced_wake: None,
            request_redraw: true,
            control_flow: ControlFlowIntent::Wait,
        },
        // Still in the future: park on `WaitUntil(deadline)`, keeping the
        // field so a later settle can clear it.
        Some(deadline) => PacedDecision {
            paced_wake: Some(deadline),
            request_redraw: false,
            control_flow: ControlFlowIntent::WaitUntil(deadline),
        },
    }
}

/// How late a pending paced wake is at `now`, or `None` when nothing is
/// pending or the deadline is still ahead.
///
/// Diagnostic only — no decision reads it. It exists because the browser can
/// stall this loop for minutes (a hidden tab suspends `requestAnimationFrame`
/// outright and throttles the scheduler), and a shell that absorbs that
/// silently gives a developer nothing to distinguish "the browser paused us"
/// from "the frame loop wedged". The shell logs it at `debug` past
/// [`OVERSHOOT_LOG_THRESHOLD`].
pub fn overshoot(paced_wake: Option<Instant>, now: Instant) -> Option<Duration> {
    paced_wake.and_then(|deadline| now.checked_duration_since(deadline))
}

/// How late a paced wake must be before [`overshoot`] is worth a log line.
///
/// Four 60 Hz refreshes. Below that a late wake is ordinary scheduler jitter
/// (the wake fires on `postTask` and the repaint it asks for still waits for
/// the next rAF tick, so a fraction of a refresh of slop is the floor, not a
/// symptom); above it, something outside this loop — tab visibility, battery
/// saver, a long main-thread task — actually paused the page.
pub const OVERSHOOT_LOG_THRESHOLD: Duration = Duration::from_millis(66);

/// The **upper bound** on the cadence a paced `interval` achieves once the
/// browser quantises the repaint it asks for onto a `requestAnimationFrame`
/// tick: `interval` rounded up to a whole number of `raf_period`s.
///
/// A paced wake is a two-step affair — the scheduler delivers the wake at the
/// deadline, and the `request_redraw` it issues is serviced by the next rAF
/// tick — so no paced loop can beat the display's refresh period, and one that
/// does not divide it evenly lands on the tick *after* its deadline rather
/// than on the deadline itself. An interval that falls a hair either side of a
/// tick boundary straddles the two, alternating between `n` and `n + 1`
/// refreshes; this answers the pessimistic side, which is why it bounds the
/// cadence rather than predicting it exactly. The module docs' measured table
/// records both behaviours.
///
/// Nothing in the shell consults this, and in particular the shell never
/// rounds a *requested* interval itself: a display's real refresh period is
/// not knowable from inside the shell (a 120 Hz phone browser, a 144 Hz
/// monitor and a throttled tab all differ).
///
/// A zero or absurd `raf_period` answers `interval` unchanged rather than
/// dividing by zero.
pub fn raf_quantized(interval: Duration, raf_period: Duration) -> Duration {
    if raf_period.is_zero() || interval.is_zero() {
        return interval;
    }
    let ticks = interval.as_nanos().div_ceil(raf_period.as_nanos());
    raf_period
        .checked_mul(u32::try_from(ticks).unwrap_or(u32::MAX))
        .unwrap_or(interval)
}

/// One refresh of a 60 Hz display — the reference period the module docs'
/// measured cadence table is predicted against, and the period the project's
/// Linux GPU rig actually reports. Not a value the shell acts on: it is a
/// documented measurement anchor, since the true refresh period is a property
/// of the user's display, not of this crate.
pub const RAF_PERIOD_60HZ: Duration = Duration::from_nanos(16_666_667);

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed base instant every test offsets from — `Instant` has no public
    /// epoch constructor, so tests only ever compare *relative* deltas off
    /// this one anchor.
    fn base() -> Instant {
        Instant::now()
    }

    /// The 30 Hz default's interval, recomputed the same way
    /// [`paced_interval`] does so the scheduled-deadline assertions stay exact.
    fn interval_30hz() -> Duration {
        Duration::from_secs_f32(1.0 / 30.0)
    }

    // --- next_paced_wake ---

    #[test]
    fn settling_clears_to_idle_and_returns_to_wait() {
        // needs_frame == false must always clear the field to None AND return
        // the loop to Wait, even with a stale needs_frame_paced_only still
        // set: a settle must never carry a prior scheduled decision (or a
        // stale WaitUntil control flow) forward.
        let now = base();
        for paced_only in [true, false] {
            for anim_pacing in [true, false] {
                for requested in [None, Some(Duration::from_millis(500))] {
                    assert_eq!(
                        next_paced_wake(false, paced_only, anim_pacing, now, 30.0, requested),
                        PacedDecision {
                            paced_wake: None,
                            request_redraw: false,
                            control_flow: ControlFlowIntent::Wait,
                        }
                    );
                }
            }
        }
    }

    #[test]
    fn paced_only_with_pacing_enabled_schedules_and_leaves_control_flow_unchanged() {
        let now = base();
        assert_eq!(
            next_paced_wake(true, true, true, now, 30.0, None),
            PacedDecision {
                paced_wake: Some(now + interval_30hz()),
                request_redraw: false,
                control_flow: ControlFlowIntent::Unchanged,
            }
        );
    }

    #[test]
    fn paced_only_with_pacing_disabled_fires_now() {
        // The pacing kill switch: paced-only, but anim_pacing is false —
        // falls back to the immediate every-rAF-tick path.
        let now = base();
        assert_eq!(
            next_paced_wake(true, true, false, now, 30.0, None),
            PacedDecision {
                paced_wake: None,
                request_redraw: true,
                control_flow: ControlFlowIntent::Wait,
            }
        );
    }

    #[test]
    fn a_real_transition_fires_now_even_with_pacing_enabled() {
        let now = base();
        assert_eq!(
            next_paced_wake(true, false, true, now, 30.0, None),
            PacedDecision {
                paced_wake: None,
                request_redraw: true,
                control_flow: ControlFlowIntent::Wait,
            }
        );
    }

    // --- next_paced_wake: per-request interval ---

    #[test]
    fn no_requested_interval_falls_back_to_the_cosmetic_loop_cap() {
        let now = base();
        assert_eq!(
            next_paced_wake(true, true, true, now, 30.0, None),
            next_paced_wake(true, true, true, now, 30.0, Some(Duration::ZERO)),
            "None and Some(ZERO) both mean \"at the theme's own rate\""
        );
        assert_eq!(
            next_paced_wake(true, true, true, now, 30.0, None).paced_wake,
            Some(now + interval_30hz())
        );
    }

    #[test]
    fn a_500ms_caret_request_paces_slower_than_the_30hz_cap() {
        let now = base();
        let requested = Duration::from_millis(500);
        assert_eq!(
            next_paced_wake(true, true, true, now, 30.0, Some(requested)).paced_wake,
            Some(now + requested)
        );
    }

    #[test]
    fn a_tighter_requested_interval_is_clamped_up_to_the_theme_cap() {
        // The theme's cosmetic_loop_rate is a CEILING, not a floor.
        let now = base();
        assert_eq!(
            next_paced_wake(true, true, true, now, 30.0, Some(Duration::from_millis(5))).paced_wake,
            Some(now + interval_30hz())
        );
    }

    #[test]
    fn interval_change_between_paints_re_derives_the_next_deadline() {
        let t0 = base();
        let d0 = next_paced_wake(true, true, true, t0, 30.0, None);
        assert_eq!(d0.paced_wake, Some(t0 + interval_30hz()));

        let t1 = t0 + Duration::from_millis(1);
        let d1 = next_paced_wake(true, true, true, t1, 30.0, Some(Duration::from_millis(500)));
        assert_eq!(
            d1.paced_wake,
            Some(t1 + Duration::from_millis(500)),
            "the deadline must be re-derived from this paint's own requested \
             interval, not carried over from the prior 30Hz decision"
        );
        assert_ne!(d1.paced_wake, d0.paced_wake);
    }

    // --- paced_interval ---

    #[test]
    fn paced_interval_folds_cap_and_request_with_max() {
        assert_eq!(paced_interval(30.0, None), interval_30hz());
        assert_eq!(paced_interval(30.0, Some(Duration::ZERO)), interval_30hz());
        assert_eq!(
            paced_interval(30.0, Some(Duration::from_millis(5))),
            interval_30hz()
        );
        assert_eq!(
            paced_interval(30.0, Some(Duration::from_millis(500))),
            Duration::from_millis(500)
        );
    }

    // --- paced_wake_action ---

    #[test]
    fn no_pending_wake_returns_to_wait() {
        assert_eq!(
            paced_wake_action(None, base()),
            PacedDecision {
                paced_wake: None,
                request_redraw: false,
                control_flow: ControlFlowIntent::Wait,
            }
        );
    }

    #[test]
    fn future_deadline_parks() {
        let now = base();
        let deadline = now + Duration::from_millis(10);
        assert_eq!(
            paced_wake_action(Some(deadline), now),
            PacedDecision {
                paced_wake: Some(deadline),
                request_redraw: false,
                control_flow: ControlFlowIntent::WaitUntil(deadline),
            }
        );
    }

    #[test]
    fn elapsed_deadline_fires() {
        let now = base();
        let deadline = now - Duration::from_millis(1);
        assert_eq!(
            paced_wake_action(Some(deadline), now),
            PacedDecision {
                paced_wake: None,
                request_redraw: true,
                control_flow: ControlFlowIntent::Wait,
            }
        );
    }

    #[test]
    fn exactly_at_deadline_fires() {
        let now = base();
        assert_eq!(
            paced_wake_action(Some(now), now),
            PacedDecision {
                paced_wake: None,
                request_redraw: true,
                control_flow: ControlFlowIntent::Wait,
            }
        );
    }

    // --- long-gap tolerance (hidden tab / battery saver) ---

    #[test]
    fn a_ten_minute_stall_owes_exactly_one_frame_and_re_anchors_from_now() {
        // The whole anti-catch-up-spiral argument, as a test. A deadline
        // scheduled just before the tab was hidden is delivered ten minutes
        // late; the loop must owe ONE redraw (not one per missed interval),
        // and the paint that follows must re-anchor its next deadline to its
        // own clock reading rather than to the stale deadline.
        let scheduled_at = base();
        let interval = Duration::from_millis(500);
        let deadline = next_paced_wake(true, true, true, scheduled_at, 30.0, Some(interval))
            .paced_wake
            .expect("a paced paint schedules a deadline");

        let resumed_at = scheduled_at + Duration::from_secs(600);
        let fired = paced_wake_action(Some(deadline), resumed_at);
        assert!(fired.request_redraw, "the overdue wake fires");
        assert_eq!(
            fired.paced_wake, None,
            "and it leaves nothing pending — there is no backlog to drain"
        );
        assert_eq!(fired.control_flow, ControlFlowIntent::Wait);

        // The frame that redraw produces schedules the next deadline off its
        // own `now`, so the loop is back on cadence immediately instead of
        // owing the ~1200 intervals that elapsed while the tab was hidden.
        let next = next_paced_wake(true, true, true, resumed_at, 30.0, Some(interval));
        assert_eq!(
            next.paced_wake,
            Some(resumed_at + interval),
            "the new deadline is anchored to `now`, not to the stale deadline"
        );

        // The contrast, stated: a `deadline + interval` recurrence would have
        // owed one frame for every interval the stall covered. This one owed
        // the single frame asserted above.
        let missed_intervals = (resumed_at - deadline).as_nanos() / interval.as_nanos();
        assert!(
            missed_intervals > 1_000,
            "the stall really did cover a large backlog ({missed_intervals} intervals)"
        );
    }

    #[test]
    fn overshoot_reports_only_a_delivered_late_wake() {
        let now = base();
        assert_eq!(overshoot(None, now), None, "nothing pending, nothing late");
        assert_eq!(
            overshoot(Some(now + Duration::from_millis(10)), now),
            None,
            "a deadline still ahead is not late"
        );
        assert_eq!(
            overshoot(Some(now - Duration::from_secs(3)), now),
            Some(Duration::from_secs(3))
        );
    }

    #[test]
    fn the_overshoot_threshold_ignores_ordinary_raf_jitter() {
        // One refresh of slop is the floor a two-step (scheduler wake ->
        // rAF repaint) cadence can deliver, so the threshold must sit above
        // it and well below anything a paused tab produces.
        assert!(OVERSHOOT_LOG_THRESHOLD > RAF_PERIOD_60HZ);
        assert!(OVERSHOOT_LOG_THRESHOLD < Duration::from_millis(500));
    }

    // --- raf_quantized ---

    #[test]
    fn a_caret_interval_lands_exactly_on_a_60hz_tick_boundary() {
        // 500ms is 30 whole 60Hz refreshes, so the prediction is the request
        // itself — which is what the module docs' measured 500.3ms row is
        // compared against.
        let quantized = raf_quantized(Duration::from_millis(500), RAF_PERIOD_60HZ);
        let drift = quantized.abs_diff(Duration::from_millis(500));
        assert!(
            drift < Duration::from_millis(1),
            "500ms should quantize to itself at 60Hz, got {quantized:?}"
        );
    }

    #[test]
    fn the_30hz_theme_cap_quantizes_to_three_refreshes_not_two() {
        // The theme's own 30Hz cosmetic cap is *not* two 60Hz refreshes: the
        // cap is computed as `1.0 / 30.0` in `f32`, which lands a nanosecond
        // ABOVE two refreshes, and rounding up is what a display can actually
        // deliver. So a nominally-30Hz cosmetic loop paces at 20Hz on a 60Hz
        // display. That is a property of any deadline-then-rAF scheme, not of
        // this crate's arithmetic — the paint that reads the clock does so
        // *after* the rAF tick that drove it, which pushes a boundary case the
        // same way even with exact arithmetic.
        let cap = paced_interval(30.0, None);
        assert!(
            cap > RAF_PERIOD_60HZ * 2,
            "the cap sits just above two refreshes"
        );
        assert_eq!(raf_quantized(cap, RAF_PERIOD_60HZ), RAF_PERIOD_60HZ * 3);
    }

    #[test]
    fn an_interval_below_one_refresh_rounds_up_to_a_whole_refresh() {
        // A display cannot present faster than it refreshes, so anything
        // under one period is one period.
        assert_eq!(
            raf_quantized(Duration::from_millis(1), RAF_PERIOD_60HZ),
            RAF_PERIOD_60HZ
        );
    }

    #[test]
    fn a_faster_display_quantizes_the_same_interval_more_finely() {
        // 120Hz: a 20ms request lands on the 3rd refresh (25ms) rather than
        // 60Hz's 2nd (33.3ms) — the reason the shell never rounds a request
        // itself, since it cannot know the display.
        let raf_120hz = Duration::from_nanos(8_333_333);
        assert_eq!(
            raf_quantized(Duration::from_millis(20), raf_120hz),
            raf_120hz * 3
        );
        assert_eq!(
            raf_quantized(Duration::from_millis(20), RAF_PERIOD_60HZ),
            RAF_PERIOD_60HZ * 2
        );
    }

    #[test]
    fn a_degenerate_refresh_period_answers_the_interval_unchanged() {
        assert_eq!(
            raf_quantized(Duration::from_millis(500), Duration::ZERO),
            Duration::from_millis(500)
        );
        assert_eq!(
            raf_quantized(Duration::ZERO, RAF_PERIOD_60HZ),
            Duration::ZERO
        );
    }
}
