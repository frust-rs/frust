//! Integration suite proving the desktop shell's paced-wake mechanism
//! (`frust_shell_desktop::paced_wake`, doc-hidden but `pub` for exactly this
//! test) both settles cleanly once a paced (`TickClass::CosmeticLoop`) loop
//! stops requesting frames **and** always leaves the event loop's control
//! flow consistent with the pending paced wake — never parked on a stale
//! `WaitUntil` once a settle clears the field.
//!
//! The regression this suite guards against: the settle path cleared
//! `paced_wake` but left `ControlFlow` on a stale `WaitUntil(deadline)`; once
//! that deadline elapsed, winit treats `WaitUntil(past)` as a zero-timeout
//! poll and busy-spins at ~100% CPU with no redraws. The fix couples the field
//! and the control flow into one [`PacedDecision`] value; this suite models
//! the [`ControlFlowIntent`] across turns (which the original pure model never
//! carried) so the busy-spin is observable as a test failure.
//!
//! Mirrors `frust-shell-common/tests/pacing_integration.rs`'s tick-driven,
//! fake-advancing-clock style: a [`ShellModel`] threads the `paced_wake` field
//! and the modeled control flow across turns, each turn either a paint
//! (`next_paced_wake`) or an `about_to_wait` (`paced_wake_action`). No real
//! winit event loop or window is ever constructed — both functions are pure
//! and take `now` as a parameter.
//!
//! **Per-request pacing interval:** the per-request pacing interval
//! (`PaintOutcome::paced_interval`) now flows into `next_paced_wake`
//! alongside `needs_frame_paced_only`, so a 500ms caret blink schedules its
//! own ~500ms wake rather than always falling back to the theme's
//! `cosmetic_loop_rate` cap. [`ShellModel::paint_at`] threads that interval
//! through; [`ShellModel::paint`] still forwards `None` (the original shape),
//! so every pre-existing test below is an unmodified regression guard for
//! "no per-request interval latched ⇒ identical to today" (acceptance
//! criterion 2). The new tests toward the end of this file cover a 500ms-only
//! loop (criterion 1) and an interval change mid-loop re-deriving the next
//! deadline rather than parking on a stale one (criterion 3).

use std::time::{Duration, Instant};

use frust_shell_desktop::paced_wake::{
    ControlFlowIntent, PacedDecision, next_paced_wake, paced_wake_action,
};

/// A 30Hz cosmetic-loop rate, the framework default
/// (`MotionScheme::cosmetic_loop_rate`).
const HZ_30: f32 = 30.0;

/// The 30Hz interval, computed the same way `next_paced_wake` does so the
/// scheduled-deadline assertions stay exact.
fn interval_30hz() -> Duration {
    Duration::from_secs_f32(1.0 / HZ_30)
}

/// The event loop's control-flow state, modeled across turns — the concrete
/// mirror of winit's `ControlFlow` (only the two states this mechanism uses).
/// The original pure model carried only the `paced_wake` field, not this, so a
/// stale `WaitUntil` surviving a settle was invisible to it; carrying it here
/// is what makes the busy-spin observable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModeledControlFlow {
    Wait,
    WaitUntil(Instant),
}

/// A minimal model of the desktop `ShellHandler`'s paced-wake state across
/// event-loop turns: the `paced_wake` field and the current control flow.
/// Each turn applies a [`PacedDecision`] wholesale — exactly as
/// `app_handler.rs` does — so the model can never update the field without
/// also applying the control flow (the coupling the fix relies on).
struct ShellModel {
    paced_wake: Option<Instant>,
    control_flow: ModeledControlFlow,
    anim_pacing: bool,
    hz: f32,
}

impl ShellModel {
    /// A fresh loop, parked on `Wait` with nothing pending.
    fn new(anim_pacing: bool, hz: f32) -> Self {
        Self {
            paced_wake: None,
            control_flow: ModeledControlFlow::Wait,
            anim_pacing,
            hz,
        }
    }

