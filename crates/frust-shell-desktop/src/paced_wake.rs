//! Pure decision logic behind the desktop shell's paced-wake mechanism
//! (`ShellHandler::paced_wake`, see `app_handler.rs`'s module docs and the
//! field's own doc comment) — extracted so the two decision points are
//! directly unit-testable without a live winit event loop. Side effects
//! (actually calling `window.request_redraw()`, calling
//! `event_loop.set_control_flow(..)`) stay in `app_handler.rs`; this module
//! only computes *what* to do next.
//!
//! # One decision value, three coupled effects
//!
//! Both decision points return a single [`PacedDecision`] carrying **all**
//! of a turn's coupled effects together: the value `ShellHandler::paced_wake`
//! must take, whether the caller still owes an immediate `request_redraw()`,
//! and the [`ControlFlowIntent`] to apply to the event loop. They are one
//! value on purpose — the bug this prevents was exactly a *split* between the
//! field and the control flow: a settle cleared `paced_wake` but left the
//! loop parked on a stale `ControlFlow::WaitUntil`, so once that deadline
//! elapsed winit treated `WaitUntil(past)` as a zero-timeout poll and
//! busy-spun at ~100% CPU with no redraws. Returning both from one value
//! makes it impossible for a call site to update the field without also
//! deciding the control flow.
//!
//! # Two decision points
//!
//! - [`next_paced_wake`] — called once per paint, right after
//!   `RenderRoot::paint` returns a `PaintOutcome`. Decides the whole
//!   [`PacedDecision`]: a genuinely paced-only loop *schedules* a future
//!   deadline (control flow left [`ControlFlowIntent::Unchanged`] — the
//!   following `about_to_wait` parks it); every other `needs_frame` case owes
//!   an immediate redraw and returns to [`ControlFlowIntent::Wait`]; and a
//!   *settle* (`needs_frame == false`) clears any stale `Some(deadline)` **and**
//!   returns the loop to `Wait`. That settle path is the fix this module
//!   exists for.
//! - [`paced_wake_action`] — called once per `about_to_wait`. Given the
//!   current `paced_wake` field value and `now`, decides whether to park on
//!   `WaitUntil(deadline)`, fire the redraw now (the deadline already
//!   elapsed, reverting to `Wait`), or — with nothing pending — defensively
//!   return the loop to `Wait` so no stale `WaitUntil` can ever survive a
//!   turn.
//!
//! Both functions take `now` as a parameter — never reading `Instant::now()`
//! internally — so a test can drive them with an explicit, controllable
//! clock (mirroring `frust-shell-common/tests/pacing_integration.rs`'s
//! tick-driven style).

use std::time::{Duration, Instant};

/// The event-loop control-flow state a paced-wake decision wants applied — a
/// winit-free mirror of the two [`winit::event_loop::ControlFlow`] states
/// this mechanism ever uses, plus an explicit [`Unchanged`](Self::Unchanged)
/// so "leave the loop parked on whatever it already is" is a *value* the
/// model returns, never an implicit fall-through a call site has to infer.
///
/// Keeping this winit-free is what lets the whole decision layer stay unit-
/// testable without an `ActiveEventLoop`; `app_handler.rs` translates each
/// variant into the matching `set_control_flow` call (or a no-op).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlFlowIntent {
    /// Park idle on `ControlFlow::Wait` — nothing paced is pending, so the
    /// loop sleeps until the next real event (input, resize, signal wake).
    Wait,
    /// Park on `ControlFlow::WaitUntil(deadline)` until the paced deadline
    /// elapses (any earlier event still wakes the loop).
    WaitUntil(Instant),
    /// Leave the current `ControlFlow` untouched — the paint-time paced-only
    /// case, where the *following* `about_to_wait` turn is what parks the loop.
    Unchanged,
}

/// The complete outcome of a paced-wake decision: the value `paced_wake`
/// must take next, whether the caller still owes an immediate
/// `request_redraw()`, and the [`ControlFlowIntent`] to apply. Returned as
/// one value from *both* decision points so a call site can never update the
/// field without also deciding the control flow — see the module docs for
/// why that coupling is load-bearing (the busy-spin bug it prevents).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacedDecision {
    /// What `ShellHandler::paced_wake` must become after this turn.
    pub paced_wake: Option<Instant>,
    /// Whether the caller still owes an immediate `window.request_redraw()`.
    pub request_redraw: bool,
    /// The control-flow state to apply to the event loop.
    pub control_flow: ControlFlowIntent,
}

