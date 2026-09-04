//! [`Presence`]: the state machine that keeps something alive long enough to
//! leave.
//!
//! Every overlay, toast, swap and morph in the catalog animates on the way out
//! as well as in. On the web that is free — React's `AnimatePresence` defers the
//! unmount until the exit finishes. frust has no such seam: **a view is gone the
//! frame after the rebuild that stopped returning it**, and an exit ramp on a
//! widget that no longer exists is an exit ramp nobody sees.
//!
//! The catalog's answer is the sibling `frust-shadcn` overlay hosts' answer,
//! generalised out of them: the consumer stays **mounted unconditionally** and
//! is handed an open flag instead. This driver is what that flag drives — it
//! knows which phase the thing is in, how far through, and when the exit has
//! finished so the owner may finally drop it.
//!
//! # The two mounting shapes
//!
//! * **Mount-on-open** — the owner mounts while its own flag is set. The
//!   entrance plays; the exit does not, because the widget is gone. Needs no
//!   `Presence` at all, and is the right shape when nothing leaves visibly.
//! * **Kept-mounted** — the owner mounts unconditionally and passes the flag to
//!   [`set_open`](Presence::set_open). Both ramps play. The cost is that a
//!   closed consumer still lays out; the benefit is an exit that exists.
//!
//! A rebuild that unmounts a kept-mounted consumer mid-exit simply truncates the
//! ramp. Nothing breaks — the thing just vanishes, which is exactly the
//! mount-on-open behaviour it fell back to.
//!
//! # Completion is reported one event late, on purpose
//!
//! [`advance`](Presence::advance) runs during **paint**, and a paint pass
//! carries no `EventCtx` — so a settling exit cannot call an owner's callback
//! from where it is noticed. It is recorded instead, and
//! [`take_exited`](Presence::take_exited) drains it from the consumer's next
//! event pass, which fires the owner's "you may drop me now" callback there.
//!
//! This is the framework's own controlled-component convention, the same
//! deferral `ScrollView::on_scroll` makes for the same reason, and it is why the
//! flag is a *drainable latch* rather than a level: an owner must be told once,
//! not on every subsequent pass.
//!
//! # Who owns "open"
//!
//! The owner, always. This driver never flips itself open or closed; it reports.
//! An exit that finishes leaves the driver [`PresencePhase::Absent`] and waiting
//! — if the owner never drains the latch, nothing happens except that the
//! consumer keeps laying out at zero presence, which is the same inert cost a
//! closed kept-mounted host already pays.

use std::time::Duration;

use frust::FrameTime;

use super::Ramp;

/// Where a [`Presence`] is in its lifecycle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PresencePhase {
    /// Closed and settled: nothing to paint, no frames to request. The starting
    /// phase, and where an exit ends.
    #[default]
    Absent,
    /// Open and playing its entrance.
    Entering,
    /// Open and settled: fully present, no frames to request.
    Present,
    /// Closed but still visible — the phase the whole module exists for.
    Exiting,
}

impl PresencePhase {
    /// Whether the consumer has anything to paint in this phase.
    ///
    /// True in every phase but [`Absent`](PresencePhase::Absent) — including
    /// [`Exiting`](PresencePhase::Exiting), which is the point.
    pub const fn is_visible(self) -> bool {
        !matches!(self, PresencePhase::Absent)
    }

    /// Whether a ramp is running, and so whether the stepping widget owes
    /// another frame.
    pub const fn is_animating(self) -> bool {
        matches!(self, PresencePhase::Entering | PresencePhase::Exiting)
    }
}

/// The enter/exit driver described in the [module docs](self).
///
/// Holds its own ramps and the frame time its current one started at; a
/// consumer calls [`set_open`](Self::set_open) when the owner's flag changes,
/// [`advance`](Self::advance) once per paint, and
/// [`take_exited`](Self::take_exited) once per event pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Presence {
    enter: Ramp,
    exit: Ramp,
    phase: PresencePhase,
    started: Option<FrameTime>,
    exited: bool,
}

