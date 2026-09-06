//! [`MotionScheme::neutral`]: the design-language-free motion scheme — six
//! spring presets plus a duration/easing token vocabulary, replacing a plain
//! duration+easing-only model.
//!
//! This crate constructs no other `MotionScheme` — a design system builds
//! its own from its own plugin crate (`frust-material`'s `tokens` module
//! carries the M3 Expressive six-spring mapping, source cited there;
//! `frust-cupertino`'s carries the community-documented iOS spring
//! baseline). Unlike [`crate::shape`]/[`crate::elevation`]/[`crate::typography`],
//! [`MotionScheme::neutral`]'s springs are **not** a reused M3 table — every
//! spring here is critically damped (no bounce), a deliberately unbranded
//! feel distinct from either design system's springier presets (see that
//! constructor's own doc comment).
//!
//! `MotionSpring` is plain data defined here rather than reusing
//! `frust-core`'s animation-core spring type: `frust-theme` already
//! depends on `frust-core` (see `docs/ARCHITECTURE.md`), so
//! [`impl From<MotionSpring> for SpringDesc`](struct.MotionSpring.html)
//! below lives right here rather than in a separate facade conversion.

use frust_core::anim::{Curve, SpringDesc};

/// A physics spring's damping ratio and stiffness (mass is implicitly `1.0`
/// for every preset; see module docs).
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

/// Converts a [`MotionSpring`] token into the generic physics type
/// [`AnimationController::fling`](frust_core::anim::AnimationController::fling)/
/// [`Spring::new`](frust_core::anim::Spring::new) consume.
///
/// Mass is always `1.0` — every preset (this crate's and each design
/// system's own) is defined purely in terms of damping ratio + stiffness, so
/// `mass` has no token-level source to convert from.
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
/// Glyph's motion language is bezier/duration-authored (unlike the
/// spring-authored presets above), so a [`MotionScheme`] carries this
/// five-slot duration vocabulary alongside the six spring presets — see
/// [`MotionScheme::neutral`] for this crate's own mapping, and
/// `frust-material`/`frust-cupertino`'s own `tokens` modules for their
/// per-baseline mappings and sources.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionDurations {
    pub instant: f64,
    pub fast: f64,
    pub base: f64,
    pub slow: f64,
    pub deliberate: f64,
}

/// The `CosmeticLoop` tick-rate cap (Hz) — an app-tunable pacing token
/// consumed by the frame gate's pacing: a
/// purely cosmetic, indefinitely-looping animation (a shimmer/pulse with no
/// user-visible endpoint) is capped to this rate rather than repainting
/// every display frame. **Uncapped (0/`None`) is not an allowed value** —
/// a cosmetic loop always paces to *some* ceiling, so this type clamps any
/// constructed value up to [`CosmeticLoopRate::FLOOR_HZ`] (10Hz), a sane
/// floor below which a "cosmetic" loop reads as visibly stuttering rather
/// than paced.
///
/// This token only *declares* the cap; nothing in `frust-theme` reads a
/// clock or paces a loop — the frame-gate consumer is what actually honors it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CosmeticLoopRate(f32);

impl CosmeticLoopRate {
    /// The floor every constructed rate clamps up to — below this a
    /// "cosmetic" pace reads as stutter rather than a deliberate cap.
    pub const FLOOR_HZ: f32 = 10.0;

    /// Builds a rate, clamping `hz` up to [`FLOOR_HZ`](Self::FLOOR_HZ) (no
    /// uncapped/zero value is representable). `const fn` so every
    /// [`MotionScheme`] baseline constructor below can stay `const`.
    ///
    /// The clamp is **NaN-safe**: a plain `hz < FLOOR_HZ` test lets `NaN`
    /// slip through (every ordered comparison with `NaN` is `false`), so `NaN`
    /// is caught explicitly and clamped to the floor alongside any finite
    /// below-floor value. This matters because the desktop shell derives its
    /// per-frame paced interval as `Duration::from_secs_f32(1.0 / hz())`, and
    /// `from_secs_f32` panics on a `NaN` argument — clamping here guarantees a
    /// finite, `>= FLOOR_HZ` rate so that division can never produce one. (The
    /// equivalent `!(hz >= FLOOR_HZ)` one-liner is clearer intent-wise but
    /// trips clippy's `neg_cmp_op_on_partial_ord`; the explicit `is_nan()`
    /// disjunction below is the lint-clean form of the same NaN-safe clamp.)
    pub const fn new(hz: f32) -> Self {
        if hz.is_nan() || hz < Self::FLOOR_HZ {
            Self(Self::FLOOR_HZ)
        } else {
            Self(hz)
        }
    }

    /// The clamped rate, in Hz.
    pub const fn hz(self) -> f32 {
        self.0
    }
}

