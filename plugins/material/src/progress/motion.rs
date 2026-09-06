//! The progress family's indeterminate choreography: the five animation
//! controllers upstream runs, the two-segment linear curve table, the circular
//! rotation/sweep resolution, and the shared wave-phase clock. Nothing here
//! touches a `PaintScene`, a `Theme`, or a callback — [`super`] owns every
//! paint call, [`super::linear`]/[`super::circular`] own the geometry these
//! numbers feed.
//!
//! Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
//! `lib/components/progress_indicators/` — `m3e_progress_indicators.dart`
//! (`_M3EProgressIndicatorState`'s five controllers, `_pingPongSweep`,
//! `_phase`, `_amplitudeFactor`, `_resolveClassicArc`),
//! `utils/m3e_progress_indicator_utils.dart`
//! (`evaluateIndeterminateSegment`),
//! `components/m3e_linear_progress_painter.dart` (`_lineEasing`,
//! `_indetSegments`),
//! `components/m3e_circular_wavy_progress_painter.dart` (`minSweep`,
//! `maxSweep`, `globalRotDeg`, `additionalRotDeg`),
//! `styles/m3e_progress_indicator_theme.dart` (`amplitudeForProgress`)
//! (retrieved 2026-08-19).
//! Upstream: <https://github.com/paadevelopments/material_3_expressive>
//!
//! # Five controllers, not one
//!
//! Upstream runs the indeterminate motion off five independent
//! `AnimationController`s rather than one shared timeline, and this port keeps
//! that shape one-to-one:
//!
//! | Reference controller | Period | Here |
//! |---|---|---|
//! | `_linearIndetController` | 1750 ms, repeating | [`LinearClock`] |
//! | `_globalRotController` | 6000 ms, repeating | [`CircularClock`]'s `global` |
//! | `_additionalRotController` | 4500 ms, repeating | [`CircularClock`]'s `additional` |
//! | `_sweepController` | 1300 ms, ping-ponged | [`CircularClock`]'s `sweep` |
//! | `_spinController` | `extraLong2` (800 ms), repeating | [`WaveClock`] |
//!
//! Each is a real repeating [`AnimationController`] advanced from the frame
//! clock (`docs/WIDGETS_CODE_STANDARDS.md`'s advance-during-paint rule),
//! never a stack of implicit animations: the whole point of the choreography
//! is that the four linear segment fractions and the two circular rotations
//! read *different* clocks, so no single controller could express it.
//!
//! # The wave phase never gates on progress
//!
//! Upstream's `_needsWavePhase` is `_isWavy` alone and `_syncSpinController`
//! repeats the phase controller for **every** wavy indicator, determinate or
//! not, at any value — a determinate wavy indicator at `value = 1.0` still
//! advances its phase. What actually flattens the wave near either end of the
//! track is the *amplitude*, not the phase: [`amplitude_ramp`]
//! (`amplitudeForProgress`) returns `0` below `0.1` and at/above `0.95`. That
//! is the rule pinned here.
//!
//! The one divergence, documented at [`super`]'s paint: this port skips the
//! phase advance (and its frame request) when the resolved amplitude is `0`,
//! since a zero-amplitude wave is a flat line whose phase is unobservable —
//! a perpetual repaint for no visible motion is the thing the paced-frame
//! convention exists to avoid.

use std::f64::consts::TAU;
use std::time::Duration;

use frust::{AnimationController, Curve, FrameTime};

use crate::tokens::MaterialMotion;

/// One full linear indeterminate cycle, in ms — the `totalDurationMs`
/// every segment's delay/duration is expressed against
/// (`evaluateIndeterminateSegment`).
const CYCLE_MS: u64 = 1750;

