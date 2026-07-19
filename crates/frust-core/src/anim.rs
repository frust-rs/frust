//! Animation vocabulary: the shared frame clock ([`FrameTime`]) plus the pure
//! math widgets consume it through — easing [`Curve`]s, [`Tween`] interpolation,
//! analytic [`Spring`] dynamics, and the duration-/spring-driven
//! [`AnimationController`].
//!
//! # Contract: this module is plain data + math, never a scheduler
//!
//! Nothing here reads a clock, spawns a task, or registers a callback.
//! [`FrameTime`] *enters from the shell* (spec §8: no `Instant::now()` inside
//! `frust-core`) and is threaded to widgets during paint as
//! [`crate::widget::PaintCtx::frame_time`]. A widget that animates does so
//! **during its own paint**: it calls [`AnimationController::advance`] with the
//! frame's time, reads [`AnimationController::value`], and — if `advance`
//! returned `true` (still animating) — calls
//! [`crate::widget::PaintCtx::request_frame`] so the shell schedules another
//! frame. There is no ambient tick; a controller that is never `advance`d never
//! moves.

use std::time::Duration;

use kurbo::{Point, Rect, Size};
use peniko::Color;

/// A point in time supplied by the shell, in monotonic nanoseconds.
///
/// The origin is **arbitrary and shell-specific** (desktop: nanos since a
/// shell-owned epoch; Android: a `Choreographer` frame nanos value; iOS: a
/// `CADisplayLink` timestamp). Widgets may therefore only *difference* two
/// `FrameTime`s (via [`FrameTime::saturating_sub`]) — never interpret one
/// absolutely, and never compare `FrameTime`s produced by different shells.
///
/// [`FrameTime::ZERO`] is the documented "no time available" fallback used by
/// legacy/test paint paths that don't thread a real clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameTime(u64);

impl FrameTime {
    /// The zero instant — also the "no time available" fallback (see the type
    /// docs). Differencing against it yields the raw nanosecond value.
    pub const ZERO: FrameTime = FrameTime(0);

    /// Build a `FrameTime` from a raw monotonic-nanosecond count.
    pub const fn from_nanos(nanos: u64) -> Self {
        FrameTime(nanos)
    }

    /// The raw monotonic-nanosecond count. Meaningful only relative to another
    /// `FrameTime` from the same shell (see the type docs).
    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    /// This instant as fractional seconds from the (arbitrary) origin. Only
    /// differences of two such values carry meaning.
    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 / 1_000_000_000.0
    }

    /// The non-negative [`Duration`] elapsed from `earlier` to `self`.
    ///
    /// Saturates at zero when `self < earlier` (a non-monotonic or reordered
    /// clock), so a widget differencing two frame times can never observe a
    /// negative delta — the basis of [`AnimationController::advance`]'s
    /// clamp-and-never-panic guarantee.
    pub fn saturating_sub(self, earlier: FrameTime) -> Duration {
        Duration::from_nanos(self.0.saturating_sub(earlier.0))
    }
}

/// An easing curve mapping a normalized time `t ∈ [0, 1]` to an eased progress
/// `∈ [0, 1]`.
///
/// The named variants are the standard CSS timing functions plus Material 3's
/// `Emphasized` easing; [`Curve::Cubic`] is a general cubic-Bézier for anything
/// else. Evaluation clamps `t` to `[0, 1]` first.
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
    /// A general cubic-Bézier timing function with control points
    /// `(x1, y1)` and `(x2, y2)` (endpoints fixed at `(0, 0)`/`(1, 1)`), the
    /// same parameterization CSS `cubic-bezier()` uses.
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
}

/// Evaluate a cubic-Bézier timing function at input `x ∈ [0, 1]`.
///
/// Control points are `(0, 0)`, `(x1, y1)`, `(x2, y2)`, `(1, 1)` — the CSS
/// `cubic-bezier()` parameterization. The curve is defined parametrically in
/// `s`; this solves `X(s) = x` for `s` (Newton's method, then a bisection
/// fallback if Newton leaves the valid interval) and returns `Y(s)`.
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

    // Newton–Raphson from x as the initial guess (X is close to identity for
    // well-behaved timing functions).
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

    // Bisection fallback (guaranteed to converge on the monotone `X` a valid
    // timing function has).
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

