//! The platform-parity physics: ports of Flutter's `BouncingScrollPhysics`
//! (iOS), `ClampingScrollPhysics` (Android), `AlwaysScrollableScrollPhysics`
//! and `NeverScrollableScrollPhysics` from `widgets/scroll_physics.dart`,
//! driving the simulations in [`super::simulation`].
//!
//! Every tuning constant is Flutter's own, stated as the expression its source
//! states it as and pinned by a test below. The two behavioral ones — the
//! bouncing friction curve and the clamping hard stop — are what make a fling
//! land where the platform's own would.
//!
//! # How a drag reaches [`Bouncing`], and what that costs
//!
//! Flutter feeds `applyPhysicsToUserOffset` a *per-frame delta* against a
//! position that may itself be out of range, so its friction tightens as the
//! pull deepens. Both frust scroll surfaces instead re-map from a **clamped
//! base** every frame ([`crate::scroll::ScrollWidget`]'s drag path): the
//! metrics they pass always report an in-range `pixels`, and the offset is the
//! whole accumulated past-edge excursion, not a delta. [`Bouncing`] answers
//! either convention correctly — it derives the resisted portion from the
//! metrics it is handed rather than assuming one — but under the clamped-base
//! convention the overscroll it reads is always `0.0`, so the factor it applies
//! is always [`DecelerationRate::NORMAL_FRICTION`]'s value at zero depth: a
//! *linear* rubber band at `0.52`, not a tightening one. Progressive tension
//! needs the surface to hand over its real out-of-range position, which is a
//! change to those widgets, not to this module.
//!
//! # Why [`Clamping`] needs no clamped simulation adapter
//!
//! Its ballistic curve ([`ClampingScrollSimulation`]) is free to run far past
//! an edge — Android's spline knows nothing about extents. Nothing here clamps
//! it, because the widget's generic ballistic driver subtracts what
//! [`ScrollPhysics::apply_boundary_conditions`] rejects from *every* simulation
//! position it paints (`ScrollWidget`'s `drive_ballistic`), and this physics
//! rejects the whole excess — so the painted offset stops dead at the edge
//! while the raw curve keeps going. `clamping_ballistic_returns_android_curve_sim`
//! re-derives that contract in-file so a driver change cannot silently break it.
//!
//! # Deviations from the Dart source
//!
//! Four, each narrow and deliberate:
//!
//! * **Easing back out of an overscroll is unresisted at both rates.** Flutter
//!   returns the delta untouched only at [`DecelerationRate::Fast`]; at
//!   `Normal` it still applies a reduced-depth friction factor. One rule for
//!   both rates keeps the asymmetry that carries the feel (resist deepening,
//!   never resist returning) without the second curve.
//! * **[`Clamping::apply_boundary_conditions`] is the two-branch form** (past
//!   an extent → reject the whole excess) rather than Flutter's four-branch
//!   one. The two differ only for a position that is *already* out of range,
//!   which this physics never produces: there, Flutter rejects only further
//!   outward travel and lets the position linger outside, while this form pulls
//!   it back to the extent on the next proposal.
//! * **A fling starts at [`ScrollPhysics::min_fling_velocity`], not at the
//!   tolerance velocity.** Flutter gates `createBallisticSimulation` on
//!   `tolerance.velocity` (~20 px/s at 1.0 dpr). The surfaces here already zero
//!   any release below the physics' own minimum before asking, so the two gates
//!   answer identically at that seam; stating the real threshold is honest
//!   about what a direct caller gets.
//! * **[`Clamping`]'s out-of-range spring keeps the release velocity.** Flutter
//!   passes `min(0.0, velocity)`, which is only meaningful at a trailing edge;
//!   the actual velocity is symmetric and is what the bouncing port already
//!   does.

use super::simulation::{
    BouncingScrollSimulation, ClampingScrollSimulation, ScrollSpringSimulation,
};
use super::{ScrollMetrics, ScrollPhysics, Simulation, SpringDescription};

