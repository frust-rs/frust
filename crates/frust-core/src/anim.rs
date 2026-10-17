//! Animation vocabulary: the shared frame clock ([`FrameTime`]) plus the pure
//! math widgets consume it through: easing [`Curve`]s, [`Tween`] interpolation,
//! analytic [`Spring`] dynamics, and the [`AnimationController`].
//!
//! # Contract: plain data + math, never a scheduler
//!
//! Nothing here reads a clock or registers a callback. [`FrameTime`] enters from
//! the shell and reaches widgets as [`crate::widget::PaintCtx::frame_time`]. A
//! widget animates during its own paint: it calls [`AnimationController::advance`],
//! reads [`AnimationController::value`], and while `advance` returns `true` calls
//! [`crate::widget::PaintCtx::request_frame`]. A controller never `advance`d
//! never moves.

use std::time::Duration;

use kurbo::{Point, Rect, Size};
use peniko::Color;

/// A point in time supplied by the shell, in monotonic nanoseconds.
///
/// The origin is arbitrary and shell-specific, so widgets may only *difference*
/// two `FrameTime`s (via [`FrameTime::saturating_sub`]), never interpret one
/// absolutely or compare times from different shells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameTime(u64);

impl FrameTime {
    /// The zero instant, also the "no time available" fallback for legacy/test paint paths.
    pub const ZERO: FrameTime = FrameTime(0);

    /// Build a `FrameTime` from a raw monotonic-nanosecond count.
    pub const fn from_nanos(nanos: u64) -> Self {
        FrameTime(nanos)
    }

    /// The raw monotonic-nanosecond count.
    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    /// This instant as fractional seconds from the (arbitrary) origin.
    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / 1_000_000_000.0
    }

    /// The non-negative [`Duration`] elapsed from `earlier` to `self`, saturating at
    /// zero for a non-monotonic clock (the basis of [`AnimationController::advance`]'s
    /// never-negative delta).
    pub fn saturating_sub(self, earlier: FrameTime) -> Duration {
        Duration::from_nanos(self.0.saturating_sub(earlier.0))
    }
}

/// An easing curve mapping normalized time `t ∈ [0, 1]` to eased progress `∈ [0, 1]`.
///
/// The named variants are the CSS timing functions plus Material 3's
/// `Emphasized`; [`Curve::Cubic`] is a general cubic-Bézier. `t` is clamped first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    /// `t` unchanged.
    Linear,
    /// CSS `ease-in` — `cubic-bezier(0.42, 0, 1, 1)`.
    EaseIn,
    /// CSS `ease-out` — `cubic-bezier(0, 0, 0.58, 1)`.
    EaseOut,
    /// CSS `ease-in-out` — `cubic-bezier(0.42, 0, 0.58, 1)`.
    EaseInOut,
    /// Material 3 emphasized easing — `cubic-bezier(0.2, 0.0, 0.0, 1.0)`.
    Emphasized,
    /// A cubic-Bézier with control points `(x1, y1)`, `(x2, y2)` (endpoints fixed at
    /// `(0, 0)`/`(1, 1)`), as CSS `cubic-bezier()`.
    Cubic(f64, f64, f64, f64),
}

impl Curve {
    /// Evaluate the eased progress at normalized time `t` (clamped to `[0, 1]`).
    pub fn transform(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Curve::Linear => t,
            Curve::EaseIn => cubic_bezier(0.42, 0.0, 1.0, 1.0, t),
            Curve::EaseOut => cubic_bezier(0.0, 0.0, 0.58, 1.0, t),
            Curve::EaseInOut => cubic_bezier(0.42, 0.0, 0.58, 1.0, t),
            Curve::Emphasized => cubic_bezier(0.2, 0.0, 0.0, 1.0, t),
            Curve::Cubic(x1, y1, x2, y2) => cubic_bezier(x1, y1, x2, y2, t),
        }
    }

    /// Confine this curve to the `[start, end]` fraction of the timeline (Flutter's
    /// `Interval`): `0.0` before `start`, `1.0` from `end`, and `self` re-evaluated
    /// at the rescaled local time between. An interval with `end <= start` is an
    /// instantaneous step at `start`.
    ///
    /// Returns a [`SegmentedCurve`] rather than `Curve` because an `Interval`
    /// variant holding a `Curve` by value would be infinitely sized, and
    /// [`AnimationController`] relies on `Curve: Copy`.
    pub fn interval(self, start: f64, end: f64) -> SegmentedCurve {
        SegmentedCurve {
            start,
            end,
            inner: self,
        }
    }
}

/// A [`Curve`] confined to a `[start, end]` sub-interval; see [`Curve::interval`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentedCurve {
    start: f64,
    end: f64,
    inner: Curve,
}

impl SegmentedCurve {
    /// Evaluate at normalized time `t` (clamped to `[0, 1]`); see [`Curve::interval`].
    pub fn transform(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        if self.end <= self.start {
            // Degenerate/inverted interval: an instantaneous step at `start`.
            return if t < self.start { 0.0 } else { 1.0 };
        }
        if t <= self.start {
            0.0
        } else if t >= self.end {
            1.0
        } else {
            let local = (t - self.start) / (self.end - self.start);
            self.inner.transform(local)
        }
    }
}

