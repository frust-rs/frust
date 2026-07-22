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
//! `frust-core`'s animation-core spring type: `frust-theme` already
//! depends on `frust-core` (see `docs/ARCHITECTURE.md`), so
//! [`impl From<MotionSpring> for SpringDesc`](struct.MotionSpring.html)
//! below lives right here rather than in a separate facade conversion.
//!
//! # Cupertino (iOS) mapping
//!
//! [`MotionScheme::cupertino`] is built from the single community-documented
//! iOS spring baseline (per
//! `workflow/plans/features/frust-phase-6c-widget-catalog/research/RESEARCH.md`'s
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

use frust_core::anim::{Curve, SpringDesc};

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
/// [`AnimationController::fling`](frust_core::anim::AnimationController::fling)/
/// [`Spring::new`](frust_core::anim::Spring::new) consume.
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

/// Named duration tokens, in milliseconds.
///
/// Glyph's motion language is bezier/duration-authored (unlike M3
/// Expressive's springs above), so a [`MotionScheme`] carries this
/// five-slot duration vocabulary alongside the six spring presets — see
/// [`MotionScheme::m3_expressive`]/[`MotionScheme::cupertino`] for the
/// per-baseline mapping and sources.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionDurations {
    pub instant: f64,
    pub fast: f64,
    pub base: f64,
    pub slow: f64,
    pub deliberate: f64,
}

/// The three-easing vocabulary Glyph's bezier-authored motion patterns are
/// built from: `spatial` (position/size changes — may overshoot), `effects`
/// (opacity/color changes — never overshoots), and `exit` (elements leaving
/// the screen — accelerates out). See [`MotionScheme::m3_expressive`]/
/// [`MotionScheme::cupertino`] for the per-baseline curve values and sources.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EasingSet {
    pub spatial: Curve,
    pub effects: Curve,
    pub exit: Curve,
}

/// The six Material 3 Expressive motion spring presets, plus the Glyph
/// duration/easing token vocabulary mapped onto each baseline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionScheme {
    pub fast_spatial: MotionSpring,
    pub fast_effects: MotionSpring,
    pub default_spatial: MotionSpring,
    pub default_effects: MotionSpring,
    pub slow_spatial: MotionSpring,
    pub slow_effects: MotionSpring,
    /// Named duration tokens (Glyph vocabulary; see [`MotionDurations`]).
    pub durations: MotionDurations,
    /// The three-easing vocabulary (Glyph vocabulary; see [`EasingSet`]).
    pub easing: EasingSet,
    /// Collapses patterned motion to a fast crossfade. Set by a shell from
    /// the OS's reduced-motion accessibility preference; the plumbing that
    /// reads the platform setting and threads it here lands in a later task
    /// — every baseline below defaults this to `false`.
    pub reduce_motion: bool,
}

impl MotionScheme {
    /// The Material 3 Expressive baseline motion scheme.
    ///
    /// Duration/easing tokens are sourced from
    /// `material-components-android` `docs/theming/Motion.md` (verified
    /// 2026-07-17, same source as the spring presets above), which
    /// publishes a 16-value duration scale in four tiers — Short (50, 100,
    /// 150, 200ms), Medium (250, 300, 350, 400ms), Long (450, 500, 550,
    /// 600ms), Extra Long (700–1000ms) — plus the easing curves below.
    /// Glyph's five-slot [`MotionDurations`] vocabulary maps onto that scale
    /// as: `instant` → Short1 (50ms), `fast` → Short3 (150ms), `base` →
    /// Medium2 (300ms, the most commonly-cited M3 "default" transition
    /// duration), `slow` → Long2 (500ms), `deliberate` → the Extra Long
    /// tier's floor (700ms).
    ///
    /// [`EasingSet`]'s three slots map onto M3's own easing-curve tokens
    /// (Jetpack Compose's `androidx.compose.material3.tokens.MotionTokens`
    /// control points, same source): `spatial` → Emphasized Decelerate
    /// (`cubic-bezier(0.05, 0.7, 0.1, 1.0)`, M3's curve for entering/spatial
    /// transitions — its steep initial deceleration reads as a slight
    /// overshoot-adjacent settle), `effects` → Standard
    /// (`cubic-bezier(0.2, 0.0, 0.0, 1.0)`, M3's curve for opacity/color
    /// fades — never overshoots), `exit` → Standard Accelerate
    /// (`cubic-bezier(0.3, 0.0, 1.0, 1.0)`, M3's curve for elements leaving
    /// the screen).
    pub const fn m3_expressive() -> Self {
        Self {
            fast_spatial: MotionSpring::new(0.9, 1400.0),
            fast_effects: MotionSpring::new(1.0, 3800.0),
            default_spatial: MotionSpring::new(0.9, 700.0),
            default_effects: MotionSpring::new(1.0, 1600.0),
            slow_spatial: MotionSpring::new(0.9, 300.0),
            slow_effects: MotionSpring::new(1.0, 800.0),
            durations: MotionDurations {
                instant: 50.0,
                fast: 150.0,
                base: 300.0,
                slow: 500.0,
                deliberate: 700.0,
            },
            easing: EasingSet {
                spatial: Curve::Cubic(0.05, 0.7, 0.1, 1.0),
                effects: Curve::Cubic(0.2, 0.0, 0.0, 1.0),
                exit: Curve::Cubic(0.3, 0.0, 1.0, 1.0),
            },
            reduce_motion: false,
        }
    }

