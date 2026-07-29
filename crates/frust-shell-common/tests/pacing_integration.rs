//! Integration suite (bug plan `catalog-animation-performance`, task 09)
//! proving pacing (task 06), tick-class aggregation (task 04), and the whole-
//! frame skip gate compose without starvation or stuck frames, and that the
//! two kill switches (`FRUST_NO_FRAME_GATE`/`FRUST_NO_ANIM_PACING`) isolate
//! their own mechanism.
//!
//! Culling itself — a widget's `request_frame_class` never bubbling out of a
//! `PaintCtx` at all because `Flex` skipped painting it (task 07) — is proven
//! at the widget layer in `frust-widgets/tests/cull_pacing.rs`; this suite
//! treats "offscreen (culled)" as the resulting shell-observable fact
//! ([`Frame::loop_visible`] `false`): nothing painted, nothing bubbled.
//! [`paint_for`] instead drives a *real* [`frust_core::PaintCtx`] for
//! whatever is actually visible/active this tick, so the [`TickClass`]
//! aggregation feeding [`FrameGate`] here is genuine, not a hand-built
//! `FrameInputs`.
//!
//! Matrix dimensions covered (`research/RESEARCH.md` §C / task 09's spec):
//! the five tick sources named in the task (paced loop onscreen, paced loop
//! offscreen/culled, transition, input event, signal write) × {pacing
//! on/off (the `anim_pacing` flag, mirroring `FRUST_NO_ANIM_PACING`), gate
//! enabled/disabled (mirroring `FRUST_NO_FRAME_GATE`)} — decomposed into
//! targeted per-property tests (cadence, immediate-input, resumption-after-
//! culled, no-starvation, the transition-dominates regression guard) rather
//! than one combinatorial loop, for a readable failure per broken property.

use std::time::Duration;

use frust_core::{FrameTime, PaintCtx, PaintOutcome, TickClass};
use frust_shell_common::frame_gate::{FrameDecision, FrameGate, FrameInputs, FramePacing};
use kurbo::{Point, Size};

/// An event-pass trigger arriving this tick, independent of what paints —
/// events/signals reach the gate through their own `FrameInputs` fields, not
/// through `PaintCtx`, and force a `Run` regardless of visibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trigger {
    None,
    InputEvent,
    SignalWrite,
}

/// One simulated tick's full dirtiness picture: what a real per-frame paint
/// pass would actually walk (`loop_visible`/`transition_active`, each
/// independently present-or-not, mirroring how a real tree can hold a
/// cosmetic loop and a concurrent transition at once) plus an independent
/// event-pass `trigger`. This is deliberately decoupled from "what caused the
/// frame to run" — a frame the scroll event forces to run still paints
/// whatever is visible in that same pass, exactly like a real shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Frame {
    /// A `TickClass::CosmeticLoop` widget is onscreen (unculled) this tick —
    /// if the frame actually paints, it requests a paced continuation.
    loop_visible: bool,
    /// A `TickClass::Transition` widget (a page transition/fling/caret
    /// blink) is active this tick — if the frame actually paints, it
    /// requests an unpaced continuation, dominating a concurrent
    /// `loop_visible` per task 04's max-lattice.
    transition_active: bool,
    /// An event/signal trigger reaching the gate this tick, independent of
    /// paint.
    trigger: Trigger,
}

impl Frame {
    const IDLE: Frame = Frame {
        loop_visible: false,
        transition_active: false,
        trigger: Trigger::None,
    };

    const PACED_LOOP_ONSCREEN: Frame = Frame {
        loop_visible: true,
        ..Frame::IDLE
    };

    /// The loop scrolled offscreen: task 07's paint-time culling means it is
    /// never invoked this tick, so nothing bubbles — the fact proven
    /// directly (via a real `Flex`) in `cull_pacing.rs`. Same shape as
    /// [`Frame::IDLE`]; named separately for the matrix's readability.
    const PACED_LOOP_OFFSCREEN_CULLED: Frame = Frame::IDLE;

    const TRANSITION: Frame = Frame {
        transition_active: true,
        ..Frame::IDLE
    };

