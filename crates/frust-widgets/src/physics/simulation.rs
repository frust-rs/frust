//! Concrete [`Simulation`] curves — the ballistic motion a
//! [`ScrollPhysics`](super::ScrollPhysics) hands back once a gesture releases.
//!
//! Four ports of Flutter's scroll math, formulas and tuning constants carried
//! over unchanged so a frust fling lands where the platform's own would:
//! `physics/friction_simulation.dart` ([`FrictionSimulation`]),
//! `physics/spring_simulation.dart` ([`SpringSimulation`],
//! [`ScrollSpringSimulation`]) and `widgets/scroll_simulation.dart`
//! ([`ClampingScrollSimulation`], [`BouncingScrollSimulation`]) — the last two
//! being in turn ports of Android's `SplineOverScroller` curve and iOS's
//! `UIScrollView` friction-then-rubber-band behavior. Every tuning constant is
//! stated as the expression its source states it as (never a transcribed
//! decimal) and pinned by a test in this file.
//!
//! [`TweenSimulation`] is frust's own, not a Flutter port: a plain eased
//! interpolation between two positions over a fixed duration, built from this
//! crate's own [`Curve`] vocabulary rather than Flutter's `Curve` class. It
//! backs [`crate::scroll_controller::ScrollController::animate_to`], draining
//! into the same ballistic driver a release fling runs through so paint
//! cadence and boundary handling are shared rather than duplicated.
//!
//! Time is in **seconds** from each simulation's own start, per the
//! [`Simulation`] contract; a simulation carries the [`Tolerance`] it settles
//! within as a constructor argument rather than reading one per call.

use frust_core::Curve;

use super::{Simulation, SpringDescription, Tolerance};

/// Flutter's `Tolerance.defaultTolerance` distance term — the fallback for a
/// simulation constructed without a device-derived [`Tolerance`], i.e.
/// [`FrictionSimulation::through`], which pins the velocity term to its own
/// requested end velocity and has no device metrics to derive the rest from.
const DEFAULT_DISTANCE_TOLERANCE: f64 = 1e-3;

/// Dart's `double.sign`, which [`f64::signum`] is not: `signum` reports `1.0`
/// for `0.0`, which would give a resting simulation a deceleration direction.
fn signum_or_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value.signum() }
}

/// Exponential drag decay: the iOS-style fling curve, and the friction phase of
/// [`BouncingScrollSimulation`].
///
/// `drag` is the fraction of velocity surviving one second and must sit in
/// `(0, 1)`. An optional constant deceleration subtracts a linear velocity term
/// on top of the drag, for a caller that wants a fling to stop sooner than pure
/// drag would; at `0.0` the curve is pure drag and only ever *approaches*
/// [`FrictionSimulation::final_x`].
#[derive(Debug, Clone)]
pub struct FrictionSimulation {
    drag: f64,
    drag_log: f64,
    position: f64,
    velocity: f64,
    /// Sign-corrected at construction so it always opposes `velocity`; `0.0`
    /// when either input is zero.
    constant_deceleration: f64,
    /// When the constant-deceleration term cancels the exponential one and the
    /// motion is over — [`f64::INFINITY`] under pure drag.
    final_time: f64,
    tolerance: Tolerance,
}

impl FrictionSimulation {
    /// A drag curve from `position` at `velocity` px/s.
    ///
    /// `constant_deceleration` is `0.0` for pure drag; a nonzero value is
    /// re-signed against `velocity` internally, so callers pass a magnitude.
    pub fn new(
        drag: f64,
        position: f64,
        velocity: f64,
        tolerance: Tolerance,
        constant_deceleration: f64,
    ) -> Self {
        let mut simulation = FrictionSimulation {
            drag,
            drag_log: drag.ln(),
            position,
            velocity,
            constant_deceleration: constant_deceleration * signum_or_zero(velocity),
            final_time: f64::INFINITY,
            tolerance,
        };
        simulation.final_time = simulation.solve_final_time();
        simulation
    }

    /// The drag curve that passes through both endpoints: at `start_x` moving
    /// `start_velocity`, at `end_x` moving `end_velocity` (Flutter's
    /// `FrictionSimulation.through`, whose `_dragFor` is the exponential
    /// below). Settles within `end_velocity`, the speed the caller asked to
    /// arrive at.
    pub fn through(start_x: f64, end_x: f64, start_velocity: f64, end_velocity: f64) -> Self {
        let drag = ((start_velocity - end_velocity) / (start_x - end_x)).exp();
        FrictionSimulation::new(
            drag,
            start_x,
            start_velocity,
            Tolerance {
                velocity: end_velocity.abs(),
                distance: DEFAULT_DISTANCE_TOLERANCE,
            },
            0.0,
        )
    }

    /// Where the curve ends up: the drag asymptote under pure drag, the
    /// position at the final time once a constant deceleration stops it early.
    pub fn final_x(&self) -> f64 {
        if self.constant_deceleration == 0.0 {
            self.position - self.velocity / self.drag_log
        } else {
            self.position_at(self.final_time)
        }
    }