    /// The Cupertino (iOS) motion scheme — the single community-documented
    /// iOS spring baseline (mass 1.0, stiffness 170.0, damping coefficient
    /// 15.0 → damping ratio ζ ≈ 0.5753) applied uniformly to all six spring
    /// slots. See the module docs for the ζ conversion and why iOS has no
    /// documented fast/default/slow or spatial/effects distinction to
    /// otherwise vary these by.
    ///
    /// **Community-approximate** duration/easing tokens: unlike M3, iOS
    /// publishes no named multi-tier duration scale or curve-category
    /// vocabulary — UIKit/SwiftUI animations are conventionally
    /// spring-authored (the six presets above), not bezier-timed. The one
    /// citable data point is `workflow/plans/features/frust-phase-6c-widget-catalog/research/RESEARCH.md`'s
    /// `cupertino-tokens-idioms` claim (retrieved 2026-07-17, citing Apple's
    /// WWDC23 "Animate with springs" talk): SwiftUI's `.bouncy` spring
    /// preset defaults to a 0.5s duration — anchoring `slow` at 500ms here.
    /// The remaining four slots (`instant`/`fast`/`base`/`deliberate`) are a
    /// community-approximate scale around that anchor, loosely following
    /// commonly-cited iOS interaction timings (a ~0.2s "quick" feel, a
    /// ~0.35s modal-presentation-adjacent "base" feel); a future task should
    /// replace them if a citable per-tier iOS source turns up. The easing
    /// vocabulary has the same gap as the springs: no published
    /// spatial/effects/exit distinction, so [`Curve::EaseInOut`] (UIKit's
    /// default `UIView.AnimationOptions` curve) is applied uniformly to all
    /// three [`EasingSet`] slots, mirroring the springs' uniform-baseline
    /// treatment above.
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
            durations: MotionDurations {
                instant: 100.0,
                fast: 200.0,
                base: 350.0,
                slow: 500.0,
                deliberate: 700.0,
            },
            easing: EasingSet {
                spatial: Curve::EaseInOut,
                effects: Curve::EaseInOut,
                exit: Curve::EaseInOut,
            },
            reduce_motion: false,
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

    #[test]
    fn m3_expressive_durations_match_the_documented_mapping() {
        let m = MotionScheme::m3_expressive();
        assert_eq!(
            m.durations,
            MotionDurations {
                instant: 50.0,
                fast: 150.0,
                base: 300.0,
                slow: 500.0,
                deliberate: 700.0,
            }
        );
    }

    #[test]
    fn m3_expressive_easing_matches_the_documented_tokens() {
        let m = MotionScheme::m3_expressive();
        assert_eq!(m.easing.spatial, Curve::Cubic(0.05, 0.7, 0.1, 1.0));
        assert_eq!(m.easing.effects, Curve::Cubic(0.2, 0.0, 0.0, 1.0));
        assert_eq!(m.easing.exit, Curve::Cubic(0.3, 0.0, 1.0, 1.0));
    }

    #[test]
    fn cupertino_durations_match_the_documented_mapping() {
        let m = MotionScheme::cupertino();
        assert_eq!(
            m.durations,
            MotionDurations {
                instant: 100.0,
                fast: 200.0,
                base: 350.0,
                slow: 500.0,
                deliberate: 700.0,
            }
        );
    }

    #[test]
    fn cupertino_easing_uses_the_single_curve_uniformly() {
        let m = MotionScheme::cupertino();
        assert_eq!(m.easing.spatial, Curve::EaseInOut);
        assert_eq!(m.easing.effects, Curve::EaseInOut);
        assert_eq!(m.easing.exit, Curve::EaseInOut);
    }

    #[test]
    fn reduce_motion_defaults_to_false_on_both_baselines() {
        assert!(!MotionScheme::m3_expressive().reduce_motion);
        assert!(!MotionScheme::cupertino().reduce_motion);
    }

    // Both constructors must stay `const fn` — a regression here is a
    // compile error, not a runtime assertion (task acceptance criterion 3).
    const M3_CONST: MotionScheme = MotionScheme::m3_expressive();
    const CUPERTINO_CONST: MotionScheme = MotionScheme::cupertino();

    #[test]
    fn constructors_stay_const_fn() {
        assert_eq!(M3_CONST, MotionScheme::m3_expressive());
        assert_eq!(CUPERTINO_CONST, MotionScheme::cupertino());
    }
}