/// How quickly a [`Bouncing`] surface's fling decays — Flutter's
/// `ScrollDecelerationRate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DecelerationRate {
    /// iOS's own rate: a long, low-friction glide with no constant
    /// deceleration term and the default edge spring.
    #[default]
    Normal,
    /// A shorter, snappier glide: half the overscroll friction, a stiffer edge
    /// spring, and a constant deceleration on top of the drag curve.
    Fast,
}

impl DecelerationRate {
    /// The overscroll friction factor at zero depth for
    /// [`DecelerationRate::Normal`] — the `0.52` in Flutter's
    /// `frictionFactor`.
    pub const NORMAL_FRICTION: f64 = 0.52;

    /// The same factor for [`DecelerationRate::Fast`]: half as much of a
    /// past-edge pull reaches the position.
    pub const FAST_FRICTION: f64 = 0.26;

    /// The constant deceleration (px/s²) [`DecelerationRate::Fast`] adds to its
    /// fling's drag curve, which is what stops it early instead of letting it
    /// approach an asymptote. `Normal` adds none.
    pub const FAST_CONSTANT_DECELERATION: f64 = 1400.0;

    /// The factor a past-edge pull is scaled by at zero overscroll depth.
    fn friction_coefficient(self) -> f64 {
        match self {
            DecelerationRate::Normal => Self::NORMAL_FRICTION,
            DecelerationRate::Fast => Self::FAST_FRICTION,
        }
    }

    /// The constant deceleration term this rate's ballistic curve carries.
    fn constant_deceleration(self) -> f64 {
        match self {
            DecelerationRate::Normal => 0.0,
            DecelerationRate::Fast => Self::FAST_CONSTANT_DECELERATION,
        }
    }

    /// The edge spring this rate bounces back on.
    ///
    /// `Normal` has no spring of its own in the Dart source —
    /// `BouncingScrollPhysics.spring` overrides only the fast case, so the
    /// normal one inherits `ScrollPhysics.spring`, i.e. `_kDefaultSpring`
    /// ([`SpringDescription::default_scroll_spring`], mass 0.5 / stiffness 100
    /// / damping ratio 1.1). A mass 0.3 / stiffness 100 / ratio 0.7 pairing
    /// circulates for it second-hand; the inheritance above is the primary
    /// source and refutes it.
    fn spring(self) -> SpringDescription {
        match self {
            DecelerationRate::Normal => SpringDescription::default_scroll_spring(),
            DecelerationRate::Fast => SpringDescription::with_damping_ratio(0.3, 75.0, 1.3),
        }
    }
}

/// Split out-of-range travel between two signed past-edge displacements into
/// the part that eases back toward the range and the part that deepens the
/// overscroll. Both terms are signed like the travel itself and sum to
/// `end − start`.
fn split_overscroll_travel(start: f64, end: f64) -> (f64, f64) {
    if start * end < 0.0 {
        // Off one edge and past the other in a single delta: all the way back
        // in first, then the whole of the new excursion is tensioning.
        (-start, end)
    } else if end.abs() >= start.abs() {
        (0.0, end - start)
    } else {
        (end - start, 0.0)
    }
}

/// iOS's scroll feel — Flutter's `BouncingScrollPhysics`: a drag may pull the
/// position past an edge against a friction factor that tightens with depth,
/// nothing is ever boundary-rejected, and a release runs an exponential
/// friction curve that hands over to a rubber-band spring at whichever edge it
/// reaches.
///
/// See the [module docs](self) for how the surfaces' drag convention flattens
/// the friction curve, and for the deviations from the Dart source.
#[derive(Debug, Default)]
pub struct Bouncing {
    rate: DecelerationRate,
    parent: Option<Box<dyn ScrollPhysics>>,
}

impl Bouncing {
    /// Flutter's `BouncingScrollPhysics.minFlingVelocity`: twice
    /// [`super::MIN_FLING_VELOCITY`], because this physics' ballistic curve
    /// decelerates more slowly than the clamping one, so a fling has to be a
    /// more deliberate flick to be worth starting.
    pub const MIN_FLING_VELOCITY: f64 = super::MIN_FLING_VELOCITY * 2.0;

