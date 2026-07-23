//! Pure decision logic behind the desktop shell's paced-wake mechanism
//! (`ShellHandler::paced_wake`, see `app_handler.rs`'s module docs and the
//! field's own doc comment) — extracted so the two decision points are
//! directly unit-testable without a live winit event loop. Side effects
//! (actually calling `window.request_redraw()`, parking on
//! `ControlFlow::WaitUntil`) stay in `app_handler.rs`; this module only
//! computes *what* to do next.
//!
//! # Two decision points
//!
//! - [`next_paced_wake`] — called once per paint, right after
//!   `RenderRoot::paint` returns a `PaintOutcome`. Decides what
//!   `ShellHandler::paced_wake` becomes next: [`NextPacedWake::Scheduled`]
//!   for a genuinely paced-only loop, [`NextPacedWake::FireNow`] for every
//!   other `needs_frame` case (an immediate `request_redraw` is still owed),
//!   or [`NextPacedWake::Idle`] when nothing needs another frame. `Idle` is
//!   the fix this module exists for: a settled paced loop (`needs_frame ==
//!   false`) must clear any stale `Some(deadline)` left over from a prior
//!   paced frame, never carry it forward — the bug was that the previous
//!   inline logic only ever *assigned* `paced_wake` inside `if
//!   paint_outcome.needs_frame { .. }`, so a settle left the stale value in
//!   place and `about_to_wait` fired one spurious `request_redraw()` after
//!   the app had gone idle.
//! - [`paced_wake_action`] — called once per `about_to_wait`. Given the
//!   current `paced_wake` field value and `now`, decides whether to park
//!   ([`PacedWakeAction::Park`]), fire the redraw now
//!   ([`PacedWakeAction::Fire`], the deadline already elapsed), or do
//!   nothing ([`PacedWakeAction::None`], no paced wake pending).
//!
//! Both functions take `now` as a parameter — never reading `Instant::now()`
//! internally — so a test can drive them with an explicit, controllable
//! clock (mirroring `frust-shell-common/tests/pacing_integration.rs`'s
//! tick-driven style).

use std::time::{Duration, Instant};

/// The next state `ShellHandler::paced_wake` should take, decided once per
/// paint from the paint outcome's truth values (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextPacedWake {
    /// Schedule a delayed redraw `interval` out from `now` — the paced-only
    /// case: `needs_frame` and `needs_frame_paced_only` are both set, and
    /// pacing is enabled.
    Scheduled(Instant),
    /// Fire the redraw immediately (the caller still calls
    /// `window.request_redraw()`) and clear any pending paced wake — every
    /// `needs_frame` case that isn't the paced-only one above (a real
    /// transition, or the pacing kill switch engaged).
    FireNow,
    /// No frame is pending — clear `paced_wake` to `None` unconditionally.
    /// The settle case: a stale `Some(deadline)` from a prior paced frame
    /// must not survive into an idle app.
    Idle,
}

/// Decide the next `paced_wake` state from this paint's outcome.
///
/// `now` is the shell's own clock reading (`Instant::now()` at the call
/// site in `app_handler.rs`) — injected so this stays a pure, directly
/// testable function; never call `Instant::now()` inside this module.
pub fn next_paced_wake(
    needs_frame: bool,
    needs_frame_paced_only: bool,
    anim_pacing: bool,
    now: Instant,
    interval: Duration,
) -> NextPacedWake {
    if !needs_frame {
        return NextPacedWake::Idle;
    }
    if anim_pacing && needs_frame_paced_only {
        NextPacedWake::Scheduled(now + interval)
    } else {
        NextPacedWake::FireNow
    }
}

/// The action `about_to_wait` should take, decided from the current
/// `paced_wake` value and `now`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacedWakeAction {
    /// Nothing pending — leave the loop parked on `ControlFlow::Wait`
    /// (untouched by the caller).
    None,
    /// Park on `ControlFlow::WaitUntil(deadline)` until the deadline elapses.
    Park(Instant),
    /// The deadline has already elapsed — fire the redraw now and clear
    /// `paced_wake` (reverting to `ControlFlow::Wait`).
    Fire,
}

/// Decide what `about_to_wait` should do this turn, given the current
/// `paced_wake` field value and `now`.
pub fn paced_wake_action(paced_wake: Option<Instant>, now: Instant) -> PacedWakeAction {
    match paced_wake {
        None => PacedWakeAction::None,
        Some(deadline) if now >= deadline => PacedWakeAction::Fire,
        Some(deadline) => PacedWakeAction::Park(deadline),
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

    // --- next_paced_wake ---

    #[test]
    fn settling_clears_to_idle_regardless_of_paced_flag() {
        // needs_frame == false must always yield Idle, even if a stale
        // needs_frame_paced_only == true is somehow still set — this is the
        // exact bug the task fixes: a settle must never carry a prior
        // Scheduled/FireNow decision forward.
        let now = base();
        let interval = Duration::from_millis(33);
        assert_eq!(
            next_paced_wake(false, true, true, now, interval),
            NextPacedWake::Idle
        );
        assert_eq!(
            next_paced_wake(false, false, true, now, interval),
            NextPacedWake::Idle
        );
        assert_eq!(
            next_paced_wake(false, true, false, now, interval),
            NextPacedWake::Idle
        );
    }

    #[test]
    fn paced_only_with_pacing_enabled_schedules() {
        let now = base();
        let interval = Duration::from_millis(33);
        assert_eq!(
            next_paced_wake(true, true, true, now, interval),
            NextPacedWake::Scheduled(now + interval)
        );
    }

    #[test]
    fn paced_only_with_pacing_disabled_fires_now() {
        // The FRUST_NO_ANIM_PACING kill switch: paced-only, but anim_pacing
        // is false — falls back to the immediate every-vsync path.
        let now = base();
        let interval = Duration::from_millis(33);
        assert_eq!(
            next_paced_wake(true, true, false, now, interval),
            NextPacedWake::FireNow
        );
    }

    #[test]
    fn a_real_transition_fires_now_even_with_pacing_enabled() {
        // needs_frame_paced_only == false: a genuine Transition request
        // (spring, finite animation) is never paced.
        let now = base();
        let interval = Duration::from_millis(33);
        assert_eq!(
            next_paced_wake(true, false, true, now, interval),
            NextPacedWake::FireNow
        );
    }

    // --- paced_wake_action ---

    #[test]
    fn no_pending_wake_yields_none() {
        assert_eq!(paced_wake_action(None, base()), PacedWakeAction::None);
    }

    #[test]
    fn future_deadline_parks() {
        let now = base();
        let deadline = now + Duration::from_millis(10);
        assert_eq!(
            paced_wake_action(Some(deadline), now),
            PacedWakeAction::Park(deadline)
        );
    }

    #[test]
    fn elapsed_deadline_fires() {
        let now = base();
        let deadline = now - Duration::from_millis(1);
        assert_eq!(
            paced_wake_action(Some(deadline), now),
            PacedWakeAction::Fire
        );
    }

    #[test]
    fn exactly_at_deadline_fires() {
        // now >= deadline fires — the boundary is inclusive.
        let now = base();
        assert_eq!(paced_wake_action(Some(now), now), PacedWakeAction::Fire);
    }
}