impl Presence {
    /// A closed presence using `enter` on the way in and `exit` on the way out.
    ///
    /// Asymmetric by default because the catalog's motion is: upstream's cascade
    /// enters on a spring and leaves on a short eased ramp, and its panels enter
    /// on `SPRING_PANEL` and leave faster than they arrived.
    pub const fn new(enter: Ramp, exit: Ramp) -> Self {
        Presence {
            enter,
            exit,
            phase: PresencePhase::Absent,
            started: None,
            exited: false,
        }
    }

    /// A closed presence using the same ramp both ways.
    pub const fn symmetric(ramp: Ramp) -> Self {
        Self::new(ramp, ramp)
    }

    /// This presence with both ramps made instantaneous — the `reduce_motion`
    /// collapse.
    ///
    /// Enter and exit still *happen* (an exit still completes, and still reports
    /// itself), they simply complete on the frame they start, so an owner's
    /// unmount bookkeeping is identical with motion on or off.
    pub const fn collapsed(mut self) -> Self {
        self.enter = Ramp::eased(Duration::ZERO, frust::Curve::Linear);
        self.exit = self.enter;
        self
    }

    /// The current phase.
    pub const fn phase(&self) -> PresencePhase {
        self.phase
    }

    /// Whether the consumer must still be laid out and painted — true through
    /// the whole exit ramp, false once it has settled.
    pub const fn is_visible(&self) -> bool {
        self.phase.is_visible()
    }

    /// Whether a ramp is in flight, so the stepping widget owes another frame.
    pub const fn is_animating(&self) -> bool {
        self.phase.is_animating()
    }

    /// The ramp currently governing motion — the entrance while opening, the
    /// exit while closing, and the exit while at rest closed.
    pub const fn active_ramp(&self) -> Ramp {
        match self.phase {
            PresencePhase::Entering | PresencePhase::Present => self.enter,
            PresencePhase::Absent | PresencePhase::Exiting => self.exit,
        }
    }

    /// Apply the owner's open flag, returning whether it changed anything.
    ///
    /// Opening from any closed phase starts the entrance; closing from any open
    /// phase starts the exit. Re-applying the flag it is already following is a
    /// no-op — a rebuild passing the same value every frame must not restart the
    /// ramp.
    ///
    /// **Reversing mid-ramp restarts the opposite ramp from its own beginning**
    /// rather than resuming from where the interrupted one had reached. The
    /// value therefore jumps at the reversal — a fast open/close/open reads as
    /// three ramps, not one continuous path. That is a deliberate v1
    /// simplification, matching the framework's own retarget contract (value
    /// continuity, never velocity continuity, since nothing exposes an
    /// in-flight ramp's instantaneous velocity to seed the next one with).
    pub fn set_open(&mut self, open: bool) -> bool {
        let next = match (open, self.phase) {
            (true, PresencePhase::Absent | PresencePhase::Exiting) => PresencePhase::Entering,
            (false, PresencePhase::Entering | PresencePhase::Present) => PresencePhase::Exiting,
            _ => return false,
        };
        self.phase = next;
        self.started = None;
        true
    }

    /// Step the driver to `now`, returning the progress a consumer stages from.
    ///
    /// Called once per paint. The first call after a phase change latches its
    /// own start time, so a ramp is timed from the frame it first painted rather
    /// than from a rebuild that may have happened several frames earlier.
    ///
    /// A settling entrance becomes [`Present`](PresencePhase::Present); a
    /// settling exit becomes [`Absent`](PresencePhase::Absent) *and* raises the
    /// latch [`take_exited`](Self::take_exited) drains.
    pub fn advance(&mut self, now: FrameTime) -> f64 {
        let started = *self.started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);