    const TRANSITION_AND_PACED_LOOP: Frame = Frame {
        loop_visible: true,
        transition_active: true,
        ..Frame::IDLE
    };

    const INPUT_EVENT: Frame = Frame {
        trigger: Trigger::InputEvent,
        ..Frame::IDLE
    };

    const SIGNAL_WRITE: Frame = Frame {
        trigger: Trigger::SignalWrite,
        ..Frame::IDLE
    };
}

/// Run one real `PaintCtx` paint pass for whatever `frame` says is
/// visible/active, returning the `PaintOutcome` a shell would latch for the
/// *next* tick's `FrameInputs` — mirrors `frust_core::app::RenderRoot::paint`'s
/// construction of `PaintOutcome` from the very same `PaintCtx` accessors.
fn paint_for(frame: Frame) -> PaintOutcome {
    let mut ctx = PaintCtx::new(Point::ZERO, Size::new(100.0, 100.0));
    if frame.loop_visible {
        ctx.request_frame_class(TickClass::CosmeticLoop);
    }
    if frame.transition_active {
        ctx.request_frame_class(TickClass::Transition);
    }
    PaintOutcome {
        needs_frame: ctx.needs_frame(),
        needs_layout: ctx.needs_layout(),
        needs_frame_paced_only: ctx.needs_frame_paced_only(),
    }
}

/// Fold `frame`'s event-pass `trigger` plus the *previous* tick's latched
/// `PaintOutcome` into this tick's `FrameInputs` — exactly how a real shell
/// gathers inputs (events/signals are their own per-tick latches;
/// `last_needs_frame*` carries over from the prior produced frame's paint).
fn tick_inputs(frame: Frame, prev_outcome: PaintOutcome) -> FrameInputs {
    let mut inputs = FrameInputs {
        last_needs_frame: prev_outcome.needs_frame,
        last_needs_frame_paced_only: prev_outcome.needs_frame_paced_only,
        ..FrameInputs::default()
    };
    match frame.trigger {
        Trigger::InputEvent => inputs.events_since_last_frame = true,
        Trigger::SignalWrite => inputs.signals_dirty = true,
        Trigger::None => {}
    }
    inputs
}

/// One simulated tick: decides whether the frame runs, and — only if it
/// actually ran — paints whatever `frame` says is visible/active and
/// re-latches `PaintOutcome` for the next tick. A skipped tick leaves
/// `prev_outcome` untouched, mirroring the frame gate's documented "a skip
/// never re-latches" starvation-avoidance contract.
fn tick(
    gate: &mut FrameGate,
    frame: Frame,
    now: FrameTime,
    pace_interval: Duration,
    prev_outcome: PaintOutcome,
) -> (FrameDecision, PaintOutcome) {
    let inputs = tick_inputs(frame, prev_outcome);
    let decision = gate.decide_paced(
        inputs,
        FramePacing {
            now,
            interval: pace_interval,
        },
    );
    let next_outcome = if decision.is_run() {
        paint_for(frame)
    } else {
        prev_outcome
    };
    (decision, next_outcome)
}

/// Run `ticks` consecutive simulated ticks of a constant `frame` at
/// `tick_dur` spacing (starting at `start_tick`), returning the run count and
/// the final latched `PaintOutcome` so a test can chain stream segments
/// (e.g. offscreen → scroll-back-in → onscreen).
fn run_stream(
    gate: &mut FrameGate,
    frame: Frame,
    start_tick: u64,
    ticks: u64,
    tick_dur: Duration,
    pace_interval: Duration,
    mut prev_outcome: PaintOutcome,
) -> (usize, PaintOutcome) {
    let mut runs = 0usize;
    for i in 0..ticks {
        let now = FrameTime::from_nanos((start_tick + i) * tick_dur.as_nanos() as u64);
        let (decision, next) = tick(gate, frame, now, pace_interval, prev_outcome);
        prev_outcome = next;
        if decision.is_run() {
            runs += 1;
        }
    }
    (runs, prev_outcome)
}