/// Evaluate a cubic-Bézier timing function (control points `(0, 0)`, `(x1, y1)`,
/// `(x2, y2)`, `(1, 1)`) at `x ∈ [0, 1]`: solve `X(s) = x` by Newton's method
/// with a bisection fallback, and return `Y(s)`.
fn cubic_bezier(x1: f64, y1: f64, x2: f64, y2: f64, x: f64) -> f64 {
    // Polynomial coefficients of X(s) and Y(s) = ((a·s + b)·s + c)·s.
    let cx = 3.0 * x1;
    let bx = 3.0 * (x2 - x1) - cx;
    let ax = 1.0 - cx - bx;
    let cy = 3.0 * y1;
    let by = 3.0 * (y2 - y1) - cy;
    let ay = 1.0 - cy - by;

    let sample_x = |s: f64| ((ax * s + bx) * s + cx) * s;
    let sample_y = |s: f64| ((ay * s + by) * s + cy) * s;
    let sample_dx = |s: f64| (3.0 * ax * s + 2.0 * bx) * s + cx;

    // Newton–Raphson from `x` (X is near identity for sane timing functions).
    let mut s = x;
    for _ in 0..8 {
        let err = sample_x(s) - x;
        if err.abs() < 1e-9 {
            return sample_y(s);
        }
        let d = sample_dx(s);
        if d.abs() < 1e-9 {
            break;
        }
        s -= err / d;
    }

    // Bisection fallback (converges on the monotone `X` of a valid timing function).
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    let mut s = x.clamp(lo, hi);
    for _ in 0..32 {
        let cur = sample_x(s);
        if (cur - x).abs() < 1e-9 {
            break;
        }
        if cur < x {
            lo = s;
        } else {
            hi = s;
        }
        s = 0.5 * (lo + hi);
    }
    sample_y(s)
}

/// Per-item timing for a staggered reveal: item `i` starts at `i * per_item_delay`
/// and runs for `item_duration`, all off one shared `0.0..=1.0` controller value.
///
/// The two fields share whatever time unit the caller picks, consistently with
/// the `Duration` of the controller sized from [`total_duration`](Self::total_duration).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StaggerSpec {
    /// Time between the start of consecutive items' sub-animations.
    pub per_item_delay: f64,
    /// Duration of each item's own sub-animation.
    pub item_duration: f64,
    /// Easing curve applied within each item's local `[0, 1]` sub-progress.
    pub item_curve: Curve,
}

impl StaggerSpec {
    /// The timeline length spanning all `n` items, `(n - 1) * per_item_delay +
    /// item_duration` (`0.0` for `n == 0`); size the controller's duration from it.
    pub fn total_duration(&self, n: usize) -> f64 {
        if n == 0 {
            return 0.0;
        }
        (n as f64 - 1.0) * self.per_item_delay + self.item_duration
    }

    /// Item `i`'s eased progress in `[0, 1]` for the overall controller value
    /// `overall` (clamped, linear across [`total_duration`](Self::total_duration)`(n)`).
    ///
    /// `0.0` before the item's window, `1.0` after, `item_curve` within. A zero
    /// `item_duration` is a step at the item's start; `n == 0` or `i >= n` returns
    /// `1.0` (nothing left to reveal).
    pub fn item_progress(&self, overall: f64, i: usize, n: usize) -> f64 {
        let total = self.total_duration(n);
        if total <= 0.0 || i >= n {
            return 1.0;
        }
        let overall = overall.clamp(0.0, 1.0);
        let elapsed = overall * total;
        let item_start = i as f64 * self.per_item_delay;
        let item_end = item_start + self.item_duration;
        if self.item_duration <= 0.0 {
            return if elapsed < item_start { 0.0 } else { 1.0 };
        }
        if elapsed <= item_start {
            0.0
        } else if elapsed >= item_end {
            1.0
        } else {
            let local = (elapsed - item_start) / self.item_duration;
            self.item_curve.transform(local)
        }
    }
}

/// Component-wise linear interpolation between two values of the same type.
///
/// Implemented for `f64`, `peniko::Color` (sRGB component lerp, a v1
/// approximation rather than a linear-light blend) and [`Point`]/[`Size`]/[`Rect`].
pub trait Lerp {
    /// Interpolate from `self` (at `t = 0`) to `other` (at `t = 1`).
    fn lerp(&self, other: &Self, t: f64) -> Self;
}

impl Lerp for f64 {
    fn lerp(&self, other: &Self, t: f64) -> Self {
        self + (other - self) * t
    }
}

impl Lerp for Point {
    fn lerp(&self, other: &Self, t: f64) -> Self {
        Point::new(self.x.lerp(&other.x, t), self.y.lerp(&other.y, t))
    }
}

impl Lerp for Size {
    fn lerp(&self, other: &Self, t: f64) -> Self {
        Size::new(
            self.width.lerp(&other.width, t),
            self.height.lerp(&other.height, t),
        )
    }
}

impl Lerp for Rect {
    fn lerp(&self, other: &Self, t: f64) -> Self {
        Rect::new(
            self.x0.lerp(&other.x0, t),
            self.y0.lerp(&other.y0, t),
            self.x1.lerp(&other.x1, t),
            self.y1.lerp(&other.y1, t),
        )
    }
}

impl Lerp for Color {
    /// Straight (non-premultiplied) per-component lerp of the encoded sRGB values.
    fn lerp(&self, other: &Self, t: f64) -> Self {
        let a = self.components;
        let b = other.components;
        let t = t as f32;
        Color::new([
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
            a[3] + (b[3] - a[3]) * t,
        ])
    }
}

/// A `begin`→`end` interpolation over a [`Lerp`] value type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tween<T> {
    /// The value at `t = 0`.
    pub begin: T,
    /// The value at `t = 1`.
    pub end: T,
}

impl<T: Lerp> Tween<T> {
    /// Construct a tween between `begin` and `end`.
    pub fn new(begin: T, end: T) -> Self {
        Tween { begin, end }
    }

