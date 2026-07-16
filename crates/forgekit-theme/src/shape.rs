//! Material 3 shape scale: 10 corner-radius tokens (post-Expressive scale).
//!
//! Source: <https://m3.material.io/styles/shape/corner-radius-scale>
//! (refuter-verified 2026-07-17; the pre-Expressive scale had 7 tokens, not
//! 10 — see
//! `workflow/plans/features/forgekit-phase-6a-foundations/research/RESEARCH.md`'s
//! refuted-claims list). Radii are dp, treated 1:1 as logical px, matching
//! this crate's other length fields.

/// The 10 Material 3 corner-radius tokens, dp (logical px). `full` is
/// represented as [`f64::INFINITY`] rather than a separate enum variant —
/// resolve it against a concrete box size with [`ShapeScale::resolve`],
/// which clamps an infinite radius to half the box's shorter side (a pill
/// shape) and passes any finite radius through unchanged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeScale {
    pub none: f64,
    pub extra_small: f64,
    pub small: f64,
    pub medium: f64,
    pub large: f64,
    pub large_increased: f64,
    pub extra_large: f64,
    pub extra_large_increased: f64,
    pub extra_extra_large: f64,
    /// Always [`f64::INFINITY`] — resolve with [`ShapeScale::resolve`].
    pub full: f64,
}

impl ShapeScale {
    /// The Material 3 baseline shape scale.
    pub const fn m3() -> Self {
        Self {
            none: 0.0,
            extra_small: 4.0,
            small: 8.0,
            medium: 12.0,
            large: 16.0,
            large_increased: 20.0,
            extra_large: 28.0,
            extra_large_increased: 32.0,
            extra_extra_large: 48.0,
            full: f64::INFINITY,
        }
    }

    /// Resolves a corner radius against a box's shorter side (`min(width,
    /// height)`), clamping an infinite ([`ShapeScale::full`]) radius to a
    /// pill shape (`shorter_side / 2.0`) and any finite radius to itself
    /// (already-finite tokens never need clamping in the M3 scale, but a
    /// caller-supplied custom radius larger than the box would produce a
    /// visually broken corner, so this also caps at the pill radius as a
    /// safety net).
    pub fn resolve(radius: f64, width: f64, height: f64) -> f64 {
        let pill = width.min(height) / 2.0;
        if radius.is_infinite() {
            pill
        } else {
            radius.min(pill)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_table() {
        let s = ShapeScale::m3();
        assert_eq!(s.none, 0.0);
        assert_eq!(s.extra_small, 4.0);
        assert_eq!(s.small, 8.0);
        assert_eq!(s.medium, 12.0);
        assert_eq!(s.large, 16.0);
        assert_eq!(s.large_increased, 20.0);
        assert_eq!(s.extra_large, 28.0);
        assert_eq!(s.extra_large_increased, 32.0);
        assert_eq!(s.extra_extra_large, 48.0);
        assert!(s.full.is_infinite());
    }

    #[test]
    fn full_resolves_to_pill_radius() {
        let s = ShapeScale::m3();
        assert_eq!(ShapeScale::resolve(s.full, 100.0, 40.0), 20.0);
    }

    #[test]
    fn finite_radius_passes_through_when_smaller_than_pill() {
        let s = ShapeScale::m3();
        assert_eq!(ShapeScale::resolve(s.medium, 200.0, 200.0), 12.0);
    }

    #[test]
    fn finite_radius_clamps_when_larger_than_pill() {
        assert_eq!(ShapeScale::resolve(1000.0, 40.0, 20.0), 10.0);
    }
}
