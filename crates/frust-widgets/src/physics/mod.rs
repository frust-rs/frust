//! Pluggable scroll physics, mirroring Flutter's `ScrollPhysics` contract: a
//! chainable strategy object a scroll surface (today [`crate::ScrollView`]/
//! [`crate::ListView`], hard-coded to one behavior) consults for user-offset
//! mapping, boundary rejection, and post-release ballistic motion instead of
//! having that math wired in directly.
//!
//! # Module map
//!
//! Three seams, each its own file:
//!
//! * This file — the shared vocabulary every physics implementation and
//!   consumer builds against: [`ScrollMetrics`], [`Tolerance`],
//!   [`SpringDescription`], the [`Simulation`] trait, the fling velocity
//!   constants, and the [`ScrollPhysics`] trait itself.
//! * [`effect`] — [`OverscrollEffect`], how boundary-rejected displacement is
//!   *visualized* (translate vs. paint-side stretch vs. none) — orthogonal to
//!   the physics that computes the displacement in the first place.
//! * [`simulation`] — concrete [`Simulation`] implementations (the ballistic
//!   decay/spring curves a physics hands back from
//!   [`ScrollPhysics::create_ballistic_simulation`]).
//! * [`rubber_band`] — the pre-seam rubber-band feel, now an opt-in.
//! * [`parity`] — the platform-parity physics (`Bouncing`/`Clamping`/
//!   `AlwaysScrollable`/`NeverScrollable`) both scroll surfaces default to.
//! * This file also carries the platform-adaptive default selection itself —
//!   [`default_physics`]/[`default_overscroll_effect`], what a `ScrollView`/
//!   `ListView` installs when the app names no physics of its own.
//!
//! # Design ruling: rejected excess is reported, not absorbed
//!
//! [`ScrollPhysics::apply_boundary_conditions`] returns the portion of a
//! proposed position a physics *rejects* — the part the scroll position must
//! not move to — separately from anything about how that rejection looks on
//! screen. A widget accumulates the rejected excess itself (as its own
//! `edge_pull` state, outside this module) rather than this trait owning any
//! visual displacement. This is deliberate: pull-to-refresh triggering and the
//! [`effect::OverscrollEffect::Stretch`] paint effect both need to read how far
//! *past* the edge a gesture is pulling even under a **clamping** physics
//! (`apply_boundary_conditions` rejecting 100% of the excess, i.e. the
//! position itself never leaves range) — so a clamping physics still supports
//! both features, it just never lets `edge_pull` show up as a position change.

pub mod effect;
pub mod parity;
pub mod rubber_band;
pub mod simulation;

use std::rc::Rc;

/// The physics a scroll surface installs when the app names none: **Android →
/// [`parity::Clamping`]** (paired with [`effect::OverscrollEffect::Stretch`],
/// the Material-3-Expressive edge stretch), **everywhere else →
/// [`parity::Bouncing`]** at [`parity::DecelerationRate::Normal`] (paired with
/// [`effect::OverscrollEffect::Translate`]).
///
/// [`rubber_band::RubberBand`] — the feel both surfaces used to hard-code — is
/// no longer any platform's default; it stays reachable as an explicit
/// `.physics(RubberBand::new())` opt-in.
///
/// `Rc<dyn ScrollPhysics>`, matching what the two scroll widgets store, so
/// installing the default is one allocation and every later (re)install is an
/// `Rc::clone`.
///
/// Selection is a `cfg!` **expression**, not a `#[cfg]` block: both arms
/// type-check on every host, so a change here cannot compile on desktop and
/// break the Android build.
pub fn default_physics() -> Rc<dyn ScrollPhysics> {
    if cfg!(target_os = "android") {
        Rc::new(parity::Clamping::new())
    } else {
        Rc::new(parity::Bouncing::new())
    }
}

/// The overscroll visual paired with [`default_physics`]: Android's clamping
/// position never leaves the range, so the pull shows as
/// [`effect::OverscrollEffect::Stretch`]; a bouncing surface moves with the
/// pull instead, so it shows as [`effect::OverscrollEffect::Translate`].
///
/// Independent of [`effect::OverscrollEffect::default()`] (`Translate`, the
/// enum's own neutral value) on purpose: this is the *platform* pairing, and a
/// caller naming an effect explicitly always wins over it.
pub fn default_overscroll_effect() -> effect::OverscrollEffect {
    if cfg!(target_os = "android") {
        effect::OverscrollEffect::Stretch
    } else {
        effect::OverscrollEffect::Translate
    }
}

