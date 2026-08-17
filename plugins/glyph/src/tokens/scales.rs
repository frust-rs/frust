//! Glyph type / shape / elevation / motion / glass scales.
//!
//! Sourced from the vendored Glyph design mockups (retrieved 2026-07-21): the
//! type scale, shape scale, per-brightness elevation shadows, and motion
//! vocabulary below are transcribed values, not invented ones. Each scale maps
//! onto the *same* fixed struct its M3/Cupertino siblings use, so every
//! existing widget keeps resolving tokens unchanged under a Glyph theme.

use frust::authoring::text::{FontFamily, FontWeight, GenericSlot, LineHeight, TextStyle};
use frust::{
    Curve, EasingSet, Elevation, ElevationLevel, GlassMaterial, GlassScale, MotionDurations,
    MotionScheme, MotionSpring, ShadowSpec, ShapeScale, SurfaceRole, TypeScale,
};

// ---- Type scale ---------------------------------------------------------

/// Which Glyph face a slot uses: **Space Mono** (700, display/headline) or
/// **IBM Plex Mono** (UI: title/body/label). Both end their fallback stack in
/// a generic `monospace` so a host without the bundled face still shapes
/// fixed-width.
#[derive(Clone, Copy)]
enum Face {
    /// Space Mono — the display/headline face.
    Display,
    /// IBM Plex Mono — the UI face.
    Ui,
}

impl Face {
    fn family(self) -> FontFamily {
        match self {
            Face::Display => FontFamily::stack_with_generic(["Space Mono"], GenericSlot::Monospace),
            Face::Ui => FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace),
        }
    }
}

/// One Glyph type token: `(size_px, line_height_px, letter_spacing_px,
/// weight, face)`.
type GlyphToken = (f32, f32, f32, FontWeight, Face);

// The six Glyph type roles. Web-authored sizes are noted where
// the mobile-legibility floor (body >= 13 logical px, tunable) bumps them.
// Line heights derive from the source's stated 1.5 body default (display/
// headline tightened, per the compact terminal aesthetic) — documented policy,
// not a per-token source value.
const GLYPH_DISPLAY: GlyphToken = (32.0, 36.0, 0.0, FontWeight::BOLD, Face::Display); // display/32, Space Mono 700
const GLYPH_HEADING: GlyphToken = (20.0, 24.0, 0.0, FontWeight::BOLD, Face::Display); // heading/20, Space Mono 700
const GLYPH_TITLE: GlyphToken = (15.0, 22.0, 0.0, FontWeight::MEDIUM, Face::Ui); // title/15, 500
const GLYPH_BODY: GlyphToken = (13.0, 20.0, 0.0, FontWeight::REGULAR, Face::Ui); // body (web 12.5 -> floor 13), 400
const GLYPH_CAPTION: GlyphToken = (11.0, 17.0, 0.0, FontWeight::REGULAR, Face::Ui); // caption/11
const GLYPH_LABEL: GlyphToken = (12.5, 18.0, 0.125, FontWeight::MEDIUM, Face::Ui); // button label 12.5/500, 0.01em
const GLYPH_MICRO: GlyphToken = (9.5, 14.0, 0.475, FontWeight::MEDIUM, Face::Ui); // micro/9.5, +0.05em tracking

fn apply_glyph(base: &TextStyle, token: GlyphToken) -> TextStyle {
    let (size, line_height_px, letter_spacing, weight, face) = token;
    TextStyle {
        family: face.family(),
        size,
        weight,
        letter_spacing,
        line_height: LineHeight::Absolute(line_height_px),
        ..base.clone()
    }
}

/// Steps a Glyph token's weight up one rung for the `_emphasized` sibling
/// (Regular -> Medium, Medium -> Semibold; Bold stays Bold — Space Mono ships
/// no heavier face), keeping size/line-height/tracking/face unchanged.
fn apply_glyph_emphasized(base: &TextStyle, token: GlyphToken) -> TextStyle {
    let (size, lh, ls, weight, face) = token;
    let up = if weight == FontWeight::REGULAR {
        FontWeight::MEDIUM
    } else if weight == FontWeight::MEDIUM {
        FontWeight::SEMI_BOLD
    } else {
        weight // Bold display/headline: no heavier Space Mono face
    };
    apply_glyph(base, (size, lh, ls, up, face))
}