/// Period of the linear indeterminate cycle (`_linearIndetController`).
pub(crate) const LINEAR_CYCLE: Duration = Duration::from_millis(CYCLE_MS);
/// Period of the circular indeterminate global rotation
/// (`_globalRotController`).
pub(crate) const GLOBAL_ROTATION: Duration = Duration::from_millis(6000);
/// Period of the circular indeterminate additional rotation
/// (`_additionalRotController`).
pub(crate) const ADDITIONAL_ROTATION: Duration = Duration::from_millis(4500);
/// Period of one leg of the circular indeterminate sweep ping-pong
/// (`_sweepController`).
pub(crate) const SWEEP: Duration = Duration::from_millis(1300);
/// Period of the wave-phase clock (`_spinController`'s `M3EMotion.extraLong2`).
/// Only the rate the clock is sampled at, never a visible cycle: [`WaveClock`]
/// accumulates across wraps, so the phase never jumps at a period boundary.
pub(crate) const WAVE_PHASE: Duration = MaterialMotion::EXTRA_LONG_2;

/// Easing applied to every linear indeterminate segment
/// (`M3ELinearProgressPainter._lineEasing`, `Cubic(0.3, 0, 0.8, 0.15)`).
pub(crate) const LINE_EASING: Curve = Curve::Cubic(0.3, 0.0, 0.8, 0.15);

/// Minimum circular indeterminate sweep, as a fraction of the full circle
/// (`M3ECircularWavyProgressPainter.minSweep`).
pub(crate) const MIN_SWEEP: f64 = 0.10;
/// Maximum circular indeterminate sweep, as a fraction of the full circle
/// (`M3ECircularWavyProgressPainter.maxSweep`).
pub(crate) const MAX_SWEEP: f64 = 0.87;
/// Degrees of rotation per full global-rotation cycle
/// (`M3ECircularWavyProgressPainter.globalRotDeg`).
pub(crate) const GLOBAL_ROT_DEG: f64 = 1080.0;
/// Degrees of rotation per full additional-rotation cycle
/// (`M3ECircularWavyProgressPainter.additionalRotDeg`).
pub(crate) const ADDITIONAL_ROT_DEG: f64 = 360.0;

/// `(delay_ms, duration_ms)` of the first line's head (`_indetSegments`).
const FIRST_HEAD: (f64, f64) = (0.0, 1000.0);
/// `(delay_ms, duration_ms)` of the first line's tail.
const FIRST_TAIL: (f64, f64) = (250.0, 1000.0);
/// `(delay_ms, duration_ms)` of the second line's head.
const SECOND_HEAD: (f64, f64) = (650.0, 850.0);
/// `(delay_ms, duration_ms)` of the second line's tail.
const SECOND_TAIL: (f64, f64) = (900.0, 850.0);

/// An app-supplied amplitude factor as a function of progress
/// (`M3EProgressIndicator.amplitudeForProgress`). `Rc`, not `Box`, so a view
/// can be cloned into its widget the same way `crate::slider`'s
/// `AmplitudeCurve` is.
pub type AmplitudeCurve = std::rc::Rc<dyn Fn(f64) -> f64>;

/// The four traveling fractions of one linear indeterminate cycle, in the
/// reference's own naming (`_indetSegments`). Each is a position along the
/// track in `0..=1`; a *line* spans `tail..head`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Segments {
    /// Leading edge of the first (rightmost) line.
    pub(crate) first_head: f64,
    /// Trailing edge of the first line.
    pub(crate) first_tail: f64,
    /// Leading edge of the second (leftmost) line.
    pub(crate) second_head: f64,
    /// Trailing edge of the second line.
    pub(crate) second_tail: f64,
}

/// One timed segment of the linear indeterminate cycle at cycle progress `t`
/// (`M3EProgressIndicatorUtils.evaluateIndeterminateSegment`).
///
/// The reference's "before the window → `0`, after it → `1`, inside it →
/// `easing.transform(localT)`" is exactly [`Curve::interval`]'s contract, so
/// the window is expressed as a segmented curve rather than re-derived.
pub(crate) fn evaluate_segment(t: f64, delay_ms: f64, duration_ms: f64) -> f64 {
    let total = CYCLE_MS as f64;
    let start = delay_ms / total;
    let end = (delay_ms + duration_ms) / total;
    LINE_EASING.interval(start, end).transform(t)
}

/// Every segment fraction at cycle progress `t` (`_indetSegments`).
pub(crate) fn segments(t: f64) -> Segments {
    Segments {
        first_head: evaluate_segment(t, FIRST_HEAD.0, FIRST_HEAD.1),
        first_tail: evaluate_segment(t, FIRST_TAIL.0, FIRST_TAIL.1),
        second_head: evaluate_segment(t, SECOND_HEAD.0, SECOND_HEAD.1),
        second_tail: evaluate_segment(t, SECOND_TAIL.0, SECOND_TAIL.1),
    }
}