    /// The time at which the curve reaches `x`, or [`f64::INFINITY`] if it
    /// never does (wrong side of the start, or beyond [`Self::final_x`]).
    ///
    /// Inverts the pure-drag position, so with a nonzero constant deceleration
    /// the answer is an over-estimate of how far the curve actually gets — the
    /// same approximation Flutter's `timeAtX` makes, and harmless for its one
    /// caller ([`BouncingScrollSimulation`] finding its edge crossing).
    pub fn time_at_x(&self, x: f64) -> f64 {
        if x == self.position {
            return 0.0;
        }
        let final_x = self.final_x();
        let out_of_reach = if self.velocity > 0.0 {
            x < self.position || x > final_x
        } else {
            x > self.position || x < final_x
        };
        if self.velocity == 0.0 || out_of_reach {
            return f64::INFINITY;
        }
        ((x - self.position + self.velocity / self.drag_log) * self.drag_log / self.velocity).ln()
            / self.drag_log
    }

    /// Position before the final-time clamp.
    fn position_at(&self, time: f64) -> f64 {
        let drag_term = self.position + self.velocity * self.drag.powf(time) / self.drag_log
            - self.velocity / self.drag_log;
        if self.constant_deceleration == 0.0 {
            // Skipping the term rather than adding a zero: `time` is
            // `INFINITY` under pure drag, and `0.0 * INFINITY` is NaN.
            drag_term
        } else {
            drag_term - self.constant_deceleration / 2.0 * time * time
        }
    }

    /// Velocity before the final-time clamp.
    fn velocity_at(&self, time: f64) -> f64 {
        let drag_term = self.velocity * self.drag.powf(time);
        if self.constant_deceleration == 0.0 {
            drag_term
        } else {
            drag_term - self.constant_deceleration * time
        }
    }

    /// Bisect for the instant the constant-deceleration term cancels the
    /// exponential one (velocity zero, motion over). `|v| / |a|` brackets that
    /// root from above for any `drag < 1`: the linear term has reached `|v|`
    /// there while the exponential one has decayed below it. A `drag` outside
    /// `(0, 1)` never cancels, and reports no final time rather than looping.
    fn solve_final_time(&self) -> f64 {
        if self.constant_deceleration == 0.0 {
            return f64::INFINITY;
        }
        let toward = signum_or_zero(self.velocity);
        let mut lower = 0.0;
        let mut upper = self.velocity.abs() / self.constant_deceleration.abs();
        if self.velocity_at(upper) * toward > 0.0 {
            return f64::INFINITY;
        }
        for _ in 0..64 {
            let middle = 0.5 * (lower + upper);
            if self.velocity_at(middle) * toward > 0.0 {
                lower = middle;
            } else {
                upper = middle;
            }
        }
        upper
    }
}

impl Simulation for FrictionSimulation {
    fn x(&self, time: f64) -> f64 {
        self.position_at(time.min(self.final_time))
    }

    fn dx(&self, time: f64) -> f64 {
        if time > self.final_time {
            0.0
        } else {
            self.velocity_at(time)
        }
    }

    /// Settled once the velocity is within tolerance — or, with a constant
    /// deceleration, once that term has cancelled the drag term outright,
    /// which a zero velocity tolerance (a [`Self::through`] curve asked to
    /// arrive stopped) would otherwise never report.
    fn is_done(&self, time: f64) -> bool {
        time >= self.final_time || self.dx(time).abs() < self.tolerance.velocity
    }
}

/// Which closed-form solution a [`SpringSimulation`] resolved to, decided by
/// `damping² − 4·mass·stiffness`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpringType {
    /// `damping² == 4·mass·stiffness` — returns in the shortest time that
    /// still never crosses the end position.
    Critical,
    /// `damping² > 4·mass·stiffness` — slower than critical, no oscillation.
    Overdamped,
    /// `damping² < 4·mass·stiffness` — oscillates about the end position while
    /// decaying.
    Underdamped,
}

/// The solved coefficients of `m·ẍ + c·ẋ + k·x = 0` on the *displacement* from
/// the end position, one variant per [`SpringType`].
#[derive(Debug, Clone, Copy)]
enum SpringSolution {
    Critical { r: f64, c1: f64, c2: f64 },
    Overdamped { r1: f64, r2: f64, c1: f64, c2: f64 },
    Underdamped { w: f64, r: f64, c1: f64, c2: f64 },
}

impl SpringSolution {
    fn new(spring: SpringDescription, distance: f64, velocity: f64) -> Self {
        let discriminant = spring.damping * spring.damping - 4.0 * spring.mass * spring.stiffness;
        if discriminant == 0.0 {
            let r = -spring.damping / (2.0 * spring.mass);
            SpringSolution::Critical {
                r,
                c1: distance,
                c2: velocity - r * distance,
            }
        } else if discriminant > 0.0 {
            let root = discriminant.sqrt();
            let r1 = (-spring.damping - root) / (2.0 * spring.mass);
            let r2 = (-spring.damping + root) / (2.0 * spring.mass);
            let c2 = (velocity - r1 * distance) / (r2 - r1);
            SpringSolution::Overdamped {
                r1,
                r2,
                c1: distance - c2,
                c2,
            }
        } else {
            let w = (4.0 * spring.mass * spring.stiffness - spring.damping * spring.damping).sqrt()
                / (2.0 * spring.mass);
            // The analytically correct decay rate, `−c / 2m`.
            let r = -spring.damping / (2.0 * spring.mass);
            SpringSolution::Underdamped {
                w,
                r,
                c1: distance,
                c2: (velocity - r * distance) / w,
            }
        }
    }

