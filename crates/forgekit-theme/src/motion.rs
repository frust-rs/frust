//! Material 3 Expressive motion: six spring presets replacing
//! duration+easing tokens.
//!
//! Source: material-components-android `docs/theming/Motion.md` (M3
//! Expressive, verified 2026-07-17): fastSpatial (0.9, 1400), fastEffects
//! (1.0, 3800), defaultSpatial (0.9, 700), defaultEffects (1.0, 1600),
//! slowSpatial (0.9, 300), slowEffects (1.0, 800) — `(damping_ratio,
//! stiffness)`, mass 1 for every preset. "Effects" springs (opacity/color)
//! are critically damped (`damping_ratio: 1.0`) by design — no bounce;
//! "spatial" springs (position/size) are `0.9`, allowing a small overshoot.
//!
//! `MotionSpring` is plain data defined here rather than reusing
//! `forgekit-core`'s animation-core spring type: `forgekit-theme` already
//! depends on `forgekit-core` (see `docs/ARCHITECTURE.md`), so
//! [`impl From<MotionSpring> for SpringDesc`](struct.MotionSpring.html)
//! below lives right here rather than in a separate facade conversion.
//!
//! # Cupertino (iOS) mapping
//!
//! [`MotionScheme::cupertino`] is built from the single community-documented
//! iOS spring baseline (per
//! `workflow/plans/features/forgekit-phase-6c-widget-catalog/research/RESEARCH.md`'s
//! `cupertino-tokens-idioms` claims, retrieved 2026-07-17): mass `1.0`,
//! stiffness `170.0`, damping (coefficient, not ratio) `15.0` — the
//! "start with a damping of 15 and a stiffness of 170" convention cited
//! across multiple SwiftUI community sources. [`MotionSpring`] stores a
//! damping *ratio* ζ (mass implicitly `1.0`, matching every M3 preset), so
//! this module converts the community coefficient via the standard
//! relation `ζ = c / (2 * sqrt(k * m))`: `15.0 / (2 * sqrt(170.0)) ≈
//! 0.5753` — an under-damped spring with a gentle overshoot, consistent
//! with iOS's typically "springy" motion feel.
//!
//! Unlike M3, **iOS has no published/community-documented distinction
//! between "fast"/"default"/"slow" speed tiers or "spatial"/"effects" motion
//! categories** — the single baseline above is the only community-converged
//! iOS spring value found. Rather than inventing an unsourced scaling factor
//! to differentiate the six [`MotionScheme`] slots, this module applies the
//! single documented baseline **uniformly** to all six — a documented
//! simplification (flagged here, not a hidden gap), not a fabricated
//! iOS-specific tuning distinction.

use forgekit_core::anim::SpringDesc;

/// A physics spring's damping ratio and stiffness (mass is implicitly `1.0`
/// for every M3 preset; see module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionSpring {
    pub damping_ratio: f64,
    pub stiffness: f64,
}

impl MotionSpring {
    const fn new(damping_ratio: f64, stiffness: f64) -> Self {
        Self {
            damping_ratio,
            stiffness,
        }
    }
}

/// Converts an M3 motion token into the generic physics type
/// [`AnimationController::fling`](forgekit_core::anim::AnimationController::fling)/
/// [`Spring::new`](forgekit_core::anim::Spring::new) consume.
///
/// Mass is always `1.0` — every M3 Expressive preset is defined purely in
/// terms of damping ratio + stiffness (see the module docs), so `mass` has no
/// token-level source to convert from.
impl From<MotionSpring> for SpringDesc {
    fn from(spring: MotionSpring) -> Self {
        SpringDesc {
            mass: 1.0,
            stiffness: spring.stiffness,
            damping_ratio: spring.damping_ratio,
        }
    }
}

/// The six Material 3 Expressive motion spring presets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionScheme {
    pub fast_spatial: MotionSpring,
    pub fast_effects: MotionSpring,
    pub default_spatial: MotionSpring,
    pub default_effects: MotionSpring,
    pub slow_spatial: MotionSpring,
    pub slow_effects: MotionSpring,
}

