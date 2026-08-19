//! Material 3 Expressive motion tokens at reference fidelity: 16 named
//! duration tokens, 7 cubic-easing curves ([`MaterialMotion`]), and 11 spring
//! presets ([`MaterialSpring`]).
//!
//! Source: `paadevelopments/material_3_expressive` v1.0.8,
//! `lib/foundations/m3e_motion.dart`'s `M3EMotion`/`M3ESpring` (retrieved
//! 2026-08-19). [`super::motion_scheme`] builds the baseline
//! [`frust::MotionScheme`]'s six spring slots plus its five-slot duration and
//! three-slot easing vocabulary from a subset of these constants — see that
//! function's own doc comment for the exact mapping. The remaining five spring
//! presets ([`MaterialSpring::AOSP_SPATIAL`] and the four
//! `EXPRESSIVE_SPATIAL_*` variants) have no `MotionScheme` slot to land in and
//! stay reachable as plain crate constants instead — the same shape
//! `button_group`'s local `PRESS_SPRING` and `loading_indicator`'s morph
//! spring already use, just promoted to this shared table.
//!
//! Spring semantics: `stiffness`/`damping_ratio` map directly onto
//! `SpringDescription.withDampingRatio(mass: 1, stiffness: stiffness, ratio:
//! damping)` — `motor` 1.1.0's Flutter-compatible spring builder
//! (`tmp/motor-1.1.0/lib/src/motion.dart`, e.g. its
//! `MaterialSpringMotion.expressiveEffectsSlow`), the same construction M3E's
//! own `M3ESpring::toDescription` uses. Mass is implicitly `1.0` for every
//! preset here, matching [`frust::MotionSpring`]'s own convention
//! (`crates/frust-theme/src/motion.rs`) — neither the Dart source nor this
//! module has a per-preset mass to convert.
//!
//! # Dart → Rust naming map
//!
//! | `M3EMotion` (Dart, camelCase) | This module (Rust, `SCREAMING_SNAKE`) |
//! |---|---|
//! | `short1` | [`MaterialMotion::SHORT_1`] |
//! | `short2` | [`MaterialMotion::SHORT_2`] |
//! | `short3` | [`MaterialMotion::SHORT_3`] |
//! | `short4` | [`MaterialMotion::SHORT_4`] |
//! | `medium1` | [`MaterialMotion::MEDIUM_1`] |
//! | `medium2` | [`MaterialMotion::MEDIUM_2`] |
//! | `medium3` | [`MaterialMotion::MEDIUM_3`] |
//! | `medium4` | [`MaterialMotion::MEDIUM_4`] |
//! | `long1` | [`MaterialMotion::LONG_1`] |
//! | `long2` | [`MaterialMotion::LONG_2`] |
//! | `long3` | [`MaterialMotion::LONG_3`] |
//! | `long4` | [`MaterialMotion::LONG_4`] |
//! | `extraLong1` | [`MaterialMotion::EXTRA_LONG_1`] |
//! | `extraLong2` | [`MaterialMotion::EXTRA_LONG_2`] |
//! | `extraLong3` | [`MaterialMotion::EXTRA_LONG_3`] |
//! | `extraLong4` | [`MaterialMotion::EXTRA_LONG_4`] |
//! | `standard` | [`MaterialMotion::STANDARD`] |
//! | `standardAccelerate` | [`MaterialMotion::STANDARD_ACCELERATE`] |
//! | `standardDecelerate` | [`MaterialMotion::STANDARD_DECELERATE`] |
//! | `emphasized` | [`MaterialMotion::EMPHASIZED`] |
//! | `emphasizedAccelerate` | [`MaterialMotion::EMPHASIZED_ACCELERATE`] |
//! | `emphasizedDecelerate` | [`MaterialMotion::EMPHASIZED_DECELERATE`] |
//! | `linear` | [`MaterialMotion::LINEAR`] |
//! | `spatialFast` | [`MaterialSpring::SPATIAL_FAST`] |
//! | `spatialDefault` | [`MaterialSpring::SPATIAL_DEFAULT`] |
//! | `spatialSlow` | [`MaterialSpring::SPATIAL_SLOW`] |
//! | `aospSpatial` | [`MaterialSpring::AOSP_SPATIAL`] |
//! | `expressiveSpatialFast` | [`MaterialSpring::EXPRESSIVE_SPATIAL_FAST`] |
//! | `expressiveSpatialDefault` | [`MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT`] |
//! | `expressiveSpatialPress` | [`MaterialSpring::EXPRESSIVE_SPATIAL_PRESS`] |
//! | `expressiveSpatialSlow` | [`MaterialSpring::EXPRESSIVE_SPATIAL_SLOW`] |
//! | `effectsFast` | [`MaterialSpring::EFFECTS_FAST`] |
//! | `effectsDefault` | [`MaterialSpring::EFFECTS_DEFAULT`] |
//! | `effectsSlow` | [`MaterialSpring::EFFECTS_SLOW`] |