/// Decide the whole [`PacedDecision`] for this paint from its outcome.
///
/// `now` is the shell's own clock reading (`Instant::now()` at the call site
/// in `app_handler.rs`) — injected so this stays a pure, directly testable
/// function; never call `Instant::now()` inside this module. `cosmetic_loop_hz`
/// is the theme's `CosmeticLoopRate::hz()` (guaranteed finite and `>= 10.0`
/// by that type's NaN-safe clamp) — the paced-loop **cap**. `requested_interval`
/// is this same paint's `PaintOutcome::paced_interval` (the MIN-aggregated
/// per-request interval, e.g. a ~500ms caret blink against a 30Hz shimmer
/// cap): `None` and `Some(Duration::ZERO)` both mean "at the theme's own
/// rate". The interval this paint actually schedules at is the **longer** of
/// the cap and the requested interval — `1.0 / cosmetic_loop_hz`
/// `.max(requested)` — the identical `max(cap, requested)` semantics as
/// [`frust_shell_common::frame_gate::FramePacing::effective_interval`], so a
/// per-request interval can only ever widen the cadence, never tighten it
/// below the theme's own ceiling. Both the cap and the fold are computed
/// **only** on the branch that actually schedules, so a settling or
/// immediate-redraw frame never runs that arithmetic.
///
/// **Overflow ceiling.** `requested_interval` always traces back to a
/// widget's `frust_core::PaintCtx::request_frame_paced_at`, which clamps to
/// `frust_core::PaintCtx::MAX_PACED_INTERVAL` (10s) before it is ever folded
/// into `PaintOutcome::paced_interval` — so the `now + interval` addition
/// below stays far below any panic-on-overflow `Instant` bound even at the
/// widest legal input. This function performs no clamp of its own; it relies
/// entirely on that upstream bound, the single entry point every paced
/// interval flows through.
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
        // return the loop to `Wait`. The busy-spin bug cleared the field here
        // but left the control flow on a stale `WaitUntil`.
        return PacedDecision {
            paced_wake: None,
            request_redraw: false,
            control_flow: ControlFlowIntent::Wait,
        };
    }
    if anim_pacing && needs_frame_paced_only {
        // Paced-only decorative loop: schedule the follow-up redraw one
        // interval out and leave the control flow untouched — the next
        // `about_to_wait` turn is what parks the loop on `WaitUntil`. The
        // theme's cap is a ceiling (a request tighter than it is clamped up),
        // while a slower per-request interval widens the cadence — see this
        // function's docs for the `max(cap, requested)` contract.
        let cap = Duration::from_secs_f32(1.0 / cosmetic_loop_hz);
        let interval = match requested_interval {
            Some(requested) => cap.max(requested),
            None => cap,
        };
        PacedDecision {
            paced_wake: Some(now + interval),
            request_redraw: false,
            control_flow: ControlFlowIntent::Unchanged,
        }
    } else {
        // A real transition (or the `FRUST_NO_ANIM_PACING` kill switch):
        // owe an immediate redraw, drop any pending paced wake, and
        // defensively return to `Wait`.
        PacedDecision {
            paced_wake: None,
            request_redraw: true,
            control_flow: ControlFlowIntent::Wait,
        }
    }
}