/// The linear indeterminate cycle clock (`_linearIndetController`): a plain
/// repeating controller whose `0..1` value is the `t` [`segments`] takes.
#[derive(Debug)]
pub(crate) struct LinearClock {
    timer: AnimationController,
}

impl LinearClock {
    /// A clock at cycle zero, already repeating.
    pub(crate) fn new() -> Self {
        let mut timer = AnimationController::new(LINEAR_CYCLE).with_curve(Curve::Linear);
        timer.repeat();
        Self { timer }
    }

    /// Advance to frame time `now`. A frozen (reduce-motion) pass simply does
    /// not call this.
    pub(crate) fn step(&mut self, now: FrameTime) {
        self.timer.advance(now);
    }

    /// The current cycle progress in `0..=1`.
    pub(crate) fn cycle(&self) -> f64 {
        self.timer.value_clamped()
    }

    /// The four segment fractions for this frame.
    pub(crate) fn segments(&self) -> Segments {
        segments(self.cycle())
    }
}

/// The circular indeterminate clock: two repeating rotations plus the
/// ping-ponged sweep (`_globalRotController`/`_additionalRotController`/
/// `_sweepController` and `_pingPongSweep`).
#[derive(Debug)]
pub(crate) struct CircularClock {
    global: AnimationController,
    additional: AnimationController,
    sweep: AnimationController,
    /// Which leg of the ping-pong the sweep is on (`_sweepExpanding`).
    expanding: bool,
}

impl CircularClock {
    /// A clock at rotation zero with the sweep starting its expanding leg —
    /// the state `_startCircularIndeterminate` leaves the three controllers in.
    pub(crate) fn new() -> Self {
        let mut global = AnimationController::new(GLOBAL_ROTATION).with_curve(Curve::Linear);
        global.repeat();
        let mut additional =
            AnimationController::new(ADDITIONAL_ROTATION).with_curve(Curve::Linear);
        additional.repeat();
        let mut sweep = AnimationController::new(SWEEP).with_curve(Curve::Linear);
        sweep.forward();
        Self {
            global,
            additional,
            sweep,
            expanding: true,
        }
    }

    /// Advance every controller to frame time `now`, flipping the sweep's leg
    /// when it lands on an endpoint.
    ///
    /// `_pingPongSweep` chains the flip off the completing controller's own
    /// future; here the completion shows up as `advance` returning `false`, so
    /// the flip happens inline and the freshly-started leg is re-seeded with
    /// the same timestamp (its first `advance` only seeds the clock).
    pub(crate) fn step(&mut self, now: FrameTime) {
        self.global.advance(now);
        self.additional.advance(now);
        if !self.sweep.advance(now) {
            self.expanding = !self.expanding;
            if self.expanding {
                self.sweep.forward();
            } else {
                self.sweep.reverse();
            }
            self.sweep.advance(now);
        }
    }

    /// Total canvas rotation for this frame, in radians
    /// (`_resolveClassicArc`/`_paintIndeterminate`'s `totalRad`).
    pub(crate) fn rotation(&self) -> f64 {
        let total_deg = self.global.value_clamped() * GLOBAL_ROT_DEG
            + self.additional.value_clamped() * ADDITIONAL_ROT_DEG;
        total_deg.to_radians()
    }

    /// The active arc's sweep for this frame, in radians — the reference's
    /// `lerpDouble(minSweep, maxSweep, _sweepController.value) * tau`, with
    /// `_paintIndeterminate`'s redundant re-clamp kept as a total-by-
    /// construction backstop.
    pub(crate) fn sweep(&self) -> f64 {
        let fraction = MIN_SWEEP + (MAX_SWEEP - MIN_SWEEP) * self.sweep.value_clamped();
        fraction.clamp(MIN_SWEEP, MAX_SWEEP) * TAU
    }
}