    /// The interpolated value at `t`. Not clamped; clamp upstream (a [`Curve`] does)
    /// if extrapolation is unwanted.
    pub fn lerp(&self, t: f64) -> T {
        self.begin.lerp(&self.end, t)
    }
}

/// Physical parameters of a damped spring.
///
/// `damping_ratio` is ζ: `< 1` under-damped (overshoots), `== 1` critically
/// damped (fastest settle without overshoot), `> 1` over-damped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringDesc {
    /// Mass of the moving body (`> 0`).
    pub mass: f64,
    /// Spring stiffness `k` (`> 0`); higher is snappier.
    pub stiffness: f64,
    /// Damping ratio ζ (`> 0`).
    pub damping_ratio: f64,
}

/// The analytic response of a [`SpringDesc`] released from an initial displacement
/// and velocity toward equilibrium at `0`.
///
/// [`Spring::position`] returns displacements from the target. The closed-form
/// solution is exact in all three damping regimes, so a fling can be evaluated
/// at any frame time without accumulating step error.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    kind: SpringKind,
}

#[derive(Clone, Copy, Debug)]
enum SpringKind {
    /// ζ < 1: decaying oscillation.
    Under {
        w0: f64,
        wd: f64,
        zeta: f64,
        a: f64,
        b: f64,
    },
    /// ζ ≈ 1: `(a + b·t)·e^{-w0·t}`.
    Critical { w0: f64, a: f64, b: f64 },
    /// ζ > 1: sum of two decaying exponentials.
    Over { r1: f64, r2: f64, c1: f64, c2: f64 },
}

impl Spring {
    /// Solve the spring released from displacement `x0` with initial velocity `v0`.
    ///
    /// Non-positive mass/stiffness fall back to a critically damped unit spring, so
    /// the constructor never produces `NaN`.
    pub fn new(desc: SpringDesc, x0: f64, v0: f64) -> Self {
        let mass = if desc.mass > 0.0 { desc.mass } else { 1.0 };
        let stiffness = if desc.stiffness > 0.0 {
            desc.stiffness
        } else {
            1.0
        };
        let zeta = desc.damping_ratio.max(0.0);
        let w0 = (stiffness / mass).sqrt();

        let kind = if (zeta - 1.0).abs() < 1e-6 {
            let a = x0;
            let b = v0 + w0 * x0;
            SpringKind::Critical { w0, a, b }
        } else if zeta < 1.0 {
            let wd = w0 * (1.0 - zeta * zeta).sqrt();
            let a = x0;
            let b = (v0 + zeta * w0 * x0) / wd;
            SpringKind::Under { w0, wd, zeta, a, b }
        } else {
            let s = (zeta * zeta - 1.0).sqrt();
            let r1 = -w0 * (zeta - s);
            let r2 = -w0 * (zeta + s);
            let c1 = (v0 - r2 * x0) / (r1 - r2);
            let c2 = x0 - c1;
            SpringKind::Over { r1, r2, c1, c2 }
        };
        Spring { kind }
    }

    /// The displacement from equilibrium at elapsed time `t` (seconds).
    pub fn position(&self, t: f64) -> f64 {
        match self.kind {
            SpringKind::Under {
                w0, wd, zeta, a, b, ..
            } => {
                let e = (-zeta * w0 * t).exp();
                e * (a * (wd * t).cos() + b * (wd * t).sin())
            }
            SpringKind::Critical { w0, a, b } => {
                let e = (-w0 * t).exp();
                e * (a + b * t)
            }
            SpringKind::Over { r1, r2, c1, c2 } => c1 * (r1 * t).exp() + c2 * (r2 * t).exp(),
        }
    }

    /// The velocity (d displacement / d t) at elapsed time `t` (seconds).
    pub fn velocity(&self, t: f64) -> f64 {
        match self.kind {
            SpringKind::Under {
                w0, wd, zeta, a, b, ..
            } => {
                let e = (-zeta * w0 * t).exp();
                let c = (wd * t).cos();
                let s = (wd * t).sin();
                e * ((b * wd - zeta * w0 * a) * c - (a * wd + zeta * w0 * b) * s)
            }
            SpringKind::Critical { w0, a, b } => {
                let e = (-w0 * t).exp();
                e * (b - w0 * (a + b * t))
            }
            SpringKind::Over { r1, r2, c1, c2 } => {
                r1 * c1 * (r1 * t).exp() + r2 * c2 * (r2 * t).exp()
            }
        }
    }

    /// Whether both `|position|` and `|velocity|` are below `epsilon` at time `t`.
    pub fn is_at_rest(&self, t: f64, epsilon: f64) -> bool {
        self.position(t).abs() < epsilon && self.velocity(t).abs() < epsilon
    }
}

/// The lifecycle status of an [`AnimationController`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationStatus {
    /// Not started, or stopped via [`AnimationController::stop`].
    Idle,
    /// Animating toward a higher value (forward).
    Forward,
    /// Animating toward a lower value (reverse).
    Reverse,
    /// Reached its forward target at rest.
    Completed,
    /// Reached its reverse target at rest.
    Dismissed,
}

/// Value/velocity threshold below which a fling snaps to its target and settles.
const SPRING_REST_EPSILON: f64 = 1e-3;