    /// The scale of Flutter's power-curve fit for momentum carried into a
    /// re-fling (`carriedMomentum`), fitted against superimposed platform
    /// scroll views rather than derived.
    const MOMENTUM_COEFFICIENT: f64 = 0.000_816;

    /// That fit's exponent.
    const MOMENTUM_EXPONENT: f64 = 1.967;

    /// The most momentum (px/s) a re-fling can carry over, so a fast enough
    /// existing motion cannot compound without bound.
    const MOMENTUM_CAP: f64 = 40_000.0;

    /// A standalone bouncing physics at [`DecelerationRate::Normal`].
    pub fn new() -> Self {
        Self::default()
    }

    /// A standalone bouncing physics at `rate`.
    pub fn with_rate(rate: DecelerationRate) -> Self {
        Self { rate, parent: None }
    }

    /// Chain `parent` behind this physics, the module-wide composition
    /// convention ([`ScrollPhysics`]' *Chaining*).
    pub fn chain(self, parent: impl ScrollPhysics + 'static) -> Self {
        Self {
            rate: self.rate,
            parent: Some(Box::new(parent)),
        }
    }

    /// The fraction of a past-edge pull that reaches the position at an
    /// overscroll of `overscroll_fraction` viewports — Flutter's
    /// `frictionFactor`: `0.52·(1 − f)²` at [`DecelerationRate::Normal`],
    /// `0.26·(1 − f)²` at [`DecelerationRate::Fast`].
    pub fn friction_factor(&self, overscroll_fraction: f64) -> f64 {
        let remaining = 1.0 - overscroll_fraction;
        self.rate.friction_coefficient() * remaining * remaining
    }

    /// How deep the metrics' position already sits past an edge, as a fraction
    /// of the viewport — the argument to [`Self::friction_factor`]. A surface
    /// with no viewport yet reports `0.0` rather than dividing by zero.
    fn overscroll_fraction(metrics: &ScrollMetrics, displacement: f64) -> f64 {
        if metrics.viewport_dimension > 0.0 {
            displacement.abs() / metrics.viewport_dimension
        } else {
            0.0
        }
    }
}

impl ScrollPhysics for Bouncing {
    fn parent(&self) -> Option<&dyn ScrollPhysics> {
        self.parent.as_deref()
    }

    /// Travel inside the range passes through untouched, travel that *deepens*
    /// an overscroll is scaled by [`Self::friction_factor`] at the depth
    /// already held, and travel easing back toward the range passes through
    /// untouched too. That asymmetry is the signature of the feel: pulling
    /// further out gets progressively heavier, letting it back never fights
    /// the finger — though the depth term reads `0.0` under the surfaces'
    /// clamped-base drag convention (see the [module docs](self)).
    fn apply_physics_to_user_offset(&self, metrics: &ScrollMetrics, offset: f64) -> f64 {
        let min = metrics.min_scroll_extent;
        let max = metrics.max_scroll_extent;
        let start = metrics.pixels;
        let end = start + offset;
        // The three parts of the delta: travel within the range, travel that
        // reduces an out-of-range displacement, and travel that grows one.
        let in_range = end.clamp(min, max) - start.clamp(min, max);
        let displacement_start = start - start.clamp(min, max);
        let displacement_end = end - end.clamp(min, max);
        let (easing, tensioning) = split_overscroll_travel(displacement_start, displacement_end);
        let friction = self.friction_factor(Self::overscroll_fraction(metrics, displacement_start));
        in_range + easing + tensioning * friction
    }

    /// Nothing is ever rejected — holding an out-of-range position is the whole
    /// point of a bouncing surface, and the edge spring (not a boundary rule)
    /// is what returns it.
    fn apply_boundary_conditions(&self, _metrics: &ScrollMetrics, _value: f64) -> f64 {
        0.0
    }