    fn spring_type(&self) -> SpringType {
        match self {
            SpringSolution::Critical { .. } => SpringType::Critical,
            SpringSolution::Overdamped { .. } => SpringType::Overdamped,
            SpringSolution::Underdamped { .. } => SpringType::Underdamped,
        }
    }

    /// Displacement from the end position at `time`.
    fn x(&self, time: f64) -> f64 {
        match *self {
            SpringSolution::Critical { r, c1, c2 } => (c1 + c2 * time) * (r * time).exp(),
            SpringSolution::Overdamped { r1, r2, c1, c2 } => {
                c1 * (r1 * time).exp() + c2 * (r2 * time).exp()
            }
            SpringSolution::Underdamped { w, r, c1, c2 } => {
                (r * time).exp() * (c1 * (w * time).cos() + c2 * (w * time).sin())
            }
        }
    }

    fn dx(&self, time: f64) -> f64 {
        match *self {
            SpringSolution::Critical { r, c1, c2 } => {
                let decay = (r * time).exp();
                r * (c1 + c2 * time) * decay + c2 * decay
            }
            SpringSolution::Overdamped { r1, r2, c1, c2 } => {
                c1 * r1 * (r1 * time).exp() + c2 * r2 * (r2 * time).exp()
            }
            SpringSolution::Underdamped { w, r, c1, c2 } => {
                let decay = (r * time).exp();
                let cosine = (w * time).cos();
                let sine = (w * time).sin();
                decay * (c2 * w * cosine - c1 * w * sine) + r * decay * (c2 * sine + c1 * cosine)
            }
        }
    }
}

/// A damped spring settling from `start` onto `end`, solving
/// `m·ẍ + c·ẋ + k·x = 0` on the displacement between them.
#[derive(Debug, Clone)]
pub struct SpringSimulation {
    end: f64,
    solution: SpringSolution,
    tolerance: Tolerance,
}

impl SpringSimulation {
    /// A spring released at `start` moving `velocity` px/s, pulling toward
    /// `end`.
    pub fn new(
        spring: SpringDescription,
        start: f64,
        end: f64,
        velocity: f64,
        tolerance: Tolerance,
    ) -> Self {
        SpringSimulation {
            end,
            solution: SpringSolution::new(spring, start - end, velocity),
            tolerance,
        }
    }

    /// The position this spring settles onto.
    pub fn end(&self) -> f64 {
        self.end
    }

    /// Which solution class the spring's parameters resolved to.
    pub fn spring_type(&self) -> SpringType {
        self.solution.spring_type()
    }
}

impl Simulation for SpringSimulation {
    fn x(&self, time: f64) -> f64 {
        self.end + self.solution.x(time)
    }

    fn dx(&self, time: f64) -> f64 {
        self.solution.dx(time)
    }

    fn is_done(&self, time: f64) -> bool {
        self.solution.x(time).abs() < self.tolerance.distance
            && self.solution.dx(time).abs() < self.tolerance.velocity
    }
}

/// A [`SpringSimulation`] that reports *exactly* `end` once settled, rather
/// than the residual sub-tolerance displacement the closed form still carries.
///
/// The scroll surfaces' spring: a bounce-back must leave the position on the
/// edge value it was springing to, not a fraction of a pixel off it, or the
/// next gesture starts from a technically-overscrolled position.
#[derive(Debug, Clone)]
pub struct ScrollSpringSimulation {
    inner: SpringSimulation,
}

impl ScrollSpringSimulation {
    /// A spring released at `start` moving `velocity` px/s, pulling toward
    /// `end`, snapping onto `end` once settled.
    pub fn new(
        spring: SpringDescription,
        start: f64,
        end: f64,
        velocity: f64,
        tolerance: Tolerance,
    ) -> Self {
        ScrollSpringSimulation {
            inner: SpringSimulation::new(spring, start, end, velocity, tolerance),
        }
    }

    /// The position this spring settles onto.
    pub fn end(&self) -> f64 {
        self.inner.end()
    }

    /// Which solution class the spring's parameters resolved to.
    pub fn spring_type(&self) -> SpringType {
        self.inner.spring_type()
    }
}

impl Simulation for ScrollSpringSimulation {
    fn x(&self, time: f64) -> f64 {
        if self.inner.is_done(time) {
            self.inner.end()
        } else {
            self.inner.x(time)
        }
    }

    fn dx(&self, time: f64) -> f64 {
        self.inner.dx(time)
    }