/// The Glyph type scale (retrieved 2026-07-21). Maps the six
/// monospace-led Glyph roles onto the 30 `TypeScale` slots with per-slot
/// families à la `TypeScale::cupertino`: Space Mono (700) fills every
/// `display_*`/`headline_*` slot, IBM Plex Mono fills `title_*`/`body_*`/
/// `label_*`. Like the Cupertino scale, `family` is Glyph-chosen per slot
/// (not inherited from `base`); `base`'s `style`/`color` are preserved.
///
/// The web-authored `body` size (12.5) is bumped to the mobile-legibility
/// floor (13, tunable); the smaller `caption`/`micro` roles keep their
/// authored sizes (documented in the const table above).
pub fn type_scale(base: &TextStyle) -> TypeScale {
    TypeScale {
        display_large: apply_glyph(base, GLYPH_DISPLAY),
        display_medium: apply_glyph(base, GLYPH_DISPLAY),
        display_small: apply_glyph(base, GLYPH_DISPLAY),
        headline_large: apply_glyph(base, GLYPH_HEADING),
        headline_medium: apply_glyph(base, GLYPH_HEADING),
        headline_small: apply_glyph(base, GLYPH_HEADING),
        title_large: apply_glyph(base, GLYPH_TITLE),
        title_medium: apply_glyph(base, GLYPH_TITLE),
        title_small: apply_glyph(base, GLYPH_TITLE),
        body_large: apply_glyph(base, GLYPH_BODY),
        body_medium: apply_glyph(base, GLYPH_BODY),
        body_small: apply_glyph(base, GLYPH_CAPTION),
        label_large: apply_glyph(base, GLYPH_LABEL),
        label_medium: apply_glyph(base, GLYPH_LABEL),
        label_small: apply_glyph(base, GLYPH_MICRO),
        display_large_emphasized: apply_glyph_emphasized(base, GLYPH_DISPLAY),
        display_medium_emphasized: apply_glyph_emphasized(base, GLYPH_DISPLAY),
        display_small_emphasized: apply_glyph_emphasized(base, GLYPH_DISPLAY),
        headline_large_emphasized: apply_glyph_emphasized(base, GLYPH_HEADING),
        headline_medium_emphasized: apply_glyph_emphasized(base, GLYPH_HEADING),
        headline_small_emphasized: apply_glyph_emphasized(base, GLYPH_HEADING),
        title_large_emphasized: apply_glyph_emphasized(base, GLYPH_TITLE),
        title_medium_emphasized: apply_glyph_emphasized(base, GLYPH_TITLE),
        title_small_emphasized: apply_glyph_emphasized(base, GLYPH_TITLE),
        body_large_emphasized: apply_glyph_emphasized(base, GLYPH_BODY),
        body_medium_emphasized: apply_glyph_emphasized(base, GLYPH_BODY),
        body_small_emphasized: apply_glyph_emphasized(base, GLYPH_CAPTION),
        label_large_emphasized: apply_glyph_emphasized(base, GLYPH_LABEL),
        label_medium_emphasized: apply_glyph_emphasized(base, GLYPH_LABEL),
        label_small_emphasized: apply_glyph_emphasized(base, GLYPH_MICRO),
    }
}

// ---- Shape scale ---------------------------------------------------------

/// The Glyph shape scale (retrieved 2026-07-21): the
/// documented **4 / 6 / 10 / 16 / 22 / full** radius scale (`--radius-xs`
/// canonical at 4, not the 3px authoring artifact; `--radius-xl` = 22px;
/// `--radius-full` = 999px → [`f64::INFINITY`]). Glyph's five finite radii
/// fill the low/mid slots; every slot above `extra_large_increased` clamps
/// to 22 (documented mapping, not an extrapolation).
pub const fn shape() -> ShapeScale {
    ShapeScale {
        none: 0.0,
        extra_small: 4.0,      // --radius-xs (canonical 4, per the documented scale)
        small: 6.0,            // --radius-sm
        medium: 10.0,          // --radius-md
        large: 16.0,           // --radius-lg
        large_increased: 22.0, // --radius-xl (terminal card)
        extra_large: 22.0,
        extra_large_increased: 22.0,
        extra_extra_large: 22.0,
        full: f64::INFINITY, // --radius-full 999px
    }
}

