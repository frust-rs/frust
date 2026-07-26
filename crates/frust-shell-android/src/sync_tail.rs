//! The shape-aware scroll-sync tail (camera task 12): the regime-gated,
//! timeline-derived hold that sits **on top of** the frame-id release gate
//! (camera task 01) for a platform-view geometry batch.
//!
//! # Why a tail exists at all
//!
//! The frame-id gate holds each batch until the frust frame that painted it has
//! actually been presented, which removes every term the app can observe (the
//! render-thread rate mismatch, the latest-wins channel's dropped scenes). What
//! it cannot see is the term that happens *after* `present` returns — the
//! platform compositor's own queue. On a deep-queue device (Xiaomi 12 `cupid`:
//! `acquire_p95 ≈ 14 ms`, swapchain back-pressure) that leaves a **constant**
//! ~3.5-display-frame residual, flat across a 5× velocity range; on a
//! submit-bound device (OnePlus 9: `acquire ≈ 0.1 ms`, no queue) the gate alone
//! already lands a **median-zero** band and what remains is a p90 *tail of the
//! distribution*, not an offset (SPIKE-SYNC §2.5/§2.6/§2.6.1).
//!
//! # Why the correction is regime-gated, not scalar
//!
//! Every scalar form is CLOSED by measurement: any fixed or derived-but-always-on
//! delay that closes cupid's constant residual overcorrects the OnePlus 9's
//! already-aligned majority and converts a lead into a *worse* lag (measured:
//! tail 0 → lead p90 5.24; tail 2 → lag p90 6.28; tail 4 → lag p90 7.12 —
//! SPIKE-SYNC §2.6.1). The correction is therefore applied **only in the regime
//! that produces a constant-shaped residual**: a deep swapchain queue, whose
//! app-visible proxy is the render tail's acquire wait. That single signal
//! separates the two measured devices by two orders of magnitude and is the
//! mechanism that makes cupid's residual constant in the first place (every
//! frame is equally late behind the same queue).
//!
//! # The two signals
//!
//! - **Regime** — [`TailSignals::acquire_wait_us`], the render side's
//!   quarter-weight acquire-wait EWMA (published through
//!   `app::RenderSignals`, an `Arc` pair across the render split — never a
//!   process-global). Above [`REGIME_ENTER_FRACTION`] of a display period the
//!   surface is acquire-bound (queued); below [`REGIME_EXIT_FRACTION`] it is
//!   submit-bound (no queue). The gap between the two is the hysteresis band,
//!   and a flip additionally needs several consecutive ticks agreeing
//!   ([`REGIME_ENTER_CONFIRM_FRAMES`] / [`REGIME_EXIT_CONFIRM_FRAMES`]), so the
//!   depth cannot flap with the EWMA.
//! - **Depth** — [`TailSignals::expected_present_delta_nanos`], the
//!   Choreographer frame timeline's `expectedPresentationTimeNanos −
//!   frameTimeNanos` (API 33+, sampled Kotlin-side and pushed in). This is the
//!   platform's *own* answer to "when does a window frame committed now reach
//!   the screen", i.e. exactly the geometry-side landing time the tail must
//!   match; measured rock-stable at 37.7 ms on cupid (→ depth 4) and 24.0 ms on
//!   the OnePlus 9 (→ depth 2), with no per-device constant anywhere.
//!
//! # Fallbacks are always "gate-only"
//!
//! No timeline sample (below API 33, or no platform view is hosted so Kotlin
//! never samples), an implausible sample, or a below-threshold regime all
//! produce depth `0`, which is a bit-for-bit pass-through to the frame-id gate's
//! own answer — already measured strictly better than shipped on both devices.
//! The hold is a smoothing device, never a correctness barrier: every held batch
//! also lands unconditionally after [`MAX_HOLD_FRAMES`] display frames
//! (the settle guarantee — a final resting geometry can never strand), and
//! [`ScrollSyncTail::clear`] releases the whole hold at once for the lifecycle
//! edges where no further frame will be presented (backgrounding, surface loss).
//!
//! # Gesture onset (camera task 12b)
//!
//! Task 12's steady state is clean (band median 0 px at every velocity,
//! SPIKE-SYNC §2.7), but the *first* frames of a gesture were not: the
//! correction could not exist until the acquire EWMA had risen under the new
//! load, been confirmed for a whole confirm window, and then been ramped in one
//! frame per tick. Three additive delays, all measured in display frames, all
//! paid at the start of **every** gesture. Two of them are closed here:
//!
//! 1. **Asymmetric confirm** ([`REGIME_ENTER_CONFIRM_FRAMES`] vs
//!    [`REGIME_EXIT_CONFIRM_FRAMES`]). The costs of the two flips are not
//!    symmetric, so the confirm windows are not either: entering wrongly costs a
//!    small hold that drains harmlessly within a few frames (and is bounded by
//!    [`MAX_HOLD_FRAMES`] regardless), while entering late costs a visible
//!    desync at the start of every scroll. Leaving late costs nothing at rest,
//!    so the exit window keeps the full eight-tick agreement that makes the
//!    stand-down conservative.
//! 2. **Pre-seeded depth** ([`ScrollSyncTail::tick`]'s seed arm). The ramp
//!    exists so a *change* in depth under a live hold does not step the geometry
//!    by several frames in one tick. On the tick the regime latches there is no
//!    such hold to step (`depth == 0` — the tail was passing the gate through),
//!    and the derived depth's input (the frame timeline's
//!    `expectedPresentationTimeNanos` delta) is rock-stable from the very first
//!    tick, measured ±0 across every run on cupid. So the ramp buys nothing at
//!    onset and costs one display frame per unit of depth: the entry edge (and
//!    any rise from a depth of zero) applies the derived depth immediately, and
//!    the ramp is kept only for depth changes *on top of* a live hold, in both
//!    directions.
//!
//! The third delay — the acquire EWMA only starts rising once the GPU is
//! actually loaded, i.e. the clock on (1) does not start at the first moved
//! pixel — is **not** addressed here. Closing it needs a speculative arm driven
//! by a touch-down signal pushed from Kotlin, which is only justified by a
//! measurement showing (1)+(2) fall short; see this task's summary and
//! SPIKE-SYNC §2.8 for the bar that would justify it.
//!
//! Everything here is pure logic driven by two scalars per tick — no clock, no
//! JNI, no platform types — so it compiles and unit-tests on the host even
//! though the shell it serves is `#[cfg(target_os = "android")]`. That includes
//! the onset behaviour: [`tests::onset_reaches_full_depth_in_three_frames`]
//! drives the acquire EWMA's own rise curve, so the frame count this task moves
//! is pinned by a host test rather than only by a device trace.