/// The wave-phase clock (`_spinController` read through
/// `lastElapsedDuration`).
///
/// [`AnimationController`] exposes only a wrapping `0..1` phase within the
/// current period, so this counts wraps to recover the monotone elapsed-seconds
/// reading upstream's `lastElapsedDuration` accumulates — the same shape
/// [`crate::loading_indicator`] uses for its shape index, and it inherits the
/// same accepted limitation: a frame gap longer than one whole period presents
/// as a single wrap, so the phase jumps rather than catching up. The wave is
/// decorative and its phase arbitrary, so a jump is invisible; it also
/// self-corrects on the next frame.
#[derive(Debug)]
pub(crate) struct WaveClock {
    timer: AnimationController,
    /// Completed periods since the clock started.
    cycles: u64,
    /// Last sampled phase within the current period, `0..1`.
    phase: f64,
}

impl WaveClock {
    /// A clock at phase zero, already repeating.
    pub(crate) fn new() -> Self {
        let mut timer = AnimationController::new(WAVE_PHASE).with_curve(Curve::Linear);
        timer.repeat();
        Self {
            timer,
            cycles: 0,
            phase: 0.0,
        }
    }

    /// Advance to frame time `now`, counting a wrap when the period rolls over.
    pub(crate) fn step(&mut self, now: FrameTime) {
        self.timer.advance(now);
        let phase = self.timer.value_clamped();
        if phase < self.phase {
            self.cycles = self.cycles.saturating_add(1);
        }
        self.phase = phase;
    }

    /// Elapsed travel time, monotone across period wraps.
    pub(crate) fn seconds(&self) -> f64 {
        (self.cycles as f64 + self.phase) * WAVE_PHASE.as_secs_f64()
    }

    /// The wave phase in radians for the resolved knobs.
    pub(crate) fn radians(&self, wavelength: f64, wave_speed: f64) -> f64 {
        wave_phase(self.seconds(), wavelength, wave_speed)
    }
}

/// The wave's phase in radians after `seconds` of travel
/// (`_M3EProgressIndicatorState._phase`). A non-positive wavelength has no
/// wave at all, so its phase is `0`.
pub(crate) fn wave_phase(seconds: f64, wavelength: f64, wave_speed: f64) -> f64 {
    if wavelength <= 0.0 {
        return 0.0;
    }
    seconds * wave_speed / wavelength * TAU
}

/// The default amplitude factor for a given progress
/// (`M3ELinearProgressTheme`/`M3ECircularProgressTheme.amplitudeForProgress`,
/// which ship the identical ramp): full amplitude through the middle of the
/// track, flat near either end so the wave never fights the round caps.
pub(crate) fn amplitude_ramp(progress: f64) -> f64 {
    if progress <= 0.1 || progress >= 0.95 {
        0.0
    } else {
        1.0
    }
}