impl MotionScheme {
    /// The Material 3 Expressive baseline motion scheme.
    pub const fn m3_expressive() -> Self {
        Self {
            fast_spatial: MotionSpring::new(0.9, 1400.0),
            fast_effects: MotionSpring::new(1.0, 3800.0),
            default_spatial: MotionSpring::new(0.9, 700.0),
            default_effects: MotionSpring::new(1.0, 1600.0),
            slow_spatial: MotionSpring::new(0.9, 300.0),
            slow_effects: MotionSpring::new(1.0, 800.0),
        }
    }

    /// The Cupertino (iOS) motion scheme — the single community-documented
    /// iOS spring baseline (mass 1.0, stiffness 170.0, damping coefficient
    /// 15.0 → damping ratio ζ ≈ 0.5753) applied uniformly to all six slots.
    /// See the module docs for the ζ conversion and why iOS has no
    /// documented fast/default/slow or spatial/effects distinction to
    /// otherwise vary these by.
    pub const fn cupertino() -> Self {
        // ζ = 15.0 / (2 * sqrt(170.0)) ≈ 0.5753 (see module docs; sqrt isn't
        // const-evaluable on stable Rust, so the ratio is precomputed here).
        const CUPERTINO_DAMPING_RATIO: f64 = 0.5753;
        const CUPERTINO_STIFFNESS: f64 = 170.0;
        let spring = MotionSpring::new(CUPERTINO_DAMPING_RATIO, CUPERTINO_STIFFNESS);
        Self {
            fast_spatial: spring,
            fast_effects: spring,
            default_spatial: spring,
            default_effects: spring,
            slow_spatial: spring,
            slow_effects: spring,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_presets_are_exact() {
        let m = MotionScheme::m3_expressive();
        assert_eq!(m.fast_spatial, MotionSpring::new(0.9, 1400.0));
        assert_eq!(m.fast_effects, MotionSpring::new(1.0, 3800.0));
        assert_eq!(m.default_spatial, MotionSpring::new(0.9, 700.0));
        assert_eq!(m.default_effects, MotionSpring::new(1.0, 1600.0));
        assert_eq!(m.slow_spatial, MotionSpring::new(0.9, 300.0));
        assert_eq!(m.slow_effects, MotionSpring::new(1.0, 800.0));
    }

    #[test]
    fn effects_springs_are_critically_damped() {
        let m = MotionScheme::m3_expressive();
        assert_eq!(m.fast_effects.damping_ratio, 1.0);
        assert_eq!(m.default_effects.damping_ratio, 1.0);
        assert_eq!(m.slow_effects.damping_ratio, 1.0);
    }

    #[test]
    fn spring_desc_round_trips_all_six_presets() {
        let m = MotionScheme::m3_expressive();
        let presets = [
            m.fast_spatial,
            m.fast_effects,
            m.default_spatial,
            m.default_effects,
            m.slow_spatial,
            m.slow_effects,
        ];
        for preset in presets {
            let desc: SpringDesc = preset.into();
            assert_eq!(desc.mass, 1.0);
            assert_eq!(desc.stiffness, preset.stiffness);
            assert_eq!(desc.damping_ratio, preset.damping_ratio);
        }
    }

    #[test]
    fn cupertino_uses_the_single_documented_baseline_uniformly() {
        let m = MotionScheme::cupertino();
        let expected = MotionSpring::new(0.5753, 170.0);
        assert_eq!(m.fast_spatial, expected);
        assert_eq!(m.fast_effects, expected);
        assert_eq!(m.default_spatial, expected);
        assert_eq!(m.default_effects, expected);
        assert_eq!(m.slow_spatial, expected);
        assert_eq!(m.slow_effects, expected);
    }

    #[test]
    fn cupertino_damping_ratio_is_under_damped() {
        // ζ ≈ 0.5753 < 1.0 — a gentle overshoot, matching iOS's springy feel
        // (see module docs).
        let m = MotionScheme::cupertino();
        assert!(m.default_spatial.damping_ratio < 1.0);
        assert!(m.default_spatial.damping_ratio > 0.0);
    }
}