use std::collections::VecDeque;

/// Acquire-wait fraction of one display period above which the surface is
/// treated as acquire-bound (deep swapchain queue → constant-shaped residual →
/// the tail applies). Half a period at 120 Hz is ~4.2 ms: cupid measures ~14 ms,
/// the OnePlus 9 ~0.1 ms, so the two devices sit three-and-a-half octaves either
/// side of it — the threshold is a regime boundary, not a tuning.
const REGIME_ENTER_FRACTION: f64 = 0.5;

/// Acquire-wait fraction below which the acquire-bound regime is left again.
/// Deliberately lower than [`REGIME_ENTER_FRACTION`] — the gap is the hysteresis
/// band a fluctuating EWMA can wander inside without flipping the regime.
const REGIME_EXIT_FRACTION: f64 = 0.25;

/// Consecutive ticks that must argue for *entering* the acquire-bound regime
/// before it latches (~17 ms at 120 Hz). Level hysteresis alone still flips on a
/// single outlier EWMA sample, so some time hysteresis is needed — but only
/// enough to reject one: the quarter-weight acquire EWMA cannot stay above
/// `period/2` for two consecutive ticks off a single spike (it decays to 75% of
/// the spike's contribution on the very next tick, pinned by
/// `a_single_acquire_spike_cannot_arm_a_submit_bound_device`). Entering late is
/// the expensive mistake — it is paid as a visible desync at the start of every
/// gesture — so the entry window is the short one (camera task 12b, rung 1).
const REGIME_ENTER_CONFIRM_FRAMES: u32 = 2;

/// Consecutive ticks that must argue for *leaving* the acquire-bound regime
/// before it stands down (~65 ms at 120 Hz). Deliberately four times the entry
/// window: leaving late costs nothing visible (the queue is draining, the hold
/// drains with it one frame per tick), while leaving early re-opens the desync
/// mid-gesture. This is task 12's original value, unchanged.
const REGIME_EXIT_CONFIRM_FRAMES: u32 = 8;

/// Latch margin subtracted from the frame-timeline delta before it is converted
/// to a depth (~half a 120 Hz period). The batch must be applied by the *start*
/// of the display frame that shows the matching frust content, not during it —
/// measured as the value that locks cupid's depth at a steady 4 (SPIKE-SYNC
/// §2.6).
const LATCH_MARGIN_MS: f64 = 4.0;

/// Upper bound on the derived depth. A timeline delta implying more than this is
/// not a scroll-sync problem any more (an eight-frame compositor queue is a
/// pathology), and the bound keeps the worst-case hold ~67 ms at 120 Hz.
const MAX_TAIL_DEPTH: u32 = 8;

/// The unconditional landing guarantee: a held batch is released after this many
/// display frames no matter what the depth says. The mobile frame gate skips
/// whole frames at rest and a regime can change under a held batch — neither may
/// ever strand a slot's final resting geometry (SPIKE-SYNC §2.4's settle bar).
const MAX_HOLD_FRAMES: u64 = 16;

/// Hard bound on tracked holds. One entry is created per display frame at worst,
/// and everything older than [`MAX_HOLD_FRAMES`] is due anyway, so the deque
/// cannot legitimately grow past a couple of dozen — this only bounds a
/// pathological caller (a poll that never advances the tick).
const MAX_TRACKED_HOLDS: usize = 64;

/// Seed display period (120 Hz), replaced by the observed EWMA within a few
/// ticks. Only used before any tick-to-tick delta has been observed.
const DEFAULT_PERIOD_MS: f64 = 8.33;

/// Weight of a fresh sample in the display-period EWMA (quarter-weight, matching
/// the render side's acquire EWMA).
const PERIOD_EWMA_WEIGHT: f64 = 0.25;

/// Plausible tick-to-tick deltas, in ms. Anything outside this is a stall, an app
/// pause, or a clock jump — not a vsync cadence — and must not drag the period
/// estimate (which is the depth's divisor).
const MIN_PLAUSIBLE_PERIOD_MS: f64 = 2.0;
const MAX_PLAUSIBLE_PERIOD_MS: f64 = 50.0;

/// Plausible frame-timeline delta, in ms. `0` means "no sample" (below API 33,
/// or nothing hosted so Kotlin never sampled); anything beyond the ceiling is a
/// bogus timeline read and is treated the same way — gate-only.
const MAX_PLAUSIBLE_PRESENT_DELTA_MS: f64 = 100.0;

