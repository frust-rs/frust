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
}