    /// An overscrolled surface always gets its spring back, in range a fling
    /// needs [`Self::MIN_FLING_VELOCITY`], and anything else is no motion at
    /// all.
    fn create_ballistic_simulation(
        &self,
        metrics: &ScrollMetrics,
        velocity: f64,
    ) -> Option<Box<dyn Simulation>> {
        if !metrics.out_of_range() && velocity.abs() < self.min_fling_velocity() {
            return None;
        }
        Some(Box::new(BouncingScrollSimulation::new(
            metrics.pixels,
            velocity,
            metrics.min_scroll_extent,
            metrics.max_scroll_extent,
            self.spring(),
            self.tolerance_for(metrics),
            self.rate.constant_deceleration(),
        )))
    }

    /// Always `true`: an iOS surface whose content fits its viewport still
    /// scrolls — it just bounces straight back.
    fn should_accept_user_offset(&self, _metrics: &ScrollMetrics) -> bool {
        true
    }

    /// Flutter's fitted power curve, capped at `MOMENTUM_CAP` (40000 px/s) and
    /// signed like the motion it carries over.
    fn carried_momentum(&self, existing_velocity: f64) -> f64 {
        let magnitude =
            Self::MOMENTUM_COEFFICIENT * existing_velocity.abs().powf(Self::MOMENTUM_EXPONENT);
        existing_velocity.signum() * magnitude.min(Self::MOMENTUM_CAP)
    }

    fn min_fling_velocity(&self) -> f64 {
        Self::MIN_FLING_VELOCITY
    }

    fn spring(&self) -> SpringDescription {
        self.rate.spring()
    }
}

/// Android's scroll feel — Flutter's `ClampingScrollPhysics`: a drag never
/// leaves the range (the excess is rejected outright, for the surface to show
/// as a stretch or a glow instead), and a release runs Android's own
/// `SplineOverScroller` deceleration to a hard stop.
///
/// See the [module docs](self) for why nothing here clamps that curve itself.
#[derive(Debug, Default)]
pub struct Clamping {
    parent: Option<Box<dyn ScrollPhysics>>,
}

impl Clamping {
    /// A standalone clamping physics (no chained parent).
    pub fn new() -> Self {
        Self::default()
    }

    /// Chain `parent` behind this physics, the module-wide composition
    /// convention ([`ScrollPhysics`]' *Chaining*).
    pub fn chain(self, parent: impl ScrollPhysics + 'static) -> Self {
        Self {
            parent: Some(Box::new(parent)),
        }
    }
}

impl ScrollPhysics for Clamping {
    fn parent(&self) -> Option<&dyn ScrollPhysics> {
        self.parent.as_deref()
    }

    /// The whole of any past-extent excess is rejected, so the position itself
    /// never leaves `[min, max]`. The caller still learns how far the proposal
    /// went — that rejected distance is what a stretch or glow effect reads
    /// (this module's parent's *design ruling*).
    fn apply_boundary_conditions(&self, metrics: &ScrollMetrics, value: f64) -> f64 {
        if value < metrics.min_scroll_extent {
            value - metrics.min_scroll_extent
        } else if value > metrics.max_scroll_extent {
            value - metrics.max_scroll_extent
        } else {
            0.0
        }
    }

    /// Android's fling curve for a release with room to travel, and — as a
    /// safety arm this physics' own boundary rule makes unreachable — a spring
    /// back to the nearest extent for a position that starts out of range.
    fn create_ballistic_simulation(
        &self,
        metrics: &ScrollMetrics,
        velocity: f64,
    ) -> Option<Box<dyn Simulation>> {
        let tolerance = self.tolerance_for(metrics);
        if metrics.out_of_range() {
            let end = if metrics.pixels > metrics.max_scroll_extent {
                metrics.max_scroll_extent
            } else {
                metrics.min_scroll_extent
            };
            return Some(Box::new(ScrollSpringSimulation::new(
                self.spring(),
                metrics.pixels,
                end,
                velocity,
                tolerance,
            )));
        }
        if velocity.abs() < self.min_fling_velocity() {
            return None;
        }
        // Already pinned against the edge the release is heading for: there is
        // nothing to fling into.
        if velocity > 0.0 && metrics.pixels >= metrics.max_scroll_extent {
            return None;
        }
        if velocity < 0.0 && metrics.pixels <= metrics.min_scroll_extent {
            return None;
        }
        Some(Box::new(ClampingScrollSimulation::new(
            metrics.pixels,
            velocity,
            ClampingScrollSimulation::DEFAULT_FRICTION,
            tolerance,
        )))
    }
}