/// Seed a stream as "already mid-flight": one paint pass for `frame` before
/// the stream under test begins, standing in for the rebuild/mount event
/// (outside this suite's scope — see `FrameInputs::change_flags_pending`)
/// that bootstraps a fresh animation's first frame. Every cadence assertion
/// below observes an already-playing loop/transition, the steady-state case
/// the task's matrix cares about; the from-a-cold-start bootstrap is a
/// `change_flags_pending`-driven `Run` task 04/06's own unit tests already
/// cover and is out of scope for this composition suite.
fn seeded(frame: Frame) -> PaintOutcome {
    paint_for(frame)
}

/// One vsync step of a `hz`-Hz refresh, as a `Duration`.
fn step(hz: f64) -> Duration {
    Duration::from_secs_f64(1.0 / hz)
}

/// The framework-default cosmetic-loop cap (30Hz — `MotionScheme::cosmetic_loop_rate`'s default).
fn cap_interval() -> Duration {
    step(30.0)
}

/// The simulated tick-stream rate every test in this file drives the gate at.
fn tick_hz() -> Duration {
    step(120.0)
}

// ---------------------------------------------------------------------
// Matrix: run cadence per source, pacing on/off, gate enabled/disabled
// ---------------------------------------------------------------------

#[test]
fn transition_input_and_signal_sources_are_never_paced() {
    // Gate enabled, pacing enabled: these three sources must still run
    // essentially every 120Hz tick — pacing only ever throttles a genuinely
    // paced-only (CosmeticLoop-only) frame.
    let cases: &[(&str, Frame)] = &[
        ("transition", Frame::TRANSITION),
        ("input_event", Frame::INPUT_EVENT),
        ("signal_write", Frame::SIGNAL_WRITE),
    ];
    for (name, frame) in cases {
        let mut gate = FrameGate::with_flags(true, true);
        let (runs, _) = run_stream(
            &mut gate,
            *frame,
            0,
            120,
            tick_hz(),
            cap_interval(),
            seeded(*frame),
        );
        assert!(
            runs >= 118,
            "{name} must run essentially every tick even with pacing on, got {runs}/120"
        );
    }
}

#[test]
fn paced_loop_onscreen_runs_at_the_cosmetic_loop_cap_when_pacing_is_on() {
    let mut gate = FrameGate::with_flags(true, true);
    let (runs, _) = run_stream(
        &mut gate,
        Frame::PACED_LOOP_ONSCREEN,
        0,
        120,
        tick_hz(),
        cap_interval(),
        seeded(Frame::PACED_LOOP_ONSCREEN),
    );
    assert!(
        (28..=32).contains(&runs),
        "a 120Hz paced-only stream should run ~30x/s at the default cap, got {runs}"
    );
}

#[test]
fn anim_pacing_kill_switch_runs_the_paced_loop_every_tick() {
    // FRUST_NO_ANIM_PACING: gate stays active, only the throttle disables.
    let mut gate = FrameGate::with_flags(true, false);
    let (runs, _) = run_stream(
        &mut gate,
        Frame::PACED_LOOP_ONSCREEN,
        0,
        120,
        tick_hz(),
        cap_interval(),
        seeded(Frame::PACED_LOOP_ONSCREEN),
    );
    assert!(
        runs >= 118,
        "with anim pacing disabled the paced loop must run every tick, got {runs}/120"
    );
}

#[test]
fn pacing_kill_switch_does_not_force_runs_on_an_idle_or_culled_stream() {
    // FRUST_NO_ANIM_PACING narrows only the throttle — it must not turn into
    // a second frame-gate kill switch: a genuinely idle/culled stream still
    // skips every tick.
    for frame in [Frame::IDLE, Frame::PACED_LOOP_OFFSCREEN_CULLED] {
        let mut gate = FrameGate::with_flags(true, false);
        let (runs, _) = run_stream(
            &mut gate,
            frame,
            0,
            60,
            tick_hz(),
            cap_interval(),
            PaintOutcome::default(),
        );
        assert_eq!(
            runs, 0,
            "{frame:?}: the anim-pacing kill switch must not force runs on a quiet stream"
        );
    }
}