/// Decide the whole [`PacedDecision`] for this `about_to_wait` turn, given
/// the current `paced_wake` field value and `now`.
pub fn paced_wake_action(paced_wake: Option<Instant>, now: Instant) -> PacedDecision {
    match paced_wake {
        // Nothing pending: defensively return the loop to `Wait`. This is
        // belt-and-suspenders against a stale `WaitUntil` surviving a turn —
        // no field is cleared here, but the control flow is still decided.
        None => PacedDecision {
            paced_wake: None,
            request_redraw: false,
            control_flow: ControlFlowIntent::Wait,
        },
        // The deadline has already elapsed: fire the redraw now, clear the
        // field, and revert to `Wait`.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed base instant every test offsets from — `Instant` has no public
    /// epoch constructor, so tests only ever compare *relative* deltas off
    /// this one anchor (mirroring `frust-core::anim::FrameTime`'s
    /// difference-only contract).
    fn base() -> Instant {
        Instant::now()
    }

    /// The 30Hz default's interval, recomputed the same way `next_paced_wake`
    /// does so the scheduled-deadline assertions stay exact.
    fn interval_30hz() -> Duration {
        Duration::from_secs_f32(1.0 / 30.0)
    }

    // --- next_paced_wake ---

    #[test]
    fn settling_clears_to_idle_and_returns_to_wait() {
        // needs_frame == false must always clear the field to None AND return
        // the loop to Wait, even if a stale needs_frame_paced_only == true is
        // somehow still set — this is the exact bug the task fixes: a settle
        // must never carry a prior Scheduled/FireNow decision (or a stale
        // WaitUntil control flow) forward.
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
        // The FRUST_NO_ANIM_PACING kill switch: paced-only, but anim_pacing
        // is false — falls back to the immediate every-vsync path (redraw
        // owed, control flow back to Wait).
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
        // needs_frame_paced_only == false: a genuine Transition request
        // (spring, finite animation) is never paced.
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

    // --- next_paced_wake: per-request interval (A2) ---

    #[test]
    fn no_requested_interval_falls_back_to_the_cosmetic_loop_cap() {
        // Acceptance criterion 2: with nothing latched (None), behavior must
        // be byte-identical to the cosmetic_loop_rate-only fallback.
        let now = base();
        assert_eq!(
            next_paced_wake(true, true, true, now, 30.0, None),
            next_paced_wake(true, true, true, now, 30.0, Some(Duration::ZERO)),
            "None and Some(ZERO) both mean \"at the theme's own rate\""
        );
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
    fn a_500ms_caret_request_paces_slower_than_the_30hz_cap() {
        // Acceptance criterion 1: a 500ms paced request, alone, must schedule
        // ~500ms out rather than the theme's 30Hz (~33ms) cap.
        let now = base();
        let requested = Duration::from_millis(500);
        assert_eq!(
            next_paced_wake(true, true, true, now, 30.0, Some(requested)),
            PacedDecision {
                paced_wake: Some(now + requested),
                request_redraw: false,
                control_flow: ControlFlowIntent::Unchanged,
            }
        );
    }

    #[test]
    fn a_tighter_requested_interval_is_clamped_up_to_the_theme_cap() {
        // The theme's cosmetic_loop_rate is a CEILING, not a floor: a request
        // faster than the cap (e.g. a stray 5ms ask) is clamped up to the cap
        // — matching `FramePacing::effective_interval`'s `max(cap, requested)`.
        let now = base();
        let too_fast = Duration::from_millis(5);
        assert_eq!(
            next_paced_wake(true, true, true, now, 30.0, Some(too_fast)),
            PacedDecision {
                paced_wake: Some(now + interval_30hz()),
                request_redraw: false,
                control_flow: ControlFlowIntent::Unchanged,
            }
        );
    }

    #[test]
    fn requested_interval_is_ignored_while_pacing_is_disabled() {
        // The FRUST_NO_ANIM_PACING kill switch still fires immediately
        // regardless of any latched per-request interval.
        let now = base();
        assert_eq!(
            next_paced_wake(
                true,
                true,
                false,
                now,
                30.0,
                Some(Duration::from_millis(500))
            ),
            PacedDecision {
                paced_wake: None,
                request_redraw: true,
                control_flow: ControlFlowIntent::Wait,
            }
        );
    }

    #[test]
    fn interval_change_between_paints_re_derives_the_next_deadline() {
        // Acceptance criterion 3: an interval CHANGE between paced paints
        // (e.g. a shimmer stops and a 500ms caret becomes the sole paced
        // request) must re-derive the next deadline from THIS paint's own
        // interval, never park on a deadline computed under the old one.
        let t0 = base();
        // First paint: a 30Hz shimmer in flight (no per-request interval —
        // folds to the theme cap), schedules ~33ms out.
        let d0 = next_paced_wake(true, true, true, t0, 30.0, None);
        assert_eq!(d0.paced_wake, Some(t0 + interval_30hz()));

        // The shimmer settles; only the 500ms caret paces now. The very next
        // paint (any time — here, immediately) must schedule off the NEW
        // 500ms interval, not the stale 30Hz one from the previous decision.
        let t1 = t0 + Duration::from_millis(1);
        let d1 = next_paced_wake(true, true, true, t1, 30.0, Some(Duration::from_millis(500)));
        assert_eq!(
            d1.paced_wake,
            Some(t1 + Duration::from_millis(500)),
            "the deadline must be re-derived from this paint's own requested \
             interval, not carried over from the prior 30Hz decision"
        );
        assert_ne!(
            d1.paced_wake, d0.paced_wake,
            "the new deadline must differ from the stale 30Hz-derived one"
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
        // now >= deadline fires — the boundary is inclusive.
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
}