/// Flutter's `AlwaysScrollableScrollPhysics`: accept a drag whether or not
/// there is anything to scroll, and defer everything else to the chained
/// parent.
///
/// Composed rather than used alone — `AlwaysScrollable::new().chain(Clamping::new())`
/// is a clamping surface that a pull-to-refresh gesture can still reach on a
/// short page.
#[derive(Debug, Default)]
pub struct AlwaysScrollable {
    parent: Option<Box<dyn ScrollPhysics>>,
}

impl AlwaysScrollable {
    /// A standalone always-scrollable physics (no chained parent).
    pub fn new() -> Self {
        Self::default()
    }

    /// Chain `parent` behind this physics, the module-wide composition
    /// convention ([`ScrollPhysics`]' *Chaining*).
    pub fn chain(self, parent: impl ScrollPhysics + 'static) -> Self {
        Self {
            parent: Some(Box::new(parent)),
        }
    }
}

impl ScrollPhysics for AlwaysScrollable {
    fn parent(&self) -> Option<&dyn ScrollPhysics> {
        self.parent.as_deref()
    }

    /// The one thing this physics has an opinion about.
    fn should_accept_user_offset(&self, _metrics: &ScrollMetrics) -> bool {
        true
    }
}

/// Flutter's `NeverScrollableScrollPhysics`: refuse every drag, deferring
/// everything else to the chained parent — an inner list inside an outer
/// scroller, or a temporarily locked surface.
#[derive(Debug, Default)]
pub struct NeverScrollable {
    parent: Option<Box<dyn ScrollPhysics>>,
}

impl NeverScrollable {
    /// A standalone never-scrollable physics (no chained parent).
    pub fn new() -> Self {
        Self::default()
    }

    /// Chain `parent` behind this physics, the module-wide composition
    /// convention ([`ScrollPhysics`]' *Chaining*).
    pub fn chain(self, parent: impl ScrollPhysics + 'static) -> Self {
        Self {
            parent: Some(Box::new(parent)),
        }
    }
}

impl ScrollPhysics for NeverScrollable {
    fn parent(&self) -> Option<&dyn ScrollPhysics> {
        self.parent.as_deref()
    }

    /// The one thing this physics has an opinion about.
    fn should_accept_user_offset(&self, _metrics: &ScrollMetrics) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::Tolerance;

    /// A 100px viewport at 1.0 dpr, so an overscroll fraction reads as
    /// hundredths and the tolerance is a round 20 px/s.
    fn metrics(pixels: f64, max: f64) -> ScrollMetrics {
        ScrollMetrics {
            pixels,
            min_scroll_extent: 0.0,
            max_scroll_extent: max,
            viewport_dimension: 100.0,
            device_pixel_ratio: 1.0,
        }
    }

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
    fn bouncing_friction_factor_curve() {
        let normal = Bouncing::new();
        assert_close(normal.friction_factor(0.0), 0.52, 1e-12, "normal at rest");
        assert_close(normal.friction_factor(0.5), 0.13, 1e-12, "normal half out");
        assert_close(
            normal.friction_factor(1.0),
            0.0,
            1e-12,
            "normal a viewport out",
        );

        let fast = Bouncing::with_rate(DecelerationRate::Fast);
        assert_close(fast.friction_factor(0.0), 0.26, 1e-12, "fast at rest");
        assert_close(fast.friction_factor(0.5), 0.065, 1e-12, "fast half out");

        // The two coefficients the curve is built from, stated directly.
        assert_eq!(DecelerationRate::NORMAL_FRICTION, 0.52);
        assert_eq!(DecelerationRate::FAST_FRICTION, 0.26);
    }

