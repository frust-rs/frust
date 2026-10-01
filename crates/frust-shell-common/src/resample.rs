//! Pointer-event resampling + deadline-aware pacing helpers shared by the
//! mobile shells.
//!
//! # What lives here
//!
//! - [`PointerResampler`] — a pure-logic, host-testable buffer of raw pointer
//!   samples (logical coords + a shell-supplied monotonic timestamp) that emits
//!   an interpolated `Move` position at each frame boundary
//!   (`frame_time − `[`SAMPLE_OFFSET_NANOS`]) with a Flutter-parity
//!   [half-frame prediction window](PREDICTION_WINDOW_NANOS). `Down`/`Up`/
//!   `Cancel` phase transitions pass through **losslessly** — never synthesized,
//!   never dropped, never repositioned — so only `Move` positions are ever
//!   resampled (Flutter's `PointerEventResampler` contract, adapted). Samples
//!   are kept in one lane per [`PointerId`](frust_core::event::PointerId), so
//!   simultaneous contacts are each resampled along their own path and never
//!   mix.
//! - [`frame_interval_nanos`] / [`deadline_overrun`] — the deadline-aware
//!   scheduling helpers: estimate a frame-target budget from the
//!   tick-to-tick timestamp delta, and decide whether a frame's measured work
//!   overran it. **Instrumentation only** — no work-dropping heuristics live
//!   here.
//!
//! # Layering choice
//!
//! Like [`crate::frame_gate`] and [`crate::perf`], this is shell-owned by
//! design and lives in `frust-shell-common`: it is platform-agnostic, contains
//! no `unsafe`, no FFI, and no clock read of its own — every timestamp is
//! handed in by the shell (which owns the monotonic clock), keeping this whole
//! module deterministically unit-testable on the host. It compiles unchanged on
//! every target (host / `aarch64-linux-android` / iOS), preserving the crate's
//! zero-`unsafe`, compiles-everywhere charter (see `docs/ARCHITECTURE.md`'s
//! Layer Dependencies).
//!
//! # Clock domain
//!
//! The resampler is domain-agnostic: it only ever *differences* two timestamps,
//! so a shell may stamp both the raw samples ([`PointerResampler::push`]) and
//! the per-frame sample query ([`PointerResampler::resample`]) from any single
//! monotonic source of its choosing, as long as **both come from the same
//! source**. The mobile shells use a per-handle `Instant` epoch for this
//! (decoupled from the vsync `FrameTime` clock that drives animation), so a
//! sample stamped at touch arrival and the frame's sample-time are always
//! comparable.

use std::collections::VecDeque;
use std::time::Duration;

use frust_core::event::{PointerButton, PointerEvent, PointerId, PointerPhase};
use kurbo::Point;

/// The kill-switch environment/compile-time variable: when set to any
/// non-`"0"` value, [`PointerResampler::new`] yields a **disabled** resampler
/// that delivers every raw sample straight through in arrival order (pre-
/// resampling behavior verbatim). Mirrors
/// [`FRUST_NO_FRAME_GATE`](crate::frame_gate::NO_FRAME_GATE_VAR)'s compile-time-
/// or-runtime parsing exactly.
pub const NO_RESAMPLE_VAR: &str = "FRUST_NO_RESAMPLE";

/// How far behind the frame deadline pointer positions are sampled, in
/// nanoseconds: a `Move` is emitted at `frame_time − SAMPLE_OFFSET`, slightly
/// in the past so the two raw samples bracketing that instant are usually
/// already in hand (**interpolation**, not extrapolation) at the common
/// touch/display cadence.
///
/// **Community-approximate** (see `docs/CODE_STANDARDS.md`): Flutter's
/// `GestureBinding` resamples at a negative `samplingOffset`, but the exact
/// default has drifted across engine versions and is not a published constant.
/// ~5ms is the modest interpolate-slightly-in-the-past value community
/// reimplementations converge on — small enough to keep input latency
/// imperceptible, large enough to bracket a newer sample most frames.
pub const SAMPLE_OFFSET_NANOS: u64 = 5_000_000;

/// The forward-prediction clamp, in nanoseconds: when the sample instant runs
/// *past* the newest buffered sample (the finger paused, or its samples lag the
/// display), the position is extrapolated along the last segment's velocity but
/// never more than this far ahead of the newest sample.
///
/// **Community-approximate**: Flutter caps pointer prediction at roughly one
/// half-refresh window to keep a paused/again-moving finger from overshooting;
/// half of a 60Hz frame (~8.33ms) is that Flutter-parity half-frame window.
pub const PREDICTION_WINDOW_NANOS: u64 = 8_333_333;

/// Fallback frame-target interval (60Hz) used by [`frame_interval_nanos`] when
/// there is no prior tick or the tick-to-tick delta is implausible.
pub const DEFAULT_REFRESH_INTERVAL_NANOS: u64 = 16_666_667;