    /// Apply a decision's three coupled effects, returning whether an
    /// immediate redraw was owed (mirrors `app_handler.rs`'s apply sites).
    fn apply(&mut self, decision: PacedDecision) -> bool {
        self.paced_wake = decision.paced_wake;
        match decision.control_flow {
            ControlFlowIntent::Wait => self.control_flow = ModeledControlFlow::Wait,
            ControlFlowIntent::WaitUntil(deadline) => {
                self.control_flow = ModeledControlFlow::WaitUntil(deadline)
            }
            // Unchanged: the paint-time paced-only case leaves the loop parked
            // on whatever it already is, for the next `about_to_wait` to park.
            ControlFlowIntent::Unchanged => {}
        }
        decision.request_redraw
    }

    /// One `RedrawRequested` paint turn.
    fn paint(&mut self, now: Instant, needs_frame: bool, needs_frame_paced_only: bool) -> bool {
        self.paint_at(now, needs_frame, needs_frame_paced_only, None)
    }

    /// One `RedrawRequested` paint turn, additionally carrying this paint's
    /// `PaintOutcome::paced_interval` — the per-request pacing interval (A2)
    /// fed straight into `next_paced_wake`, mirroring `app_handler.rs`'s call
    /// site (no persisted latch field: desktop's decision runs synchronously
    /// right after paint, so every call always sees the freshest interval).
    fn paint_at(
        &mut self,
        now: Instant,
        needs_frame: bool,
        needs_frame_paced_only: bool,
        requested_interval: Option<Duration>,
    ) -> bool {
        let decision = next_paced_wake(
            needs_frame,
            needs_frame_paced_only,
            self.anim_pacing,
            now,
            self.hz,
            requested_interval,
        );
        self.apply(decision)
    }

    /// One `about_to_wait` turn.
    fn about_to_wait(&mut self, now: Instant) -> bool {
        let decision = paced_wake_action(self.paced_wake, now);
        self.apply(decision)
    }
}

/// (a) The busy-spin regression's exact trace (steps 1–4), written as the
/// regression test: a paced loop parks on `WaitUntil`, then a state/signal
/// wake repaints with `needs_frame == false` *before* the deadline. The
/// settle must clear the field AND return the loop to `Wait` — not leave it
/// parked on the stale `WaitUntil` that busy-spins once the deadline passes.
#[test]
fn settle_while_parked_returns_control_flow_to_wait() {
    let iv = interval_30hz();
    let t0 = Instant::now();
    let mut m = ShellModel::new(true, HZ_30);

    // 1. A paced-only paint schedules a future wake (control flow untouched).
    m.paint(t0, true, true);
    assert_eq!(m.paced_wake, Some(t0 + iv));
    assert_eq!(
        m.control_flow,
        ModeledControlFlow::Wait,
        "the paint arm leaves the control flow for about_to_wait to park"
    );

    // 2. about_to_wait parks the loop on WaitUntil(deadline).
    m.about_to_wait(t0);
    assert_eq!(m.control_flow, ModeledControlFlow::WaitUntil(t0 + iv));

    // 3. BEFORE the deadline, a state/signal wake repaints with
    //    needs_frame == false. This alone must restore Wait (the Idle-arm fix).
    let t1 = t0 + Duration::from_millis(5);
    m.paint(t1, false, false);
    assert_eq!(m.paced_wake, None, "the settle clears the stale deadline");
    assert_eq!(
        m.control_flow,
        ModeledControlFlow::Wait,
        "the settle paint must itself restore ControlFlow::Wait, not leave the \
         stale WaitUntil for a later turn"
    );

    // 4. about_to_wait with nothing pending keeps the loop on Wait (the
    //    None-arm defensive fix — belt-and-suspenders).
    m.about_to_wait(t1);
    assert_eq!(m.control_flow, ModeledControlFlow::Wait);

    // Past the old deadline: no spurious redraw, no reversion to a stale
    // WaitUntil — the busy-spin the original code exhibited is gone.
    let t2 = t0 + iv + Duration::from_millis(50);
    let requested = m.about_to_wait(t2);
    assert!(!requested, "no spurious redraw after a settle");
    assert_eq!(m.control_flow, ModeledControlFlow::Wait);
}