    fn is_done(&self, time: f64) -> bool {
        self.inner.is_done(time)
    }
}

/// Android's fling curve — the `SplineOverScroller` deceleration a
/// clamping scroll surface runs after a release, which decays to a hard stop
/// at a known duration and distance rather than an asymptote.
#[derive(Debug, Clone)]
pub struct ClampingScrollSimulation {
    position: f64,
    velocity: f64,
    /// Seconds until the curve stops.
    duration: f64,
    /// Total signed travel over `duration`.
    distance: f64,
    tolerance: Tolerance,
}

impl ClampingScrollSimulation {
    /// Android `ViewConfiguration`'s default scroll friction.
    pub const DEFAULT_FRICTION: f64 = 0.015;

    /// Android `SplineOverScroller.INFLEXION`, where the fling spline's two
    /// tension segments cross, and the velocity scale in the deceleration term
    /// below.
    pub const INFLEXION: f64 = 0.35;

    /// Android's `mPhysicalCoeff`: gravity (m/s²) × inches per meter × dpi ×
    /// an empirical tuning factor. The dpi term is fixed at 160 — one Android
    /// density-independent pixel, which is what this crate's logical pixel is.
    pub const PHYSICAL_COEFF: f64 = 9.80665 * 39.37 * 160.0 * 0.84;

    /// Android's `SplineOverScroller.DECELERATION_RATE` (≈ 2.358202).
    ///
    /// Computed from Android's own expression rather than transcribed as a
    /// decimal; a function rather than a `const` because [`f64::ln`] is not
    /// `const`.
    #[inline]
    pub fn deceleration_rate() -> f64 {
        0.78_f64.ln() / 0.9_f64.ln()
    }

    /// A fling from `position` at `velocity` px/s under `friction`
    /// ([`Self::DEFAULT_FRICTION`] for the platform default).
    pub fn new(position: f64, velocity: f64, friction: f64, tolerance: Tolerance) -> Self {
        let deceleration =
            (Self::INFLEXION * velocity.abs() / (friction * Self::PHYSICAL_COEFF)).ln();
        let duration = (deceleration / (Self::deceleration_rate() - 1.0)).exp();
        ClampingScrollSimulation {
            position,
            velocity,
            duration,
            distance: velocity * duration / Self::deceleration_rate(),
            tolerance,
        }
    }

    /// Seconds until the fling stops.
    pub fn duration(&self) -> f64 {
        self.duration
    }

    /// Where the fling stops.
    pub fn final_x(&self) -> f64 {
        self.position + self.distance
    }

    /// Fraction of the curve elapsed, clamped into `[0, 1]`. A zero-velocity
    /// fling has no duration at all and reads as already over, rather than
    /// dividing by zero.
    fn progress(&self, time: f64) -> f64 {
        if self.duration > 0.0 {
            (time / self.duration).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }
}

impl Simulation for ClampingScrollSimulation {
    fn x(&self, time: f64) -> f64 {
        let remaining = 1.0 - self.progress(time);
        self.position + self.distance * (1.0 - remaining.powf(Self::deceleration_rate()))
    }

    fn dx(&self, time: f64) -> f64 {
        let remaining = 1.0 - self.progress(time);
        self.velocity * remaining.powf(Self::deceleration_rate() - 1.0)
    }

    fn is_done(&self, time: f64) -> bool {
        time >= self.duration || self.dx(time).abs() < self.tolerance.velocity
    }
}

/// Which curve a [`BouncingScrollSimulation`] is running, and when it hands
/// over.
#[derive(Debug, Clone)]
enum BouncingPhase {
    /// Released already outside the range: the edge spring owns the whole
    /// motion.
    Spring(ScrollSpringSimulation),
    /// Released in range, decaying to a stop before either edge.
    Friction(FrictionSimulation),
    /// Released in range on course to hit an edge: friction until
    /// `spring_time`, the edge spring (started at that instant) after it.
    FrictionThenSpring {
        friction: FrictionSimulation,
        spring: ScrollSpringSimulation,
        spring_time: f64,
    },
}

/// iOS's fling: exponential friction while the position is in range, handing
/// over to a rubber-band spring at whichever edge the fling reaches.
#[derive(Debug, Clone)]
pub struct BouncingScrollSimulation {
    phase: BouncingPhase,
}

impl BouncingScrollSimulation {
    /// The fastest velocity handed to an edge spring. A fling arriving faster
    /// transfers this instead, so an arbitrarily hard flick cannot turn into an
    /// arbitrarily deep bounce.
    pub const MAX_SPRING_TRANSFER_VELOCITY: f64 = 5000.0;

    /// The friction phase's drag: `UIScrollView`'s normal deceleration rate is
    /// 0.998 per millisecond, and `0.998^1000 ≈ 0.135` per second.
    pub const FRICTION_DRAG: f64 = 0.135;