/// A read-only snapshot of a scroll surface's extent/position, the argument
/// every [`ScrollPhysics`] method reasons over (Flutter's `ScrollMetrics`).
///
/// `min_scroll_extent` is carried for parity/chaining even though frust's
/// scroll surfaces always run it at `0.0` today — nothing in this crate
/// produces a nonzero value yet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollMetrics {
    /// The current scroll position (px scrolled down/along the axis).
    pub pixels: f64,
    /// The minimum in-range position. Always `0.0` in this crate today.
    pub min_scroll_extent: f64,
    /// The maximum in-range position (`content − viewport`, never negative).
    pub max_scroll_extent: f64,
    /// The visible extent along the scroll axis.
    pub viewport_dimension: f64,
    /// The device's logical-to-physical pixel scale, feeding
    /// [`Tolerance::for_device_pixel_ratio`].
    pub device_pixel_ratio: f64,
}

impl ScrollMetrics {
    /// Whether `pixels` currently sits outside `[min_scroll_extent,
    /// max_scroll_extent]`.
    pub fn out_of_range(&self) -> bool {
        self.pixels < self.min_scroll_extent || self.pixels > self.max_scroll_extent
    }

    /// How far past the leading (top/start) edge `pixels` currently sits,
    /// `0.0` if not past it.
    pub fn overscroll_past_leading(&self) -> f64 {
        (self.min_scroll_extent - self.pixels).max(0.0)
    }

    /// How far past the trailing (bottom/end) edge `pixels` currently sits,
    /// `0.0` if not past it.
    pub fn overscroll_past_trailing(&self) -> f64 {
        (self.pixels - self.max_scroll_extent).max(0.0)
    }
}

/// The velocity/distance thresholds below which a ballistic simulation is
/// considered settled — Flutter's `Tolerance`, produced by `toleranceFor`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tolerance {
    /// Velocity (px/s) below which motion counts as stopped.
    pub velocity: f64,
    /// Distance (logical px) below which position counts as arrived.
    pub distance: f64,
}

impl Tolerance {
    /// Flutter's `toleranceFor` formula exactly: velocity tolerance tightens
    /// (and distance tolerance loosens) as `dpr` grows, since a physical pixel
    /// covers less logical distance on a denser screen.
    pub fn for_device_pixel_ratio(dpr: f64) -> Self {
        Tolerance {
            velocity: 1.0 / (0.050 * dpr),
            distance: 1.0 / dpr,
        }
    }
}

/// A critically-damped-family spring's physical parameters, feeding a
/// [`Simulation`] built from [`ScrollPhysics::spring`] (Flutter's
/// `SpringDescription`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringDescription {
    /// The spring's mass.
    pub mass: f64,
    /// The spring's stiffness.
    pub stiffness: f64,
    /// The spring's damping coefficient.
    pub damping: f64,
}

impl SpringDescription {
    /// Derive `damping` from a damping *ratio* instead of stating it directly
    /// — `damping = 2 · ratio · sqrt(mass · stiffness)` (Flutter's
    /// `SpringDescription.withDampingRatio`).
    pub fn with_damping_ratio(mass: f64, stiffness: f64, ratio: f64) -> Self {
        SpringDescription {
            mass,
            stiffness,
            damping: 2.0 * ratio * (mass * stiffness).sqrt(),
        }
    }

    /// Flutter's `_kDefaultSpring`: the spring an overscrolled bouncing
    /// surface uses to return to its edge.
    pub fn default_scroll_spring() -> Self {
        Self::with_damping_ratio(0.5, 100.0, 1.1)
    }
}

/// A ballistic motion curve over time, produced by
/// [`ScrollPhysics::create_ballistic_simulation`] and driven by the consuming
/// widget after a gesture release (fling decay, a spring-back, or any other
/// closed-form or iterative curve).
///
/// `time` is in **seconds** from the simulation's own start (`0.0` at
/// creation) — a scroll surface that ticks in milliseconds (frust's scroll
/// surfaces do) converts to seconds before calling in. An implementation
/// carries its own [`Tolerance`] (typically threaded in at construction from
/// [`ScrollPhysics::tolerance_for`]) rather than this trait supplying one.
pub trait Simulation {
    /// The position at `time` seconds.
    fn x(&self, time: f64) -> f64;
    /// The velocity at `time` seconds.
    fn dx(&self, time: f64) -> f64;
    /// Whether the simulation has settled (within its own tolerance) by `time`
    /// seconds.
    fn is_done(&self, time: f64) -> bool;
}