/// Bounded interval, in display frames, at which [`ScrollSyncTail::tick`]
/// emits a diagnostic [`TailTrace`] **even with no depth-or-regime change**
/// (g2, C3). The change-only trace answers nothing on a device where the
/// regime never latches at all, or hovers near a threshold without crossing
/// it — exactly the "why didn't it latch" question a new-device gate check
/// needs, and the reason a throwaway instrumented APK was previously the only
/// way to see it. 120 display frames is ~1 s at 120 Hz / ~2 s at 60 Hz: short
/// enough that a single gesture (typically several hundred ms to a few
/// seconds) produces at least one snapshot even with zero changes, long
/// enough that it stays a handful of lines per session rather than
/// perturbing what it measures like a per-frame line would.
const DIAGNOSTIC_INTERVAL_DISPLAY_FRAMES: u64 = 120;

/// One tick's worth of signal, gathered by the shell at the top of every
/// Choreographer frame (skipped frames included — the hold ages in *display*
/// frames, which keep coming while frust produces nothing).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TailSignals {
    /// This tick's `frameTimeNanos` (Choreographer clock), the period EWMA's
    /// only input.
    pub frame_time_nanos: i64,
    /// `expectedPresentationTimeNanos − frameTimeNanos` from the Choreographer
    /// frame timeline (API 33+), pushed in from Kotlin; `0` = no sample.
    pub expected_present_delta_nanos: u64,
    /// The render side's acquire-wait EWMA in µs — the regime discriminator.
    pub acquire_wait_us: u64,
}

/// Which residual shape the surface is currently producing, as inferred from the
/// acquire-wait regime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Regime {
    /// Submit-bound (no swapchain queue): the residual is a distribution tail,
    /// not an offset — a hold would overcorrect the aligned majority, so the
    /// tail stays off and the frame-id gate is the whole correction.
    SubmitBound,
    /// Acquire-bound (deep swapchain queue): every frame is equally late behind
    /// the same queue, so the residual is a constant and a derived hold closes
    /// it.
    AcquireBound,
}

/// One tick's diagnostic read-out, returned by [`ScrollSyncTail::tick`] on the
/// ticks where the depth or the regime actually changed, **plus** at least
/// once every [`DIAGNOSTIC_INTERVAL_DISPLAY_FRAMES`] regardless (g2, C3) — a
/// handful of lines per gesture instead of one per frame, and the only
/// on-device read-out of what the regime decided, including the "it never
/// latched" case a change-only trace cannot show. `display_frame` is what
/// makes an onset measurable: subtract the frame stamped on the gesture's
/// `Down` from the frame the depth reached its target (camera task 12b,
/// SPIKE-SYNC §2.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TailTrace {
    /// The hold depth now in force, in display frames (`0` = gate-only).
    pub depth: u32,
    /// The depth the signals asked for this tick — equal to `depth` except
    /// while a mid-gesture change is ramping.
    pub target_depth: u32,
    /// Whether the acquire-bound regime is latched (the depth alone cannot
    /// distinguish "submit-bound" from "acquire-bound with no timeline sample").
    pub acquire_bound: bool,
    /// This tail's monotonic display-frame counter at the change.
    pub display_frame: u64,
}

/// One batch waiting out its hold: the differ generation the frame-id gate has
/// released, plus the display frame on which it *became* releasable.
#[derive(Debug, Clone, Copy)]
struct Hold {
    generation: u64,
    seen_frame: u64,
}

/// The regime-gated tail: [`Self::tick`] once per Choreographer frame,
/// [`Self::releasable`] once per platform-view poll.
#[derive(Debug)]
pub(crate) struct ScrollSyncTail {
    /// Monotonic display-frame counter (every tick, gated frames included).
    display_frame: u64,
    /// Previous tick's `frameTimeNanos`, for the period delta.
    last_frame_time_nanos: Option<i64>,
    /// Observed display period EWMA, in ms.
    period_ms: f64,
    /// Current regime, and how many consecutive ticks have argued for flipping
    /// it.
    regime: Regime,
    pending_flip_frames: u32,
    /// The depth the signals currently ask for, and the depth actually in force.
    /// The two differ only while a mid-gesture change ramps toward the target
    /// (one step per tick, so a live hold is never stepped by several frames at
    /// once); the entry edge itself is applied whole — see [`Self::tick`].
    target_depth: u32,
    depth: u32,
    /// Gate-released generations still inside their hold, ascending in both
    /// fields.
    holds: VecDeque<Hold>,
    /// The highest generation the gate has offered so far (the deduplicating
    /// edge detector — a generation is recorded once, on the tick it first
    /// becomes releasable).
    last_gate_generation: u64,
    /// The highest generation whose hold has expired: this tail's own answer,
    /// clamped against the gate's on every poll so it can only ever *delay*.
    released_generation: u64,
    /// The `display_frame` the last [`TailTrace`] was emitted on (change or
    /// periodic) — [`Self::tick`]'s rate-limit clock for the bounded-interval
    /// diagnostic (g2, C3).
    last_diagnostic_frame: u64,
}

impl ScrollSyncTail {
    pub(crate) fn new() -> Self {
        Self {
            display_frame: 0,
            last_frame_time_nanos: None,
            period_ms: DEFAULT_PERIOD_MS,
            regime: Regime::SubmitBound,
            pending_flip_frames: 0,
            target_depth: 0,
            depth: 0,
            holds: VecDeque::new(),
            last_gate_generation: 0,
            released_generation: 0,
            last_diagnostic_frame: 0,
        }
    }