// ---- Elevation -------------------------------------------------------

/// A Glyph elevation level: the two source-anchored recipes are the toast
/// (`0 12px 32px`) and modal (`0 24px 60px`) shadows; the low levels are
/// near-zero because Glyph is **borders-first, not shadow-first**.
/// Dark uses black at `.4`/`.5`; light uses warm ink (`ColorScheme`'s
/// `shadow` role) at `.12`/`.18` — a glow reads as haze on white, a shadow
/// reads as lift. The CSS blur-radius px is stored directly as
/// `blur_std_dev` (tunable policy, matching `frust-theme`'s own elevation v1
/// treatment). Levels 1-3 are a documented interpolation between the
/// near-zero floor and the toast recipe; levels 4/5 are the exact source
/// values.
const fn glyph_level(
    dp: f64,
    y: f64,
    blur: f64,
    dark_alpha: f32,
    light_alpha: f32,
    surface_role: SurfaceRole,
) -> ElevationLevel {
    ElevationLevel {
        dp,
        shadow_light: ShadowSpec {
            y_offset: y,
            blur_std_dev: blur,
            color_alpha: light_alpha,
        },
        shadow_dark: ShadowSpec {
            y_offset: y,
            blur_std_dev: blur,
            color_alpha: dark_alpha,
        },
        surface_role,
    }
}

/// The Glyph elevation table (retrieved 2026-07-21) —
/// **per-brightness** shadows: black `.4`/`.5`
/// dark vs warm-ink `.12`/`.18` light. Borders-first, so levels 0-1 carry
/// no/near-no shadow; levels 4 (toast `0 12px 32px`) and 5 (modal `0 24px
/// 60px`) are the exact source recipes.
pub const fn elevation() -> Elevation {
    Elevation {
        level0: glyph_level(0.0, 0.0, 0.0, 0.0, 0.0, SurfaceRole::Surface),
        // Borders-first: a barely-there lift.
        level1: glyph_level(1.0, 1.0, 3.0, 0.10, 0.04, SurfaceRole::SurfaceContainerLow),
        level2: glyph_level(3.0, 4.0, 12.0, 0.22, 0.06, SurfaceRole::SurfaceContainer),
        level3: glyph_level(
            6.0,
            8.0,
            20.0,
            0.30,
            0.09,
            SurfaceRole::SurfaceContainerHigh,
        ),
        // Source: toast `0 12px 32px rgba(0,0,0,.4)` / warm-ink `.12`.
        level4: glyph_level(
            8.0,
            12.0,
            32.0,
            0.40,
            0.12,
            SurfaceRole::SurfaceContainerHigh,
        ),
        // Source: modal `0 24px 60px rgba(0,0,0,.5)` / warm-ink `.18`.
        level5: glyph_level(
            12.0,
            24.0,
            60.0,
            0.50,
            0.18,
            SurfaceRole::SurfaceContainerHighest,
        ),
    }
}

// ---- Motion ---------------------------------------------------------------