/// (a', the None-arm in isolation) Belt-and-suspenders: even if the loop were
/// somehow left parked on a stale `WaitUntil` with `paced_wake` already
/// cleared, the next `about_to_wait` with nothing pending must return it to
/// `Wait` rather than leave a `WaitUntil(past)` that busy-spins. This exercises
/// the `None` arm's control-flow restore independently of the settle paint.
#[test]
fn about_to_wait_with_nothing_pending_restores_wait_defensively() {
    let t0 = Instant::now();
    let mut m = ShellModel::new(true, HZ_30);
    // Force the pathological state directly: field cleared, loop still parked.
    m.paced_wake = None;
    m.control_flow = ModeledControlFlow::WaitUntil(t0);

    let requested = m.about_to_wait(t0 + Duration::from_millis(10));
    assert!(!requested, "nothing pending owes no redraw");
    assert_eq!(
        m.control_flow,
        ModeledControlFlow::Wait,
        "about_to_wait with nothing pending must restore Wait"
    );
}

/// (b) The settle-on-fire path (the existing, already-correct route) still
/// ends at `Wait`: a scheduled deadline elapses, `about_to_wait` fires the
/// redraw once and reverts to `Wait`, and the follow-up settle paint keeps it
/// there.
#[test]
fn settle_on_fire_ends_at_wait() {
    let iv = interval_30hz();
    let t0 = Instant::now();
    let mut m = ShellModel::new(true, HZ_30);

    m.paint(t0, true, true); // schedule
    m.about_to_wait(t0); // park WaitUntil(t0 + iv)
    assert_eq!(m.control_flow, ModeledControlFlow::WaitUntil(t0 + iv));

    // The deadline elapses: about_to_wait fires the paced redraw and reverts.
    let fired = m.about_to_wait(t0 + iv);
    assert!(fired, "an elapsed deadline fires the paced redraw");
    assert_eq!(m.paced_wake, None);
    assert_eq!(m.control_flow, ModeledControlFlow::Wait);

    // The fired redraw repaints and the loop settles — still Wait.
    m.paint(t0 + iv, false, false);
    m.about_to_wait(t0 + iv);
    assert_eq!(m.control_flow, ModeledControlFlow::Wait);
}

/// (c) A continuously-paced loop keeps its `WaitUntil` deadline *fresh* every
/// turn — the parked deadline is always this turn's `now + interval`, never a
/// stale past value carried over.
#[test]
fn paced_loop_keeps_waituntil_fresh_every_turn() {
    let iv = interval_30hz();
    let mut m = ShellModel::new(true, HZ_30);
    let mut now = Instant::now();

    for _ in 0..10 {
        m.paint(now, true, true);
        m.about_to_wait(now);
        assert_eq!(
            m.control_flow,
            ModeledControlFlow::WaitUntil(now + iv),
            "each paced turn must park on a deadline one interval out from THIS \
             turn's now — never a stale earlier deadline"
        );
        // The loop wakes at the deadline and repaints on the next iteration.
        now += iv;
    }
}

/// (c, exhaustive) The core invariant, swept over *every* interleaving of
/// paint/wait operations up to length 5: after any `about_to_wait` turn (the
/// point at which the real loop actually parks), the modeled control flow is
/// consistent with the pending `paced_wake` — `WaitUntil(d)` iff a deadline
/// `d` is pending, `Wait` otherwise. No stale `WaitUntil` ever survives a turn
/// in any sequence, under a clock that advances enough for deadlines to elapse
/// mid-sequence.
#[test]
fn no_stale_wait_until_survives_any_interleaving() {
    // The operation alphabet: three paint shapes + an about_to_wait.
    #[derive(Clone, Copy)]
    enum Op {
        PacedPaint,      // needs_frame, paced-only
        TransitionPaint, // needs_frame, not paced-only
        SettlePaint,     // !needs_frame
        Wait,            // about_to_wait
    }
    const OPS: [Op; 4] = [
        Op::PacedPaint,
        Op::TransitionPaint,
        Op::SettlePaint,
        Op::Wait,
    ];

    let iv = interval_30hz();
    // Advance by half an interval each op, so a deadline scheduled on one op
    // elapses a couple of ops later — exercising both the park and fire arms
    // within a single swept sequence.
    let step = iv / 2;
    let base = Instant::now();

    // All 4^5 sequences of length 5.
    let len = 5u32;
    let total = 4usize.pow(len);
    for seq in 0..total {
        let mut m = ShellModel::new(true, HZ_30);
        let mut code = seq;
        for i in 0..len {
            let op = OPS[code % 4];
            code /= 4;
            let now = base + step * i;
            match op {
                Op::PacedPaint => {
                    m.paint(now, true, true);
                }
                Op::TransitionPaint => {
                    m.paint(now, true, false);
                }
                Op::SettlePaint => {
                    m.paint(now, false, false);
                }
                Op::Wait => {
                    m.about_to_wait(now);
                    // The invariant, checked only where the real loop parks.
                    match m.paced_wake {
                        None => assert_eq!(
                            m.control_flow,
                            ModeledControlFlow::Wait,
                            "seq {seq}: nothing pending must leave the loop on Wait, \
                             never a stale WaitUntil"
                        ),
                        Some(deadline) => assert_eq!(
                            m.control_flow,
                            ModeledControlFlow::WaitUntil(deadline),
                            "seq {seq}: a pending paced_wake must be reflected by a \
                             matching WaitUntil"
                        ),
                    }
                }
            }
        }
    }
}