    /// Advance one display frame and re-derive the hold depth from this tick's
    /// signals. Called at the top of the shell's frame callback — *before* the
    /// surface-ready and frame-gate early returns, because the hold ages in
    /// display frames and a skipped or surface-less tick is still a display
    /// frame.
    ///
    /// Returns a [`TailTrace`] on the ticks where the depth or the regime
    /// changed, **plus** at least once every
    /// [`DIAGNOSTIC_INTERVAL_DISPLAY_FRAMES`] regardless (g2, C3's
    /// rate-limited diagnostic) — the decision of *when* to emit lives here,
    /// host-tested, so the shell's log call site stays a single
    /// `perf::enabled()`-gated `log::info!`.
    pub(crate) fn tick(&mut self, signals: TailSignals) -> Option<TailTrace> {
        self.display_frame += 1;
        self.update_period(signals.frame_time_nanos);
        let regime_before = self.regime;
        self.update_regime(signals.acquire_wait_us);
        let regime_changed = self.regime != regime_before;
        self.target_depth = self.derive_depth(signals.expected_present_delta_nanos);

        let previous = self.depth;
        self.depth = match self.depth.cmp(&self.target_depth) {
            // Pre-seed (camera task 12b, rung 2): a rise off a depth of zero, or
            // off the tick the regime just latched, applies the derived depth
            // whole. The ramp's job is to stop a depth *change* from stepping a
            // live hold by several frames at once — at onset there is no live
            // hold to step (the tail was passing the gate through), and the
            // timeline delta the depth derives from is already stable, so
            // ramping here only postpones the correction one display frame per
            // unit of depth at the exact moment the desync is visible.
            std::cmp::Ordering::Less if regime_changed || self.depth == 0 => self.target_depth,
            std::cmp::Ordering::Less => self.depth + 1,
            // Downward is always ramped, entry edge or not: dropping the depth
            // whole would release every held batch in one tick and jump the
            // geometry forward by `depth` frames — the mirror of the artifact
            // the ramp exists to prevent. Draining one frame per tick is also
            // what keeps the stand-down invisible.
            std::cmp::Ordering::Greater => self.depth - 1,
            std::cmp::Ordering::Equal => self.depth,
        };
        let changed = self.depth != previous || regime_changed;
        // Bounded-interval fallback (g2, C3): a change-only trace is silent
        // for an entire session on a device whose regime never latches (or
        // sits just under the threshold without crossing it), which is
        // exactly the case a new-device gate check needs to see. Firing this
        // in `saturating_sub` terms (not `%`) means a change resets the
        // clock rather than leaving the periodic cadence out of phase with
        // it.
        let interval_elapsed = self
            .display_frame
            .saturating_sub(self.last_diagnostic_frame)
            >= DIAGNOSTIC_INTERVAL_DISPLAY_FRAMES;
        (changed || interval_elapsed).then(|| {
            self.last_diagnostic_frame = self.display_frame;
            TailTrace {
                depth: self.depth,
                target_depth: self.target_depth,
                acquire_bound: self.regime_active(),
                display_frame: self.display_frame,
            }
        })
    }

    /// The tail's answer for this poll: the highest generation whose hold has
    /// expired, never above `gate_generation` (the frame-id gate's own answer —
    /// this stage can only ever delay, never release something the gate is still
    /// holding).
    ///
    /// `gate_generation` must be a **real** backlog generation. The frame-id
    /// gate reports "nothing held" as `u64::MAX`, which the caller resolves
    /// against the differ's live tip first: a saturated sentinel would be
    /// recorded once and never rise again, so every later batch would sail
    /// through un-held (measured on cupid — the tail read a steady depth while
    /// the band stayed at the gate-only residual).
    ///
    /// Idempotent within a tick: polling twice on the same display frame records
    /// nothing new and returns the same answer, which is what lets the shell call
    /// it straight from the JNI poll.
    pub(crate) fn releasable(&mut self, gate_generation: u64) -> u64 {
        if gate_generation > self.last_gate_generation {
            self.last_gate_generation = gate_generation;
            // Depth 0 (gate-only) needs no bookkeeping at all: the entry would
            // be due on the very tick it was recorded.
            if self.depth == 0 && self.holds.is_empty() {
                self.released_generation = gate_generation;
            } else {
                self.holds.push_back(Hold {
                    generation: gate_generation,
                    seen_frame: self.display_frame,
                });
            }
        }
        self.drain_due();
        self.released_generation.min(gate_generation)
    }

    /// Release the whole hold immediately — the lifecycle edges where the frame
    /// that would justify holding is never coming: backgrounding
    /// (`suspend_all`'s hide) and surface recreation (the replay belongs to a
    /// surface whose frames are gone). Mirrors `FramePairing::clear`'s contract
    /// in `frust-shell-common`; the two are cleared together.
    pub(crate) fn clear(&mut self) {
        if let Some(last) = self.holds.back() {
            self.released_generation = self.released_generation.max(last.generation);
        }
        self.holds.clear();
    }

    /// The hold depth currently in force, in display frames (`0` = gate-only).
    /// Diagnostics/tests only.
    #[cfg(test)]
    pub(crate) fn depth(&self) -> u32 {
        self.depth
    }

    /// Whether the acquire-bound regime is currently latched — the tail's
    /// diagnostic read-out beside the depth (the depth alone cannot distinguish
    /// "submit-bound" from "acquire-bound with no timeline sample").
    pub(crate) fn regime_active(&self) -> bool {
        self.regime == Regime::AcquireBound
    }