/// The three-easing vocabulary Glyph's bezier-authored motion patterns are
/// built from: `spatial` (position/size changes — may overshoot), `effects`
/// (opacity/color changes — never overshoots), and `exit` (elements leaving
/// the screen — accelerates out). See [`MotionScheme::neutral`] for this
/// crate's own curve values, and `frust-material`/`frust-cupertino`'s own
/// `tokens` modules for their per-baseline curve values and sources.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EasingSet {
    pub spatial: Curve,
    pub effects: Curve,
    pub exit: Curve,
}

/// Six motion spring presets, plus the Glyph duration/easing token
/// vocabulary mapped onto each baseline.
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
    /// Collapses patterned motion to a fast crossfade. Every baseline below
    /// defaults this to `false` — the correct value absent an OS signal — and
    /// a mobile shell raises it from the platform's own reduced-motion
    /// accessibility preference:
    ///
    /// - **Android**: `Settings.Global.ANIMATOR_DURATION_SCALE`, reduced when
    ///   the scale is exactly `0` (the same signal `ValueAnimator::
    ///   areAnimatorsEnabled` consults). It is **not** a `Configuration`
    ///   field, so `onConfigurationChanged` never reports it — the embedding
    ///   registers a `ContentObserver` on that setting's URI and re-reads it
    ///   on every resume.
    /// - **iOS**: `UIAccessibility.isReduceMotionEnabled`, observed through
    ///   `UIAccessibility.reduceMotionStatusDidChangeNotification`. It is
    ///   **not** a `UITraitCollection` trait, so `traitCollectionDidChange`
    ///   never fires for it.
    /// - **Desktop**: no source. winit 0.30 exposes no reduced-motion (or any
    ///   other accessibility-preference) accessor and nothing else in the
    ///   desktop path reads one, so the desktop shell leaves this at whatever
    ///   the active theme authored — a deliberate, checked gap (2026-07-31),
    ///   not an oversight to re-investigate.
    ///
    /// Both mobile shells apply the OS report as a **floor**: the platform's
    /// value is OR'd over the active theme's own authored token (each shell's
    /// `effective_reduce_motion`), never assigned over it. So a theme built
    /// with `reduce_motion: true` keeps it while the OS setting is off, and
    /// the OS setting still wins whenever it is on.
    pub reduce_motion: bool,
    /// The `CosmeticLoop` tick-rate cap — see [`CosmeticLoopRate`]. Every
    /// baseline below defaults this to 30Hz; override via
    /// [`crate::builder::ThemeBuilder::map_motion`]. `reduce_motion`
    /// interplay: a widget that honors `reduce_motion` collapses its loop
    /// entirely (this cap never applies then) — this token only paces the
    /// loops that still run when `reduce_motion` is off/unhonored by that
    /// widget.
    pub cosmetic_loop_rate: CosmeticLoopRate,
}