/// Component-wise linear interpolation between two values of the same type.
///
/// Implemented for the value types animations move: `f64`, `peniko::Color`
/// (component lerp in sRGB — an acceptable v1 approximation, not a
/// perceptual/linear-light blend), and the `kurbo` geometry types
/// [`Point`]/[`Size`]/[`Rect`].
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
    /// Straight (non-premultiplied) per-component lerp in the sRGB encoding.
    ///
    /// This blends the encoded sRGB components directly, which is cheap and
    /// visually acceptable for v1 but is *not* a linear-light or perceptual
    /// interpolation — a future revision may move to linear-light blending.
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

    /// The interpolated value at `t` (typically the eased progress from a
    /// [`Curve`] or an [`AnimationController::value`]). `t` is not clamped here;
    /// clamp upstream (a [`Curve`] already does) if extrapolation is unwanted.
    pub fn lerp(&self, t: f64) -> T {
        self.begin.lerp(&self.end, t)
    }
}

/// Physical parameters of a damped spring (a mass on a spring with a damper).
///
/// `damping_ratio` is the dimensionless ζ: `< 1` under-damped (overshoots and
/// oscillates), `== 1` critically damped (fastest non-overshooting settle),
/// `> 1` over-damped (slow, no overshoot). M3's standard spring presets (which
/// live in `frust-theme`, not here) are expressed in these terms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringDesc {
    /// Mass of the moving body (`> 0`).
    pub mass: f64,
    /// Spring stiffness `k` (`> 0`); higher is snappier.
    pub stiffness: f64,
    /// Damping ratio ζ (`> 0`); see the type docs.
    pub damping_ratio: f64,
}