/// The Glyph motion scheme (retrieved 2026-07-21).
/// Durations `100/150/220/340/600ms` and the three literal
/// easings are **exact source values**; the six spring slots are
/// **Community-approximate** hand-tuned physics approximations of the
/// authored beziers (Glyph authors motion as bezier+duration, not springs,
/// so no spring source exists) — spatial springs are under-damped (ζ < 1)
/// to reproduce the `spatial cubic-bezier(0.34,1.35,0.64,1)` overshoot,
/// effects springs are critically damped (ζ = 1) since `effects
/// cubic-bezier(0.16,1,0.3,1)` never overshoots. A consumer that wants the
/// authored feel should prefer the `durations`/`easing` tokens; the springs
/// exist so a spring-driven widget still has a Glyph-flavored value.
pub const fn motion() -> MotionScheme {
    // Community-approximate spatial/effects springs (see the doc comment):
    // ζ ≈ 0.6 for the overshooting spatial curve, ζ = 1.0 (critical) for
    // effects; stiffness scaled by speed tier (fast > default > slow).
    const SPATIAL_ZETA: f64 = 0.6;
    MotionScheme {
        fast_spatial: MotionSpring {
            damping_ratio: SPATIAL_ZETA,
            stiffness: 900.0,
        },
        fast_effects: MotionSpring {
            damping_ratio: 1.0,
            stiffness: 1400.0,
        },
        default_spatial: MotionSpring {
            damping_ratio: SPATIAL_ZETA,
            stiffness: 500.0,
        },
        default_effects: MotionSpring {
            damping_ratio: 1.0,
            stiffness: 900.0,
        },
        slow_spatial: MotionSpring {
            damping_ratio: SPATIAL_ZETA,
            stiffness: 250.0,
        },
        slow_effects: MotionSpring {
            damping_ratio: 1.0,
            stiffness: 500.0,
        },
        durations: MotionDurations {
            instant: 100.0,
            fast: 150.0,
            base: 220.0,
            slow: 340.0,
            deliberate: 600.0,
        },
        easing: EasingSet {
            spatial: Curve::Cubic(0.34, 1.35, 0.64, 1.0),
            effects: Curve::Cubic(0.16, 1.0, 0.3, 1.0),
            exit: Curve::Cubic(0.4, 0.0, 1.0, 1.0),
        },
        // `false` like every other baseline: the correct value absent an
        // OS signal. A mobile shell ORs the platform's own reduced-motion
        // preference over this authored token (see
        // `MotionScheme::reduce_motion`), so Glyph needs no per-catalog
        // reduced-motion handling of its own.
        reduce_motion: false,
        // 30Hz, carried over from a built-in baseline rather than
        // written literally: `CosmeticLoopRate` is not nameable through
        // the `frust` facade (only the `MotionScheme` field that holds
        // one is), and every built-in baseline already declares the same
        // 30Hz cap this scale wants.
        cosmetic_loop_rate: MotionScheme::neutral().cosmetic_loop_rate,
    }
}

// ---- Glass (opaque-material shaped) ------------------------------------