/// (d) Fire-once semantics preserved: an elapsed deadline fires exactly once;
/// a subsequent `about_to_wait` turn with no new paint reports no redraw and
/// stays on `Wait`, never re-firing.
#[test]
fn fired_wake_does_not_refire() {
    let iv = interval_30hz();
    let t0 = Instant::now();
    let mut m = ShellModel::new(true, HZ_30);

    m.paint(t0, true, true);
    let deadline = m
        .paced_wake
        .expect("a paced-only frame schedules a deadline");
    m.about_to_wait(t0); // park

    // The deadline elapses: fire once.
    let fired = m.about_to_wait(deadline + Duration::from_millis(1));
    assert!(fired, "an elapsed deadline fires once");
    assert_eq!(m.paced_wake, None);
    assert_eq!(m.control_flow, ModeledControlFlow::Wait);

    // The next turn with no new paint does not re-fire.
    let refired = m.about_to_wait(deadline + iv);
    assert!(!refired, "a fired wake must not re-fire with no new paint");
    assert_eq!(m.control_flow, ModeledControlFlow::Wait);
}

/// The `FRUST_NO_ANIM_PACING` kill switch (`anim_pacing == false`): a
/// paced-only frame falls back to an immediate redraw with no scheduled
/// deadline, and the loop stays on `Wait` throughout — settling needs no
/// special-casing.
#[test]
fn pacing_disabled_fires_now_and_stays_on_wait() {
    let mut m = ShellModel::new(false, HZ_30);
    let mut now = Instant::now();

    for _ in 0..3 {
        let requested = m.paint(now, true, true);
        assert!(
            requested,
            "with pacing disabled a paced-only frame requests an immediate redraw"
        );
        assert_eq!(
            m.paced_wake, None,
            "pacing disabled never schedules a deadline"
        );
        assert_eq!(m.control_flow, ModeledControlFlow::Wait);
        m.about_to_wait(now);
        assert_eq!(m.control_flow, ModeledControlFlow::Wait);
        now += interval_30hz();
    }

    // A final settle changes nothing.
    m.paint(now, false, false);
    m.about_to_wait(now);
    assert_eq!(m.control_flow, ModeledControlFlow::Wait);
}

// -----------------------------------------------------------------------
// Per-request pacing interval: the desktop shell must honor
// `PaintOutcome::paced_interval` — the caret's own ~500ms ask — rather than
// always deriving the wake deadline from `cosmetic_loop_rate` alone.
// -----------------------------------------------------------------------

/// A ~2Hz caret blink — deliberately far slower than the 30Hz cosmetic-loop
/// cap, the motivating per-request interval this task wires through.
fn interval_500ms() -> Duration {
    Duration::from_millis(500)
}

