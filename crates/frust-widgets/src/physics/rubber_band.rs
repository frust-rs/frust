//! [`RubberBand`] — the iOS-style rubber-band feel `ScrollView`/`ListView`
//! used to have wired in, lifted out of those two widgets and behind the
//! [`ScrollPhysics`] seam unchanged. It is **no longer any platform's
//! default**: both surfaces now install
//! [`crate::physics::default_physics`]'s platform-parity choice, and this is
//! the opt-in an app names (`.physics(RubberBand::new())`) to keep the
//! pre-seam feel — a flat resistance with the widgets' own legacy
//! fling/settle, rather than a depth-aware curve with a ballistic spring.
//!
//! # What lives here, and what deliberately does not
//!
//! Only the parts of the feel the trait has a seat for:
//!
//! * [`ScrollPhysics::apply_physics_to_user_offset`] — the past-edge portion of
//!   a drag scaled by [`OVERSCROLL_RESISTANCE`], the in-range portion passed
//!   through untouched.
//! * [`ScrollPhysics::apply_boundary_conditions`] — nothing rejected: a
//!   rubber-band surface is *allowed* to sit out of range, which is the whole
//!   point of it.
//! * [`ScrollPhysics::should_accept_user_offset`] — always `true`, overriding
//!   the trait's has-scrollable-content default (below).
//!
//! The release-settle constants (`SETTLE_DECAY` 0.988 per ms, `SETTLE_STOP_PX`
//! 0.5) stay in `scroll.rs` with the widgets, because they belong to the
//! **legacy hand-rolled settle path**, not to this trait: the seam expresses
//! post-release motion as a [`Simulation`] out of
//! [`ScrollPhysics::create_ballistic_simulation`], and this physics returns
//! `None` there on purpose (see the impl) so both widgets keep running their
//! existing fling/settle code verbatim. Unifying the two is future work, not a
//! behavior-preserving extraction. [`OVERSCROLL_RESISTANCE`] likewise keeps its
//! existing home beside them: it is one definition, imported here, so both
//! widgets' `pub(crate)` paths to it are untouched.
//!
//! # Under the surfaces' per-move drag convention
//!
//! The scroll surfaces hand every physics *this move's* delta measured from
//! the live position (`scroll.rs`'s *Drag convention*). Because the mapping
//! above is linear, a pull delivered over any number of moves telescopes to
//! exactly the `0.5 × raw pull` the pre-seam whole-excursion re-map produced —
//! every shipped rubber-band number is unchanged. The one place the two
//! conventions part is a **pull-and-return inside a single gesture**: with no
//! easing/tensioning split of its own (unlike `Bouncing`), this physics
//! resists the past-edge part of a *returning* delta too, so bringing the
//! finger all the way back leaves the surface slightly scrolled instead of
//! exactly at rest. Pinned by `per_move_deltas_telescope_to_the_pre_seam_pull`
//! rather than papered over — it is the cost of one convention for every
//! physics, and the tensioning direction (all of pull-to-refresh, overscroll
//! and stretch) is untouched by it.

use super::{ScrollMetrics, ScrollPhysics, Simulation};
use crate::scroll::OVERSCROLL_RESISTANCE;

/// The iOS-style rubber-band scroll feel: a drag may pull the position past an
/// edge, resisted by [`OVERSCROLL_RESISTANCE`], and nothing about a boundary is
/// rejected. See the [module docs](self) for what the trait does *not* carry
/// for this physics.
#[derive(Debug, Default)]
pub struct RubberBand {
    parent: Option<Box<dyn ScrollPhysics>>,
}

impl RubberBand {
    /// A standalone rubber-band physics (no chained parent).
    pub fn new() -> Self {
        Self { parent: None }
    }

    /// Chain `parent` behind this physics, the module-wide composition
    /// convention ([`ScrollPhysics`]' *Chaining*): every method this type does
    /// not override walks up to `parent`.
    pub fn chain(self, parent: impl ScrollPhysics + 'static) -> Self {
        Self {
            parent: Some(Box::new(parent)),
        }
    }
}

impl ScrollPhysics for RubberBand {
    fn parent(&self) -> Option<&dyn ScrollPhysics> {
        self.parent.as_deref()
    }

    /// The past-edge portion of the drag is scaled by
    /// [`OVERSCROLL_RESISTANCE`]; the portion that keeps the position inside
    /// `[min, max]` passes through untouched. A delta straddling an edge is
    /// split between the two rather than resisted whole.
    fn apply_physics_to_user_offset(&self, metrics: &ScrollMetrics, offset: f64) -> f64 {
        let min = metrics.min_scroll_extent;
        let max = metrics.max_scroll_extent;
        let start = metrics.pixels.clamp(min, max);
        let end = (metrics.pixels + offset).clamp(min, max);
        // The part of the delta that moves the position *within* range, and
        // whatever is left over — the part that would leave it.
        let in_range = end - start;
        let past_edge = offset - in_range;
        in_range + past_edge * OVERSCROLL_RESISTANCE
    }

    /// Nothing is ever rejected: a rubber-band surface holds an out-of-range
    /// position for as long as the finger asks it to, and the release-settle
    /// (not a boundary rule) is what brings it back.
    fn apply_boundary_conditions(&self, _metrics: &ScrollMetrics, _value: f64) -> f64 {
        0.0
    }