/// The amplitude factor for one paint pass, in the reference's own precedence
/// order (`_M3EProgressIndicatorState._amplitudeFactor`).
///
/// `progress` is `None` for an indeterminate indicator, where upstream takes
/// the app-supplied constant or `1` and consults neither the curve nor the
/// ramp. Always clamped to `0..=1`: upstream leaves the indeterminate arm
/// unclamped and re-clamps inside each painter instead, which the wavy linear
/// *container height* is the one place that can observe — clamping once here
/// keeps the height bounded too.
pub(crate) fn amplitude_factor(
    progress: Option<f64>,
    fixed: Option<f64>,
    for_progress: Option<&dyn Fn(f64) -> f64>,
) -> f64 {
    let raw = match progress {
        None => fixed.unwrap_or(1.0),
        Some(p) => match (for_progress, fixed) {
            (Some(curve), _) => curve(p),
            (None, Some(a)) => a,
            (None, None) => amplitude_ramp(p),
        },
    };
    if raw.is_nan() {
        0.0
    } else {
        raw.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(t: f64) -> FrameTime {
        FrameTime::from_nanos((t * 1e9) as u64)
    }

    // -- the transcribed curve table --------------------------------------

    #[test]
    fn a_segment_is_zero_before_its_window_and_one_after_it() {
        // `secondHead` is delay 650 / duration 850 against a 1750 ms cycle, so
        // its window is 650/1750 ..= 1500/1750.
        let before = 649.0 / 1750.0;
        let after = 1501.0 / 1750.0;
        assert_eq!(segments(before).second_head, 0.0);
        assert_eq!(segments(after).second_head, 1.0);
        // `secondTail` (delay 900 / duration 850) closes exactly at the cycle
        // end, so it is still short of 1 just before it.
        assert!(segments(1749.0 / 1750.0).second_tail < 1.0);
        assert_eq!(segments(1.0).second_tail, 1.0);
    }

    #[test]
    fn every_segment_starts_at_zero_and_ends_at_one() {
        let start = segments(0.0);
        assert_eq!(
            (
                start.first_head,
                start.first_tail,
                start.second_head,
                start.second_tail
            ),
            (0.0, 0.0, 0.0, 0.0)
        );
        let end = segments(1.0);
        assert_eq!(
            (
                end.first_head,
                end.first_tail,
                end.second_head,
                end.second_tail
            ),
            (1.0, 1.0, 1.0, 1.0)
        );
    }

    #[test]
    fn a_head_never_trails_its_own_tail() {
        // The first line spans `firstTail..firstHead`, the second
        // `secondTail..secondHead`; a negative span would paint backwards.
        for i in 0..=175 {
            let t = i as f64 / 175.0;
            let s = segments(t);
            assert!(
                s.first_head >= s.first_tail,
                "first line inverted at t = {t}"
            );
            assert!(
                s.second_head >= s.second_tail,
                "second line inverted at t = {t}"
            );
        }
    }

    #[test]
    fn the_first_head_matches_the_line_easing_at_its_window_midpoint() {
        // Half-way through `firstHead`'s 1000 ms window (t = 500/1750 = 2/7),
        // `Cubic(0.3, 0, 0.8, 0.15)` evaluated at local time 0.5 is ~0.1539 —
        // solved independently from the CSS cubic-bezier definition
        // (X(s) = 0.5 at s ~ 0.46672; Y(s) = (0.55s + 0.45)s^2).
        let t = 500.0 / 1750.0;
        let head = segments(t).first_head;
        assert!(
            (head - 0.1539).abs() < 1e-3,
            "first_head at the window midpoint was {head}"
        );
    }

    #[test]
    fn the_two_lines_are_offset_by_their_delay_table() {
        // At t = 650/1750 the second line has not started at all while the
        // first is already well under way — the visible two-segment stagger.
        let s = segments(650.0 / 1750.0);
        assert_eq!(s.second_head, 0.0);
        assert_eq!(s.second_tail, 0.0);
        assert!(s.first_head > 0.0);
        assert!(s.first_tail > 0.0);
    }

    // -- clocks -------------------------------------------------------------

    #[test]
    fn the_linear_clock_runs_one_cycle_per_1750ms() {
        let mut clock = LinearClock::new();
        clock.step(secs(0.0));
        clock.step(secs(0.875));
        assert!((clock.cycle() - 0.5).abs() < 1e-9);
        clock.step(secs(1.75));
        assert!(clock.cycle() < 1e-9, "the cycle wraps at its period");
    }

    #[test]
    fn the_circular_clock_pins_rotation_and_sweep_at_a_fixed_timestamp() {
        let mut clock = CircularClock::new();
        clock.step(secs(0.0));
        clock.step(secs(0.65));
        // global: 0.65/6 of 1080 deg = 117 deg; additional: 0.65/4.5 of 360 deg
        // = 52 deg; total 169 deg.
        assert!((clock.rotation() - 169.0_f64.to_radians()).abs() < 1e-9);
        // sweep: half-way through its 1300 ms expanding leg, so
        // lerp(0.10, 0.87, 0.5) = 0.485 of a full turn.
        assert!((clock.sweep() - 0.485 * TAU).abs() < 1e-9);
    }

    #[test]
    fn the_circular_sweep_ping_pongs_between_its_bounds() {
        let mut clock = CircularClock::new();
        clock.step(secs(0.0));
        clock.step(secs(1.3));
        // The expanding leg completed: at its maximum, now flipped to contract.
        assert!((clock.sweep() - MAX_SWEEP * TAU).abs() < 1e-9);
        assert!(!clock.expanding);
        clock.step(secs(1.95));
        assert!(
            (clock.sweep() - 0.485 * TAU).abs() < 1e-9,
            "half-way back down the contracting leg"
        );
        clock.step(secs(2.6));
        assert!((clock.sweep() - MIN_SWEEP * TAU).abs() < 1e-9);
        assert!(clock.expanding, "and flipped back to expanding");
    }

    #[test]
    fn the_wave_clock_accumulates_seconds_across_period_wraps() {
        let mut clock = WaveClock::new();
        clock.step(secs(0.0));
        clock.step(secs(0.4));
        assert!((clock.seconds() - 0.4).abs() < 1e-9);
        // Past one 800 ms period: the wrapped phase must not read as a rewind.
        for (t, expected) in [(1.0, 1.0), (1.4, 1.4), (1.8, 1.8), (2.4, 2.4)] {
            clock.step(secs(t));
            assert!(
                (clock.seconds() - expected).abs() < 1e-9,
                "elapsed must stay monotone at t = {t}, read {}",
                clock.seconds()
            );
        }
    }

    #[test]
    fn a_frame_gap_longer_than_a_period_costs_the_wave_clock_its_whole_cycles() {
        // The accepted limitation documented on `WaveClock`: at most one wrap
        // is observable per step, so a 2.5-period gap keeps only the fractional
        // remainder. Pinned so a future change to the wrap accounting is a
        // deliberate one.
        let mut clock = WaveClock::new();
        clock.step(secs(0.0));
        clock.step(secs(2.0));
        assert!(
            (clock.seconds() - 0.4).abs() < 1e-9,
            "read {}",
            clock.seconds()
        );
    }

    #[test]
    fn the_wave_phase_advances_one_turn_per_wavelength_of_travel() {
        // Default `waveSpeed` is the wavelength, i.e. one cycle per second.
        assert!((wave_phase(1.0, 20.0, 20.0) - TAU).abs() < 1e-9);
        assert!((wave_phase(0.5, 20.0, 20.0) - TAU / 2.0).abs() < 1e-9);
        // Double the speed, double the phase.
        assert!((wave_phase(1.0, 20.0, 40.0) - 2.0 * TAU).abs() < 1e-9);
        // A degenerate wavelength has no wave to advance.
        assert_eq!(wave_phase(1.0, 0.0, 20.0), 0.0);
    }

    // -- amplitude ----------------------------------------------------------

    #[test]
    fn the_amplitude_ramp_flattens_the_wave_near_both_ends() {
        assert_eq!(amplitude_ramp(0.0), 0.0);
        assert_eq!(amplitude_ramp(0.1), 0.0);
        assert_eq!(amplitude_ramp(0.11), 1.0);
        assert_eq!(amplitude_ramp(0.94), 1.0);
        assert_eq!(amplitude_ramp(0.95), 0.0);
        assert_eq!(amplitude_ramp(1.0), 0.0);
    }

    #[test]
    fn amplitude_precedence_is_curve_then_constant_then_ramp() {
        let curve = |_: f64| 0.25;
        assert_eq!(
            amplitude_factor(Some(0.5), Some(0.75), Some(&curve)),
            0.25,
            "an app-supplied curve outranks a constant"
        );
        assert_eq!(amplitude_factor(Some(0.5), Some(0.75), None), 0.75);
        assert_eq!(amplitude_factor(Some(0.5), None, None), 1.0);
        assert_eq!(amplitude_factor(Some(0.99), None, None), 0.0);
    }

    #[test]
    fn an_indeterminate_amplitude_is_the_constant_or_one() {
        let curve = |_: f64| 0.25;
        assert_eq!(
            amplitude_factor(None, None, Some(&curve)),
            1.0,
            "indeterminate consults neither the curve nor the ramp"
        );
        assert_eq!(amplitude_factor(None, Some(0.4), Some(&curve)), 0.4);
    }

    #[test]
    fn an_out_of_range_amplitude_is_clamped_not_propagated() {
        assert_eq!(amplitude_factor(Some(0.5), Some(9.0), None), 1.0);
        assert_eq!(amplitude_factor(Some(0.5), Some(-9.0), None), 0.0);
        assert_eq!(amplitude_factor(None, Some(f64::NAN), None), 0.0);
    }
}