/// Lower plausibility bound for a tick-to-tick interval (1ms ≈ a 1000Hz
/// ceiling): a smaller delta is treated as a clock glitch and replaced by
/// [`DEFAULT_REFRESH_INTERVAL_NANOS`].
pub const MIN_PLAUSIBLE_INTERVAL_NANOS: u64 = 1_000_000;

/// Upper plausibility bound for a tick-to-tick interval (100ms ≈ a 10Hz floor):
/// a larger delta (a long idle across skipped ticks, a resumed app) is treated
/// as non-representative and replaced by [`DEFAULT_REFRESH_INTERVAL_NANOS`].
pub const MAX_PLAUSIBLE_INTERVAL_NANOS: u64 = 100_000_000;

/// One raw pointer contact as delivered by a platform touch entry point, before
/// resampling: which contact it is, the phase transition, the **logical**
/// (density-independent) position the shell already converted, the button
/// (always [`PointerButton::Primary`] for touch), and a shell-supplied monotonic
/// timestamp (see the module's *Clock domain* note).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawPointerSample {
    /// Which contact the sample belongs to — selects its resampling lane.
    pub pointer_id: PointerId,
    pub phase: PointerPhase,
    pub position: Point,
    pub button: PointerButton,
    pub time_nanos: u64,
}

impl RawPointerSample {
    /// The raw sample as a [`PointerEvent`] with its reported position (the
    /// verbatim form used on the disabled/direct-delivery path and for phase
    /// transitions).
    fn as_event(&self) -> PointerEvent {
        PointerEvent {
            phase: self.phase,
            position: self.position,
            button: self.button,
        }
    }
}

/// One resampled event and the contact it belongs to — what
/// [`PointerResampler::resample`] emits, so the shell can rebuild the
/// [`InputEvent::PointerContact`](frust_core::event::InputEvent::PointerContact)
/// carrier for it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResampledPointer {
    /// The contact the event belongs to.
    pub pointer_id: PointerId,
    /// The (possibly resampled) event.
    pub event: PointerEvent,
}

/// A buffered raw sample plus its global arrival sequence number — the
/// tiebreak that keeps the merged output of several lanes in arrival order.
#[derive(Debug, Clone, Copy)]
struct Queued {
    seq: u64,
    sample: RawPointerSample,
}

/// One contact's resampling state: its own buffered samples and the position it
/// last emitted. Lanes never share samples or positions, so two fingers moving
/// at once are each interpolated along their own path.
#[derive(Debug)]
struct Lane {
    pointer_id: PointerId,
    /// This contact's raw samples in arrival (== timestamp) order, drained up to
    /// each frame's sample instant.
    queue: VecDeque<Queued>,
    /// The position of the last event this lane emitted, so a `Move`
    /// interpolation dedups a no-op re-emit and a query with no bracketing pair
    /// can hold the pointer where it was. Cleared to `None` on an `Up`/`Cancel`
    /// (the contact ended — no position to hold), which is also what lets the
    /// lane be retired once its queue is empty.
    last_emitted: Option<Point>,
}

/// One emitted event awaiting the cross-lane merge, keyed by when it happened:
/// `(time, seq)` of the raw sample it came from (for a coalesced `Move`, the
/// last move sample of its run).
type Keyed = ((u64, u64), ResampledPointer);

/// Buffers raw pointer samples and emits frame-boundary-resampled events, one
/// independent **lane per [`PointerId`]**. See the module docs for the
/// interpolation/prediction contract; construct one per app handle and drive it
/// from the shell's touch and frame paths.
///
/// A lane opens with its contact's first sample and ends once its `Up`/`Cancel`
/// has been emitted. Each lane resamples exactly as a single-pointer resampler
/// would; the events of several lanes are merged back into arrival order.
#[derive(Debug)]
pub struct PointerResampler {
    /// When `false`, [`resample`](Self::resample) drains every buffered sample
    /// verbatim in arrival order — the [`NO_RESAMPLE_VAR`] kill switch and
    /// [`disabled`](Self::disabled) path (pre-resampling behavior verbatim).
    enabled: bool,
    /// The live lanes, in the order their contacts first appeared.
    lanes: Vec<Lane>,
    /// The next arrival sequence number [`push`](Self::push) hands out.
    next_seq: u64,
    /// The lanes' keyed output before the merge, reused across frames (cleared,
    /// not reallocated) so a drag's per-frame resample allocates nothing.
    merge: Vec<Keyed>,
}

impl PointerResampler {
    /// A resampler honoring the [`NO_RESAMPLE_VAR`] kill switch — what every
    /// shell constructs. When the variable is set (compile-time `--define` or
    /// runtime env, any non-`"0"` value), this is equivalent to
    /// [`disabled`](Self::disabled).
    pub fn new() -> Self {
        Self::with_enabled(!kill_switch_engaged())
    }

    /// A resampler that always delivers raw samples straight through — the
    /// explicit disabled/kill-switch form (and a test seam bypassing the env
    /// read). Mirrors [`Self::new`]'s behavior when [`NO_RESAMPLE_VAR`] is set.
    pub fn disabled() -> Self {
        Self::with_enabled(false)
    }