    #[test]
    fn bouncing_resists_increasing_overscroll_only() {
        let physics = Bouncing::new();

        // Wholly in range: the identity mapping, both directions.
        assert_eq!(
            physics.apply_physics_to_user_offset(&metrics(100.0, 500.0), 30.0),
            30.0
        );
        assert_eq!(
            physics.apply_physics_to_user_offset(&metrics(100.0, 500.0), -50.0),
            -50.0
        );

        // At the leading edge, pulled 20px past it: the whole delta is
        // tensioning, at zero depth (0.52).
        assert_close(
            physics.apply_physics_to_user_offset(&metrics(0.0, 500.0), -20.0),
            -20.0 * 0.52,
            1e-12,
            "tensioning off the leading edge",
        );
        // Same at the trailing edge.
        assert_close(
            physics.apply_physics_to_user_offset(&metrics(500.0, 500.0), 20.0),
            20.0 * 0.52,
            1e-12,
            "tensioning off the trailing edge",
        );

        // Already 30px out of a 100px viewport, pulled 10px deeper: the factor
        // has tightened to 0.52·(1 − 0.3)².
        assert_close(
            physics.apply_physics_to_user_offset(&metrics(-30.0, 500.0), -10.0),
            -10.0 * 0.52 * 0.49,
            1e-12,
            "tensioning deeper",
        );

        // The same 30px out, easing back: untouched, in both the partial and
        // the all-the-way-back-into-range cases.
        assert_eq!(
            physics.apply_physics_to_user_offset(&metrics(-30.0, 500.0), 10.0),
            10.0
        );
        assert_eq!(
            physics.apply_physics_to_user_offset(&metrics(-30.0, 500.0), 50.0),
            50.0
        );

        // A delta straddling the edge resists only its past-edge part: 5px of
        // in-range travel, then 20px tensioned.
        assert_close(
            physics.apply_physics_to_user_offset(&metrics(5.0, 500.0), -25.0),
            -5.0 + -20.0 * 0.52,
            1e-12,
            "straddling the leading edge",
        );
    }

    #[test]
    fn bouncing_carried_momentum_formula() {
        let physics = Bouncing::new();
        let expected = 0.000_816 * 1000.0_f64.powf(1.967);
        let carried = physics.carried_momentum(1000.0);
        assert!(
            (carried - expected).abs() / expected < 1e-6,
            "carried {carried} is not within 1e-6 relative of {expected}"
        );
        // Sign follows the motion being carried over.
        assert_close(
            physics.carried_momentum(-1000.0),
            -expected,
            1e-9,
            "negative carry",
        );
        assert_eq!(physics.carried_momentum(0.0), 0.0);
        // Fast enough and the fit saturates rather than compounding.
        assert_eq!(physics.carried_momentum(1e6), 40_000.0);
        assert_eq!(physics.carried_momentum(-1e6), -40_000.0);
    }

    #[test]
    fn bouncing_min_fling_is_100() {
        assert_eq!(Bouncing::new().min_fling_velocity(), 100.0);
        assert_eq!(
            Bouncing::MIN_FLING_VELOCITY,
            crate::physics::MIN_FLING_VELOCITY * 2.0
        );
        // …twice what the clamping physics (the trait default) asks for.
        assert_eq!(Clamping::new().min_fling_velocity(), 50.0);
    }

    #[test]
    fn bouncing_spring_per_rate() {
        let normal = Bouncing::new().spring();
        assert_eq!(normal.mass, 0.5);
        assert_eq!(normal.stiffness, 100.0);
        assert_close(
            normal.damping,
            2.0 * 1.1 * (0.5 * 100.0_f64).sqrt(),
            1e-12,
            "normal damping",
        );
        assert_eq!(normal, SpringDescription::default_scroll_spring());

        let fast = Bouncing::with_rate(DecelerationRate::Fast).spring();
        assert_eq!(fast.mass, 0.3);
        assert_eq!(fast.stiffness, 75.0);
        assert_close(
            fast.damping,
            2.0 * 1.3 * (0.3 * 75.0_f64).sqrt(),
            1e-12,
            "fast damping",
        );
    }