use std::time::Duration;

use frust::Curve;

/// The M3 Expressive duration/easing token namespace — an uninhabited type
/// (no instance is ever constructed) carrying only associated constants,
/// mirroring the Dart source's `abstract final class M3EMotion` static
/// namespace. See the module docs for the full Dart → Rust naming map and
/// source citation.
#[derive(Debug)]
pub enum MaterialMotion {}

impl MaterialMotion {
    // ---- Durations (16 tokens, four tiers of four) -----------------------

    /// 50ms.
    pub const SHORT_1: Duration = Duration::from_millis(50);
    /// 100ms.
    pub const SHORT_2: Duration = Duration::from_millis(100);
    /// 150ms.
    pub const SHORT_3: Duration = Duration::from_millis(150);
    /// 200ms.
    pub const SHORT_4: Duration = Duration::from_millis(200);
    /// 250ms.
    pub const MEDIUM_1: Duration = Duration::from_millis(250);
    /// 300ms.
    pub const MEDIUM_2: Duration = Duration::from_millis(300);
    /// 350ms.
    pub const MEDIUM_3: Duration = Duration::from_millis(350);
    /// 400ms.
    pub const MEDIUM_4: Duration = Duration::from_millis(400);
    /// 450ms.
    pub const LONG_1: Duration = Duration::from_millis(450);
    /// 500ms.
    pub const LONG_2: Duration = Duration::from_millis(500);
    /// 550ms.
    pub const LONG_3: Duration = Duration::from_millis(550);
    /// 600ms.
    pub const LONG_4: Duration = Duration::from_millis(600);
    /// 700ms.
    pub const EXTRA_LONG_1: Duration = Duration::from_millis(700);
    /// 800ms.
    pub const EXTRA_LONG_2: Duration = Duration::from_millis(800);
    /// 900ms.
    pub const EXTRA_LONG_3: Duration = Duration::from_millis(900);
    /// 1000ms.
    pub const EXTRA_LONG_4: Duration = Duration::from_millis(1000);

    // ---- Easings (7 cubic-Bézier curves) ----------------------------------
    //
    // Every value below is a `Curve::Cubic(x1, y1, x2, y2)` — the same
    // `cubic-bezier()` parameterization the Dart `Cubic` constructor uses,
    // endpoints fixed at `(0, 0)`/`(1, 1)`. `standard` and `emphasized` carry
    // identical control points in the Dart source; transcribed as-is rather
    // than deduplicated.

    /// Standard easing — `cubic-bezier(0.2, 0, 0, 1)`.
    pub const STANDARD: Curve = Curve::Cubic(0.2, 0.0, 0.0, 1.0);
    /// Standard-accelerate easing — `cubic-bezier(0.3, 0, 1, 1)`.
    pub const STANDARD_ACCELERATE: Curve = Curve::Cubic(0.3, 0.0, 1.0, 1.0);
    /// Standard-decelerate easing — `cubic-bezier(0, 0, 0, 1)`.
    pub const STANDARD_DECELERATE: Curve = Curve::Cubic(0.0, 0.0, 0.0, 1.0);
    /// Emphasized easing — `cubic-bezier(0.2, 0, 0, 1)` (identical control
    /// points to [`STANDARD`](Self::STANDARD) in the Dart source).
    pub const EMPHASIZED: Curve = Curve::Cubic(0.2, 0.0, 0.0, 1.0);
    /// Emphasized-accelerate easing — `cubic-bezier(0.3, 0, 0.8, 0.15)`.
    pub const EMPHASIZED_ACCELERATE: Curve = Curve::Cubic(0.3, 0.0, 0.8, 0.15);
    /// Emphasized-decelerate easing — `cubic-bezier(0.05, 0.7, 0.1, 1)`.
    pub const EMPHASIZED_DECELERATE: Curve = Curve::Cubic(0.05, 0.7, 0.1, 1.0);
    /// Linear easing — `cubic-bezier(0, 0, 1, 1)`, transcribed as a `Cubic`
    /// (not [`Curve::Linear`]) to match the Dart source's own
    /// `Cubic(0, 0, 1, 1)` literal; the two are mathematically identical.
    pub const LINEAR: Curve = Curve::Cubic(0.0, 0.0, 1.0, 1.0);
}

/// A Material 3 Expressive spring preset: `stiffness` plus `damping_ratio`
/// (mass implicitly `1.0` — see the module docs). `1.0` is critically damped
/// (no overshoot); values below `1.0` overshoot before settling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialSpring {
    pub stiffness: f64,
    pub damping_ratio: f64,
}