/// Acceptance criterion 1: with only a 500ms paced request active, the
/// desktop loop must park ~500ms between paced redraws — not the theme's
/// ~33ms (30Hz) cap.
#[test]
fn a_500ms_paced_request_parks_500ms_between_redraws_not_the_30hz_cap() {
    let iv500 = interval_500ms();
    let iv30 = interval_30hz();
    let mut m = ShellModel::new(true, HZ_30);
    let mut now = Instant::now();

    for turn in 0..5 {
        m.paint_at(now, true, true, Some(iv500));
        assert_eq!(
            m.paced_wake,
            Some(now + iv500),
            "turn {turn}: the scheduled deadline must be the requested 500ms, \
             not the 30Hz cap"
        );
        m.about_to_wait(now);
        assert_eq!(
            m.control_flow,
            ModeledControlFlow::WaitUntil(now + iv500),
            "turn {turn}: the loop must park on the 500ms deadline"
        );
        assert!(
            iv500 > iv30 * 10,
            "sanity: 500ms is far slower than the 30Hz cap this test guards against"
        );
        // The loop wakes at the deadline and repaints on the next iteration.
        now += iv500;
    }
}

/// Acceptance criterion 2: with no per-request interval latched (`None`),
/// behavior is byte-identical to the pre-A2 `cosmetic_loop_rate`-only
/// fallback — the default `paint` helper (which forwards `None`) reproduces
/// every existing assertion in this suite unchanged; this test additionally
/// cross-checks `paint`/`paint_at(.., None)` produce the exact same decision.
#[test]
fn no_requested_interval_is_byte_identical_to_the_cosmetic_loop_rate_fallback() {
    let iv = interval_30hz();
    let t0 = Instant::now();

    let mut via_paint = ShellModel::new(true, HZ_30);
    let mut via_paint_at_none = ShellModel::new(true, HZ_30);

    let requested_via_paint = via_paint.paint(t0, true, true);
    let requested_via_paint_at = via_paint_at_none.paint_at(t0, true, true, None);

    assert_eq!(requested_via_paint, requested_via_paint_at);
    assert_eq!(via_paint.paced_wake, via_paint_at_none.paced_wake);
    assert_eq!(via_paint.paced_wake, Some(t0 + iv));
}

/// Acceptance criterion 3 (interval-change case): a paced-only loop is first
/// driven by a fast (30Hz-cap) request, then — mid-loop — the only paced
/// requester becomes the slow 500ms caret. The very next `paint` turn must
/// re-derive the deadline from the NEW interval; the following
/// `about_to_wait` must park on that fresh deadline, never on a value implied
/// by the old, now-stale 30Hz cadence (no busy-spin / no stretched wait on a
/// deadline that no longer reflects reality).
#[test]
fn interval_change_between_paced_paints_re_derives_the_next_deadline() {
    let iv30 = interval_30hz();
    let iv500 = interval_500ms();
    let mut m = ShellModel::new(true, HZ_30);
    let t0 = Instant::now();

    // Turn 1: a 30Hz shimmer paces this loop (no per-request interval).
    m.paint_at(t0, true, true, None);
    assert_eq!(m.paced_wake, Some(t0 + iv30));
    m.about_to_wait(t0);
    assert_eq!(m.control_flow, ModeledControlFlow::WaitUntil(t0 + iv30));

    // The shimmer settles and the 500ms caret becomes the sole paced
    // requester, on the very next paint (well before the stale 30Hz deadline
    // would have elapsed).
    let t1 = t0 + Duration::from_millis(5);
    m.paint_at(t1, true, true, Some(iv500));
    assert_eq!(
        m.paced_wake,
        Some(t1 + iv500),
        "the new deadline must be derived from THIS paint's 500ms request, \
         not the stale 30Hz-derived one from the previous turn"
    );
    assert_ne!(
        m.paced_wake,
        Some(t0 + iv30),
        "the old 30Hz deadline must not survive the interval change"
    );

    // about_to_wait must park on the fresh, re-derived deadline.
    m.about_to_wait(t1);
    assert_eq!(
        m.control_flow,
        ModeledControlFlow::WaitUntil(t1 + iv500),
        "the loop must park on the freshly re-derived 500ms deadline, never \
         a stale WaitUntil implied by the old cadence"
    );

    // Confirm no busy-spin: at the OLD (now long-elapsed) 30Hz deadline, the
    // loop must still be correctly parked on the NEW deadline, not fired
    // early and not treated as an elapsed stale WaitUntil.
    let stale_old_deadline = t0 + iv30;
    assert!(stale_old_deadline < t1 + iv500);
    assert_eq!(
        m.control_flow,
        ModeledControlFlow::WaitUntil(t1 + iv500),
        "the modeled control flow reflects only the fresh deadline"
    );
}