/// The minimum release speed (px/s) that starts a fling — Flutter's
/// `kMinFlingVelocity`. A release slower than this is treated as a plain
/// drag-end, never a fling.
pub const MIN_FLING_VELOCITY: f64 = 50.0;

/// The fastest fling speed (px/s) a physics honors — Flutter's
/// `kMaxFlingVelocity`. A release faster than this clamps to it.
pub const MAX_FLING_VELOCITY: f64 = 8000.0;

/// A pluggable scroll-motion strategy — Flutter's `ScrollPhysics` contract.
///
/// # Chaining
///
/// Physics compose by **parenting**, not inheritance: Flutter's
/// `const BouncingScrollPhysics().applyTo(const AlwaysScrollableScrollPhysics())`
/// idiom becomes a concrete type storing an optional boxed parent
/// (`Option<Box<dyn ScrollPhysics>>`) behind [`ScrollPhysics::parent`], and a
/// child overrides only the method(s) its own domain cares about — every other
/// method's **default body here** asks the parent for its answer, and only
/// falls back to a hardcoded value when there is no parent at all. A type that
/// overrides a method entirely opts out of that delegation for that method
/// only (e.g. `Snap(parent: Bouncing)` overrides just
/// `create_ballistic_simulation`, so every other method — including
/// `apply_boundary_conditions` — still walks up to `Bouncing`).
///
/// Concrete physics expose a `pub fn chain(self, parent: impl ScrollPhysics +
/// 'static) -> Self` building `Some(Box::new(parent))` for
/// [`ScrollPhysics::parent`] to return — every later physics type in this
/// module follows that exact convention (same method name, same signature
/// shape) so they compose with each other and with a caller's own type
/// uniformly.
pub trait ScrollPhysics: std::fmt::Debug {
    /// The chained parent physics, if this one was built via `chain(...)`.
    /// `None` for a physics built standalone (the chain's root).
    fn parent(&self) -> Option<&dyn ScrollPhysics> {
        None
    }

    /// Map a raw user drag delta (finger px, signed in the content's own
    /// direction) to the delta actually applied to the position. Must not
    /// alter the in-bounds portion of a drag — only a physics with an
    /// out-of-bounds opinion (e.g. added resistance) touches this.
    ///
    /// Default: delegates to [`ScrollPhysics::parent`] if chained, otherwise
    /// the identity mapping (`offset` unchanged).
    fn apply_physics_to_user_offset(&self, metrics: &ScrollMetrics, offset: f64) -> f64 {
        self.parent().map_or(offset, |parent| {
            parent.apply_physics_to_user_offset(metrics, offset)
        })
    }

    /// Given a proposed new `pixels` value, return the portion the position
    /// must **not** absorb — the boundary-rejected excess. `0.0` means the
    /// proposal is fully allowed (a bouncing physics past an edge); the full
    /// `proposed − clamped` distance means none of it is (a clamping
    /// physics).
    ///
    /// Default: delegates to [`ScrollPhysics::parent`] if chained, otherwise
    /// `0.0` (nothing rejected).
    fn apply_boundary_conditions(&self, metrics: &ScrollMetrics, value: f64) -> f64 {
        self.parent().map_or(0.0, |parent| {
            parent.apply_boundary_conditions(metrics, value)
        })
    }

    /// Build the ballistic motion to run after a gesture releases with
    /// `velocity` (px/s, signed like [`ScrollMetrics::pixels`]). `None` means
    /// no animation — the consuming widget falls back to its own legacy
    /// path/rest handling.
    ///
    /// Default: delegates to [`ScrollPhysics::parent`] if chained, otherwise
    /// `None`.
    fn create_ballistic_simulation(
        &self,
        metrics: &ScrollMetrics,
        velocity: f64,
    ) -> Option<Box<dyn Simulation>> {
        self.parent()
            .and_then(|parent| parent.create_ballistic_simulation(metrics, velocity))
    }