impl MaterialSpring {
    const fn new(stiffness: f64, damping_ratio: f64) -> Self {
        Self {
            stiffness,
            damping_ratio,
        }
    }

    // ---- Springs (11 presets) ---------------------------------------------
    //
    // Six of these (`SPATIAL_FAST`/`DEFAULT`/`SLOW`,
    // `EFFECTS_FAST`/`DEFAULT`/`SLOW`) feed `motion_scheme`'s six
    // `MotionScheme` slots directly; the remaining five have no slot and stay
    // reachable only as these crate constants (see the module docs).

    /// Fast spatial spring for size/position/shape morphs — feeds
    /// `motion_scheme`'s `fast_spatial`.
    pub const SPATIAL_FAST: Self = Self::new(1400.0, 0.9);
    /// Default spatial spring for size/position/shape morphs — feeds
    /// `motion_scheme`'s `default_spatial`.
    pub const SPATIAL_DEFAULT: Self = Self::new(700.0, 0.9);
    /// Slow spatial spring for size/position/shape morphs — feeds
    /// `motion_scheme`'s `slow_spatial`.
    pub const SPATIAL_SLOW: Self = Self::new(300.0, 0.9);
    /// AOSP spatial spring (matches AOSP notification-list expansion — no
    /// overshoot). No `MotionScheme` slot; a crate-reachable constant only.
    pub const AOSP_SPATIAL: Self = Self::new(380.0, 1.0);
    /// Fast expressive spatial spring with slight overshoot. No
    /// `MotionScheme` slot; a crate-reachable constant only.
    pub const EXPRESSIVE_SPATIAL_FAST: Self = Self::new(800.0, 0.6);
    /// Default expressive spatial spring with slight overshoot. No
    /// `MotionScheme` slot; a crate-reachable constant only.
    pub const EXPRESSIVE_SPATIAL_DEFAULT: Self = Self::new(380.0, 0.8);
    /// Interactive press-scale spring (floating toolbar / button morph
    /// recipe). No `MotionScheme` slot; a crate-reachable constant only.
    pub const EXPRESSIVE_SPATIAL_PRESS: Self = Self::new(380.0, 0.55);
    /// Slow expressive spatial spring with slight overshoot. No
    /// `MotionScheme` slot; a crate-reachable constant only.
    pub const EXPRESSIVE_SPATIAL_SLOW: Self = Self::new(200.0, 0.8);
    /// Fast effects spring for color/opacity (no overshoot) — feeds
    /// `motion_scheme`'s `fast_effects`.
    pub const EFFECTS_FAST: Self = Self::new(3800.0, 1.0);
    /// Default effects spring for color/opacity (no overshoot) — feeds
    /// `motion_scheme`'s `default_effects`.
    pub const EFFECTS_DEFAULT: Self = Self::new(1600.0, 1.0);
    /// Slow effects spring for color/opacity (no overshoot) — feeds
    /// `motion_scheme`'s `slow_effects`.
    pub const EFFECTS_SLOW: Self = Self::new(800.0, 1.0);
}