    /// Construct with an explicit enabled flag, bypassing the env read — the
    /// test/advanced seam (mirrors [`crate::frame_gate::FrameGate::with_enabled`]).
    pub fn with_enabled(enabled: bool) -> Self {
        Self {
            enabled,
            lanes: Vec::new(),
            next_seq: 0,
            merge: Vec::new(),
        }
    }

    /// Whether resampling is active. `false` for a [`disabled`](Self::disabled)
    /// resampler or when the kill switch is engaged — in which case the shell
    /// should deliver touches directly rather than buffering them here.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Whether any raw sample is still buffered, in any lane. The shell ORs
    /// this into its frame-gate input (`events_since_last_frame`) so a frame
    /// that could not yet drain a too-new sample still runs on the next tick —
    /// the "pending buffered input never starves the gate" contract (see
    /// `docs/CODE_STANDARDS.md`'s default-to-run rule).
    pub fn has_pending(&self) -> bool {
        self.lanes.iter().any(|lane| !lane.queue.is_empty())
    }

    /// Buffer one raw platform sample in its contact's lane (a touch entry
    /// point calls this per contact). Each contact's samples must be pushed in
    /// nondecreasing timestamp order (the natural arrival order of one
    /// pointer's stream).
    pub fn push(&mut self, sample: RawPointerSample) {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        let queued = Queued { seq, sample };
        match self
            .lanes
            .iter_mut()
            .find(|lane| lane.pointer_id == sample.pointer_id)
        {
            Some(lane) => lane.queue.push_back(queued),
            None => self.lanes.push(Lane {
                pointer_id: sample.pointer_id,
                queue: VecDeque::from([queued]),
                last_emitted: None,
            }),
        }
    }

    /// Drain the buffered samples up to this frame's sample instant into `out`,
    /// appending the resampled events — each tagged with its contact — that the
    /// shell should feed into the tree this frame, in order. `out` is appended
    /// to, not cleared — the caller owns/reuses the buffer.
    ///
    /// `frame_time_nanos` is the frame's sample query time in the shell's chosen
    /// monotonic domain (the same domain [`push`](Self::push) stamped with).
    ///
    /// On a **disabled** resampler every buffered sample is emitted verbatim in
    /// arrival order (direct delivery). On an enabled one, each lane
    /// independently:
    /// - emits `Down`/`Up`/`Cancel` whose timestamp has reached the sample
    ///   instant **losslessly** in order, each at its raw reported position;
    /// - coalesces a run of `Move` samples up to the sample instant into a
    ///   single `Move` at the position interpolated at
    ///   `frame_time − `[`SAMPLE_OFFSET_NANOS`] (or extrapolated within the
    ///   [`PREDICTION_WINDOW_NANOS`] clamp when the finger has outrun its
    ///   samples);
    /// - leaves samples still ahead of the sample instant buffered (see
    ///   [`has_pending`](Self::has_pending)).
    ///
    /// The lanes' events are then merged by the arrival order of the samples
    /// they came from, so a second finger's `Down` lands after the first
    /// finger's `Down` that preceded it. A lane whose `Up`/`Cancel` was emitted
    /// and that has nothing left buffered is retired.
    pub fn resample(&mut self, frame_time_nanos: u64, out: &mut Vec<ResampledPointer>) {
        if !self.enabled {
            // Verbatim, in arrival order across every lane.
            self.merge.clear();
            for lane in &mut self.lanes {
                self.merge.extend(lane.queue.drain(..).map(|queued| {
                    (
                        (0, queued.seq),
                        ResampledPointer {
                            pointer_id: queued.sample.pointer_id,
                            event: queued.sample.as_event(),
                        },
                    )
                }));
            }
            self.merge.sort_by_key(|(key, _)| *key);
            out.extend(self.merge.drain(..).map(|(_, event)| event));
            self.lanes.clear();
            return;
        }

        let sample_time = frame_time_nanos.saturating_sub(SAMPLE_OFFSET_NANOS);
        self.merge.clear();
        for lane in &mut self.lanes {
            lane.resample(sample_time, &mut self.merge);
        }
        // Stable, and each lane's own events are already in key order, so this
        // only interleaves lanes — it never reorders within one (and a single
        // lane, every gesture today, comes out exactly as it went in).
        if self.lanes.len() > 1 {
            self.merge.sort_by_key(|(key, _)| *key);
        }
        out.extend(self.merge.drain(..).map(|(_, event)| event));
        self.lanes
            .retain(|lane| !lane.queue.is_empty() || lane.last_emitted.is_some());
    }
}

