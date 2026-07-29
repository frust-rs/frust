//! Pure input/gesture helpers: slop/wheel constants, a trailing-window
//! [`VelocityTracker`], and the fling-decay math (numbers cross-checked
//! against masonry's implementation).
//!
//! Everything here is deterministic and dependency-free so it is exercised
//! entirely by unit tests — the interactive widgets layer their
//! event/paint behaviour on top of these primitives. Times are **logical
//! milliseconds** and positions/velocities are **logical pixels** (px, px/s) to
//! match the density-independent coordinate space events arrive in.

use std::collections::VecDeque;

/// Touch drag threshold: how far a contact may move before a press becomes a
/// drag/scroll (Flutter's tap-vs-drag slop, 18 logical px).
pub const TOUCH_SLOP: f64 = 18.0;

/// Mouse drag threshold (kept smaller than [`TOUCH_SLOP`] because a mouse is far
/// more precise). Reserved for when events carry an input-kind flag; v1 widgets
/// use [`TOUCH_SLOP`] uniformly.
pub const MOUSE_SLOP: f64 = 3.0;

/// Logical pixels per wheel *line* — the constant a `LineDelta` wheel notch is
/// multiplied by to get a pixel scroll amount (a sane cross-toolkit value).
pub const WHEEL_LINE_PX: f64 = 40.0;

/// Per-millisecond exponential fling decay factor (iOS "normal" deceleration):
/// `v(t) = v0 · FLING_DECAY.powf(t_ms)`.
pub const FLING_DECAY: f64 = 0.998;

/// Fling stop threshold, in px/s (~0.5 px/frame at 60 Hz): below this speed a
/// fling is finished and the animation stops requesting frames.
pub const FLING_STOP: f64 = 30.0;

/// Trailing window, in milliseconds, over which [`VelocityTracker`] estimates
/// velocity — older samples are pruned.
pub const VELOCITY_WINDOW_MS: f64 = 100.0;

/// Velocity of `v0` after `elapsed_ms` of exponential fling decay.
///
/// `v(t) = v0 · FLING_DECAY^t` with `t` in milliseconds. Halves in ≈347 ms
/// (`ln 0.5 / ln 0.998`).
pub fn fling_decay(v0: f64, elapsed_ms: f64) -> f64 {
    v0 * FLING_DECAY.powf(elapsed_ms)
}

/// Closed-form displacement (in px) travelled by a fling of initial velocity
/// `v0` (px/s) over `elapsed_ms` milliseconds.
///
/// This is the exact integral of [`fling_decay`] over `[0, elapsed_ms]`:
/// `x(T) = (v0/1000) · (FLING_DECAY^T − 1) / ln(FLING_DECAY)`. The `/1000`
/// converts the px/s velocity into px given the millisecond time base; the
/// result matches a numeric integration of [`fling_decay`] to well within 1%.
pub fn fling_displacement(v0: f64, elapsed_ms: f64) -> f64 {
    (v0 / 1000.0) * (FLING_DECAY.powf(elapsed_ms) - 1.0) / FLING_DECAY.ln()
}

/// A ring of recent `(time_ms, position)` samples used to estimate the release
/// velocity of a drag for flinging.
///
/// Samples older than [`VELOCITY_WINDOW_MS`] (relative to the newest) are
/// pruned on [`record`](Self::record); [`velocity`](Self::velocity) is a simple
/// delta-over-window estimate (px/s) — enough for v1 fling, without Flutter's
/// least-squares regression.
#[derive(Clone, Debug, Default)]
pub struct VelocityTracker {
    samples: VecDeque<(f64, f64)>,
}

impl VelocityTracker {
    /// An empty tracker.
    pub fn new() -> Self {
        Self {
            samples: VecDeque::new(),
        }
    }

    /// Drop all samples (call on the `Down` that begins a fresh gesture).
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Record a `(time_ms, position)` sample, pruning any now older than the
    /// trailing window.
    pub fn record(&mut self, time_ms: f64, position: f64) {
        self.samples.push_back((time_ms, position));
        let cutoff = time_ms - VELOCITY_WINDOW_MS;
        while let Some(&(t, _)) = self.samples.front() {
            if t < cutoff {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    /// Estimate the current velocity in px/s from the delta across the retained
    /// window. Returns `0.0` with fewer than two samples or a zero time span.
    pub fn velocity(&self) -> f64 {
        if self.samples.len() < 2 {
            return 0.0;
        }
        let (t0, p0) = *self.samples.front().unwrap();
        let (t1, p1) = *self.samples.back().unwrap();
        let dt = t1 - t0;
        if dt <= 0.0 {
            return 0.0;
        }
        (p1 - p0) / dt * 1000.0
    }

    /// The number of retained samples (for tests/introspection).
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether the tracker holds no samples.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fling_decay_halves_in_about_347ms() {
        let v = fling_decay(1000.0, 347.0);
        assert!((v - 500.0).abs() < 1.0, "expected ~500, got {v}");
        // Exactly at t=0 the velocity is unchanged.
        assert_eq!(fling_decay(1000.0, 0.0), 1000.0);
    }

    #[test]
    fn fling_displacement_matches_numeric_integration() {
        let v0 = 800.0;
        let total_ms = 500.0;
        // Numeric integration of fling_decay over [0, total_ms] in 1 ms steps.
        let mut numeric = 0.0;
        let step = 1.0;
        let mut t = 0.0;
        while t < total_ms {
            numeric += fling_decay(v0, t) / 1000.0 * step;
            t += step;
        }
        let closed = fling_displacement(v0, total_ms);
        let rel_err = (closed - numeric).abs() / numeric.abs();
        assert!(rel_err < 0.01, "closed {closed} vs numeric {numeric}");
        assert!(closed > 0.0);
    }

    #[test]
    fn fling_displacement_sign_follows_velocity() {
        assert!(fling_displacement(500.0, 100.0) > 0.0);
        assert!(fling_displacement(-500.0, 100.0) < 0.0);
        assert_eq!(fling_displacement(0.0, 100.0), 0.0);
    }

    #[test]
    fn velocity_is_delta_over_window() {
        let mut vt = VelocityTracker::new();
        vt.record(0.0, 100.0);
        vt.record(50.0, 50.0); // moved -50 px in 50 ms → -1000 px/s
        assert!((vt.velocity() - (-1000.0)).abs() < 1e-9);
    }

    #[test]
    fn velocity_empty_or_single_sample_is_zero() {
        let mut vt = VelocityTracker::new();
        assert_eq!(vt.velocity(), 0.0);
        vt.record(0.0, 10.0);
        assert_eq!(vt.velocity(), 0.0);
        assert!(!vt.is_empty());
    }

    #[test]
    fn old_samples_are_pruned_from_the_window() {
        let mut vt = VelocityTracker::new();
        vt.record(0.0, 0.0);
        vt.record(50.0, 10.0);
        // 200 ms is > VELOCITY_WINDOW_MS past the first two samples.
        vt.record(200.0, 40.0);
        // Only samples within [100, 200] survive → just the last one, plus any
        // at/after the cutoff. The t=0 and t=50 samples are pruned.
        assert_eq!(vt.len(), 1);
        assert_eq!(vt.velocity(), 0.0); // single surviving sample
    }

    #[test]
    fn clear_resets_the_tracker() {
        let mut vt = VelocityTracker::new();
        vt.record(0.0, 1.0);
        vt.record(10.0, 2.0);
        vt.clear();
        assert!(vt.is_empty());
        assert_eq!(vt.velocity(), 0.0);
    }
}