/// The analytic response of a [`SpringDesc`] released from an initial
/// displacement + velocity toward equilibrium at `0`.
///
/// Positions returned by [`Spring::position`] are **displacements from
/// equilibrium** (the target), so equilibrium is always `0`. The closed-form
/// solution branches on the damping regime; all three are exact (no numerical
/// integration), which is what lets a fling be evaluated at an arbitrary frame
/// time without accumulating step error.
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
    /// Solve the spring released from displacement `x0` (relative to
    /// equilibrium) with initial velocity `v0`.
    ///
    /// Degenerate parameters (non-positive mass/stiffness) fall back to a
    /// critically-damped unit spring so the constructor never produces `NaN`.
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
            // Critically damped.
            let a = x0;
            let b = v0 + w0 * x0;
            SpringKind::Critical { w0, a, b }
        } else if zeta < 1.0 {
            // Under-damped.
            let wd = w0 * (1.0 - zeta * zeta).sqrt();
            let a = x0;
            let b = (v0 + zeta * w0 * x0) / wd;
            SpringKind::Under { w0, wd, zeta, a, b }
        } else {
            // Over-damped.
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

    /// Whether the spring has effectively settled at equilibrium by time `t`:
    /// both `|position|` and `|velocity|` are below `epsilon`.
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

/// The rest threshold (in value- and velocity-units) a spring fling must fall
/// below before the controller snaps to its target and reports `Completed`/
/// `Dismissed`.
const SPRING_REST_EPSILON: f64 = 1e-3;

/// A `0.0..=1.0` animation value driven either by a duration + [`Curve`] or by
/// a [`Spring`] fling.
///
/// It is **plain data + math** (see the module docs' contract): it holds no
/// clock and no scheduler. A widget advances it during paint with the frame's
/// [`FrameTime`], reads [`value`](Self::value), and re-requests a frame while
/// [`advance`](Self::advance) keeps returning `true`.
///
/// # Status semantics
///
/// [`forward`](Self::forward)/[`reverse`](Self::reverse) drive to the `1.0`/
/// `0.0` bounds and settle as `Completed`/`Dismissed`. [`animate_to`](Self::animate_to)
/// settles as `Completed` when it moved forward (target ≥ start) and `Dismissed`
/// when it moved reverse. [`repeat`](Self::repeat) never completes ([`advance`](Self::advance)
/// always returns `true`). A [`fling`](Self::fling) settles at `1.0` (positive
/// velocity) or `0.0` (negative) as `Completed`/`Dismissed`.
///
/// # Overshoot (spring-driven only)
///
/// Duration+[`Curve`] motion ([`forward`]/[`reverse`]/[`animate_to`]/[`repeat`])
/// always keeps [`value`](Self::value) in `0.0..=1.0` — unchanged from before
/// this contract existed. A [`fling`](Self::fling), by contrast, is **not**
/// clamped while in flight: an under-damped [`SpringDesc`] (M3's spatial
/// presets use `damping_ratio: 0.9`) genuinely overshoots its target before
/// settling, and that overshoot is the whole visual point of a "bouncy"
/// spring — clamping it away would hide it. `value()` may therefore transiently
/// read outside `[0, 1]` mid-fling; it always lands exactly on the target
/// (`0.0`/`1.0`) once [`advance`](Self::advance) reports settled (`status()`
/// becomes `Completed`/`Dismissed`). A consumer that needs the value
/// pinned to `[0, 1]` at every frame (e.g. to feed a `Lerp`/`Tween` that
/// assumes bounded input) should use [`value_clamped`](Self::value_clamped)
/// instead. A critically-/over-damped spring (`damping_ratio >= 1.0`, e.g.
/// M3's "effects" presets) released with zero velocity (the common
/// "settle to target" fling usage) never overshoots in the first place, so
/// `value()`/`value_clamped()` agree for that case; a large enough release
/// velocity can still carry even a critically-/over-damped spring past its
/// target before it settles back — damping ratio bounds *oscillation*
/// (repeated overshoot), not a single one.
///
/// [`forward`]: Self::forward
/// [`reverse`]: Self::reverse
/// [`animate_to`]: Self::animate_to
/// [`repeat`]: Self::repeat
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
    /// Duration-driven interpolation from `start` to `target` over `duration`
    /// seconds, `elapsed` so far, eased by the controller's [`Curve`].
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
    /// Create an idle controller at value `0.0` with the given default duration
    /// and a [`Curve::Linear`] easing.
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

    /// Set the easing curve applied to duration-driven motion (returns `self`
    /// for builder-style construction).
    pub fn with_curve(mut self, curve: Curve) -> Self {
        self.curve = curve;
        self
    }

    /// The current value.
    ///
    /// For duration+[`Curve`] motion this is always in `0.0..=1.0`. For a
    /// spring [`fling`](Self::fling) it may transiently read outside that
    /// range — an under-damped spring's overshoot is real motion, not a bug
    /// (see the type docs' Overshoot section) — but always lands exactly on
    /// the target once the fling settles. Use
    /// [`value_clamped`](Self::value_clamped) if a bounded `[0, 1]` read is
    /// required instead.
    pub fn value(&self) -> f64 {
        self.value
    }

    /// [`value`](Self::value), clamped to `0.0..=1.0`.
    ///
    /// Identical to `value()` for duration+[`Curve`] motion (already
    /// bounded); for a spring [`fling`](Self::fling) mid-overshoot this
    /// clips the transient excursion past the target — for consumers (e.g. a
    /// `Lerp`/`Tween` feed) that need a bounded value and don't want the
    /// bounce visually represented.
    pub fn value_clamped(&self) -> f64 {
        self.value.clamp(0.0, 1.0)
    }

    /// The current lifecycle status.
    pub fn status(&self) -> AnimationStatus {
        self.status
    }

    /// Whether a motion is currently in progress (the next [`advance`](Self::advance)
    /// will make progress).
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

    /// Start a spring fling from the current value with initial `velocity` (in
    /// value-units per second). It settles toward `1.0` for a non-negative
    /// velocity, `0.0` otherwise.
    ///
    /// Unlike duration+[`Curve`] motion, the value driven by a fling is
    /// **not clamped to `[0, 1]` while in flight** — see the type docs'
    /// Overshoot section and [`value`](Self::value)/
    /// [`value_clamped`](Self::value_clamped).
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

    /// Advance the animation to frame time `now`, returning whether it is still
    /// animating (in which case the caller must request another frame).
    ///
    /// The delta from the previous `advance` is derived via
    /// [`FrameTime::saturating_sub`], so a repeated or out-of-order timestamp
    /// yields a zero (never negative) delta: safe, no panic, no `NaN`. The first
    /// `advance` after starting a motion only seeds the clock (zero delta); the
    /// next one makes progress.
    pub fn advance(&mut self, now: FrameTime) -> bool {
        let dt = match self.last_time {
            Some(last) => now.saturating_sub(last).as_secs_f64(),
            None => 0.0,
        };
        self.last_time = Some(now);
        // `saturating_sub` already guarantees `dt >= 0`; guard against a
        // non-finite value defensively so a downstream `value` is never `NaN`.
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
                    // Deliberately unclamped: an under-damped spring's
                    // overshoot past the target is real, intended motion
                    // (see the type docs' Overshoot section), and clamping
                    // it here would hide it. `value_clamped()` is the
                    // bounded-read escape hatch for consumers that need one.
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