    /// Whether a user drag is allowed to move the position at all.
    ///
    /// Default: delegates to [`ScrollPhysics::parent`] if chained, otherwise
    /// whether the surface actually has scrollable content
    /// (`max_scroll_extent > min_scroll_extent`).
    fn should_accept_user_offset(&self, metrics: &ScrollMetrics) -> bool {
        self.parent().map_or_else(
            || metrics.max_scroll_extent > metrics.min_scroll_extent,
            |parent| parent.should_accept_user_offset(metrics),
        )
    }

    /// Momentum (px/s) to add onto a new fling's initial velocity when motion
    /// is already live (a re-fling mid-animation carries some of the old
    /// velocity forward rather than starting cold).
    ///
    /// Default: delegates to [`ScrollPhysics::parent`] if chained, otherwise
    /// `0.0`.
    fn carried_momentum(&self, existing_velocity: f64) -> f64 {
        self.parent()
            .map_or(0.0, |parent| parent.carried_momentum(existing_velocity))
    }

    /// The minimum release speed that starts a fling.
    ///
    /// Default: delegates to [`ScrollPhysics::parent`] if chained, otherwise
    /// [`MIN_FLING_VELOCITY`].
    fn min_fling_velocity(&self) -> f64 {
        self.parent()
            .map_or(MIN_FLING_VELOCITY, |parent| parent.min_fling_velocity())
    }

    /// The fastest fling speed this physics honors.
    ///
    /// Default: delegates to [`ScrollPhysics::parent`] if chained, otherwise
    /// [`MAX_FLING_VELOCITY`].
    fn max_fling_velocity(&self) -> f64 {
        self.parent()
            .map_or(MAX_FLING_VELOCITY, |parent| parent.max_fling_velocity())
    }

    /// The velocity/distance tolerance a ballistic simulation settles within.
    ///
    /// Default: delegates to [`ScrollPhysics::parent`] if chained, otherwise
    /// [`Tolerance::for_device_pixel_ratio`] over `metrics.device_pixel_ratio`.
    fn tolerance_for(&self, metrics: &ScrollMetrics) -> Tolerance {
        self.parent().map_or_else(
            || Tolerance::for_device_pixel_ratio(metrics.device_pixel_ratio),
            |parent| parent.tolerance_for(metrics),
        )
    }