    /// The observed display period in ms, for the same diagnostic line — the
    /// depth's divisor, and the scale every regime threshold is expressed in.
    pub(crate) fn period_ms(&self) -> f64 {
        self.period_ms
    }

    /// This tail's monotonic display-frame counter — the clock every onset
    /// measurement counts in. The shell stamps it on a gesture's `Down` so the
    /// frames between the first touch and the depth reaching its target can be
    /// read straight out of a trace (camera task 12b, SPIKE-SYNC §2.8).
    pub(crate) fn display_frame(&self) -> u64 {
        self.display_frame
    }

    /// Fold this tick's cadence into the display-period EWMA, ignoring gaps (app
    /// pause, long stall, clock jump) so only plausible vsync deltas feed the
    /// depth's divisor.
    fn update_period(&mut self, frame_time_nanos: i64) {
        if let Some(previous) = self.last_frame_time_nanos.replace(frame_time_nanos) {
            let delta_ms = (frame_time_nanos - previous) as f64 / 1e6;
            if (MIN_PLAUSIBLE_PERIOD_MS..=MAX_PLAUSIBLE_PERIOD_MS).contains(&delta_ms) {
                self.period_ms =
                    (1.0 - PERIOD_EWMA_WEIGHT) * self.period_ms + PERIOD_EWMA_WEIGHT * delta_ms;
            }
        }
    }

    /// Run the regime state machine over this tick's acquire-wait EWMA: level
    /// hysteresis (two thresholds) plus time hysteresis (consecutive agreeing
    /// ticks) before a flip. The time hysteresis is **asymmetric** — see
    /// [`REGIME_ENTER_CONFIRM_FRAMES`] / [`REGIME_EXIT_CONFIRM_FRAMES`] for why
    /// the two directions do not cost the same.
    fn update_regime(&mut self, acquire_wait_us: u64) {
        let acquire_ms = acquire_wait_us as f64 / 1000.0;
        let (wants_flip, confirm_frames) = match self.regime {
            Regime::SubmitBound => (
                acquire_ms > self.period_ms * REGIME_ENTER_FRACTION,
                REGIME_ENTER_CONFIRM_FRAMES,
            ),
            Regime::AcquireBound => (
                acquire_ms < self.period_ms * REGIME_EXIT_FRACTION,
                REGIME_EXIT_CONFIRM_FRAMES,
            ),
        };
        if !wants_flip {
            self.pending_flip_frames = 0;
            return;
        }
        self.pending_flip_frames += 1;
        if self.pending_flip_frames >= confirm_frames {
            self.regime = match self.regime {
                Regime::SubmitBound => Regime::AcquireBound,
                Regime::AcquireBound => Regime::SubmitBound,
            };
            self.pending_flip_frames = 0;
        }
    }

    /// Convert this tick's frame-timeline delta into a hold depth — zero unless
    /// the acquire-bound regime is latched AND the sample is present and
    /// plausible (every fallback is gate-only).
    fn derive_depth(&self, expected_present_delta_nanos: u64) -> u32 {
        if self.regime != Regime::AcquireBound {
            return 0;
        }
        let delta_ms = expected_present_delta_nanos as f64 / 1e6;
        if delta_ms <= 0.0 || delta_ms > MAX_PLAUSIBLE_PRESENT_DELTA_MS {
            return 0; // no sample (below API 33 / nothing hosted) or implausible
        }
        let tail_ms = delta_ms - LATCH_MARGIN_MS;
        if tail_ms <= 0.0 || self.period_ms <= 0.0 {
            return 0;
        }
        ((tail_ms / self.period_ms).round().max(0.0) as u32).min(MAX_TAIL_DEPTH)
    }