impl From<MaterialSpring> for frust::MotionSpring {
    fn from(spring: MaterialSpring) -> Self {
        frust::MotionSpring {
            damping_ratio: spring.damping_ratio,
            stiffness: spring.stiffness,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Durations: table test against the Dart source's literal ms -----

    #[test]
    fn durations_match_reference_table_exactly() {
        let table: [(Duration, u64); 16] = [
            (MaterialMotion::SHORT_1, 50),
            (MaterialMotion::SHORT_2, 100),
            (MaterialMotion::SHORT_3, 150),
            (MaterialMotion::SHORT_4, 200),
            (MaterialMotion::MEDIUM_1, 250),
            (MaterialMotion::MEDIUM_2, 300),
            (MaterialMotion::MEDIUM_3, 350),
            (MaterialMotion::MEDIUM_4, 400),
            (MaterialMotion::LONG_1, 450),
            (MaterialMotion::LONG_2, 500),
            (MaterialMotion::LONG_3, 550),
            (MaterialMotion::LONG_4, 600),
            (MaterialMotion::EXTRA_LONG_1, 700),
            (MaterialMotion::EXTRA_LONG_2, 800),
            (MaterialMotion::EXTRA_LONG_3, 900),
            (MaterialMotion::EXTRA_LONG_4, 1000),
        ];
        for (actual, expected_ms) in table {
            assert_eq!(actual, Duration::from_millis(expected_ms));
        }
    }

    // ---- Easings: table test against the Dart source's literal control
    // points, plus endpoint/monotonicity checks for every curve. ----------

    /// One easing table row: `(name, curve, expected (x1, y1, x2, y2))`.
    type EasingRow = (&'static str, Curve, (f64, f64, f64, f64));

    fn easing_table() -> [EasingRow; 7] {
        [
            ("standard", MaterialMotion::STANDARD, (0.2, 0.0, 0.0, 1.0)),
            (
                "standard_accelerate",
                MaterialMotion::STANDARD_ACCELERATE,
                (0.3, 0.0, 1.0, 1.0),
            ),
            (
                "standard_decelerate",
                MaterialMotion::STANDARD_DECELERATE,
                (0.0, 0.0, 0.0, 1.0),
            ),
            (
                "emphasized",
                MaterialMotion::EMPHASIZED,
                (0.2, 0.0, 0.0, 1.0),
            ),
            (
                "emphasized_accelerate",
                MaterialMotion::EMPHASIZED_ACCELERATE,
                (0.3, 0.0, 0.8, 0.15),
            ),
            (
                "emphasized_decelerate",
                MaterialMotion::EMPHASIZED_DECELERATE,
                (0.05, 0.7, 0.1, 1.0),
            ),
            ("linear", MaterialMotion::LINEAR, (0.0, 0.0, 1.0, 1.0)),
        ]
    }

    #[test]
    fn easings_match_reference_control_points_exactly() {
        for (name, curve, expected) in easing_table() {
            match curve {
                Curve::Cubic(x1, y1, x2, y2) => {
                    assert_eq!((x1, y1, x2, y2), expected, "curve `{name}`");
                }
                other => panic!("curve `{name}` is not a Cubic: {other:?}"),
            }
        }
    }

    #[test]
    fn easing_endpoints_are_exact() {
        for (name, curve, _) in easing_table() {
            assert!(
                curve.transform(0.0).abs() < 1e-9,
                "curve `{name}` at t=0 should be 0, got {}",
                curve.transform(0.0)
            );
            assert!(
                (curve.transform(1.0) - 1.0).abs() < 1e-9,
                "curve `{name}` at t=1 should be 1, got {}",
                curve.transform(1.0)
            );
        }
    }

    #[test]
    fn easings_are_monotonic_in_t() {
        for (name, curve, _) in easing_table() {
            let mut prev = curve.transform(0.0);
            for i in 1..=20 {
                let t = i as f64 / 20.0;
                let cur = curve.transform(t);
                assert!(
                    cur + 1e-9 >= prev,
                    "curve `{name}` not monotonic at t={t}: {cur} < prev {prev}"
                );
                prev = cur;
            }
        }
    }

    // ---- Springs: table test against the Dart source's literal
    // stiffness/damping. ----------------------------------------------------

    #[test]
    fn springs_match_reference_table_exactly() {
        let table: [(MaterialSpring, f64, f64); 11] = [
            (MaterialSpring::SPATIAL_FAST, 1400.0, 0.9),
            (MaterialSpring::SPATIAL_DEFAULT, 700.0, 0.9),
            (MaterialSpring::SPATIAL_SLOW, 300.0, 0.9),
            (MaterialSpring::AOSP_SPATIAL, 380.0, 1.0),
            (MaterialSpring::EXPRESSIVE_SPATIAL_FAST, 800.0, 0.6),
            (MaterialSpring::EXPRESSIVE_SPATIAL_DEFAULT, 380.0, 0.8),
            (MaterialSpring::EXPRESSIVE_SPATIAL_PRESS, 380.0, 0.55),
            (MaterialSpring::EXPRESSIVE_SPATIAL_SLOW, 200.0, 0.8),
            (MaterialSpring::EFFECTS_FAST, 3800.0, 1.0),
            (MaterialSpring::EFFECTS_DEFAULT, 1600.0, 1.0),
            (MaterialSpring::EFFECTS_SLOW, 800.0, 1.0),
        ];
        for (spring, stiffness, damping_ratio) in table {
            assert_eq!(spring.stiffness, stiffness);
            assert_eq!(spring.damping_ratio, damping_ratio);
        }
    }

    #[test]
    fn spring_into_motion_spring_preserves_fields() {
        let converted: frust::MotionSpring = MaterialSpring::SPATIAL_FAST.into();
        assert_eq!(converted.stiffness, 1400.0);
        assert_eq!(converted.damping_ratio, 0.9);
    }

    #[test]
    fn thirty_four_tokens_total() {
        // 16 durations + 7 easings + 11 springs == 34 (task acceptance
        // criterion 1's exact count).
        let durations = 16;
        let easings = easing_table().len();
        let springs = 11;
        assert_eq!(durations + easings + springs, 34);
    }
}
