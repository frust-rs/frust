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
//! `forgekit-core`'s animation-core spring type: this crate stays
//! dependency-clean (no `forgekit-core`, see `docs/ARCHITECTURE.md`) — a
//! later task's facade provides a conversion where both crate's types are
//! visible.

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
}