#[test]
fn frame_gate_kill_switch_runs_every_source_every_tick() {
    // FRUST_NO_FRAME_GATE: the whole gate is disabled, so every source
    // (including a genuinely idle/culled one) runs every tick.
    let cases: &[(&str, Frame)] = &[
        ("paced_loop_onscreen", Frame::PACED_LOOP_ONSCREEN),
        (
            "paced_loop_offscreen_culled",
            Frame::PACED_LOOP_OFFSCREEN_CULLED,
        ),
        ("transition", Frame::TRANSITION),
        (
            "transition_and_paced_loop",
            Frame::TRANSITION_AND_PACED_LOOP,
        ),
        ("input_event", Frame::INPUT_EVENT),
        ("signal_write", Frame::SIGNAL_WRITE),
        ("idle", Frame::IDLE),
    ];
    for (name, frame) in cases {
        let mut gate = FrameGate::disabled();
        let (runs, _) = run_stream(
            &mut gate,
            *frame,
            0,
            30,
            tick_hz(),
            cap_interval(),
            PaintOutcome::default(),
        );
        assert_eq!(
            runs, 30,
            "{name}: a disabled gate (FRUST_NO_FRAME_GATE) must run every tick"
        );
    }
}

// ---------------------------------------------------------------------
// Culling composition: offscreen → no runs; scroll back in → cadence resumes
// ---------------------------------------------------------------------

#[test]
fn culled_paced_loop_never_wakes_the_gate_while_offscreen() {
    let mut gate = FrameGate::with_flags(true, true);
    let (runs, outcome) = run_stream(
        &mut gate,
        Frame::PACED_LOOP_OFFSCREEN_CULLED,
        0,
        120,
        tick_hz(),
        cap_interval(),
        PaintOutcome::default(),
    );
    assert_eq!(
        runs, 0,
        "an offscreen (culled) loop must never wake the gate"
    );
    assert!(!outcome.needs_frame);
}

#[test]
fn resumption_after_scroll_back_in_restores_the_paced_cadence() {
    let tick_dur = tick_hz();
    let pace = cap_interval();
    let mut gate = FrameGate::with_flags(true, true);

    // 60 ticks fully offscreen (culled): never wakes the gate.
    let (offscreen_runs, outcome_after_offscreen) = run_stream(
        &mut gate,
        Frame::PACED_LOOP_OFFSCREEN_CULLED,
        0,
        60,
        tick_dur,
        pace,
        PaintOutcome::default(),
    );
    assert_eq!(offscreen_runs, 0);

    // The scroll gesture arrives on tick 60: an input trigger AND the loop is
    // back in the visible tree in this same paint pass (a real shell paints
    // whatever is currently onscreen on any Run, regardless of what forced
    // it) — so this tick both runs immediately (input) and re-arms the
    // paced-loop's continuation for the frames after it.
    let scroll_frame = Frame {
        loop_visible: true,
        trigger: Trigger::InputEvent,
        ..Frame::IDLE
    };
    let scroll_now = FrameTime::from_nanos(60 * tick_dur.as_nanos() as u64);
    let (scroll_decision, outcome_after_scroll) = tick(
        &mut gate,
        scroll_frame,
        scroll_now,
        pace,
        outcome_after_offscreen,
    );
    assert!(
        scroll_decision.is_run(),
        "the scroll-back-in event itself must run immediately"
    );

    // 120 further ticks with the loop back onscreen (no further events):
    // cadence resumes at the cap.
    let (runs, _) = run_stream(
        &mut gate,
        Frame::PACED_LOOP_ONSCREEN,
        61,
        120,
        tick_dur,
        pace,
        outcome_after_scroll,
    );
    assert!(
        (28..=33).contains(&runs),
        "cadence should resume at ~30/s once scrolled back into view, got {runs}"
    );
}

// ---------------------------------------------------------------------
// Regression guard: a concurrent transition + paced loop, then settling
// ---------------------------------------------------------------------