/// A `0.0..=1.0` value driven by a duration + [`Curve`] or a [`Spring`] fling.
///
/// Plain data + math (see the module contract): a widget advances it during
/// paint and re-requests a frame while [`advance`](Self::advance) returns `true`.
/// `forward`/`reverse`/`animate_to` settle as `Completed`/`Dismissed`; `repeat`
/// never completes; a `fling` settles at `1.0` (non-negative velocity) or `0.0`.
///
/// # Overshoot (spring-driven only)
///
/// A fling is not clamped in flight: an under-damped [`SpringDesc`] (M3's spatial
/// presets use `damping_ratio: 0.9`) overshoots on purpose, then lands exactly on
/// the target. Use [`value_clamped`](Self::value_clamped) for a bounded read.
#[derive(Clone, Copy, Debug)]
pub struct AnimationController {
    value: f64,
    duration: Duration,
    curve: Curve,
    status: AnimationStatus,
    drive: Drive,
    last_time: Option<FrameTime>,
}

/// The active motion an [`AnimationController`] is running, if any.
#[derive(Clone, Copy, Debug)]
enum Drive {
    /// Not animating.
    Idle,
    /// Duration-driven `start`→`target` over `duration` seconds, eased by the controller's [`Curve`].
    Duration {
        start: f64,
        target: f64,
        elapsed: f64,
        duration: f64,
    },
    /// Looping `0.0→1.0` (eased) with period `period` seconds.
    Repeat { elapsed: f64, period: f64 },
    /// Spring-driven fling toward `target`, `elapsed` seconds in.
    Fling {
        spring: Spring,
        target: f64,
        elapsed: f64,
    },
}

impl AnimationController {
    /// Create an idle controller at `0.0` with the given default duration and [`Curve::Linear`].
    pub fn new(duration: Duration) -> Self {
        AnimationController {
            value: 0.0,
            duration,
            curve: Curve::Linear,
            status: AnimationStatus::Idle,
            drive: Drive::Idle,
            last_time: None,
        }
    }

    /// Set the easing curve applied to duration-driven motion (builder-style).
    pub fn with_curve(mut self, curve: Curve) -> Self {
        self.curve = curve;
        self
    }

    /// The current value: within `0.0..=1.0` for duration motion, but a spring
    /// [`fling`](Self::fling) may transiently leave that range (see the type docs'
    /// Overshoot section); see [`value_clamped`](Self::value_clamped).
    pub fn value(&self) -> f64 {
        self.value
    }

    /// [`value`](Self::value) clamped to `0.0..=1.0`; differs from it only while a
    /// fling overshoots.
    pub fn value_clamped(&self) -> f64 {
        self.value.clamp(0.0, 1.0)
    }

    /// The current lifecycle status.
    pub fn status(&self) -> AnimationStatus {
        self.status
    }

    /// Whether a motion is in progress (the next [`advance`](Self::advance) makes progress).
    pub fn is_animating(&self) -> bool {
        !matches!(self.drive, Drive::Idle)
    }

    /// Animate forward to `1.0` over the controller's duration.
    pub fn forward(&mut self) {
        self.start_duration(self.value, 1.0, AnimationStatus::Forward);
    }

    /// Animate reverse to `0.0` over the controller's duration.
    pub fn reverse(&mut self) {
        self.start_duration(self.value, 0.0, AnimationStatus::Reverse);
    }

    /// Animate from the current value to `target` (clamped to `0.0..=1.0`) over
    /// the controller's duration.
    pub fn animate_to(&mut self, target: f64) {
        let target = target.clamp(0.0, 1.0);
        let status = if target >= self.value {
            AnimationStatus::Forward
        } else {
            AnimationStatus::Reverse
        };
        self.start_duration(self.value, target, status);
    }

    /// Loop `0.0→1.0` (eased) indefinitely with a period of the controller's
    /// duration. [`advance`](Self::advance) always returns `true` for a repeat.
    pub fn repeat(&mut self) {
        let period = self.duration.as_secs_f64();
        self.value = 0.0;
        self.status = AnimationStatus::Forward;
        self.last_time = None;
        self.drive = if period > 0.0 {
            Drive::Repeat {
                elapsed: 0.0,
                period,
            }
        } else {
            // A zero-period repeat can't advance; degrade to idle.
            Drive::Idle
        };
    }

    /// Start a spring fling from the current value with initial `velocity` (value
    /// units per second), settling at `1.0` for a non-negative velocity, else `0.0`.
    /// The value is not clamped in flight (see the type docs' Overshoot section).
    pub fn fling(&mut self, velocity: f64, spring: SpringDesc) {
        let target = if velocity >= 0.0 { 1.0 } else { 0.0 };
        let x0 = self.value - target;
        self.status = if target >= self.value {
            AnimationStatus::Forward
        } else {
            AnimationStatus::Reverse
        };
        self.last_time = None;
        self.drive = Drive::Fling {
            spring: Spring::new(spring, x0, velocity),
            target,
            elapsed: 0.0,
        };
    }

    /// Halt any in-progress motion, leaving the value where it is and the status
    /// [`AnimationStatus::Idle`].
    pub fn stop(&mut self) {
        self.drive = Drive::Idle;
        self.status = AnimationStatus::Idle;
        self.last_time = None;
    }

    /// Advance to frame time `now`, returning whether it is still animating (the
    /// caller must then request another frame).
    ///
    /// The delta uses [`FrameTime::saturating_sub`], so a repeated or out-of-order
    /// timestamp yields a zero delta, never a panic or `NaN`. The first `advance`
    /// after starting a motion only seeds the clock.
    pub fn advance(&mut self, now: FrameTime) -> bool {
        let dt = match self.last_time {
            Some(last) => now.saturating_sub(last).as_secs_f64(),
            None => 0.0,
        };
        self.last_time = Some(now);
        // Defensive: a non-finite delta must never reach `value`.
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };

        match &mut self.drive {
            Drive::Idle => false,
            Drive::Duration {
                start,
                target,
                elapsed,
                duration,
            } => {
                let (start, target, duration) = (*start, *target, *duration);
                *elapsed += dt;
                let elapsed = *elapsed;
                if duration <= 0.0 || elapsed >= duration {
                    self.value = target;
                    self.status = if target >= start {
                        AnimationStatus::Completed
                    } else {
                        AnimationStatus::Dismissed
                    };
                    self.drive = Drive::Idle;
                    false
                } else {
                    let frac = (elapsed / duration).clamp(0.0, 1.0);
                    let eased = self.curve.transform(frac);
                    self.value = start + (target - start) * eased;
                    true
                }
            }
            Drive::Repeat { elapsed, period } => {
                let period = *period;
                *elapsed += dt;
                let frac = if period > 0.0 {
                    (*elapsed / period).rem_euclid(1.0)
                } else {
                    0.0
                };
                self.value = self.curve.transform(frac);
                true
            }
            Drive::Fling {
                spring,
                target,
                elapsed,
            } => {
                let spring = *spring;
                let target = *target;
                *elapsed += dt;
                let elapsed = *elapsed;
                if spring.is_at_rest(elapsed, SPRING_REST_EPSILON) {
                    self.value = target;
                    self.status = if target >= 0.5 {
                        AnimationStatus::Completed
                    } else {
                        AnimationStatus::Dismissed
                    };
                    self.drive = Drive::Idle;
                    false
                } else {
                    // Deliberately unclamped: overshoot is intended motion (see `value_clamped`).
                    self.value = target + spring.position(elapsed);
                    true
                }
            }
        }
    }

    /// Shared entry for the duration-driven motions.
    fn start_duration(&mut self, start: f64, target: f64, status: AnimationStatus) {
        let duration = self.duration.as_secs_f64();
        self.status = status;
        self.last_time = None;
        if duration <= 0.0 || (target - start).abs() < f64::EPSILON {
            // Nothing to animate: snap and settle immediately.
            self.value = target;
            self.status = if target >= start {
                AnimationStatus::Completed
            } else {
                AnimationStatus::Dismissed
            };
            self.drive = Drive::Idle;
        } else {
            self.drive = Drive::Duration {
                start,
                target,
                elapsed: 0.0,
                duration,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ft_secs(s: f64) -> FrameTime {
        FrameTime::from_nanos((s * 1_000_000_000.0) as u64)
    }

    #[test]
    fn frame_time_differences_only() {
        let a = FrameTime::from_nanos(1_000);
        let b = FrameTime::from_nanos(3_500);
        assert_eq!(b.saturating_sub(a), Duration::from_nanos(2_500));
        // Non-monotonic: saturates at zero.
        assert_eq!(a.saturating_sub(b), Duration::ZERO);
        assert!((ft_secs(2.0).as_secs_f64() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn ease_in_out_matches_css_reference_at_half() {
        // ease-in-out is symmetric → its midpoint is exactly 0.5.
        assert!((Curve::EaseInOut.transform(0.5) - 0.5).abs() < 1e-4);
        // Endpoints are pinned.
        assert!((Curve::EaseInOut.transform(0.0)).abs() < 1e-9);
        assert!((Curve::EaseInOut.transform(1.0) - 1.0).abs() < 1e-9);
        // ease-in starts slow (below linear at the midpoint); ease-out ends slow
        // (above linear). Symmetric pair: ease-in(x) + ease-out(1-x) == 1.
        assert!(Curve::EaseIn.transform(0.5) < 0.5);
        assert!(Curve::EaseOut.transform(0.5) > 0.5);
        assert!((Curve::EaseIn.transform(0.5) + Curve::EaseOut.transform(0.5) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn linear_curve_is_identity() {
        for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
            assert!((Curve::Linear.transform(t) - t).abs() < 1e-12);
        }
        // Out-of-range inputs clamp.
        assert_eq!(Curve::Linear.transform(-1.0), 0.0);
        assert_eq!(Curve::Linear.transform(2.0), 1.0);
    }

    #[test]
    fn interval_endpoints_and_midpoint() {
        let seg = Curve::EaseInOut.interval(0.3, 0.7);
        // Pre-start.
        assert_eq!(seg.transform(0.0), 0.0);
        assert_eq!(seg.transform(0.3), 0.0);
        assert_eq!(seg.transform(0.1), 0.0);
        // Post-end.
        assert_eq!(seg.transform(0.7), 1.0);
        assert_eq!(seg.transform(0.9), 1.0);
        assert_eq!(seg.transform(1.0), 1.0);
        // Midpoint of the interval == midpoint of the inner curve (ease-in-out
        // is symmetric, so its own midpoint is exactly 0.5).
        let local_mid = 0.3 + 0.5 * (0.7 - 0.3);
        assert!((seg.transform(local_mid) - 0.5).abs() < 1e-4);
        // A quarter into the interval matches the inner curve at local t=0.25.
        let local_quarter = 0.3 + 0.25 * (0.7 - 0.3);
        assert!((seg.transform(local_quarter) - Curve::EaseInOut.transform(0.25)).abs() < 1e-9);
    }

    #[test]
    fn interval_linear_matches_flutter_fade_through_shape() {
        // Fade-through staging: outgoing on [0.0, 0.3], incoming on [0.3, 1.0].
        let out_seg = Curve::Linear.interval(0.0, 0.3);
        let in_seg = Curve::Linear.interval(0.3, 1.0);
        assert_eq!(out_seg.transform(0.0), 0.0);
        assert!((out_seg.transform(0.15) - 0.5).abs() < 1e-9);
        assert_eq!(out_seg.transform(0.3), 1.0);
        assert_eq!(in_seg.transform(0.3), 0.0);
        assert!((in_seg.transform(0.65) - 0.5).abs() < 1e-9);
        assert_eq!(in_seg.transform(1.0), 1.0);
    }

    #[test]
    fn interval_degenerate_start_equals_end_is_an_instant_step() {
        let seg = Curve::Linear.interval(0.3, 0.3);
        assert_eq!(seg.transform(0.0), 0.0);
        assert_eq!(seg.transform(0.2), 0.0);
        // Just below the step point: still 0.0.
        assert_eq!(seg.transform(0.3), 1.0);
        assert_eq!(seg.transform(0.5), 1.0);
        assert_eq!(seg.transform(1.0), 1.0);
    }

    #[test]
    fn interval_degenerate_inverted_is_an_instant_step_at_start() {
        // end < start also degrades to the same instant-step rule.
        let seg = Curve::Linear.interval(0.6, 0.2);
        assert_eq!(seg.transform(0.0), 0.0);
        assert_eq!(seg.transform(0.59), 0.0);
        assert_eq!(seg.transform(0.6), 1.0);
        assert_eq!(seg.transform(1.0), 1.0);
    }

    #[test]
    fn interval_out_of_range_t_clamps_first() {
        let seg = Curve::Linear.interval(0.3, 0.7);
        assert_eq!(seg.transform(-1.0), 0.0);
        assert_eq!(seg.transform(2.0), 1.0);
    }

    /// Glyph log-reveal shape: 150ms items, 90ms per-item delay, 5 items —
    /// hand-computed against `StaggerSpec::item_progress`'s contract.
    fn glyph_stagger() -> StaggerSpec {
        StaggerSpec {
            per_item_delay: 90.0,
            item_duration: 150.0,
            item_curve: Curve::Linear,
        }
    }

    #[test]
    fn stagger_total_duration_spans_last_items_window() {
        let spec = glyph_stagger();
        // Item 4 (last of 5) starts at 4*90=360 and runs 150ms -> ends 510.
        assert_eq!(spec.total_duration(5), 510.0);
        assert_eq!(spec.total_duration(1), 150.0);
        assert_eq!(spec.total_duration(0), 0.0);
    }

    #[test]
    fn stagger_item_progress_hand_computed_table() {
        let spec = glyph_stagger();
        let n = 5;
        let total = spec.total_duration(n); // 510.0

        // Item windows (start..end): 0..150, 90..240, 180..330, 270..420, 360..510.
        // overall=0.0 -> elapsed=0: only item 0 has started (at its own start -> 0.0).
        for i in 0..n {
            assert_eq!(spec.item_progress(0.0, i, n), 0.0, "i={i} at overall=0.0");
        }

        // overall=1.0 -> elapsed=510: every item has reached/passed its end -> 1.0.
        for i in 0..n {
            assert_eq!(spec.item_progress(1.0, i, n), 1.0, "i={i} at overall=1.0");
        }

        // elapsed = 270 (item 3's start; item 2's local 90/150=0.6; item 1 and
        // item 0 already past their end -> 1.0; item 4 not started -> 0.0).
        let overall_270 = 270.0 / total;
        assert_eq!(spec.item_progress(overall_270, 0, n), 1.0);
        assert_eq!(spec.item_progress(overall_270, 1, n), 1.0);
        assert!((spec.item_progress(overall_270, 2, n) - 0.6).abs() < 1e-9);
        assert_eq!(spec.item_progress(overall_270, 3, n), 0.0);
        assert_eq!(spec.item_progress(overall_270, 4, n), 0.0);

        // elapsed = 45ms: only item 0 has started, at local 45/150 = 0.3.
        let overall_45 = 45.0 / total;
        assert!((spec.item_progress(overall_45, 0, n) - 0.3).abs() < 1e-9);
        assert_eq!(spec.item_progress(overall_45, 1, n), 0.0);

        // elapsed = 135ms: item 0 local 135/150=0.9; item 1 not started (starts
        // at 90, so local (135-90)/150=0.3).
        let overall_135 = 135.0 / total;
        assert!((spec.item_progress(overall_135, 0, n) - 0.9).abs() < 1e-9);
        assert!((spec.item_progress(overall_135, 1, n) - 0.3).abs() < 1e-9);
    }

    #[test]
    fn stagger_item_progress_out_of_range_index_or_empty_set_is_complete() {
        let spec = glyph_stagger();
        // i >= n has no defined window -> fully revealed.
        assert_eq!(spec.item_progress(0.5, 5, 5), 1.0);
        // n == 0 has no timeline -> fully revealed.
        assert_eq!(spec.item_progress(0.5, 0, 0), 1.0);
    }

    #[test]
    fn stagger_zero_duration_item_is_an_instant_step() {
        let spec = StaggerSpec {
            per_item_delay: 100.0,
            item_duration: 0.0,
            item_curve: Curve::Linear,
        };
        let n = 3;
        let total = spec.total_duration(n); // 2*100 + 0 = 200
        assert_eq!(total, 200.0);
        // Item 1 starts at 100: strictly before -> 0.0, at/after -> 1.0.
        assert_eq!(spec.item_progress(99.0 / total, 1, n), 0.0);
        assert_eq!(spec.item_progress(100.0 / total, 1, n), 1.0);
    }

    #[test]
    fn tween_interpolates_value_types() {
        assert!((Tween::new(0.0_f64, 10.0).lerp(0.25) - 2.5).abs() < 1e-12);
        assert_eq!(
            Tween::new(Point::new(0.0, 0.0), Point::new(4.0, 8.0)).lerp(0.5),
            Point::new(2.0, 4.0)
        );
        assert_eq!(
            Tween::new(Size::new(0.0, 0.0), Size::new(10.0, 20.0)).lerp(0.1),
            Size::new(1.0, 2.0)
        );
        assert_eq!(
            Tween::new(Rect::new(0.0, 0.0, 2.0, 2.0), Rect::new(2.0, 2.0, 6.0, 6.0)).lerp(0.5),
            Rect::new(1.0, 1.0, 4.0, 4.0)
        );
        let c = Tween::new(Color::BLACK, Color::WHITE).lerp(0.5);
        for ch in &c.components[..3] {
            assert!((ch - 0.5).abs() < 1e-6);
        }
    }

    /// RK4-integrate `m·x'' + c·x' + k·x = 0` to `t`, returning position.
    fn integrate_spring(desc: SpringDesc, x0: f64, v0: f64, t_end: f64) -> f64 {
        let m = desc.mass;
        let k = desc.stiffness;
        let c = 2.0 * desc.damping_ratio * (k * m).sqrt();
        let accel = |x: f64, v: f64| -(k * x + c * v) / m;
        let dt = 1e-5;
        let steps = (t_end / dt).round() as usize;
        let (mut x, mut v) = (x0, v0);
        for _ in 0..steps {
            let (k1x, k1v) = (v, accel(x, v));
            let (k2x, k2v) = (
                v + 0.5 * dt * k1v,
                accel(x + 0.5 * dt * k1x, v + 0.5 * dt * k1v),
            );
            let (k3x, k3v) = (
                v + 0.5 * dt * k2v,
                accel(x + 0.5 * dt * k2x, v + 0.5 * dt * k2v),
            );
            let (k4x, k4v) = (v + dt * k3v, accel(x + dt * k3x, v + dt * k3v));
            x += dt / 6.0 * (k1x + 2.0 * k2x + 2.0 * k3x + k4x);
            v += dt / 6.0 * (k1v + 2.0 * k2v + 2.0 * k3v + k4v);
        }
        x
    }

    #[test]
    fn spring_analytic_matches_numeric_under_critical_over() {
        let cases = [
            // M3-flavored under-damped case: stiffness 700, damping 0.9.
            SpringDesc {
                mass: 1.0,
                stiffness: 700.0,
                damping_ratio: 0.9,
            },
            SpringDesc {
                mass: 1.0,
                stiffness: 700.0,
                damping_ratio: 1.0,
            },
            SpringDesc {
                mass: 1.0,
                stiffness: 700.0,
                damping_ratio: 1.5,
            },
        ];
        for desc in cases {
            let spring = Spring::new(desc, 1.0, 0.0);
            for &t in &[0.005, 0.01, 0.02, 0.03] {
                let analytic = spring.position(t);
                let numeric = integrate_spring(desc, 1.0, 0.0, t);
                assert!(
                    (analytic - numeric).abs() < 1e-6,
                    "regime ζ={} at t={t}: analytic {analytic} vs numeric {numeric}",
                    desc.damping_ratio
                );
            }
        }
    }

    #[test]
    fn spring_velocity_matches_finite_difference() {
        let desc = SpringDesc {
            mass: 1.0,
            stiffness: 700.0,
            damping_ratio: 0.9,
        };
        let spring = Spring::new(desc, 1.0, 0.0);
        let t = 0.01;
        let h = 1e-7;
        let fd = (spring.position(t + h) - spring.position(t - h)) / (2.0 * h);
        assert!((spring.velocity(t) - fd).abs() < 1e-4);
    }

    #[test]
    fn duration_forward_reaches_completed() {
        let mut c = AnimationController::new(Duration::from_millis(100));
        c.forward();
        assert_eq!(c.status(), AnimationStatus::Forward);
        // Seed the clock.
        assert!(c.advance(ft_secs(0.0)));
        assert!(c.advance(ft_secs(0.05)));
        assert!((c.value() - 0.5).abs() < 1e-6);
        // Past the end: settles Completed at 1.0, no longer animating.
        assert!(!c.advance(ft_secs(0.2)));
        assert_eq!(c.value(), 1.0);
        assert_eq!(c.status(), AnimationStatus::Completed);
    }

    #[test]
    fn reverse_reaches_dismissed() {
        let mut c = AnimationController::new(Duration::from_millis(100));
        // Start from the top.
        c.forward();
        c.advance(ft_secs(0.0));
        c.advance(ft_secs(0.2));
        assert_eq!(c.value(), 1.0);

        c.reverse();
        assert_eq!(c.status(), AnimationStatus::Reverse);
        c.advance(ft_secs(1.0));
        assert!(!c.advance(ft_secs(1.2)));
        assert_eq!(c.value(), 0.0);
        assert_eq!(c.status(), AnimationStatus::Dismissed);
    }

    #[test]
    fn repeat_wraps_and_never_completes() {
        let mut c = AnimationController::new(Duration::from_secs(1));
        c.repeat();
        assert!(c.advance(ft_secs(0.0)));
        assert!(c.advance(ft_secs(0.5)));
        assert!((c.value() - 0.5).abs() < 1e-6);
        // Wrap past one period back toward 0.
        assert!(c.advance(ft_secs(1.5)));
        assert!((c.value() - 0.5).abs() < 1e-6);
        assert!(c.advance(ft_secs(2.0)));
        assert!(c.value() < 1e-6);
        // Always still animating.
        assert!(c.advance(ft_secs(100.0)));
    }

    #[test]
    fn animate_to_partial_target() {
        let mut c = AnimationController::new(Duration::from_millis(100));
        c.animate_to(0.3);
        assert_eq!(c.status(), AnimationStatus::Forward);
        c.advance(ft_secs(0.0));
        assert!(!c.advance(ft_secs(0.2)));
        assert!((c.value() - 0.3).abs() < 1e-6);
        assert_eq!(c.status(), AnimationStatus::Completed);
    }

    #[test]
    fn fling_settles_at_target() {
        let mut c = AnimationController::new(Duration::from_millis(100));
        c.fling(
            2.0,
            SpringDesc {
                mass: 1.0,
                stiffness: 700.0,
                damping_ratio: 0.9,
            },
        );
        let mut t = 0.0;
        let mut running = true;
        // Advance at ~120fps until settled (bounded so a bug can't hang the test).
        for _ in 0..100_000 {
            running = c.advance(ft_secs(t));
            if !running {
                break;
            }
            t += 1.0 / 120.0;
        }
        assert!(!running, "fling failed to settle");
        assert!((c.value() - 1.0).abs() < 1e-6);
        assert_eq!(c.status(), AnimationStatus::Completed);
    }

    #[test]
    fn fling_overshoots_past_target_for_underdamped_spring() {
        // M3's default-spatial preset: ζ = 0.9, k = 700 — under-damped, so a
        // fling released with zero velocity still oscillates around the
        // target before settling.
        let desc = SpringDesc {
            mass: 1.0,
            stiffness: 700.0,
            damping_ratio: 0.9,
        };
        let mut c = AnimationController::new(Duration::from_millis(100));
        c.fling(0.0, desc);

        // Track the max value observed while flying; the analytic
        // cross-check against `Spring` directly lives in the sibling test
        // `fling_overshoot_values_match_analytic_spring`.
        let mut max_value = f64::MIN;
        let mut t = 0.0;
        let mut running = true;
        for _ in 0..100_000 {
            running = c.advance(ft_secs(t));
            if !running {
                break;
            }
            max_value = max_value.max(c.value());
            t += 1.0 / 120.0;
        }
        assert!(!running, "fling failed to settle");
        assert!(
            max_value > 1.0 + 1e-3,
            "expected a demonstrable overshoot past 1.0, got max {max_value}"
        );
        assert_eq!(c.value(), 1.0);
        assert_eq!(c.status(), AnimationStatus::Completed);
    }

    #[test]
    fn fling_overshoot_values_match_analytic_spring() {
        let desc = SpringDesc {
            mass: 1.0,
            stiffness: 700.0,
            damping_ratio: 0.9,
        };
        let mut c = AnimationController::new(Duration::from_millis(100));
        c.fling(0.0, desc);
        let spring = Spring::new(desc, -1.0, 0.0);

        // Seed the clock, then check a handful of in-flight samples against
        // the analytic spring directly.
        assert!(c.advance(ft_secs(0.0)));
        for &t in &[0.01, 0.02, 0.03, 0.05, 0.08] {
            assert!(c.advance(ft_secs(t)));
            let expected = 1.0 + spring.position(t);
            assert!(
                (c.value() - expected).abs() < 1e-6,
                "at t={t}: controller {} vs analytic {expected}",
                c.value()
            );
        }
    }

    #[test]
    fn effects_spring_never_exceeds_target() {
        // M3's default-effects preset: ζ = 1.0 — critically damped, released
        // from rest (velocity 0), so it approaches the target monotonically
        // with no overshoot.
        let desc = SpringDesc {
            mass: 1.0,
            stiffness: 1600.0,
            damping_ratio: 1.0,
        };
        let mut c = AnimationController::new(Duration::from_millis(100));
        c.fling(0.0, desc);

        let mut t = 0.0;
        let mut running = true;
        for _ in 0..100_000 {
            running = c.advance(ft_secs(t));
            assert!(
                c.value() <= 1.0 + 1e-9,
                "critically damped spring released from rest overshot: value {} at t={t}",
                c.value()
            );
            if !running {
                break;
            }
            t += 1.0 / 120.0;
        }
        assert!(!running, "fling failed to settle");
        assert_eq!(c.value(), 1.0);
        assert_eq!(c.status(), AnimationStatus::Completed);
    }

    #[test]
    fn value_clamped_bounds_an_overshooting_fling() {
        let desc = SpringDesc {
            mass: 1.0,
            stiffness: 700.0,
            damping_ratio: 0.9,
        };
        let mut c = AnimationController::new(Duration::from_millis(100));
        c.fling(0.0, desc);
        c.advance(ft_secs(0.0));
        c.advance(ft_secs(0.02));
        // Overshoot is expected in `value()` but `value_clamped()` must stay
        // bounded regardless.
        assert!(c.value_clamped() >= 0.0 && c.value_clamped() <= 1.0);
        assert_eq!(c.value_clamped(), c.value().clamp(0.0, 1.0));
    }

    #[test]
    fn advance_is_safe_on_equal_and_backward_timestamps() {
        let mut c = AnimationController::new(Duration::from_millis(100));
        c.forward();
        c.advance(ft_secs(0.05));
        let v_seed = c.value();
        // Equal timestamp: zero delta, no progress, no panic/NaN.
        c.advance(ft_secs(0.05));
        assert_eq!(c.value(), v_seed);
        assert!(c.value().is_finite());
        // Backward timestamp: clamped to zero delta.
        c.advance(ft_secs(0.01));
        assert_eq!(c.value(), v_seed);
        assert!(c.value().is_finite());
    }

    #[test]
    fn stop_halts_progress() {
        let mut c = AnimationController::new(Duration::from_millis(100));
        c.forward();
        c.advance(ft_secs(0.0));
        c.advance(ft_secs(0.05));
        let v = c.value();
        c.stop();
        assert_eq!(c.status(), AnimationStatus::Idle);
        assert!(!c.advance(ft_secs(0.5)));
        assert_eq!(c.value(), v);
    }
}