        match self.phase {
            PresencePhase::Absent => 0.0,
            PresencePhase::Present => 1.0,
            PresencePhase::Entering => {
                if self.enter.is_settled(elapsed) {
                    self.phase = PresencePhase::Present;
                    return 1.0;
                }
                self.enter.progress(elapsed)
            }
            PresencePhase::Exiting => {
                if self.exit.is_settled(elapsed) {
                    self.phase = PresencePhase::Absent;
                    self.exited = true;
                    return 0.0;
                }
                1.0 - self.exit.progress(elapsed)
            }
        }
    }

    /// How present the consumer is right now, without stepping the clock —
    /// `0.0` absent, `1.0` fully present, in between while a ramp runs.
    ///
    /// The read for a second paint-time consumer of the same driver (a scrim
    /// alongside its panel); the widget that owns the driver uses
    /// [`advance`](Self::advance)'s return instead.
    pub fn presence(&self, now: FrameTime) -> f64 {
        let Some(started) = self.started else {
            return match self.phase {
                PresencePhase::Present => 1.0,
                _ => 0.0,
            };
        };
        let elapsed = now.saturating_sub(started);
        match self.phase {
            PresencePhase::Absent => 0.0,
            PresencePhase::Present => 1.0,
            PresencePhase::Entering => self.enter.progress(elapsed),
            PresencePhase::Exiting => 1.0 - self.exit.progress(elapsed),
        }
    }

    /// Drain the "the exit has finished" latch, returning whether one was
    /// pending.
    ///
    /// Called from the consumer's **event** pass — see the [module docs](self)
    /// on why completion cannot be reported from where it is noticed. Reports
    /// `true` exactly once per completed exit; the owner turns that into its
    /// unmount.
    pub fn take_exited(&mut self) -> bool {
        std::mem::replace(&mut self.exited, false)
    }

    /// Whether an exit completion is waiting to be drained, without draining it.
    pub const fn has_exited(&self) -> bool {
        self.exited
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::motion::{EASE_OUT, SPRING_PANEL};

    const ENTER_MS: u64 = 200;
    const EXIT_MS: u64 = 120;

    fn presence() -> Presence {
        Presence::new(
            Ramp::eased(Duration::from_millis(ENTER_MS), EASE_OUT),
            Ramp::eased(Duration::from_millis(EXIT_MS), EASE_OUT),
        )
    }

    fn at(ms: u64) -> FrameTime {
        FrameTime::from_nanos(ms * 1_000_000)
    }

    /// A fresh presence is closed, invisible and asks for nothing.
    #[test]
    fn a_fresh_presence_is_absent_and_idle() {
        let mut driver = presence();
        assert_eq!(driver.phase(), PresencePhase::Absent);
        assert!(!driver.is_visible());
        assert!(!driver.is_animating());
        assert_eq!(driver.advance(at(0)), 0.0);
        assert!(!driver.take_exited());
    }

    /// The whole point: after closing, the consumer stays visible for the length
    /// of the exit ramp and only then reports itself unmountable.
    #[test]
    fn the_exit_ramp_completes_before_the_unmount_is_reported() {
        let mut driver = presence();
        driver.set_open(true);
        driver.advance(at(0));
        driver.advance(at(ENTER_MS));
        assert_eq!(driver.phase(), PresencePhase::Present);

        driver.set_open(false);
        let start = ENTER_MS;
        driver.advance(at(start));
        assert_eq!(driver.phase(), PresencePhase::Exiting);

        // Mid-exit: still visible, still animating, nothing reported yet.
        let mid = driver.advance(at(start + EXIT_MS / 2));
        assert!(driver.is_visible(), "the child is still mounted mid-exit");
        assert!(driver.is_animating());
        assert!(mid > 0.0 && mid < 1.0, "and partway out: {mid}");
        assert!(!driver.take_exited(), "the unmount is not reported early");

        // The exit settles: gone, idle, reported exactly once.
        assert_eq!(driver.advance(at(start + EXIT_MS)), 0.0);
        assert_eq!(driver.phase(), PresencePhase::Absent);
        assert!(!driver.is_visible());
        assert!(!driver.is_animating());
        assert!(driver.has_exited());
        assert!(driver.take_exited(), "reported once");
        assert!(!driver.take_exited(), "and not again");
    }

    /// The entrance runs from zero to one and settles.
    #[test]
    fn the_entrance_runs_to_full_presence() {
        let mut driver = presence();
        assert!(driver.set_open(true));
        assert_eq!(driver.advance(at(1_000)), 0.0, "timed from its first paint");
        assert_eq!(driver.phase(), PresencePhase::Entering);
        assert!(driver.is_visible());

        let mid = driver.advance(at(1_000 + ENTER_MS / 2));
        assert!(mid > 0.0 && mid < 1.0, "{mid}");

        assert_eq!(driver.advance(at(1_000 + ENTER_MS)), 1.0);
        assert_eq!(driver.phase(), PresencePhase::Present);
        assert!(!driver.is_animating());
    }

    /// The ramp is timed from the frame it first painted, not from the rebuild
    /// that set the flag — a driver opened and then not painted for a while must
    /// not skip its entrance.
    #[test]
    fn a_ramp_is_timed_from_its_first_paint() {
        let mut driver = presence();
        driver.set_open(true);
        // First paint lands long after the flag flipped; progress is still zero.
        assert_eq!(driver.advance(at(10_000)), 0.0);
        assert!(driver.advance(at(10_000 + ENTER_MS / 2)) < 1.0);
        assert_eq!(driver.phase(), PresencePhase::Entering);
    }

    /// Re-applying the flag already being followed must not restart the ramp.
    #[test]
    fn re_applying_the_same_flag_is_a_no_op() {
        let mut driver = presence();
        assert!(driver.set_open(true));
        assert!(!driver.set_open(true));
        driver.advance(at(0));
        let mid = driver.advance(at(ENTER_MS / 2));
        assert!(
            !driver.set_open(true),
            "a rebuild re-passing `true` changes nothing"
        );
        assert_eq!(driver.advance(at(ENTER_MS / 2)), mid, "and does not rewind");
    }

    /// Reversing mid-ramp switches phase and restarts from the other ramp's
    /// beginning — the documented discontinuity.
    #[test]
    fn reversing_mid_ramp_starts_the_opposite_ramp_over() {
        let mut driver = presence();
        driver.set_open(true);
        driver.advance(at(0));
        driver.advance(at(ENTER_MS / 2));

        assert!(driver.set_open(false));
        assert_eq!(driver.phase(), PresencePhase::Exiting);
        assert_eq!(
            driver.advance(at(ENTER_MS / 2)),
            1.0,
            "the exit begins at full presence regardless of where the entrance was"
        );

        // And back again, from a partial exit.
        driver.advance(at(ENTER_MS / 2 + EXIT_MS / 2));
        assert!(driver.set_open(true));
        assert_eq!(driver.phase(), PresencePhase::Entering);
        assert_eq!(driver.advance(at(9_999)), 0.0);
    }

    /// A reopen mid-exit means no unmount ever happened, so nothing is reported.
    #[test]
    fn an_interrupted_exit_reports_no_unmount() {
        let mut driver = presence();
        driver.set_open(true);
        driver.advance(at(0));
        driver.advance(at(ENTER_MS));

        driver.set_open(false);
        driver.advance(at(ENTER_MS));
        driver.advance(at(ENTER_MS + EXIT_MS / 2));
        driver.set_open(true);
        driver.advance(at(ENTER_MS + EXIT_MS / 2));
        driver.advance(at(10_000));

        assert!(!driver.take_exited(), "the exit never completed");
        assert_eq!(driver.phase(), PresencePhase::Present);
    }

    /// The `reduce_motion` collapse keeps the lifecycle and drops the time: the
    /// exit still completes and still reports itself, on the frame it starts.
    #[test]
    fn a_collapsed_presence_still_completes_its_exit() {
        let mut driver = presence().collapsed();
        driver.set_open(true);
        assert_eq!(driver.advance(at(0)), 1.0);
        assert_eq!(driver.phase(), PresencePhase::Present);

        driver.set_open(false);
        assert_eq!(driver.advance(at(0)), 0.0);
        assert_eq!(driver.phase(), PresencePhase::Absent);
        assert!(driver.take_exited(), "the owner is still told to unmount");
    }

    /// The passive read agrees with the stepping read without advancing the
    /// state machine — a second consumer of the same driver sees the same value.
    #[test]
    fn the_passive_read_agrees_without_stepping() {
        let mut driver = Presence::symmetric(Ramp::spring(SPRING_PANEL));
        driver.set_open(true);
        driver.advance(at(0));

        let sampled = driver.presence(at(40));
        assert_eq!(
            driver.phase(),
            PresencePhase::Entering,
            "reading did not step it"
        );
        assert_eq!(driver.advance(at(40)), sampled);
    }

    /// A non-monotonic clock reading cannot produce a negative elapsed time, so
    /// progress never runs backwards past zero.
    #[test]
    fn a_clock_that_goes_backwards_saturates_at_the_start() {
        let mut driver = presence();
        driver.set_open(true);
        driver.advance(at(1_000));
        assert_eq!(driver.advance(at(500)), 0.0);
    }
}