    /// Pop every hold whose wait is over — its depth has elapsed, the
    /// unconditional landing guarantee has, or the tracked-hold bound was hit —
    /// advancing this tail's released cursor to the last one popped.
    fn drain_due(&mut self) {
        while let Some(front) = self.holds.front() {
            let age = self.display_frame.saturating_sub(front.seen_frame);
            let due = age >= u64::from(self.depth)
                || age >= MAX_HOLD_FRAMES
                || self.holds.len() > MAX_TRACKED_HOLDS;
            if !due {
                break;
            }
            self.released_generation = self.released_generation.max(front.generation);
            self.holds.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERIOD_NANOS: i64 = 8_333_333; // 120 Hz

    /// cupid (Xiaomi 12), as measured: acquire-bound with a rock-stable 37.7 ms
    /// frame-timeline delta (SPIKE-SYNC §2.6/§2.6.1).
    fn cupid_signals(frame: i64) -> TailSignals {
        TailSignals {
            frame_time_nanos: frame * PERIOD_NANOS,
            expected_present_delta_nanos: 37_700_000,
            acquire_wait_us: 14_000,
        }
    }

    /// OnePlus 9, as measured: submit-bound (acquire ~0.1 ms) with a stable
    /// 24.0 ms delta — the signal transfers, the correction must not apply.
    fn op9_signals(frame: i64) -> TailSignals {
        TailSignals {
            frame_time_nanos: frame * 8_264_000, // ~121 Hz, as measured
            expected_present_delta_nanos: 23_999_000,
            acquire_wait_us: 120,
        }
    }

    fn run_ticks(tail: &mut ScrollSyncTail, count: i64, signals: impl Fn(i64) -> TailSignals) {
        for frame in 1..=count {
            tail.tick(signals(frame));
        }
    }

    #[test]
    fn cupid_signals_derive_depth_four() {
        let mut tail = ScrollSyncTail::new();
        run_ticks(&mut tail, 40, cupid_signals);
        assert!(tail.regime_active(), "acquire 14 ms is acquire-bound");
        // (37.7 - 4) / 8.33 = 4.04 -> 4, the measured depth on cupid.
        assert_eq!(tail.depth(), 4);
    }

    #[test]
    fn op9_signals_never_activate_the_tail() {
        let mut tail = ScrollSyncTail::new();
        run_ticks(&mut tail, 200, op9_signals);
        assert!(!tail.regime_active(), "acquire 0.12 ms is submit-bound");
        assert_eq!(tail.depth(), 0, "an OP9-like signal must stay gate-only");
    }

    #[test]
    fn op9_like_signal_passes_the_gate_through_unchanged() {
        let mut tail = ScrollSyncTail::new();
        // Every tick offers a fresh generation, exactly as a fling does.
        for frame in 1..=60 {
            tail.tick(op9_signals(frame));
            let generation = frame as u64;
            assert_eq!(
                tail.releasable(generation),
                generation,
                "gate-only means bit-for-bit pass-through"
            );
        }
    }

    #[test]
    fn a_batch_is_held_exactly_depth_display_frames() {
        let mut tail = ScrollSyncTail::new();
        run_ticks(&mut tail, 40, cupid_signals);
        assert_eq!(tail.depth(), 4);

        // Generation 7 becomes gate-releasable on this tick.
        assert_eq!(tail.releasable(7), 0, "held on the tick it is recorded");
        for elapsed in 1..4 {
            tail.tick(cupid_signals(40 + elapsed));
            assert_eq!(tail.releasable(7), 0, "still inside the hold");
        }
        tail.tick(cupid_signals(44));
        assert_eq!(
            tail.releasable(7),
            7,
            "released after exactly `depth` frames"
        );
    }

    #[test]
    fn the_hold_never_releases_past_the_gate() {
        let mut tail = ScrollSyncTail::new();
        run_ticks(&mut tail, 40, cupid_signals);
        assert_eq!(tail.releasable(7), 0);
        run_ticks(&mut tail, 8, |f| cupid_signals(40 + f));
        // Generation 7's hold has expired, but the gate has since pulled its own
        // answer back to 5 (a newer batch's frame is not on screen): the tail
        // must not release past it.
        assert_eq!(tail.releasable(5), 5);
    }

    #[test]
    fn a_held_batch_lands_at_rest_without_further_batches() {
        let mut tail = ScrollSyncTail::new();
        run_ticks(&mut tail, 40, cupid_signals);
        assert_eq!(tail.releasable(9), 0);
        // The scroll stops: no new generations arrive, and (worst case) the
        // depth keeps growing under the already-held batch. The hold ages in
        // DISPLAY frames — which the Choreographer keeps delivering even while
        // the frame gate skips every frust frame — so it lands regardless, and
        // `MAX_HOLD_FRAMES` bounds it even if the depth outran the age.
        for frame in 41..=(41 + MAX_HOLD_FRAMES as i64) {
            tail.tick(TailSignals {
                frame_time_nanos: frame * PERIOD_NANOS,
                expected_present_delta_nanos: 66_000_000, // implies depth 7-8
                acquire_wait_us: 14_000,
            });
        }
        assert_eq!(
            tail.releasable(9),
            9,
            "settle can never strand the final geometry"
        );
    }

    #[test]
    fn clear_releases_every_held_batch() {
        let mut tail = ScrollSyncTail::new();
        run_ticks(&mut tail, 40, cupid_signals);
        assert_eq!(tail.releasable(11), 0);
        tail.clear();
        assert_eq!(
            tail.releasable(11),
            11,
            "backgrounding must not hold a hide"
        );
    }

    #[test]
    fn regime_entry_seeds_the_depth_and_exit_ramps_one_frame_per_tick() {
        let mut tail = ScrollSyncTail::new();
        // Entry: nothing happens until the (short) entry confirm window fills...
        run_ticks(
            &mut tail,
            REGIME_ENTER_CONFIRM_FRAMES as i64 - 1,
            cupid_signals,
        );
        assert!(!tail.regime_active());
        assert_eq!(tail.depth(), 0);
        // ...and on the tick it does, the derived depth applies WHOLE (camera
        // task 12b, rung 2): there is no live hold to step, and every ramped
        // frame here is a frame of visible desync at the start of the gesture.
        tail.tick(cupid_signals(REGIME_ENTER_CONFIRM_FRAMES as i64));
        assert!(tail.regime_active());
        assert_eq!(tail.depth(), 4, "the entry edge is not ramped");
        tail.tick(cupid_signals(100));
        assert_eq!(tail.depth(), 4, "and then holds at the derived depth");

        // Exit: still ramped, one frame per tick, once the queue drains away —
        // dropping the depth whole would release every held batch at once.
        let submit_bound = |frame: i64| TailSignals {
            acquire_wait_us: 100,
            ..cupid_signals(frame)
        };
        run_ticks(&mut tail, REGIME_EXIT_CONFIRM_FRAMES as i64 - 1, |f| {
            submit_bound(200 + f)
        });
        assert!(
            tail.regime_active(),
            "still latched inside the exit confirm window"
        );
        assert_eq!(tail.depth(), 4);
        for (tick, expected) in (0u32..4).rev().enumerate() {
            tail.tick(submit_bound(300 + tick as i64));
            assert!(!tail.regime_active());
            assert_eq!(tail.depth(), expected);
        }
    }

    /// The quarter-weight acquire EWMA the render side publishes, replayed here
    /// so the onset tests drive the *real* rise curve rather than a step: the
    /// regime cannot latch before this signal has climbed past `period/2`, which
    /// is the first of the three onset delays (camera task 12b).
    struct AcquireEwma {
        value_ms: f64,
    }

    impl AcquireEwma {
        fn idle() -> Self {
            Self { value_ms: 0.0 }
        }

        /// Fold one frame's raw acquire wait in and return the published µs.
        fn push(&mut self, sample_ms: f64) -> u64 {
            self.value_ms = 0.75 * self.value_ms + 0.25 * sample_ms;
            (self.value_ms * 1000.0) as u64
        }
    }

    #[test]
    fn onset_reaches_full_depth_in_three_frames() {
        let mut tail = ScrollSyncTail::new();
        let mut acquire = AcquireEwma::idle();
        // At rest the GPU is unloaded, so the acquire wait is ~0 and the tail is
        // gate-only — the state every gesture starts from.
        for frame in 1..=30 {
            tail.tick(TailSignals {
                acquire_wait_us: acquire.push(0.05),
                ..cupid_signals(frame)
            });
        }
        assert!(!tail.regime_active());
        assert_eq!(tail.depth(), 0);

        // The gesture starts: the swapchain queue fills and each frame now waits
        // ~14 ms in acquire (cupid, SPIKE-SYNC §2.5). Count the display frames
        // from the first loaded frame to the hold reaching its full depth.
        let mut onset_frames = 0;
        for frame in 31..=60 {
            onset_frames += 1;
            tail.tick(TailSignals {
                acquire_wait_us: acquire.push(14.0),
                ..cupid_signals(frame)
            });
            if tail.depth() == 4 {
                break;
            }
        }
        // Two frames for the EWMA to climb past period/2 (3.50 ms, then
        // 6.13 ms against a 4.17 ms threshold), a third to confirm it, and the
        // depth applies whole on that tick. The as-merged task-12 constants took
        // **twelve** frames for the same signal (6 more confirm ticks + a
        // 4-frame ramp) — ~100 ms at 120 Hz, which is the onset desync Ed saw.
        assert_eq!(
            onset_frames, 3,
            "onset must not cost a whole confirm window"
        );
        assert_eq!(tail.depth(), 4);
    }

    #[test]
    fn a_single_acquire_spike_cannot_arm_a_submit_bound_device() {
        let mut tail = ScrollSyncTail::new();
        let mut acquire = AcquireEwma::idle();
        // OP9-shaped: ~0.12 ms acquire, with a one-frame 20 ms stall dropped in
        // every twelfth frame (a compositor hiccup, not a queue). The EWMA jumps
        // above the enter threshold on the spike's own tick and falls back below
        // it on the next, so the two-tick entry window is never satisfied —
        // which is what makes rung 1's short window safe.
        for frame in 1..=240 {
            let sample_ms = if frame % 12 == 0 { 20.0 } else { 0.12 };
            tail.tick(TailSignals {
                acquire_wait_us: acquire.push(sample_ms),
                ..op9_signals(frame)
            });
            assert!(
                !tail.regime_active(),
                "a spike is not a queue (frame {frame})"
            );
            assert_eq!(tail.depth(), 0);
        }
    }

    #[test]
    fn a_mid_gesture_depth_change_still_ramps() {
        let mut tail = ScrollSyncTail::new();
        run_ticks(&mut tail, 40, cupid_signals);
        assert_eq!(tail.depth(), 4);
        // The timeline delta shortens mid-fling (58 ms -> depth 6 -> 4 again).
        // A live hold is stepped one frame per tick in both directions: the seed
        // arm is the entry edge only, never a running adjustment.
        for expected in [5, 6] {
            tail.tick(TailSignals {
                expected_present_delta_nanos: 54_000_000,
                ..cupid_signals(41)
            });
            assert_eq!(tail.depth(), expected);
        }
        for expected in [5, 4] {
            tail.tick(cupid_signals(42));
            assert_eq!(tail.depth(), expected);
        }
    }

    #[test]
    fn tick_reports_a_trace_only_on_depth_or_regime_changes() {
        let mut tail = ScrollSyncTail::new();
        let mut traces = Vec::new();
        // 40 ticks stays well inside `DIAGNOSTIC_INTERVAL_DISPLAY_FRAMES`, so
        // this only exercises the change-triggered path — the periodic
        // fallback is covered separately below.
        for frame in 1..=40 {
            if let Some(trace) = tail.tick(cupid_signals(frame)) {
                traces.push(trace);
            }
        }
        // Exactly one change over a steady gesture: the latch, which also seeds
        // the depth. Its `display_frame` is what an onset measurement subtracts
        // the gesture's `Down` frame from.
        assert_eq!(
            traces,
            vec![TailTrace {
                depth: 4,
                target_depth: 4,
                acquire_bound: true,
                display_frame: u64::from(REGIME_ENTER_CONFIRM_FRAMES),
            }]
        );
    }

    #[test]
    fn a_steady_state_still_gets_a_periodic_diagnostic_after_the_latch() {
        let mut tail = ScrollSyncTail::new();
        let mut trace_frames = Vec::new();
        for frame in 1..=(DIAGNOSTIC_INTERVAL_DISPLAY_FRAMES as i64 * 3) {
            if let Some(trace) = tail.tick(cupid_signals(frame)) {
                trace_frames.push(trace.display_frame);
            }
        }
        // One trace for the latch itself (resets the interval clock), then one
        // every bounded interval thereafter — three elapsed intervals means two
        // more beyond the latch.
        assert_eq!(
            trace_frames.len(),
            3,
            "latch + two periodic snapshots over three elapsed intervals: {trace_frames:?}"
        );
        for pair in trace_frames.windows(2) {
            assert!(
                pair[1] - pair[0] >= DIAGNOSTIC_INTERVAL_DISPLAY_FRAMES,
                "no two diagnostics closer than the bounded interval: {trace_frames:?}"
            );
        }
    }

    #[test]
    fn a_device_whose_regime_never_latches_still_gets_periodic_snapshots() {
        // The exact gap g2 exists to close: a change-only trace is silent for
        // the whole session here, which previously meant no on-device signal
        // existed to answer "why didn't it latch" without a throwaway build.
        let mut tail = ScrollSyncTail::new();
        let mut trace_count = 0;
        for frame in 1..=(DIAGNOSTIC_INTERVAL_DISPLAY_FRAMES as i64 * 2) {
            if tail.tick(op9_signals(frame)).is_some() {
                trace_count += 1;
            }
        }
        assert!(!tail.regime_active(), "OP9-shaped signal never latches");
        assert_eq!(tail.depth(), 0);
        assert_eq!(
            trace_count, 2,
            "one periodic snapshot per elapsed interval, no change required"
        );
    }

    #[test]
    fn hysteresis_holds_the_regime_across_a_wandering_ewma() {
        let mut tail = ScrollSyncTail::new();
        run_ticks(&mut tail, 40, cupid_signals);
        assert!(tail.regime_active());
        assert_eq!(tail.depth(), 4);

        // The EWMA dips into the hysteresis band (between exit and enter
        // thresholds: 2.1-4.2 ms at 120 Hz) for a long stretch — no flip.
        for frame in 41..=120 {
            tail.tick(TailSignals {
                acquire_wait_us: if frame % 2 == 0 { 3_000 } else { 4_000 },
                ..cupid_signals(frame)
            });
        }
        assert!(tail.regime_active(), "the band is not an exit");
        assert_eq!(tail.depth(), 4, "so the depth never flapped");
    }

    #[test]
    fn a_single_outlier_tick_cannot_flip_the_regime() {
        let mut tail = ScrollSyncTail::new();
        run_ticks(&mut tail, 40, cupid_signals);
        assert!(tail.regime_active());
        for frame in 41..=120 {
            // One below-exit outlier every few frames: the confirm window never
            // fills, so the regime holds.
            let acquire_wait_us = if frame % 4 == 0 { 50 } else { 14_000 };
            tail.tick(TailSignals {
                acquire_wait_us,
                ..cupid_signals(frame)
            });
        }
        assert!(tail.regime_active());
        assert_eq!(tail.depth(), 4);
    }

    #[test]
    fn no_timeline_sample_is_gate_only_even_when_acquire_bound() {
        let mut tail = ScrollSyncTail::new();
        // Below API 33 (or nothing hosted, so Kotlin never samples): the regime
        // still latches, but the depth has nothing to derive from.
        run_ticks(&mut tail, 60, |frame| TailSignals {
            expected_present_delta_nanos: 0,
            ..cupid_signals(frame)
        });
        assert!(tail.regime_active());
        assert_eq!(tail.depth(), 0);
        assert_eq!(tail.releasable(3), 3);
    }

    #[test]
    fn an_implausible_timeline_sample_is_gate_only() {
        let mut tail = ScrollSyncTail::new();
        run_ticks(&mut tail, 60, |frame| TailSignals {
            expected_present_delta_nanos: 500_000_000, // 500 ms — not a timeline
            ..cupid_signals(frame)
        });
        assert!(tail.regime_active());
        assert_eq!(tail.depth(), 0);
    }

    #[test]
    fn the_depth_is_capped() {
        let mut tail = ScrollSyncTail::new();
        run_ticks(&mut tail, 200, |frame| TailSignals {
            expected_present_delta_nanos: 99_000_000, // 99 ms / 8.33 = ~11
            ..cupid_signals(frame)
        });
        assert_eq!(tail.depth(), MAX_TAIL_DEPTH);
    }

    #[test]
    fn a_stalled_tick_delta_does_not_poison_the_period() {
        let mut tail = ScrollSyncTail::new();
        run_ticks(&mut tail, 40, cupid_signals);
        assert_eq!(tail.depth(), 4);
        // A 900 ms gap (app paused, long stall): implausible as a vsync cadence,
        // so the period estimate — and therefore the depth — is unchanged.
        tail.tick(TailSignals {
            frame_time_nanos: 40 * PERIOD_NANOS + 900_000_000,
            ..cupid_signals(41)
        });
        assert_eq!(tail.depth(), 4);
    }

    #[test]
    fn a_60hz_device_derives_a_shallower_depth() {
        let mut tail = ScrollSyncTail::new();
        // Same 37.7 ms landing time on a 60 Hz panel is half as many frames:
        // (37.7 - 4) / 16.67 = 2.02 -> 2. The depth follows the period, not a
        // per-device constant.
        run_ticks(&mut tail, 200, |frame| TailSignals {
            frame_time_nanos: frame * 16_666_666,
            ..cupid_signals(frame)
        });
        assert!(tail.regime_active());
        assert_eq!(tail.depth(), 2);
    }

    #[test]
    fn holds_stay_bounded_under_a_poll_that_never_ticks() {
        let mut tail = ScrollSyncTail::new();
        run_ticks(&mut tail, 40, cupid_signals);
        for generation in 1..=(MAX_TRACKED_HOLDS as u64 * 3) {
            tail.releasable(generation);
        }
        assert!(tail.holds.len() <= MAX_TRACKED_HOLDS + 1);
    }
}