    #[test]
    fn bouncing_ballistic_in_range_low_velocity_is_none() {
        let physics = Bouncing::new();
        let m = metrics(100.0, 500.0);
        assert!(physics.create_ballistic_simulation(&m, 0.0).is_none());
        assert!(physics.create_ballistic_simulation(&m, 99.0).is_none());
        // Exactly at the threshold is a fling.
        assert!(physics.create_ballistic_simulation(&m, 100.0).is_some());
    }

    #[test]
    fn bouncing_ballistic_out_of_range_is_some() {
        let physics = Bouncing::new();
        // Resting 30px past the leading edge: no fling velocity at all, but the
        // spring still has to bring it home.
        let sim = physics
            .create_ballistic_simulation(&metrics(-30.0, 500.0), 0.0)
            .expect("an overscrolled release must spring back");
        assert_close(sim.x(0.0), -30.0, 1e-9, "starts where released");
        assert!(
            sim.x(0.1) > sim.x(0.0),
            "must travel back toward the leading extent"
        );
        assert_close(sim.x(3.0), 0.0, 1e-9, "settles on the extent");
        assert!(sim.is_done(3.0), "the bounce-back never settled");
    }

    #[test]
    fn bouncing_fast_rate_carries_the_constant_deceleration() {
        assert_eq!(DecelerationRate::FAST_CONSTANT_DECELERATION, 1400.0);
        let m = metrics(100.0, 5000.0);
        let fast = Bouncing::with_rate(DecelerationRate::Fast)
            .create_ballistic_simulation(&m, 2000.0)
            .expect("a fast fling must be ballistic");
        // The same curve, spelled out: the fast spring plus a 1400 px/s²
        // constant deceleration on top of the drag.
        let expected = BouncingScrollSimulation::new(
            100.0,
            2000.0,
            0.0,
            5000.0,
            SpringDescription::with_damping_ratio(0.3, 75.0, 1.3),
            tol(),
            1400.0,
        );
        assert_close(fast.x(0.3), expected.x(0.3), 1e-9, "fast position");
        assert_close(fast.dx(0.3), expected.dx(0.3), 1e-9, "fast velocity");

        // …and it is genuinely slower than the normal rate's pure-drag curve.
        let normal = Bouncing::new()
            .create_ballistic_simulation(&m, 2000.0)
            .expect("a normal fling must be ballistic");
        assert!(
            normal.x(0.3) > fast.x(0.3),
            "the fast rate reached {}, no nearer than the normal rate's {}",
            fast.x(0.3),
            normal.x(0.3)
        );
    }

    #[test]
    fn clamping_boundary_conditions_reject_excess() {
        let physics = Clamping::new();
        let m = metrics(0.0, 500.0);
        assert_eq!(physics.apply_boundary_conditions(&m, -10.0), -10.0);
        assert_eq!(physics.apply_boundary_conditions(&m, 510.0), 10.0);
        assert_eq!(physics.apply_boundary_conditions(&m, 250.0), 0.0);
        // The extents themselves are in range.
        assert_eq!(physics.apply_boundary_conditions(&m, 0.0), 0.0);
        assert_eq!(physics.apply_boundary_conditions(&m, 500.0), 0.0);
    }