    /// The spring a bounce-back/snap simulation is built from.
    ///
    /// Default: delegates to [`ScrollPhysics::parent`] if chained, otherwise
    /// [`SpringDescription::default_scroll_spring`].
    fn spring(&self) -> SpringDescription {
        self.parent()
            .map_or_else(SpringDescription::default_scroll_spring, |parent| {
                parent.spring()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(pixels: f64, max: f64, dpr: f64) -> ScrollMetrics {
        ScrollMetrics {
            pixels,
            min_scroll_extent: 0.0,
            max_scroll_extent: max,
            viewport_dimension: 100.0,
            device_pixel_ratio: dpr,
        }
    }

    #[test]
    fn tolerance_matches_flutter_formula() {
        let t = Tolerance::for_device_pixel_ratio(2.0);
        assert_eq!(t.velocity, 10.0);
        assert_eq!(t.distance, 0.5);
    }

    #[test]
    fn spring_damping_ratio_math() {
        let spring = SpringDescription::with_damping_ratio(0.5, 100.0, 1.1);
        let expected = 2.0 * 1.1 * (50.0f64).sqrt();
        assert!(
            (spring.damping - expected).abs() < 1e-9,
            "damping {} did not match expected {}",
            spring.damping,
            expected
        );
    }

    /// A toy leaf physics overriding nothing — every method should fall
    /// through to its parent when one is chained.
    #[derive(Debug)]
    struct Bare;
    impl ScrollPhysics for Bare {}

    /// A toy parent physics: rejects the full excess of any boundary proposal
    /// (a stand-in "clamping" answer distinct from the trait's own `0.0`
    /// no-parent fallback, so a test can tell the two apart) and reports an
    /// unusually low minimum fling velocity.
    #[derive(Debug)]
    struct TestParent;
    impl ScrollPhysics for TestParent {
        fn apply_boundary_conditions(&self, _metrics: &ScrollMetrics, _value: f64) -> f64 {
            7.0
        }
    }

    /// A child chained onto [`TestParent`] that overrides nothing itself.
    #[derive(Debug)]
    struct DelegatingChild {
        parent: Box<dyn ScrollPhysics>,
    }
    impl ScrollPhysics for DelegatingChild {
        fn parent(&self) -> Option<&dyn ScrollPhysics> {
            Some(self.parent.as_ref())
        }
    }

    /// A child chained onto [`TestParent`] that overrides
    /// `min_fling_velocity` itself — its own answer must win over the
    /// parent's.
    #[derive(Debug)]
    struct OverridingChild {
        parent: Box<dyn ScrollPhysics>,
    }
    impl ScrollPhysics for OverridingChild {
        fn parent(&self) -> Option<&dyn ScrollPhysics> {
            Some(self.parent.as_ref())
        }
        fn min_fling_velocity(&self) -> f64 {
            999.0
        }
    }

    #[test]
    fn chaining_delegates_through_parent() {
        let m = metrics(10.0, 100.0, 1.0);

        // A leaf with no parent at all falls back to the trait's own default.
        assert_eq!(Bare.apply_boundary_conditions(&m, 50.0), 0.0);

        // A child overriding nothing delegates straight through to the parent.
        let child = DelegatingChild {
            parent: Box::new(TestParent),
        };
        assert_eq!(child.apply_boundary_conditions(&m, 50.0), 7.0);

        // A child overriding one method wins over the parent for that method,
        // while an un-overridden method still walks up to the parent's own
        // fallback (the trait default, since TestParent doesn't override it
        // either).
        let overriding = OverridingChild {
            parent: Box::new(TestParent),
        };
        assert_eq!(overriding.min_fling_velocity(), 999.0);
        assert_eq!(overriding.apply_boundary_conditions(&m, 50.0), 7.0);
    }

    /// The non-Android arm of [`default_physics`]/[`default_overscroll_effect`]
    /// — read back behaviorally (the doubled fling minimum, nothing rejected
    /// past an edge, the depth-aware friction at zero depth) rather than by
    /// downcasting, plus the `Debug` name so a swap to another
    /// nothing-rejected physics still trips this.
    #[cfg(not(target_os = "android"))]
    #[test]
    fn non_android_default_is_bouncing_plus_translate() {
        let physics = default_physics();
        assert!(
            format!("{physics:?}").starts_with("Bouncing"),
            "the desktop/iOS default is Bouncing, got {physics:?}"
        );
        assert_eq!(
            physics.min_fling_velocity(),
            parity::Bouncing::MIN_FLING_VELOCITY
        );
        let m = metrics(0.0, 500.0, 1.0);
        assert_eq!(
            physics.apply_boundary_conditions(&m, -30.0),
            0.0,
            "a bouncing surface rejects nothing — it holds the displacement"
        );
        let mapped = physics.apply_physics_to_user_offset(&m, -20.0);
        assert!(
            (mapped - -20.0 * parity::DecelerationRate::NORMAL_FRICTION).abs() < 1e-12,
            "the past-edge pull is scaled by the normal-rate friction: {mapped}"
        );
        assert_eq!(
            default_overscroll_effect(),
            effect::OverscrollEffect::Translate
        );
    }

    /// The Android arm of the same pair. Compiled only for an Android target,
    /// so an ordinary host `cargo test` never runs it — a real device/emulator
    /// build is what exercises this half.
    #[cfg(target_os = "android")]
    #[test]
    fn android_default_is_clamping_plus_stretch() {
        let physics = default_physics();
        assert!(
            format!("{physics:?}").starts_with("Clamping"),
            "the Android default is Clamping, got {physics:?}"
        );
        let m = metrics(0.0, 500.0, 1.0);
        assert_eq!(
            physics.apply_boundary_conditions(&m, -30.0),
            -30.0,
            "a clamping surface rejects the whole past-edge excess"
        );
        assert_eq!(
            physics.apply_physics_to_user_offset(&m, -20.0),
            -20.0,
            "…and resists the drag itself not at all (the trait's identity map)"
        );
        assert_eq!(
            default_overscroll_effect(),
            effect::OverscrollEffect::Stretch
        );
    }

    #[test]
    fn default_should_accept_requires_scrollable_content() {
        let scrollable = metrics(0.0, 500.0, 1.0);
        assert!(Bare.should_accept_user_offset(&scrollable));

        let not_scrollable = metrics(0.0, 0.0, 1.0);
        assert!(!Bare.should_accept_user_offset(&not_scrollable));
    }
}