    /// A fling from `position` at `velocity` px/s within
    /// `leading_extent..=trailing_extent`, bouncing off whichever edge it
    /// reaches (or starting mid-bounce if `position` is already outside).
    pub fn new(
        position: f64,
        velocity: f64,
        leading_extent: f64,
        trailing_extent: f64,
        spring: SpringDescription,
        tolerance: Tolerance,
        constant_deceleration: f64,
    ) -> Self {
        debug_assert!(
            leading_extent <= trailing_extent,
            "leading extent {leading_extent} must not exceed trailing extent {trailing_extent}"
        );
        let phase = if position < leading_extent {
            BouncingPhase::Spring(Self::edge_spring(
                spring,
                position,
                leading_extent,
                velocity,
                tolerance,
            ))
        } else if position > trailing_extent {
            BouncingPhase::Spring(Self::edge_spring(
                spring,
                position,
                trailing_extent,
                velocity,
                tolerance,
            ))
        } else {
            let friction = FrictionSimulation::new(
                Self::FRICTION_DRAG,
                position,
                velocity,
                tolerance,
                constant_deceleration,
            );
            let heading_for = if velocity > 0.0 {
                trailing_extent
            } else if velocity < 0.0 {
                leading_extent
            } else {
                // A release with no velocity in range never reaches an edge.
                return BouncingScrollSimulation {
                    phase: BouncingPhase::Friction(friction),
                };
            };
            // Infinite means the friction curve stops short of that edge.
            let spring_time = friction.time_at_x(heading_for);
            if spring_time.is_finite() {
                let transfer = friction.dx(spring_time);
                BouncingPhase::FrictionThenSpring {
                    spring: Self::edge_spring(
                        spring,
                        heading_for,
                        heading_for,
                        transfer,
                        tolerance,
                    ),
                    friction,
                    spring_time,
                }
            } else {
                BouncingPhase::Friction(friction)
            }
        };
        BouncingScrollSimulation { phase }
    }

    /// The rubber-band spring back onto `extent`, its seed velocity clamped to
    /// ±[`Self::MAX_SPRING_TRANSFER_VELOCITY`].
    fn edge_spring(
        spring: SpringDescription,
        start: f64,
        extent: f64,
        velocity: f64,
        tolerance: Tolerance,
    ) -> ScrollSpringSimulation {
        ScrollSpringSimulation::new(
            spring,
            start,
            extent,
            velocity.clamp(
                -Self::MAX_SPRING_TRANSFER_VELOCITY,
                Self::MAX_SPRING_TRANSFER_VELOCITY,
            ),
            tolerance,
        )
    }

    /// The curve owning `time`, plus the offset to shift `time` by before
    /// querying it (a spring handed over mid-fling starts its own clock at the
    /// handover).
    fn active(&self, time: f64) -> (&dyn Simulation, f64) {
        match &self.phase {
            BouncingPhase::Spring(spring) => (spring, 0.0),
            BouncingPhase::Friction(friction) => (friction, 0.0),
            BouncingPhase::FrictionThenSpring {
                friction,
                spring,
                spring_time,
            } => {
                if time >= *spring_time {
                    (spring, *spring_time)
                } else {
                    (friction, 0.0)
                }
            }
        }
    }
}

impl Simulation for BouncingScrollSimulation {
    fn x(&self, time: f64) -> f64 {
        let (simulation, offset) = self.active(time);
        simulation.x(time - offset)
    }

    fn dx(&self, time: f64) -> f64 {
        let (simulation, offset) = self.active(time);
        simulation.dx(time - offset)
    }

    fn is_done(&self, time: f64) -> bool {
        let (simulation, offset) = self.active(time);
        simulation.is_done(time - offset)
    }
}

/// An eased tween from `start` to `end` over a fixed `duration` (seconds) —
/// see the module docs. Not a port: frust's own, built from [`Curve`] rather
/// than a Flutter source.
#[derive(Debug, Clone, Copy)]
pub struct TweenSimulation {
    start: f64,
    end: f64,
    /// `<= 0.0` (including a non-finite input, floored by
    /// [`TweenSimulation::new`]) means "immediately done at `end`" rather
    /// than a division by zero.
    duration: f64,
    curve: Curve,
}

impl TweenSimulation {
    /// A tween from `start` to `end` over `duration` seconds, eased by
    /// `curve`. A non-positive or non-finite `duration` (`0.0`, negative, or
    /// `NaN`, each a caller mistake this never panics on) collapses to a
    /// simulation that is immediately [`TweenSimulation::is_done`] at `end` —
    /// callers that actually want an instant move should prefer
    /// [`crate::scroll_controller::ScrollController::jump_to`], which never
    /// constructs a simulation at all.
    pub fn new(start: f64, end: f64, duration: f64, curve: Curve) -> Self {
        Self {
            start,
            end,
            duration: if duration.is_finite() {
                duration.max(0.0)
            } else {
                0.0
            },
            curve,
        }
    }
}

impl Simulation for TweenSimulation {
    fn x(&self, time: f64) -> f64 {
        if self.duration <= 0.0 {
            return self.end;
        }
        let t = (time / self.duration).clamp(0.0, 1.0);
        self.start + (self.end - self.start) * self.curve.transform(t)
    }