    #[test]
    fn clamping_ballistic_returns_android_curve_sim() {
        let physics = Clamping::new();
        let m = metrics(0.0, 500.0);
        let sim = physics
            .create_ballistic_simulation(&m, 3000.0)
            .expect("a fast in-range fling must be ballistic");

        // Android's own spline, at this crate's default friction.
        let expected = ClampingScrollSimulation::new(
            0.0,
            3000.0,
            ClampingScrollSimulation::DEFAULT_FRICTION,
            tol(),
        );
        assert_close(sim.dx(0.0), 3000.0, 1e-9, "release velocity");
        assert_close(sim.x(0.1), expected.x(0.1), 1e-9, "position on the curve");
        assert_close(sim.x(0.5), expected.x(0.5), 1e-9, "position later on");

        // The curve itself overshoots the extent — nothing in this physics
        // clamps it. What keeps the surface in range is the driver contract:
        // `ScrollWidget::drive_ballistic` paints `x − apply_boundary_conditions(x)`
        // for every simulation position, and this physics rejects the whole
        // excess. Re-derived here so a driver change cannot silently break it.
        let mut overshot = false;
        for step in 0..=200 {
            let time = f64::from(step) / 100.0;
            let raw = sim.x(time);
            overshot |= raw > 500.0;
            let painted = raw - physics.apply_boundary_conditions(&m, raw);
            assert!(
                (0.0..=500.0).contains(&painted),
                "painted offset {painted} left the range at t={time}s (raw {raw})"
            );
        }
        assert!(
            overshot,
            "the raw curve must overshoot for this test to mean anything"
        );

        // A release with nowhere to go is not a fling.
        assert!(
            physics
                .create_ballistic_simulation(&metrics(500.0, 500.0), 3000.0)
                .is_none()
        );
        assert!(
            physics
                .create_ballistic_simulation(&metrics(0.0, 500.0), -3000.0)
                .is_none()
        );
        assert!(physics.create_ballistic_simulation(&m, 40.0).is_none());
    }

    #[test]
    fn clamping_out_of_range_springs_back() {
        // The safety arm: this physics' own boundary rule never lets a position
        // get here, but a metrics change (or a physics swap) can.
        let sim = Clamping::new()
            .create_ballistic_simulation(&metrics(560.0, 500.0), 0.0)
            .expect("an out-of-range release must spring back");
        assert_close(sim.x(0.0), 560.0, 1e-9, "starts where released");
        assert_close(sim.x(3.0), 500.0, 1e-9, "settles on the nearest extent");
    }

    #[test]
    fn always_scrollable_accepts_on_short_content() {
        // Content that fits: the trait default (and a bare clamping physics)
        // refuses the drag outright.
        let short = metrics(0.0, 0.0);
        assert!(!Clamping::new().should_accept_user_offset(&short));
        assert!(AlwaysScrollable::new().should_accept_user_offset(&short));
    }

    #[test]
    fn never_scrollable_rejects_user_offset() {
        let physics = NeverScrollable::new();
        assert!(!physics.should_accept_user_offset(&metrics(0.0, 500.0)));
        // Even chained onto a physics that would accept.
        let chained = NeverScrollable::new().chain(Bouncing::new());
        assert!(!chained.should_accept_user_offset(&metrics(0.0, 500.0)));
    }

    #[test]
    fn chain_composition_defers_boundary_to_parent() {
        let physics = AlwaysScrollable::new().chain(Clamping::new());

        // Its own domain: a drag is accepted even with nothing to scroll.
        assert!(physics.should_accept_user_offset(&metrics(0.0, 0.0)));

        // Everything else walks up to the clamping parent — the boundary rule…
        let m = metrics(0.0, 500.0);
        assert_eq!(physics.apply_boundary_conditions(&m, -10.0), -10.0);
        assert_eq!(physics.apply_boundary_conditions(&m, 510.0), 10.0);
        assert_eq!(physics.apply_boundary_conditions(&m, 250.0), 0.0);

        // …the ballistic curve…
        assert!(physics.create_ballistic_simulation(&m, 3000.0).is_some());
        assert!(physics.create_ballistic_simulation(&m, 40.0).is_none());

        // …and the fling threshold, which the parent leaves at the default.
        assert_eq!(physics.min_fling_velocity(), 50.0);

        // Chained the other way the bouncing parent's answers come through
        // instead, including its doubled minimum.
        let bouncing = AlwaysScrollable::new().chain(Bouncing::new());
        assert_eq!(bouncing.min_fling_velocity(), Bouncing::MIN_FLING_VELOCITY);
        assert_eq!(bouncing.apply_boundary_conditions(&m, 510.0), 0.0);
    }
}