    /// **No generic ballistic simulation** — both scroll surfaces keep their
    /// own hand-rolled fling (`fling_decay`/`fling_displacement`) and
    /// release-settle for this physics, which is exactly what makes installing
    /// it a no-op on the shipped feel. A `None` here is the documented contract
    /// for "the widget's legacy path owns post-release motion", not a gap to
    /// fill in later by this type: a physics that *does* want the generic
    /// driver returns a [`Simulation`], and the widget then drives that instead.
    fn create_ballistic_simulation(
        &self,
        _metrics: &ScrollMetrics,
        _velocity: f64,
    ) -> Option<Box<dyn Simulation>> {
        None
    }

    /// A re-fling during live motion starts cold, matching both widgets'
    /// shipped behavior (a new `Down` kills the fling outright).
    fn carried_momentum(&self, _existing_velocity: f64) -> f64 {
        0.0
    }

    /// Always `true`, deliberately overriding the trait's
    /// has-scrollable-content default: a `ScrollView`/`ListView` whose content
    /// fits its viewport still rubber-bands under the finger today, and
    /// matching the shipped feel wins over the more principled default.
    fn should_accept_user_offset(&self, _metrics: &ScrollMetrics) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(pixels: f64, max: f64) -> ScrollMetrics {
        ScrollMetrics {
            pixels,
            min_scroll_extent: 0.0,
            max_scroll_extent: max,
            viewport_dimension: 100.0,
            device_pixel_ratio: 1.0,
        }
    }

    #[test]
    fn rubber_band_drag_mapping_matches_legacy_math() {
        let physics = RubberBand::new();

        // Wholly in range: the identity mapping, both directions.
        assert_eq!(
            physics.apply_physics_to_user_offset(&metrics(0.0, 900.0), 30.0),
            30.0
        );
        assert_eq!(
            physics.apply_physics_to_user_offset(&metrics(500.0, 900.0), -120.0),
            -120.0
        );

        // Pinned at the top edge, pulled 20px further past it: the widgets'
        // `raw * OVERSCROLL_RESISTANCE`.
        assert_eq!(
            physics.apply_physics_to_user_offset(&metrics(0.0, 900.0), -20.0),
            -10.0
        );
        // Pinned at the bottom edge: `(raw - max) * OVERSCROLL_RESISTANCE`.
        assert_eq!(
            physics.apply_physics_to_user_offset(&metrics(900.0, 900.0), 60.0),
            30.0
        );

        // A delta straddling the top edge resists only its past-edge part:
        // 5px of in-range travel, then 20px resisted to 10px.
        assert_eq!(
            physics.apply_physics_to_user_offset(&metrics(5.0, 900.0), -25.0),
            -15.0
        );

        // Content that fits (max == min) is all past-edge in both directions.
        assert_eq!(
            physics.apply_physics_to_user_offset(&metrics(0.0, 0.0), 40.0),
            20.0
        );
    }

    /// The per-move delta convention, traced across a whole gesture — see the
    /// [module docs](self)' section on it.
    #[test]
    fn per_move_deltas_telescope_to_the_pre_seam_pull() {
        let physics = RubberBand::new();

        // Four 10px moves past the top, each mapped from where the last left
        // the position: 5px each, summing to exactly the 0.5 × 40 the
        // whole-excursion re-map produced in one go.
        let mut position = 0.0;
        for _ in 0..4 {
            position += physics.apply_physics_to_user_offset(&metrics(position, 900.0), -10.0);
        }
        assert_eq!(position, -20.0);
        assert_eq!(
            physics.apply_physics_to_user_offset(&metrics(0.0, 900.0), -40.0),
            -20.0,
            "…the same number the whole pull in one move gives"
        );

        // Easing back is not symmetric: the past-edge part of the returning
        // delta is resisted too, so the finger coming all the way back leaves
        // the surface 10px scrolled rather than exactly at rest.
        let returned =
            position + physics.apply_physics_to_user_offset(&metrics(position, 900.0), 40.0);
        assert_eq!(returned, 10.0);
    }

    #[test]
    fn rubber_band_rejects_nothing_and_carries_no_momentum() {
        let physics = RubberBand::new();
        let m = metrics(0.0, 900.0);
        assert_eq!(physics.apply_boundary_conditions(&m, -500.0), 0.0);
        assert_eq!(physics.apply_boundary_conditions(&m, 1400.0), 0.0);
        assert_eq!(physics.carried_momentum(2000.0), 0.0);
        assert!(physics.create_ballistic_simulation(&m, 2000.0).is_none());
    }

    #[test]
    fn rubber_band_accepts_a_drag_even_when_content_fits() {
        // The trait default would refuse (max == min, nothing to scroll); the
        // shipped feel rubber-bands anyway, so this override wins.
        assert!(RubberBand::new().should_accept_user_offset(&metrics(0.0, 0.0)));
    }

    /// A toy parent proving the chain is wired: `RubberBand` overrides neither
    /// `min_fling_velocity` nor `spring`, so both must reach the parent.
    #[derive(Debug)]
    struct Slow;
    impl ScrollPhysics for Slow {
        fn min_fling_velocity(&self) -> f64 {
            5.0
        }
    }

    #[test]
    fn chaining_delegates_unoverridden_methods_to_the_parent() {
        let chained = RubberBand::new().chain(Slow);
        assert_eq!(chained.min_fling_velocity(), 5.0);
        // …while its own domain still answers locally.
        assert_eq!(
            chained.apply_physics_to_user_offset(&metrics(0.0, 900.0), -20.0),
            -10.0
        );
        assert_eq!(
            RubberBand::new().min_fling_velocity(),
            crate::physics::MIN_FLING_VELOCITY
        );
    }
}