impl MotionScheme {
    /// The neutral, design-language-free motion scheme.
    ///
    /// Every spring is **critically damped** (`damping_ratio: 1.0`, no
    /// overshoot) — unlike `frust-material`'s under-damped `0.9` spatial
    /// springs (a deliberate M3 Expressive personality trait) or
    /// `frust-cupertino`'s community-documented ζ≈0.5753 iOS spring, both of
    /// which bounce by design (see each plugin's own `tokens` module). A
    /// neutral-themed transition settles without overshoot — a deliberately
    /// unbranded feel, not a missing feature. Stiffness still varies by
    /// speed tier (fast > default > slow), the same three-tier shape every
    /// design system's own baseline uses.
    ///
    /// Duration/easing tokens are a plain, **Frust-authored round-number
    /// scale** — not sourced from any published design system's table,
    /// unlike the M3/Cupertino baselines each plugin crate carries — using
    /// only the generic CSS-keyword [`Curve`] variants
    /// (`EaseOut`/`EaseInOut`/`EaseIn`), never a design-language-specific
    /// bezier.
    pub const fn neutral() -> Self {
        Self {
            fast_spatial: MotionSpring::new(1.0, 700.0),
            fast_effects: MotionSpring::new(1.0, 1800.0),
            default_spatial: MotionSpring::new(1.0, 400.0),
            default_effects: MotionSpring::new(1.0, 900.0),
            slow_spatial: MotionSpring::new(1.0, 200.0),
            slow_effects: MotionSpring::new(1.0, 450.0),
            durations: MotionDurations {
                instant: 100.0,
                fast: 150.0,
                base: 250.0,
                slow: 400.0,
                deliberate: 600.0,
            },
            easing: EasingSet {
                spatial: Curve::EaseOut,
                effects: Curve::EaseInOut,
                exit: Curve::EaseIn,
            },
            reduce_motion: false,
            cosmetic_loop_rate: CosmeticLoopRate::new(30.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_desc_round_trips_neutral_presets() {
        let m = MotionScheme::neutral();
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
    fn cosmetic_loop_rate_clamps_to_the_floor() {
        assert_eq!(CosmeticLoopRate::new(0.0).hz(), CosmeticLoopRate::FLOOR_HZ);
        assert_eq!(CosmeticLoopRate::new(-5.0).hz(), CosmeticLoopRate::FLOOR_HZ);
        assert_eq!(CosmeticLoopRate::new(5.0).hz(), CosmeticLoopRate::FLOOR_HZ);
        assert_eq!(
            CosmeticLoopRate::new(CosmeticLoopRate::FLOOR_HZ).hz(),
            CosmeticLoopRate::FLOOR_HZ
        );
    }

    #[test]
    fn cosmetic_loop_rate_passes_through_above_the_floor() {
        assert_eq!(CosmeticLoopRate::new(30.0).hz(), 30.0);
        assert_eq!(CosmeticLoopRate::new(60.0).hz(), 60.0);
    }

    #[test]
    fn cosmetic_loop_rate_clamps_nan_to_the_floor() {
        // NaN-safe clamp: `NaN >= FLOOR` is false, so `new` clamps NaN up to
        // the floor. The resulting rate must be finite so the desktop shell's
        // `Duration::from_secs_f32(1.0 / hz())` per-frame interval cannot
        // panic (`from_secs_f32` panics on a NaN argument).
        let rate = CosmeticLoopRate::new(f32::NAN);
        assert_eq!(rate.hz(), CosmeticLoopRate::FLOOR_HZ);
        assert!(rate.hz().is_finite());
        // And `1.0 / hz()` — the value that actually reaches `from_secs_f32` —
        // is finite and positive, never NaN.
        assert!((1.0 / rate.hz()).is_finite());
    }

    #[test]
    fn cosmetic_loop_rate_new_is_const_fn() {
        const RATE: CosmeticLoopRate = CosmeticLoopRate::new(45.0);
        assert_eq!(RATE.hz(), 45.0);
    }

    #[test]
    fn theme_builder_map_motion_overrides_the_cosmetic_loop_rate() {
        // Task acceptance criterion 1: ThemeBuilder override test.
        use crate::theme::Theme;

        let base = Theme::neutral();
        let theme = Theme::builder(base.clone())
            .map_motion(|m| MotionScheme {
                cosmetic_loop_rate: CosmeticLoopRate::new(15.0),
                ..m
            })
            .build();

        assert_eq!(theme.motion.cosmetic_loop_rate.hz(), 15.0);
        // Nothing else in the motion group moved.
        assert_eq!(theme.motion.fast_spatial, base.motion.fast_spatial);
        assert_eq!(theme.motion.durations, base.motion.durations);
    }

    // ---- Neutral scheme -----------------------------------------------

    #[test]
    fn neutral_springs_are_all_critically_damped() {
        // No overshoot anywhere — unlike frust-material's 0.9 spatial
        // springs or frust-cupertino's ~0.5753 uniform spring.
        let m = MotionScheme::neutral();
        for s in [
            m.fast_spatial,
            m.fast_effects,
            m.default_spatial,
            m.default_effects,
            m.slow_spatial,
            m.slow_effects,
        ] {
            assert_eq!(s.damping_ratio, 1.0);
        }
    }

    #[test]
    fn neutral_stiffness_still_varies_by_speed_tier() {
        let m = MotionScheme::neutral();
        assert!(m.fast_spatial.stiffness > m.default_spatial.stiffness);
        assert!(m.default_spatial.stiffness > m.slow_spatial.stiffness);
        assert!(m.fast_effects.stiffness > m.default_effects.stiffness);
        assert!(m.default_effects.stiffness > m.slow_effects.stiffness);
    }

    #[test]
    fn neutral_easing_uses_only_generic_curves() {
        let m = MotionScheme::neutral();
        assert_eq!(m.easing.spatial, Curve::EaseOut);
        assert_eq!(m.easing.effects, Curve::EaseInOut);
        assert_eq!(m.easing.exit, Curve::EaseIn);
    }

    #[test]
    fn neutral_cosmetic_loop_rate_defaults_to_30hz() {
        assert_eq!(MotionScheme::neutral().cosmetic_loop_rate.hz(), 30.0);
    }

    #[test]
    fn neutral_reduce_motion_defaults_to_false() {
        assert!(!MotionScheme::neutral().reduce_motion);
    }

    const NEUTRAL_CONST: MotionScheme = MotionScheme::neutral();

    #[test]
    fn neutral_stays_const_fn() {
        assert_eq!(NEUTRAL_CONST, MotionScheme::neutral());
    }
}
