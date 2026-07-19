//! Material 3 shape scale: 10 corner-radius tokens (post-Expressive scale).
//!
//! Source: <https://m3.material.io/styles/shape/corner-radius-scale>
//! (refuter-verified 2026-07-17; the pre-Expressive scale had 7 tokens, not
//! 10 — see
//! `workflow/plans/features/frust-phase-6a-foundations/research/RESEARCH.md`'s
//! refuted-claims list). Radii are dp, treated 1:1 as logical px, matching
//! this crate's other length fields.
//!
//! # Cupertino (iOS) mapping
//!
//! [`ShapeScale::cupertino`] fills the same 10 slots with iOS-idiom corner
//! radii. Apple does not publish a numeric corner-radius scale the way M3
//! does — every value below is a **community-approximate** convention (see
//! `workflow/plans/features/frust-phase-6c-widget-catalog/research/RESEARCH.md`'s
//! `cupertino-tokens-idioms` claims, retrieved 2026-07-17): `small` (8pt) is
//! the widely-cited standard `UIButton`/control corner radius; `medium`
//! (10pt) approximates a text field/small card; `large`/`large_increased`
//! (13pt/14pt) bracket the community-cited alert/action-sheet radius range
//! (C13-adjacent: Apple's own HIG text does not publish this figure);
//! `extra_large`/`extra_large_increased` (20pt/24pt) approximate a sheet or
//! large modal card; `extra_extra_large` (36pt) has no iOS precedent at all
//! and is a Frust linear extrapolation to fill the scale's largest slot.
//! **iOS corners are visually "continuous" (superellipse/"squircle") curves,
//! not circular arcs** — a numeric radius here approximates the visual size
//! of that curve, not an exact geometric equivalent; Frust's own corner
//! painting (a circular-arc rounded rect) does not reproduce the continuous
//! curve shape, only its rough footprint.

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

    /// The Cupertino (iOS) shape scale — see the module docs' "Cupertino
    /// (iOS) mapping" section for the per-field rationale; every radius
    /// here is community-approximate, not an Apple-published spec.
    pub const fn cupertino() -> Self {
        Self {
            none: 0.0,
            extra_small: 4.0,
            small: 8.0,
            medium: 10.0,
            large: 13.0,
            large_increased: 14.0,
            extra_large: 20.0,
            extra_large_increased: 24.0,
            extra_extra_large: 36.0,
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

    /// Computes the inner corner radius for a concentrically-nested element.
    ///
    /// The **concentric-corner principle** (Liquid Glass, iOS 26+; see WWDC
    /// 2025 session 219 and Apple HIG) defines inner and outer shapes'
    /// rounded corners as sharing the same geometric center, with the inner
    /// radius derived from the outer radius minus the inset depth. This
    /// applies when a glass container (outer, radius `outer_radius`) nests a
    /// child container (inner, inset by `inset` on all sides); the child's
    /// corner radius is `max(outer_radius - inset, 0.0)`.
    ///
    /// # Formula
    ///
    /// - If `outer_radius` is infinite ([`f64::INFINITY`]), return infinite
    ///   (a full pill shape has no inner maximum).
    /// - Otherwise, return `max(outer_radius - inset, 0.0)` (clamped at zero
    ///   to prevent negative radii).
    ///
    /// # Examples
    ///
    /// ```
    /// use frust_theme::ShapeScale;
    ///
    /// // Normal nesting: glass bar (outer r=20) with inner pill (inset 4)
    /// assert_eq!(ShapeScale::concentric_inner(20.0, 4.0), 16.0);
    ///
    /// // Inset larger than outer: clamp at zero
    /// assert_eq!(ShapeScale::concentric_inner(8.0, 12.0), 0.0);
    ///
    /// // Infinite outer radius stays infinite (no clamping)
    /// assert!(ShapeScale::concentric_inner(f64::INFINITY, 4.0).is_infinite());
    /// ```
    pub fn concentric_inner(outer_radius: f64, inset: f64) -> f64 {
        if outer_radius.is_infinite() {
            f64::INFINITY
        } else {
            (outer_radius - inset).max(0.0)
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
    fn cupertino_matches_table() {
        let s = ShapeScale::cupertino();
        assert_eq!(s.none, 0.0);
        assert_eq!(s.small, 8.0);
        assert_eq!(s.medium, 10.0);
        assert_eq!(s.large, 13.0);
        assert_eq!(s.large_increased, 14.0);
        assert_eq!(s.extra_large, 20.0);
        assert_eq!(s.extra_large_increased, 24.0);
        assert_eq!(s.extra_extra_large, 36.0);
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

    #[test]
    fn concentric_inner_normal_nesting() {
        // Glass bar (outer r=20) with inner element (inset 4)
        assert_eq!(ShapeScale::concentric_inner(20.0, 4.0), 16.0);
    }

    #[test]
    fn concentric_inner_zero_outer_radius() {
        // Zero outer radius stays zero regardless of inset
        assert_eq!(ShapeScale::concentric_inner(0.0, 5.0), 0.0);
    }

    #[test]
    fn concentric_inner_zero_inset() {
        // No inset: inner radius equals outer radius
        assert_eq!(ShapeScale::concentric_inner(20.0, 0.0), 20.0);
    }

    #[test]
    fn concentric_inner_clamps_at_zero() {
        // Inset larger than outer radius: clamp at zero (no negative radii)
        assert_eq!(ShapeScale::concentric_inner(8.0, 12.0), 0.0);
    }

    #[test]
    fn concentric_inner_inset_equals_outer() {
        // Exact match: inset equals outer radius, result is zero
        assert_eq!(ShapeScale::concentric_inner(15.0, 15.0), 0.0);
    }

    #[test]
    fn concentric_inner_with_infinity() {
        // Infinite outer radius stays infinite (a full pill has no inner limit)
        assert!(ShapeScale::concentric_inner(f64::INFINITY, 4.0).is_infinite());
        assert!(ShapeScale::concentric_inner(f64::INFINITY, 100.0).is_infinite());
    }
}