    fn dx(&self, time: f64) -> f64 {
        if self.duration <= 0.0 || time < 0.0 || time >= self.duration {
            return 0.0;
        }
        // A numeric derivative: cheap, and the only option for an arbitrary
        // cubic-Bézier `Curve` with no closed-form velocity. Only
        // `ScrollWidget::ballistic_is_pinned_outward`'s sign check and a
        // carried-momentum read (if a fling starts mid-tween) ever read this,
        // neither of which needs analytic precision.
        const DT: f64 = 1e-4;
        let clamped_time = time.min(self.duration - DT);
        (self.x(clamped_time + DT) - self.x(clamped_time)) / DT
    }

    fn is_done(&self, time: f64) -> bool {
        self.duration <= 0.0 || time >= self.duration
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1.0-dpr device tolerance: 20 px/s, 1 px.
    fn tol() -> Tolerance {
        Tolerance::for_device_pixel_ratio(1.0)
    }

    fn assert_close(actual: f64, expected: f64, epsilon: f64, what: &str) {
        assert!(
            (actual - expected).abs() < epsilon,
            "{what}: {actual} is not within {epsilon} of {expected}"
        );
    }

    #[test]
    fn friction_final_x_matches_closed_form() {
        let simulation = FrictionSimulation::new(0.135, 20.0, 1000.0, tol(), 0.0);
        let expected = 20.0 - 1000.0 / 0.135_f64.ln();
        assert_close(simulation.final_x(), expected, 1e-9, "final_x");
        assert_close(simulation.x(f64::INFINITY), expected, 1e-9, "x at infinity");
        assert_close(simulation.x(20.0), expected, 1e-6, "x long after release");
        assert!(!simulation.is_done(0.0), "a live fling reported settled");
        assert!(simulation.is_done(20.0), "a decayed fling never settled");
    }

    #[test]
    fn friction_through_passes_endpoints() {
        let simulation = FrictionSimulation::through(0.0, 100.0, 500.0, 100.0);
        let arrival = simulation.time_at_x(100.0);
        assert!(arrival.is_finite(), "end position unreachable at {arrival}");
        assert_close(simulation.x(0.0), 0.0, 1e-9, "start position");
        assert_close(simulation.dx(0.0), 500.0, 1e-9, "start velocity");
        assert_close(simulation.x(arrival), 100.0, 1e-9, "end position");
        assert_close(simulation.dx(arrival), 100.0, 1e-9, "end velocity");

        // Arriving stopped is the asymptote case: the curve only approaches the
        // end position, so `through` puts it exactly at `final_x`.
        let asymptotic = FrictionSimulation::through(0.0, 100.0, 500.0, 0.0);
        assert_close(asymptotic.final_x(), 100.0, 1e-9, "asymptotic final_x");
        assert!(
            asymptotic.time_at_x(200.0).is_infinite(),
            "a position past final_x must be unreachable"
        );
        assert!(
            asymptotic.time_at_x(-10.0).is_infinite(),
            "a position behind the start must be unreachable"
        );
    }

    #[test]
    fn friction_velocity_decays_exponentially() {
        let simulation = FrictionSimulation::new(0.135, 0.0, 800.0, tol(), 0.0);
        let ratio = simulation.dx(0.5) / simulation.dx(0.0);
        assert_close(ratio, 0.135_f64.powf(0.5), 1e-9, "half-second decay ratio");
    }

    #[test]
    fn friction_constant_deceleration_ends_the_motion() {
        let plain = FrictionSimulation::new(0.135, 0.0, 1000.0, tol(), 0.0);
        let decelerated = FrictionSimulation::new(0.135, 0.0, 1000.0, tol(), 3000.0);
        assert!(
            decelerated.final_x() < plain.final_x(),
            "constant deceleration travelled {}, no further than pure drag's {}",
            decelerated.final_x(),
            plain.final_x()
        );
        assert!(decelerated.is_done(2.0), "the motion never ended");
        assert_eq!(decelerated.dx(5.0), 0.0, "velocity past the end");
        assert_close(
            decelerated.x(5.0),
            decelerated.final_x(),
            1e-9,
            "position past the end",
        );
    }

    #[test]
    fn spring_critical_over_under_damped_all_converge_to_end() {
        // `damping² − 4·mass·stiffness` picks the class: 400 − 400 == 0,
        // 900 − 400 > 0, 25 − 400 < 0.
        let cases = [
            (20.0, SpringType::Critical),
            (30.0, SpringType::Overdamped),
            (5.0, SpringType::Underdamped),
        ];
        for (damping, expected_type) in cases {
            let spring = SpringDescription {
                mass: 1.0,
                stiffness: 100.0,
                damping,
            };
            let simulation = SpringSimulation::new(spring, 100.0, 0.0, 0.0, tol());
            assert_eq!(simulation.spring_type(), expected_type);
            assert_close(simulation.x(0.0), 100.0, 1e-9, "start position");
            assert_close(simulation.dx(0.0), 0.0, 1e-9, "start velocity");
            assert_close(simulation.x(10.0), 0.0, 1e-6, "converged position");
            assert!(
                !simulation.is_done(0.0),
                "{expected_type:?} spring reported settled at release"
            );
            assert!(
                simulation.is_done(10.0),
                "{expected_type:?} spring never settled"
            );
        }
    }

    #[test]
    fn spring_with_damping_ratio_1_1_is_overdamped_no_oscillation() {
        let simulation = SpringSimulation::new(
            SpringDescription::default_scroll_spring(),
            100.0,
            0.0,
            0.0,
            tol(),
        );
        assert_eq!(simulation.spring_type(), SpringType::Overdamped);
        for step in 0..=2000 {
            let time = f64::from(step) / 1000.0;
            let displacement = simulation.x(time) - simulation.end();
            assert!(
                displacement >= 0.0,
                "crossed the end position at t={time}s (displacement {displacement})"
            );
        }
    }

    #[test]
    fn scroll_spring_snaps_to_end_once_settled() {
        let spring = SpringDescription::default_scroll_spring();
        let plain = SpringSimulation::new(spring, -50.0, 0.0, 0.0, tol());
        let snapping = ScrollSpringSimulation::new(spring, -50.0, 0.0, 0.0, tol());

        // Live: the two agree exactly.
        assert_eq!(snapping.x(0.05), plain.x(0.05));
        assert_eq!(snapping.dx(0.05), plain.dx(0.05));

        // Settled: the plain spring still carries a sub-tolerance residual, the
        // scroll spring reports the edge itself.
        assert!(snapping.is_done(3.0));
        assert_ne!(plain.x(3.0), 0.0);
        assert_eq!(snapping.x(3.0), snapping.end());
    }

    #[test]
    fn clamping_deceleration_rate_value() {
        assert_close(
            ClampingScrollSimulation::deceleration_rate(),
            2.358_202,
            1e-5,
            "DECELERATION_RATE",
        );
        assert_close(
            ClampingScrollSimulation::PHYSICAL_COEFF,
            51_890.2,
            0.1,
            "PHYSICAL_COEFF",
        );
        assert_eq!(ClampingScrollSimulation::INFLEXION, 0.35);
        assert_eq!(ClampingScrollSimulation::DEFAULT_FRICTION, 0.015);
    }

    #[test]
    fn clamping_dx_at_zero_equals_initial_velocity() {
        for velocity in [2000.0, -2000.0, 350.0] {
            let simulation = ClampingScrollSimulation::new(
                0.0,
                velocity,
                ClampingScrollSimulation::DEFAULT_FRICTION,
                tol(),
            );
            assert_close(simulation.dx(0.0), velocity, 1e-9, "dx at release");
        }
    }

    #[test]
    fn clamping_stops_at_duration() {
        let simulation = ClampingScrollSimulation::new(
            10.0,
            2000.0,
            ClampingScrollSimulation::DEFAULT_FRICTION,
            tol(),
        );
        let duration = simulation.duration();
        let target = simulation.final_x();
        assert!(
            duration > 0.0 && duration.is_finite(),
            "implausible duration {duration}"
        );
        assert!(target > 10.0, "a positive fling must travel forward");
        assert!(!simulation.is_done(0.0), "a live fling reported settled");
        assert!(simulation.is_done(duration), "the fling never stopped");
        assert_close(simulation.x(duration), target, 1e-9, "position at duration");
        assert_close(
            simulation.x(duration * 2.0),
            target,
            1e-9,
            "position past duration",
        );
        assert_close(simulation.dx(duration), 0.0, 1e-9, "velocity at duration");

        let mut previous = simulation.x(0.0);
        for step in 1..=100 {
            let position = simulation.x(duration * f64::from(step) / 100.0);
            assert!(position >= previous, "backtracked to {position}");
            assert!(position <= target + 1e-9, "overshot the target: {position}");
            previous = position;
        }
    }

    #[test]
    fn bouncing_switches_friction_to_spring_at_extent() {
        let trailing = 100.0;
        let simulation = BouncingScrollSimulation::new(
            0.0,
            2000.0,
            0.0,
            trailing,
            SpringDescription::default_scroll_spring(),
            tol(),
            0.0,
        );
        let friction = FrictionSimulation::new(
            BouncingScrollSimulation::FRICTION_DRAG,
            0.0,
            2000.0,
            tol(),
            0.0,
        );
        let handover = friction.time_at_x(trailing);
        assert!(handover.is_finite(), "the fling must reach the extent");

        // Friction phase: identical to the standalone friction curve.
        let early = handover / 2.0;
        assert_eq!(simulation.x(early), friction.x(early));
        assert_eq!(simulation.dx(early), friction.dx(early));

        // Handover: the spring starts at the extent carrying the friction
        // curve's velocity.
        assert_close(
            simulation.x(handover),
            trailing,
            1e-9,
            "position at handover",
        );
        assert_close(
            simulation.dx(handover),
            friction.dx(handover),
            1e-9,
            "velocity at handover",
        );

        // Spring phase: bounded overshoot, then back onto the extent.
        let mut furthest = f64::MIN;
        for step in 0..=500 {
            furthest = furthest.max(simulation.x(handover + f64::from(step) / 100.0));
        }
        assert!(
            furthest > trailing,
            "the bounce never passed the extent (peaked at {furthest})"
        );
        assert!(
            furthest < trailing + 60.0,
            "the bounce ran away past the extent (peaked at {furthest})"
        );
        assert_close(
            simulation.x(handover + 5.0),
            trailing,
            1e-9,
            "settled position",
        );
        assert!(
            simulation.is_done(handover + 5.0),
            "the bounce never settled"
        );
    }

    #[test]
    fn bouncing_stays_friction_when_no_extent_is_reached() {
        let simulation = BouncingScrollSimulation::new(
            0.0,
            100.0,
            0.0,
            10_000.0,
            SpringDescription::default_scroll_spring(),
            tol(),
            0.0,
        );
        let friction = FrictionSimulation::new(
            BouncingScrollSimulation::FRICTION_DRAG,
            0.0,
            100.0,
            tol(),
            0.0,
        );
        assert!(
            friction.time_at_x(10_000.0).is_infinite(),
            "this fling must stop short of the extent for the test to mean anything"
        );
        for step in 0..=100 {
            let time = f64::from(step) / 10.0;
            assert_eq!(simulation.x(time), friction.x(time));
            assert_eq!(simulation.dx(time), friction.dx(time));
        }
    }

    #[test]
    fn bouncing_underscroll_starts_as_spring() {
        let leading = 0.0;
        let simulation = BouncingScrollSimulation::new(
            -50.0,
            0.0,
            leading,
            100.0,
            SpringDescription::default_scroll_spring(),
            tol(),
            0.0,
        );
        assert_close(simulation.x(0.0), -50.0, 1e-9, "starts where released");
        assert!(
            simulation.x(0.1) > simulation.x(0.0),
            "must travel back toward the leading extent"
        );
        assert!(!simulation.is_done(0.0), "an overscrolled rest is not done");
        assert_close(simulation.x(3.0), leading, 1e-9, "settled position");
        assert!(simulation.is_done(3.0), "the bounce-back never settled");
    }

    #[test]
    fn bouncing_spring_transfer_velocity_capped() {
        let spring = SpringDescription::default_scroll_spring();
        let cap = BouncingScrollSimulation::MAX_SPRING_TRANSFER_VELOCITY;

        // Released already past the leading edge, absurdly fast.
        let underscroll =
            BouncingScrollSimulation::new(-50.0, -20_000.0, 0.0, 100.0, spring, tol(), 0.0);
        assert_close(underscroll.dx(0.0), -cap, 1e-9, "clamped underscroll seed");

        // Reaching the trailing edge under friction faster than the cap.
        let fling = BouncingScrollSimulation::new(0.0, 50_000.0, 0.0, 100.0, spring, tol(), 0.0);
        let friction = FrictionSimulation::new(
            BouncingScrollSimulation::FRICTION_DRAG,
            0.0,
            50_000.0,
            tol(),
            0.0,
        );
        let handover = friction.time_at_x(100.0);
        assert!(
            friction.dx(handover) > cap,
            "the fling must arrive above the cap for the test to mean anything"
        );
        assert_close(fling.dx(handover), cap, 1e-9, "clamped handover seed");
    }

    #[test]
    fn tween_reaches_the_target_exactly_at_its_duration() {
        let tween = TweenSimulation::new(0.0, 100.0, 0.5, Curve::Linear);
        assert_close(tween.x(0.0), 0.0, 1e-9, "starts at the source position");
        assert_close(tween.x(0.25), 50.0, 1e-9, "linear curve, halfway in time");
        assert!(!tween.is_done(0.25));
        assert_close(tween.x(0.5), 100.0, 1e-9, "lands exactly on the target");
        assert!(tween.is_done(0.5));
        // Past its own duration the position holds at the target rather than
        // extrapolating past it.
        assert_close(tween.x(1.0), 100.0, 1e-9, "clamped past the duration");
        assert!(tween.is_done(1.0));
    }

    #[test]
    fn tween_velocity_is_positive_mid_flight_and_zero_once_done() {
        let tween = TweenSimulation::new(0.0, 100.0, 1.0, Curve::EaseInOut);
        assert!(
            tween.dx(0.5) > 0.0,
            "moving from start to end, mid-flight velocity is positive"
        );
        assert_eq!(tween.dx(1.0), 0.0, "settled — no velocity once done");
        assert_eq!(tween.dx(-1.0), 0.0, "never queried before its own start");
    }

    #[test]
    fn tween_non_positive_duration_is_immediately_done() {
        // A `0.0`/negative/NaN duration never divides by zero — it is
        // immediately done at `t = 0.0`, at the end position.
        for duration in [0.0, -5.0, f64::NAN] {
            let tween = TweenSimulation::new(10.0, 20.0, duration, Curve::Linear);
            assert_close(tween.x(0.0), 20.0, 1e-9, "immediately at the target");
            assert!(tween.is_done(0.0));
        }
    }
}
