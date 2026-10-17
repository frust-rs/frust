//! Pure, deterministic gesture helpers: slop/wheel constants, a trailing-window
//! [`VelocityTracker`] and fling-decay math (cross-checked against masonry's).
//! Times are logical ms; positions and velocities are logical px and px/s.

use std::collections::VecDeque;

/// Press-to-drag threshold for touch (Flutter's tap-vs-drag slop, 18 px).
pub const TOUCH_SLOP: f64 = 18.0;

/// Mouse drag threshold; unused until events carry an input kind, so widgets
/// use [`TOUCH_SLOP`] for every pointer.
pub const MOUSE_SLOP: f64 = 3.0;

/// Logical px per wheel `LineDelta` line (a common cross-toolkit value).
pub const WHEEL_LINE_PX: f64 = 40.0;

/// Per-ms fling decay (iOS "normal" deceleration): `v(t) = v0 · FLING_DECAY^t`.
pub const FLING_DECAY: f64 = 0.998;

/// Speed in px/s (~0.5 px/frame at 60 Hz) below which a fling stops.
pub const FLING_STOP: f64 = 30.0;

/// The trailing window [`VelocityTracker`] keeps samples for.
pub const VELOCITY_WINDOW_MS: f64 = 100.0;

/// `v0` after `elapsed_ms` of decay; halves in ≈347 ms.
pub fn fling_decay(v0: f64, elapsed_ms: f64) -> f64 {
    v0 * FLING_DECAY.powf(elapsed_ms)
}

/// Distance in px a fling of `v0` px/s covers in `elapsed_ms`: the exact
/// integral of [`fling_decay`], `(v0/1000) · (FLING_DECAY^T − 1) / ln(FLING_DECAY)`.
pub fn fling_displacement(v0: f64, elapsed_ms: f64) -> f64 {
    (v0 / 1000.0) * (FLING_DECAY.powf(elapsed_ms) - 1.0) / FLING_DECAY.ln()
}

/// Recent `(time_ms, position)` samples for a drag's release velocity: a
/// delta-over-window estimate, not Flutter's least-squares fit.
#[derive(Clone, Debug, Default)]
pub struct VelocityTracker {
    samples: VecDeque<(f64, f64)>,
}

impl VelocityTracker {
    pub fn new() -> Self {
        Self {
            samples: VecDeque::new(),
        }
    }

    /// Drop all samples; call on the `Down` that starts a gesture.
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Record a sample, pruning those older than the window.
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

    /// Velocity in px/s across the window; `0.0` with under two samples or no span.
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

    pub fn len(&self) -> usize {
        self.samples.len()
    }

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