#[test]
fn transition_dominates_a_concurrent_paced_loop_until_it_settles_then_cadence_drops_to_cap() {
    let tick_dur = tick_hz();
    let pace = cap_interval();
    let mut gate = FrameGate::with_flags(true, true);

    // Phase 1 (250ms): a transition and a paced loop both request every tick
    // — the max-lattice aggregation (task 04) makes every one of these ticks
    // Transition-class, so the gate must never pace it away.
    let (transition_runs, outcome_after_transition) = run_stream(
        &mut gate,
        Frame::TRANSITION_AND_PACED_LOOP,
        0,
        30,
        tick_dur,
        pace,
        seeded(Frame::TRANSITION_AND_PACED_LOOP),
    );
    assert_eq!(
        transition_runs, 30,
        "a concurrent transition must run every tick, never paced"
    );

    // Phase 2 (1s): the transition settles — only the paced loop keeps
    // requesting. Cadence must drop to the cosmetic-loop cap.
    let (settled_runs, _) = run_stream(
        &mut gate,
        Frame::PACED_LOOP_ONSCREEN,
        30,
        120,
        tick_dur,
        pace,
        outcome_after_transition,
    );
    assert!(
        (28..=33).contains(&settled_runs),
        "cadence should drop to the cap once the transition settles, got {settled_runs}"
    );
}

// ---------------------------------------------------------------------
// Starvation guard
// ---------------------------------------------------------------------

#[test]
fn paced_only_stream_never_starves_across_a_long_run() {
    let tick_dur = tick_hz();
    let pace = cap_interval();
    let mut gate = FrameGate::with_flags(true, true);
    let mut prev = seeded(Frame::PACED_LOOP_ONSCREEN);

    let mut last_run_tick: Option<u64> = None;
    let mut max_gap = 0u64;
    let mut total_runs = 0usize;
    for i in 0..600u64 {
        // 5 simulated seconds.
        let now = FrameTime::from_nanos(i * tick_dur.as_nanos() as u64);
        let (decision, next) = tick(&mut gate, Frame::PACED_LOOP_ONSCREEN, now, pace, prev);
        prev = next;
        if decision.is_run() {
            total_runs += 1;
            if let Some(last) = last_run_tick {
                max_gap = max_gap.max(i - last);
            }
            last_run_tick = Some(i);
        }
    }
    assert!(
        total_runs > 0,
        "a paced-only stream must keep producing frames"
    );
    assert!(
        max_gap <= 5,
        "paced runs must stay periodic; observed max gap of {max_gap} ticks"
    );
}

// ---------------------------------------------------------------------
// Input never paced: interleaved mid-interval event/signal
// ---------------------------------------------------------------------

#[test]
fn an_input_event_mid_paced_interval_runs_immediately() {
    let tick_dur = tick_hz();
    let pace = cap_interval();
    let mut gate = FrameGate::with_flags(true, true);

    // Anchor the pace clock with one produced paced frame at t=0.
    let (_, outcome) = tick(
        &mut gate,
        Frame::PACED_LOOP_ONSCREEN,
        FrameTime::from_nanos(0),
        pace,
        seeded(Frame::PACED_LOOP_ONSCREEN),
    );

    // One 120Hz tick later — well inside the 33ms cap interval — an input
    // event arrives (the loop itself has scrolled away, so only the event
    // triggers this tick). It must run immediately, never waiting for the
    // cap.
    let (decision, _) = tick(
        &mut gate,
        Frame::INPUT_EVENT,
        FrameTime::from_nanos(tick_dur.as_nanos() as u64),
        pace,
        outcome,
    );
    assert_eq!(
        decision,
        FrameDecision::Run,
        "an input event mid-interval must run immediately, never paced"
    );
}

#[test]
fn a_signal_write_mid_paced_interval_runs_immediately() {
    let tick_dur = tick_hz();
    let pace = cap_interval();
    let mut gate = FrameGate::with_flags(true, true);

    let (_, outcome) = tick(
        &mut gate,
        Frame::PACED_LOOP_ONSCREEN,
        FrameTime::from_nanos(0),
        pace,
        seeded(Frame::PACED_LOOP_ONSCREEN),
    );
    let (decision, _) = tick(
        &mut gate,
        Frame::SIGNAL_WRITE,
        FrameTime::from_nanos(tick_dur.as_nanos() as u64),
        pace,
        outcome,
    );
    assert_eq!(
        decision,
        FrameDecision::Run,
        "a signal write mid-interval must run immediately, never paced"
    );
}