impl Lane {
    /// This lane's half of [`PointerResampler::resample`]: drain its samples up
    /// to `sample_time` into `out`, each keyed for the cross-lane merge.
    fn resample(&mut self, sample_time: u64, out: &mut Vec<Keyed>) {
        // Position at the sample instant, computed from the pre-drain queue
        // snapshot so every coalesced Move this frame lands on the same point.
        let sample_pos = self.position_at(sample_time);

        // The last move sample of the run being coalesced, if any.
        let mut pending_move: Option<Queued> = None;

        // Copy the front's timestamp out so the immutable `front()` borrow
        // ends before the body pops/mutates.
        while let Some(time_nanos) = self.queue.front().map(|q| q.sample.time_nanos) {
            if time_nanos > sample_time {
                break; // not yet reached — leave it (and everything after) buffered
            }
            let queued = self.queue.pop_front().expect("front was just observed");
            match queued.sample.phase {
                PointerPhase::Move => {
                    pending_move = Some(queued);
                }
                PointerPhase::Down => {
                    // A transition ends any coalesced Move run before it (a
                    // well-formed stream never nests one, but keep ordering
                    // lossless regardless).
                    self.flush_move(pending_move.take(), sample_pos, out);
                    self.emit(queued, out);
                    self.last_emitted = Some(queued.sample.position);
                }
                PointerPhase::Up | PointerPhase::Cancel => {
                    // Bring the pointer to its resampled position first, then
                    // lift/cancel at the raw reported position.
                    self.flush_move(pending_move.take(), sample_pos, out);
                    self.emit(queued, out);
                    self.last_emitted = None; // contact ended — nothing to hold
                }
            }
        }

        // Trailing coalesced Moves become one interpolated Move at the sample
        // position.
        self.flush_move(pending_move, sample_pos, out);
    }

    /// Emit one raw transition verbatim.
    fn emit(&self, queued: Queued, out: &mut Vec<Keyed>) {
        out.push((
            (queued.sample.time_nanos, queued.seq),
            ResampledPointer {
                pointer_id: self.pointer_id,
                event: queued.sample.as_event(),
            },
        ));
    }

    /// Emit the single coalesced `Move` for a run of buffered move samples
    /// (`last` is the run's final sample), at the frame's resampled position —
    /// skipped when there was no move, no resolvable position, or the position
    /// is unchanged from the last emit.
    fn flush_move(
        &mut self,
        last: Option<Queued>,
        sample_pos: Option<Point>,
        out: &mut Vec<Keyed>,
    ) {
        let Some(last) = last else {
            return;
        };
        if let Some(pos) = sample_pos
            && self.last_emitted != Some(pos)
        {
            out.push((
                (last.sample.time_nanos, last.seq),
                ResampledPointer {
                    pointer_id: self.pointer_id,
                    event: PointerEvent {
                        phase: PointerPhase::Move,
                        position: pos,
                        button: last.sample.button,
                    },
                },
            ));
            self.last_emitted = Some(pos);
        }
    }

    /// The interpolated (or clamped-extrapolated) pointer position at
    /// `sample_time`, from the current queue snapshot. `None` only when there is
    /// no sample and no prior emit to hold onto.
    fn position_at(&self, sample_time: u64) -> Option<Point> {
        if self.queue.is_empty() {
            return self.last_emitted;
        }

        // The last sample at/before the instant, and the first strictly after.
        let mut before: Option<&RawPointerSample> = None;
        let mut after: Option<&RawPointerSample> = None;
        for queued in &self.queue {
            let sample = &queued.sample;
            if sample.time_nanos <= sample_time {
                before = Some(sample);
            } else {
                after = Some(sample);
                break;
            }
        }

        match (before, after) {
            // Bracketed: linear interpolation between the two.
            (Some(a), Some(b)) => Some(lerp_point(
                a.position,
                b.position,
                fraction(a.time_nanos, b.time_nanos, sample_time),
            )),
            // The instant is past the newest sample: predict forward, clamped.
            (Some(a), None) => Some(self.predict_forward(a, sample_time)),
            // The instant precedes the first sample: hold at the last emit (or
            // the first sample if nothing was ever emitted).
            (None, Some(b)) => self.last_emitted.or(Some(b.position)),
            (None, None) => self.last_emitted,
        }
    }

    /// Extrapolate past `newest` along the last segment's velocity, clamped so
    /// the prediction never runs more than [`PREDICTION_WINDOW_NANOS`] ahead of
    /// `newest`. Falls back to holding at `newest.position` when there is no
    /// prior sample to derive a velocity from (a single-sample queue).
    fn predict_forward(&self, newest: &RawPointerSample, sample_time: u64) -> Point {
        // The sample immediately before `newest` (the second-to-last element).
        match self.queue.iter().rev().nth(1).map(|queued| &queued.sample) {
            Some(prior) if newest.time_nanos > prior.time_nanos => {
                let ahead = (sample_time - newest.time_nanos).min(PREDICTION_WINDOW_NANOS);
                let span = newest.time_nanos - prior.time_nanos;
                let t = ahead as f64 / span as f64;
                Point::new(
                    newest.position.x + (newest.position.x - prior.position.x) * t,
                    newest.position.y + (newest.position.y - prior.position.y) * t,
                )
            }
            _ => newest.position,
        }
    }
}