/// The Glyph glass scale (retrieved 2026-07-21). Glyph is **borders-first
/// and opaque** — its backdrop-blur topbar and scanline overlay are
/// web-presentation affordances, not required system features — so this
/// mirrors `GlassScale::opaque_material`'s shape
/// (`blur_radius_intent == 0`, [`GlassMaterial::is_opaque`] true, empty
/// fills → a widget paints its surface-container role). It differs in two
/// Glyph-specific ways: a **visible hairline** (`border-bright`'s `.18`
/// alpha) survives even on the opaque path, since Glyph's identity is the
/// hairline edge; and the shadows come from [`elevation`] rather
/// than `Elevation::m3`.
///
/// [`GlassMaterial`] carries a **single** [`ShadowSpec`], not a
/// per-brightness pair, so each tier picks the **dark** Glyph shadow — the
/// compromise value for Glyph's dark-first system (the lighter warm-ink
/// shadow is available via [`elevation`] for a widget that resolves
/// the elevation table directly).
pub fn glass() -> GlassScale {
    let elev = elevation();
    // Glyph's signature hairline: border-bright rgba(...,.18).
    const HAIRLINE: f32 = 0.18;
    let opaque = |shadow: ShadowSpec, hairline: f32| GlassMaterial {
        blur_radius_intent: 0.0,
        fills_light: Vec::new(),
        fills_dark: Vec::new(),
        hairline_alpha: hairline,
        shadow,
    };
    GlassScale {
        // chrome = menus/dialogs → the higher lift; bar = level 2; control
        // = level 1 (mirrors opaque_material's level 3/2/1 assignment).
        chrome: opaque(elev.level3.shadow_dark, HAIRLINE),
        bar: opaque(elev.level2.shadow_dark, HAIRLINE),
        control: opaque(elev.level1.shadow_dark, HAIRLINE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust::Color;

    fn base() -> TextStyle {
        TextStyle::new(16.0, Color::BLACK)
    }

    // ---- Type scale -----------------------------------------------------

    #[test]
    fn glyph_type_sizes_match_source_roles() {
        let s = type_scale(&base());
        assert_eq!(s.display_large.size, 32.0); // display/32
        assert_eq!(s.headline_large.size, 20.0); // heading/20
        assert_eq!(s.title_large.size, 15.0); // title/15
        assert_eq!(s.body_large.size, 13.0); // body 12.5 -> floor 13
        assert_eq!(s.body_small.size, 11.0); // caption/11
        assert_eq!(s.label_small.size, 9.5); // micro/9.5
    }

    #[test]
    fn glyph_type_weights_and_faces_split_by_role() {
        let s = type_scale(&base());
        let display = FontFamily::stack_with_generic(["Space Mono"], GenericSlot::Monospace);
        let ui = FontFamily::stack_with_generic(["IBM Plex Mono"], GenericSlot::Monospace);
        // Space Mono 700 on display/headline; IBM Plex Mono on UI slots.
        assert_eq!(s.display_large.family, display);
        assert_eq!(s.display_large.weight, FontWeight::BOLD);
        assert_eq!(s.headline_large.family, display);
        assert_eq!(s.title_large.family, ui);
        assert_eq!(s.title_large.weight, FontWeight::MEDIUM);
        assert_eq!(s.body_large.family, ui);
        assert_eq!(s.body_large.weight, FontWeight::REGULAR);
    }

    #[test]
    fn glyph_micro_carries_tracking() {
        // micro/9.5 has +0.05em tracking (0.05 * 9.5 = 0.475 logical px).
        let s = type_scale(&base());
        assert!((s.label_small.letter_spacing - 0.475).abs() < 1e-6);
    }

    #[test]
    fn glyph_emphasized_steps_weight_up_where_it_can() {
        let s = type_scale(&base());
        // Regular -> Medium (body); Medium -> Semibold (title); Bold stays
        // Bold (display: Space Mono has no heavier face).
        assert_eq!(s.body_large_emphasized.weight, FontWeight::MEDIUM);
        assert_eq!(s.title_large_emphasized.weight, FontWeight::SEMI_BOLD);
        assert_eq!(s.display_large_emphasized.weight, FontWeight::BOLD);
        // Size/line-height unchanged between base and emphasized.
        assert_eq!(s.body_large.size, s.body_large_emphasized.size);
        assert_eq!(
            s.body_large.line_height,
            s.body_large_emphasized.line_height
        );
    }

    #[test]
    fn glyph_type_preserves_base_color_but_overrides_family() {
        let b = TextStyle {
            family: FontFamily::named("Roboto"),
            ..TextStyle::new(16.0, Color::from_rgb8(1, 2, 3))
        };
        let s = type_scale(&b);
        assert_ne!(s.body_large.family, b.family); // Glyph chooses the face
        assert_eq!(s.body_large.color, b.color);
    }

    // ---- Shape scale ----------------------------------------------------

    #[test]
    fn glyph_shape_matches_documented_scale() {
        let s = shape();
        assert_eq!(s.none, 0.0);
        assert_eq!(s.extra_small, 4.0); // canonical xs (not the 3px artifact)
        assert_eq!(s.small, 6.0);
        assert_eq!(s.medium, 10.0);
        assert_eq!(s.large, 16.0);
        // --radius-xl (terminal card).
        assert_eq!(s.large_increased, 22.0);
        // Everything above `large_increased` clamps to 22 (documented mapping).
        assert_eq!(s.extra_large, 22.0);
        assert_eq!(s.extra_extra_large, 22.0);
        assert!(s.full.is_infinite());
    }

    // ---- Elevation ------------------------------------------------------

    #[test]
    fn glyph_elevation_is_per_brightness_and_source_anchored() {
        let e = elevation();
        // Toast (level 4): 0 12px 32px, dark .4 / light warm-ink .12.
        assert_eq!(e.level4.shadow_dark.y_offset, 12.0);
        assert_eq!(e.level4.shadow_dark.blur_std_dev, 32.0);
        assert_eq!(e.level4.shadow_dark.color_alpha, 0.40);
        assert_eq!(e.level4.shadow_light.color_alpha, 0.12);
        // Modal (level 5): 0 24px 60px, dark .5 / light .18.
        assert_eq!(e.level5.shadow_dark.y_offset, 24.0);
        assert_eq!(e.level5.shadow_dark.blur_std_dev, 60.0);
        assert_eq!(e.level5.shadow_dark.color_alpha, 0.50);
        assert_eq!(e.level5.shadow_light.color_alpha, 0.18);
    }

    #[test]
    fn glyph_elevation_low_levels_are_borders_first() {
        let e = elevation();
        // Level 0: no shadow at all. Level 1: near-zero.
        assert_eq!(e.level0.shadow_dark.color_alpha, 0.0);
        assert!(e.level1.shadow_dark.color_alpha < e.level4.shadow_dark.color_alpha);
        // Dark shadows are heavier than light (black vs warm-ink) at every
        // shadowed level.
        for lvl in [&e.level2, &e.level3, &e.level4, &e.level5] {
            assert!(lvl.shadow_dark.color_alpha > lvl.shadow_light.color_alpha);
        }
    }

    // ---- Motion ---------------------------------------------------------

    #[test]
    fn glyph_motion_durations_and_easings_are_exact_source_values() {
        let m = motion();
        assert_eq!(
            m.durations,
            MotionDurations {
                instant: 100.0,
                fast: 150.0,
                base: 220.0,
                slow: 340.0,
                deliberate: 600.0,
            }
        );
        assert_eq!(m.easing.spatial, Curve::Cubic(0.34, 1.35, 0.64, 1.0));
        assert_eq!(m.easing.effects, Curve::Cubic(0.16, 1.0, 0.3, 1.0));
        assert_eq!(m.easing.exit, Curve::Cubic(0.4, 0.0, 1.0, 1.0));
    }

    #[test]
    fn glyph_cosmetic_loop_rate_defaults_to_30hz() {
        // The Glyph scale keeps the same 30Hz cosmetic-loop cap every
        // built-in baseline declares.
        assert_eq!(motion().cosmetic_loop_rate.hz(), 30.0);
    }

    #[test]
    fn glyph_motion_springs_match_the_curve_overshoot_split() {
        let m = motion();
        // Spatial springs under-damped (overshoot, matching spatial bezier);
        // effects springs critically damped (never overshoot).
        for s in [&m.fast_spatial, &m.default_spatial, &m.slow_spatial] {
            assert!(s.damping_ratio < 1.0 && s.damping_ratio > 0.0);
        }
        for s in [&m.fast_effects, &m.default_effects, &m.slow_effects] {
            assert_eq!(s.damping_ratio, 1.0);
        }
    }

    // ---- Glass ----------------------------------------------------------

    #[test]
    fn glyph_glass_is_opaque_with_a_hairline_and_glyph_shadows() {
        let g = glass();
        let e = elevation();
        for tier in [&g.chrome, &g.bar, &g.control] {
            assert!(tier.is_opaque());
            assert!(tier.fills_light.is_empty());
            assert!(tier.fills_dark.is_empty());
            // Glyph keeps its hairline even on the opaque path.
            assert_eq!(tier.hairline_alpha, 0.18);
        }
        // Shadows come from `elevation()` (dark-first single value).
        assert_eq!(g.chrome.shadow, e.level3.shadow_dark);
        assert_eq!(g.bar.shadow, e.level2.shadow_dark);
        assert_eq!(g.control.shadow, e.level1.shadow_dark);
    }

    #[test]
    fn glyph_scales_are_const_where_expected() {
        const SHAPE: ShapeScale = shape();
        const ELEV: Elevation = elevation();
        const MOTION: MotionScheme = motion();
        assert_eq!(SHAPE.large, 16.0);
        assert_eq!(ELEV.level5.shadow_dark.color_alpha, 0.50);
        assert_eq!(MOTION.durations.base, 220.0);
    }
}
