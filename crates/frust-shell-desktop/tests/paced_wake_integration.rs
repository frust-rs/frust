//! Integration suite (bug plan `catalog-animation-performance` followup
//! phase1-fix-1, task f1) proving the desktop shell's paced-wake mechanism
//! (`frust_shell_desktop::paced_wake`, doc-hidden but `pub` for exactly this
//! test) settles cleanly once a paced (`TickClass::CosmeticLoop`) loop stops
//! requesting frames — the bug this task fixes: a stale `Some(deadline)`
//! surviving a settle and firing one spurious `request_redraw()` after the
//! app went idle.
//!
//! Mirrors `frust-shell-common/tests/pacing_integration.rs`'s tick-driven,
//! fake-advancing-clock style: each simulated tick is (1) a paint outcome fed
//! to [`next_paced_wake`] to update the shell's `paced_wake` field, then (2)
//! an `about_to_wait` turn fed to [`paced_wake_action`] to decide what the
//! event loop does. No real winit event loop or window is ever constructed —
//! both functions are pure and take `now` as a parameter.

use std::time::{Duration, Instant};

use frust_shell_desktop::paced_wake::{
    NextPacedWake, PacedWakeAction, next_paced_wake, paced_wake_action,
};

/// A 30Hz cosmetic-loop interval, the framework default
/// (`MotionScheme::cosmetic_loop_rate`).
fn interval_30hz() -> Duration {
    Duration::from_secs_f64(1.0 / 30.0)
}

/// Simulates one `ShellHandler` turn: a paint outcome updates `paced_wake`
/// (mirroring the `RedrawRequested` arm), then an `about_to_wait` turn is
/// evaluated against it, returning the `paced_wake` action taken and the
/// updated `paced_wake` value (mirroring what the real handler stores back
/// onto `self.paced_wake`).
fn tick(
    paced_wake: Option<Instant>,
    needs_frame: bool,
    needs_frame_paced_only: bool,
    anim_pacing: bool,
    now: Instant,
    interval: Duration,
) -> (Option<Instant>, PacedWakeAction) {
    // (1) RedrawRequested's paint-outcome arm.
    let after_paint = match next_paced_wake(
        needs_frame,
        needs_frame_paced_only,
        anim_pacing,
        now,
        interval,
    ) {
        NextPacedWake::Scheduled(deadline) => Some(deadline),
        NextPacedWake::FireNow => None,
        NextPacedWake::Idle => None,
    };
    let _ = paced_wake; // the paint arm always overwrites paced_wake, mirroring app_handler.rs
    // (2) about_to_wait's park/fire decision over the freshly updated value.
    let action = paced_wake_action(after_paint, now);
    let after_wait = match action {
        PacedWakeAction::Fire => None,
        PacedWakeAction::Park(deadline) => Some(deadline),
        PacedWakeAction::None => after_paint,
    };
    (after_wait, action)
}

#[test]
fn paced_loop_then_settle_leaves_paced_wake_none() {
    // A paced-only loop runs for several ticks (scheduling a future wake each
    // time), then settles (needs_frame == false). The settle tick must leave
    // paced_wake at None — not the stale Some(deadline) the bug carried
    // forward — and about_to_wait must report no pending action from then on.
    let interval = interval_30hz();
    let mut paced_wake: Option<Instant> = None;
    let mut now = Instant::now();

    for _ in 0..5 {
        let (next, _action) = tick(paced_wake, true, true, true, now, interval);
        assert!(
            next.is_some(),
            "a paced-only frame must schedule a future wake"
        );
        paced_wake = next;
        now += interval;
    }

    // Settle: the loop stops requesting frames entirely.
    let (next, action) = tick(paced_wake, false, false, true, now, interval);
    assert_eq!(
        next, None,
        "settling (needs_frame == false) must clear paced_wake, not carry the \
         prior deadline forward"
    );
    assert_eq!(
        action,
        PacedWakeAction::None,
        "no pending wake means about_to_wait takes no action"
    );

    // The idle state is stable: further about_to_wait turns with no new paint
    // keep reporting no pending action (no spurious fire).
    for _ in 0..3 {
        assert_eq!(paced_wake_action(next, now), PacedWakeAction::None);
        now += interval;
    }
}

#[test]
fn stale_paced_flag_alongside_settle_still_clears() {
    // Defensive variant of the above: even if needs_frame_paced_only were
    // somehow still true on the settling tick (needs_frame == false always
    // wins), the result must still be Idle/None — the exact regression guard
    // for the bug ("only assigned inside `if needs_frame`").
    let interval = interval_30hz();
    let now = Instant::now();
    let stale_deadline = Some(now + interval);

    let (next, action) = tick(stale_deadline, false, true, true, now, interval);
    assert_eq!(next, None);
    assert_eq!(action, PacedWakeAction::None);
}

#[test]
fn fire_once_semantics_then_wait() {
    // A scheduled deadline that has just elapsed fires exactly once (Fire),
    // clearing paced_wake — a subsequent idle turn with no new paint request
    // reports None, never a repeated Fire.
    let interval = interval_30hz();
    let now = Instant::now();

    // Schedule at t0.
    let (scheduled, _) = tick(None, true, true, true, now, interval);
    let deadline = scheduled.expect("paced-only frame schedules a deadline");

    // Advance past the deadline with no new paint (about_to_wait alone).
    let past_deadline = deadline + Duration::from_millis(1);
    let fire_action = paced_wake_action(scheduled, past_deadline);
    assert_eq!(
        fire_action,
        PacedWakeAction::Fire,
        "elapsed deadline fires once"
    );
    let after_fire = match fire_action {
        PacedWakeAction::Fire => None,
        other => panic!("expected Fire, got {other:?}"),
    };

    // The next about_to_wait turn, still with no new paint, reports None —
    // the fire is not repeated.
    assert_eq!(
        paced_wake_action(after_fire, past_deadline + interval),
        PacedWakeAction::None,
        "a fired wake must not re-fire on the next turn with no new paint"
    );
}

#[test]
fn pacing_disabled_never_schedules_but_still_settles_cleanly() {
    // FRUST_NO_ANIM_PACING (anim_pacing == false): a paced-only frame falls
    // back to FireNow (immediate redraw, no scheduled deadline) every tick;
    // settling still clears to None with no special-casing needed.
    let interval = interval_30hz();
    let mut paced_wake: Option<Instant> = None;
    let mut now = Instant::now();

    for _ in 0..3 {
        let (next, action) = tick(paced_wake, true, true, false, now, interval);
        assert_eq!(
            next, None,
            "with pacing disabled a paced-only frame never schedules a deadline"
        );
        assert_eq!(action, PacedWakeAction::None);
        paced_wake = next;
        now += interval;
    }

    let (next, action) = tick(paced_wake, false, false, false, now, interval);
    assert_eq!(next, None);
    assert_eq!(action, PacedWakeAction::None);
}