impl Default for PointerResampler {
    fn default() -> Self {
        Self::new()
    }
}

/// The normalized position of `t` within `[a, b]` (`0.0` at `a`, `1.0` at `b`),
/// guarding a zero-width span (two samples at the same timestamp) by returning
/// `1.0` so the later sample wins.
fn fraction(a: u64, b: u64, t: u64) -> f64 {
    let span = b.saturating_sub(a);
    if span == 0 {
        return 1.0;
    }
    (t.saturating_sub(a)) as f64 / span as f64
}

/// Linear interpolation between two points at normalized `t`.
fn lerp_point(a: Point, b: Point, t: f64) -> Point {
    Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

/// Estimate this frame's deadline budget (the frame-target interval) from two
/// consecutive tick timestamps. Returns the tick-to-tick
/// delta when it is plausible (`[`[`MIN_PLAUSIBLE_INTERVAL_NANOS`]`,
/// `[`MAX_PLAUSIBLE_INTERVAL_NANOS`]`]`), else [`DEFAULT_REFRESH_INTERVAL_NANOS`]
/// (60Hz) — covering the first tick (no prior), a clock glitch, and a long idle
/// across skipped ticks.
pub fn frame_interval_nanos(prev_tick: Option<u64>, cur_tick: u64) -> u64 {
    match prev_tick {
        Some(prev) if cur_tick > prev => {
            let delta = cur_tick - prev;
            if (MIN_PLAUSIBLE_INTERVAL_NANOS..=MAX_PLAUSIBLE_INTERVAL_NANOS).contains(&delta) {
                delta
            } else {
                DEFAULT_REFRESH_INTERVAL_NANOS
            }
        }
        _ => DEFAULT_REFRESH_INTERVAL_NANOS,
    }
}

/// Whether a frame's measured `work` overran its `budget_nanos` deadline.
/// **Instrumentation only** — the shell records the overrun
/// (a counter, gated behind `perf::enabled()`); it never drops or reshapes work
/// on the strength of this.
pub fn deadline_overrun(work: Duration, budget_nanos: u64) -> bool {
    (work.as_nanos() as u64) > budget_nanos
}

/// Reads the [`NO_RESAMPLE_VAR`] kill switch from the compile-time define and
/// the process environment, mirroring [`crate::frame_gate`]'s
/// `kill_switch_engaged`: either source set to a non-`"0"` value engages it.
fn kill_switch_engaged() -> bool {
    kill_switch(
        option_env!("FRUST_NO_RESAMPLE"),
        std::env::var(NO_RESAMPLE_VAR).ok().as_deref(),
    )
}

/// The pure decision [`kill_switch_engaged`] wraps: a non-empty, non-`"0"`
/// value from either the compile-time or runtime source engages the switch.
/// Split out so it is directly unit-testable without touching the process
/// environment (see [`crate::frame_gate`]'s `kill_switch`).
fn kill_switch(compile_time: Option<&str>, runtime: Option<&str>) -> bool {
    fn is_set_non_zero(value: Option<&str>) -> bool {
        matches!(value, Some(v) if v != "0")
    }
    is_set_non_zero(compile_time) || is_set_non_zero(runtime)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(phase: PointerPhase, x: f64, y: f64, time_nanos: u64) -> RawPointerSample {
        sample_for(PointerId::touch(0), phase, x, y, time_nanos)
    }

    fn sample_for(
        pointer_id: PointerId,
        phase: PointerPhase,
        x: f64,
        y: f64,
        time_nanos: u64,
    ) -> RawPointerSample {
        RawPointerSample {
            pointer_id,
            phase,
            position: Point::new(x, y),
            button: PointerButton::Primary,
            time_nanos,
        }
    }

    /// A frame time whose sample instant (`frame_time − SAMPLE_OFFSET`) is
    /// exactly `sample_time` — the tests reason in sample-instant terms.
    fn frame_time_for(sample_time: u64) -> u64 {
        sample_time + SAMPLE_OFFSET_NANOS
    }

    /// The resampled events of one frame, with their contact ids.
    fn drain_tagged(resampler: &mut PointerResampler, sample_time: u64) -> Vec<ResampledPointer> {
        let mut out = Vec::new();
        resampler.resample(frame_time_for(sample_time), &mut out);
        out
    }

    /// The resampled events of one frame — the single-contact tests' view.
    fn drain(resampler: &mut PointerResampler, sample_time: u64) -> Vec<PointerEvent> {
        drain_tagged(resampler, sample_time)
            .into_iter()
            .map(|r| r.event)
            .collect()
    }

    // -----------------------------------------------------------------
    // kill_switch (pure) — mirrors frame_gate's coverage
    // -----------------------------------------------------------------

    #[test]
    fn kill_switch_off_when_neither_set() {
        assert!(!kill_switch(None, None));
    }

    #[test]
    fn kill_switch_on_when_either_source_wins() {
        assert!(kill_switch(Some("1"), None));
        assert!(kill_switch(None, Some("1")));
        assert!(kill_switch(Some("0"), Some("1")));
        assert!(kill_switch(Some("1"), Some("0")));
    }

    #[test]
    fn kill_switch_off_when_either_is_literal_zero_and_other_unset() {
        assert!(!kill_switch(Some("0"), None));
        assert!(!kill_switch(None, Some("0")));
    }

    // -----------------------------------------------------------------
    // Kill-switch off path: direct verbatim delivery
    // -----------------------------------------------------------------

    #[test]
    fn disabled_resampler_delivers_every_sample_verbatim_in_order() {
        let mut r = PointerResampler::disabled();
        assert!(!r.is_enabled());
        r.push(sample(PointerPhase::Down, 1.0, 2.0, 0));
        r.push(sample(PointerPhase::Move, 3.0, 4.0, 5));
        r.push(sample(PointerPhase::Up, 5.0, 6.0, 10));

        // Sample instant is irrelevant when disabled — everything drains.
        let out = drain(&mut r, 0);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].phase, PointerPhase::Down);
        assert_eq!(out[0].position, Point::new(1.0, 2.0));
        assert_eq!(out[1].phase, PointerPhase::Move);
        assert_eq!(out[1].position, Point::new(3.0, 4.0));
        assert_eq!(out[2].phase, PointerPhase::Up);
        assert_eq!(out[2].position, Point::new(5.0, 6.0));
        assert!(!r.has_pending());
    }

    // -----------------------------------------------------------------
    // Interpolation math
    // -----------------------------------------------------------------

    #[test]
    fn move_position_interpolated_between_bracketing_samples() {
        let mut r = PointerResampler::with_enabled(true);
        // Two moves 10ms apart along x; sample the midpoint (5ms).
        r.push(sample(PointerPhase::Move, 0.0, 0.0, 0));
        r.push(sample(PointerPhase::Move, 10.0, 0.0, 10_000_000));

        let out = drain(&mut r, 5_000_000);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].phase, PointerPhase::Move);
        assert_eq!(out[0].position, Point::new(5.0, 0.0));
        // The later sample is still ahead of the instant → buffered.
        assert!(r.has_pending());
    }

    #[test]
    fn interpolation_fraction_is_time_weighted() {
        let mut r = PointerResampler::with_enabled(true);
        r.push(sample(PointerPhase::Move, 0.0, 0.0, 0));
        r.push(sample(PointerPhase::Move, 100.0, 40.0, 10_000_000));
        // 25% of the way through the segment.
        let out = drain(&mut r, 2_500_000);
        assert_eq!(out[0].position, Point::new(25.0, 10.0));
    }

    // -----------------------------------------------------------------
    // Phase-transition passthrough (lossless, raw positions)
    // -----------------------------------------------------------------

    #[test]
    fn down_move_up_pass_transitions_through_losslessly() {
        let mut r = PointerResampler::with_enabled(true);
        r.push(sample(PointerPhase::Down, 0.0, 0.0, 0));
        r.push(sample(PointerPhase::Move, 4.0, 0.0, 4_000_000));
        r.push(sample(PointerPhase::Up, 8.0, 0.0, 8_000_000));

        // Sample instant reaches the Up.
        let out = drain(&mut r, 8_000_000);
        let phases: Vec<PointerPhase> = out.iter().map(|e| e.phase).collect();
        assert_eq!(
            phases,
            vec![PointerPhase::Down, PointerPhase::Move, PointerPhase::Up]
        );
        // Down and Up keep their raw positions (only Move is resampled).
        assert_eq!(out.first().unwrap().position, Point::new(0.0, 0.0));
        assert_eq!(out.last().unwrap().position, Point::new(8.0, 0.0));
        assert!(!r.has_pending());
    }

    #[test]
    fn cancel_passes_through_and_ends_the_gesture() {
        let mut r = PointerResampler::with_enabled(true);
        r.push(sample(PointerPhase::Down, 1.0, 1.0, 0));
        r.push(sample(PointerPhase::Cancel, 1.0, 1.0, 2_000_000));
        let out = drain(&mut r, 2_000_000);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].phase, PointerPhase::Cancel);
        assert_eq!(out[1].position, Point::new(1.0, 1.0));
        assert!(!r.has_pending());
    }

    #[test]
    fn too_new_transition_stays_buffered_until_its_instant_arrives() {
        let mut r = PointerResampler::with_enabled(true);
        r.push(sample(PointerPhase::Down, 0.0, 0.0, 0));
        r.push(sample(PointerPhase::Up, 0.0, 0.0, 10_000_000));

        // Instant only reaches the Down — the Up is not yet due, never dropped.
        let out = drain(&mut r, 0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].phase, PointerPhase::Down);
        assert!(r.has_pending());

        // A later frame whose instant reaches the Up delivers it.
        let out = drain(&mut r, 10_000_000);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].phase, PointerPhase::Up);
        assert!(!r.has_pending());
    }

    // -----------------------------------------------------------------
    // Prediction clamp
    // -----------------------------------------------------------------

    #[test]
    fn prediction_extrapolates_within_the_window() {
        let mut r = PointerResampler::with_enabled(true);
        // 10px over 10ms → 1px/ms. Sample 4ms past the newest → +4px (< window).
        r.push(sample(PointerPhase::Move, 0.0, 0.0, 0));
        r.push(sample(PointerPhase::Move, 10.0, 0.0, 10_000_000));
        let out = drain(&mut r, 14_000_000);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].position, Point::new(14.0, 0.0));
        assert!(!r.has_pending(), "both samples were at/before the instant");
    }

    #[test]
    fn prediction_is_clamped_to_the_half_frame_window() {
        let mut r = PointerResampler::with_enabled(true);
        // 10px/10ms again, but sample far past the newest (50ms ahead): the
        // extrapolation clamps at PREDICTION_WINDOW_NANOS (~8.33ms → +8.33px),
        // NOT the full 50px an unclamped predictor would give.
        r.push(sample(PointerPhase::Move, 0.0, 0.0, 0));
        r.push(sample(PointerPhase::Move, 10.0, 0.0, 10_000_000));
        let out = drain(&mut r, 60_000_000);
        assert_eq!(out.len(), 1);
        let predicted_x = out[0].position.x;
        let expected = 10.0 + PREDICTION_WINDOW_NANOS as f64 / 1_000_000.0;
        assert!(
            (predicted_x - expected).abs() < 1e-6,
            "clamped prediction {predicted_x} should be {expected}"
        );
    }

    // -----------------------------------------------------------------
    // Empty / one-sample edge cases
    // -----------------------------------------------------------------

    #[test]
    fn empty_queue_resamples_to_nothing() {
        let mut r = PointerResampler::with_enabled(true);
        let out = drain(&mut r, 1_000_000);
        assert!(out.is_empty());
        assert!(!r.has_pending());
    }

    #[test]
    fn single_move_sample_holds_its_position() {
        let mut r = PointerResampler::with_enabled(true);
        // One move; the instant is past it → predict_forward with no prior
        // sample holds at the sample's own position.
        r.push(sample(PointerPhase::Move, 7.0, 3.0, 0));
        let out = drain(&mut r, 5_000_000);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].position, Point::new(7.0, 3.0));
    }

    #[test]
    fn move_before_first_sample_instant_stays_buffered() {
        let mut r = PointerResampler::with_enabled(true);
        // The only sample is newer than the instant → nothing drains yet.
        r.push(sample(PointerPhase::Move, 2.0, 2.0, 10_000_000));
        let out = drain(&mut r, 0);
        assert!(out.is_empty());
        assert!(r.has_pending());
    }

    #[test]
    fn stationary_finger_does_not_re_emit_the_same_move() {
        let mut r = PointerResampler::with_enabled(true);
        r.push(sample(PointerPhase::Down, 5.0, 5.0, 0));
        // Two moves at the same position → the coalesced move equals the Down's
        // recorded position, so no redundant Move is emitted.
        r.push(sample(PointerPhase::Move, 5.0, 5.0, 2_000_000));
        r.push(sample(PointerPhase::Move, 5.0, 5.0, 4_000_000));
        let out = drain(&mut r, 4_000_000);
        assert_eq!(out.len(), 1, "only the Down; the no-op moves are deduped");
        assert_eq!(out[0].phase, PointerPhase::Down);
    }

    // -----------------------------------------------------------------
    // Per-contact lanes
    // -----------------------------------------------------------------

    #[test]
    fn two_interleaved_contacts_resample_in_separate_lanes() {
        let a = PointerId::touch(0);
        let b = PointerId::touch(1);
        let mut r = PointerResampler::with_enabled(true);
        // Two fingers moving in opposite directions, samples interleaved in
        // arrival order. A shared lane would interpolate between the two
        // fingers' positions; separate lanes keep each on its own path.
        r.push(sample_for(a, PointerPhase::Down, 0.0, 0.0, 0));
        r.push(sample_for(b, PointerPhase::Down, 100.0, 0.0, 1_000_000));
        r.push(sample_for(a, PointerPhase::Move, 10.0, 0.0, 10_000_000));
        r.push(sample_for(b, PointerPhase::Move, 90.0, 0.0, 11_000_000));
        r.push(sample_for(a, PointerPhase::Move, 20.0, 0.0, 20_000_000));
        r.push(sample_for(b, PointerPhase::Move, 80.0, 0.0, 21_000_000));

        // Sample at 15ms: lane a interpolates halfway between 10 and 20; lane b
        // between its own 11ms/21ms samples (90 → 80, 40% of the way).
        let out = drain_tagged(&mut r, 15_000_000);
        let ids: Vec<PointerId> = out.iter().map(|e| e.pointer_id).collect();
        let phases: Vec<PointerPhase> = out.iter().map(|e| e.event.phase).collect();
        assert_eq!(ids, vec![a, b, a, b], "merged back into arrival order");
        assert_eq!(
            phases,
            vec![
                PointerPhase::Down,
                PointerPhase::Down,
                PointerPhase::Move,
                PointerPhase::Move
            ]
        );
        assert_eq!(out[0].event.position, Point::new(0.0, 0.0));
        assert_eq!(out[1].event.position, Point::new(100.0, 0.0));
        assert_eq!(out[2].event.position, Point::new(15.0, 0.0));
        let b_x = out[3].event.position.x;
        assert!((b_x - 86.0).abs() < 1e-9, "lane b moved to {b_x}, not 86");
        assert!(r.has_pending(), "both lanes still hold a too-new sample");
    }

    #[test]
    fn a_lane_ends_on_its_own_up_without_ending_the_other() {
        let a = PointerId::touch(0);
        let b = PointerId::touch(1);
        let mut r = PointerResampler::with_enabled(true);
        r.push(sample_for(a, PointerPhase::Down, 0.0, 0.0, 0));
        r.push(sample_for(b, PointerPhase::Down, 50.0, 50.0, 1_000_000));
        r.push(sample_for(b, PointerPhase::Up, 50.0, 50.0, 2_000_000));
        let out = drain_tagged(&mut r, 2_000_000);
        assert_eq!(out.len(), 3);
        assert_eq!(
            (out[2].pointer_id, out[2].event.phase),
            (b, PointerPhase::Up)
        );
        assert_eq!(r.lanes.len(), 1, "b's lane retired on its Up; a's lives on");
        assert_eq!(r.lanes[0].pointer_id, a);

        // Lane a still holds its own position: a later move interpolates from
        // a's Down, never from b's.
        r.push(sample_for(a, PointerPhase::Move, 10.0, 0.0, 10_000_000));
        let out = drain_tagged(&mut r, 10_000_000);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].pointer_id, a);
        assert_eq!(out[0].event.position, Point::new(10.0, 0.0));

        r.push(sample_for(a, PointerPhase::Cancel, 10.0, 0.0, 12_000_000));
        let out = drain_tagged(&mut r, 12_000_000);
        assert_eq!(out[0].event.phase, PointerPhase::Cancel);
        assert!(r.lanes.is_empty(), "a's lane retired on its Cancel");
    }

    #[test]
    fn disabled_resampler_keeps_arrival_order_across_contacts() {
        let a = PointerId::touch(0);
        let b = PointerId::touch(1);
        let mut r = PointerResampler::disabled();
        r.push(sample_for(a, PointerPhase::Down, 0.0, 0.0, 0));
        r.push(sample_for(b, PointerPhase::Down, 9.0, 9.0, 1));
        r.push(sample_for(a, PointerPhase::Move, 1.0, 0.0, 2));
        r.push(sample_for(b, PointerPhase::Up, 9.0, 9.0, 3));
        let out = drain_tagged(&mut r, 0);
        let order: Vec<(PointerId, PointerPhase)> =
            out.iter().map(|e| (e.pointer_id, e.event.phase)).collect();
        assert_eq!(
            order,
            vec![
                (a, PointerPhase::Down),
                (b, PointerPhase::Down),
                (a, PointerPhase::Move),
                (b, PointerPhase::Up),
            ]
        );
        assert!(!r.has_pending());
    }

    // -----------------------------------------------------------------
    // Deadline helpers
    // -----------------------------------------------------------------

    #[test]
    fn frame_interval_uses_plausible_tick_delta() {
        // A clean 60Hz tick delta passes through.
        assert_eq!(frame_interval_nanos(Some(0), 16_666_667), 16_666_667);
    }

    #[test]
    fn frame_interval_falls_back_on_no_prior_or_implausible_delta() {
        assert_eq!(
            frame_interval_nanos(None, 1_000),
            DEFAULT_REFRESH_INTERVAL_NANOS
        );
        // Non-monotonic / equal ticks → fallback.
        assert_eq!(
            frame_interval_nanos(Some(100), 100),
            DEFAULT_REFRESH_INTERVAL_NANOS
        );
        // Too small (sub-1ms) and too large (>100ms idle) → fallback.
        assert_eq!(
            frame_interval_nanos(Some(0), 500),
            DEFAULT_REFRESH_INTERVAL_NANOS
        );
        assert_eq!(
            frame_interval_nanos(Some(0), 200_000_000),
            DEFAULT_REFRESH_INTERVAL_NANOS
        );
    }

    #[test]
    fn deadline_overrun_compares_work_to_budget() {
        let budget = 16_666_667;
        assert!(deadline_overrun(Duration::from_millis(20), budget));
        assert!(!deadline_overrun(Duration::from_millis(10), budget));
        // Exactly at budget is not an overrun.
        assert!(!deadline_overrun(Duration::from_nanos(budget), budget));
    }
}
