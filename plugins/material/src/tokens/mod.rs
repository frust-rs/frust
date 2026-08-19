//! The Material 3 baseline design tokens this crate installs: a [`Theme`]
//! aggregate plus its constituent light/dark [`ColorScheme`](frust::ColorScheme)s
//! and [`MaterialTokens`] semantic extension, the M3 type scale/shape
//! scale/elevation table/Expressive motion scheme, and the
//! success/warning/info [`StatusPalette`] extension.
//!
//! Values sourced from Google's Material 3 design-system tokens v0.192
//! (`material-components/material-web` `tokens/versions/v0_192/
//! _md-sys-color.scss` + `_md-ref-palette.scss`), resolved 2026-07-17, seed
//! color `#6750A4`. These tables lived in `frust-theme` until the Material
//! catalog moved out of tree; nothing in the framework constructs them any
//! more, so this crate owns them outright.
//!
//! Module layout:
//!
//! - This module — `baseline()`, the public re-export surface, the bundled
//!   font family constants + `NativeTypefaces` attach, and (still here) the
//!   shape scale and `StatusPalette` bodies.
//! - [`color`] — [`color_scheme_light`]/[`color_scheme_dark`] plus the M3E
//!   semantic-role expansion ([`MaterialSemanticColors`]).
//! - [`extension`] — [`MaterialTokens`], the `ThemeExtensions` payload
//!   wrapping [`color::MaterialSemanticColors`] for both brightnesses.
//! - [`hct`] — the material-color-utilities port behind [`from_seed`]/
//!   [`theme_from_seed`].
//! - [`motion`] — the 16-duration/7-easing/11-spring M3E motion tables
//!   ([`MaterialMotion`]/[`MaterialSpring`]) `motion_scheme()` is built from.
//! - [`type_scale`] — the 30-role M3 type scale, resolved to Roboto Flex.
//! - [`metrics`] — the elevation table plus the [`MaterialSpacing`]/
//!   [`MaterialDimensions`] const catalogs.
//! - [`fonts`] — the bundled Roboto Flex/Mono font bytes ([`font_data`]).
//!
//! # M3E role partition
//!
//! The Material 3 Expressive reference (`paadevelopments/material_3_expressive`
//! v1.0.8, `lib/foundations/m3e_color_scheme.dart`, retrieved 2026-08-19)'s
//! `M3EColorScheme` carries 43 `Color` roles. 34 have a direct analogue in
//! frust-theme's baseline `ColorScheme` and flow through
//! [`color_scheme_light`]/[`color_scheme_dark`]; the remaining 9 — the
//! semantic set this `ColorScheme` has no fixed slot for — are
//! [`MaterialTokens`] fields instead:
//!
//! | M3E role (`m3e_color_scheme.dart`) | Carried by |
//! |---|---|
//! | `primary` | `ColorScheme::primary` |
//! | `onPrimary` | `ColorScheme::on_primary` |
//! | `primaryContainer` | `ColorScheme::primary_container` |
//! | `onPrimaryContainer` | `ColorScheme::on_primary_container` |
//! | `secondary` | `ColorScheme::secondary` |
//! | `onSecondary` | `ColorScheme::on_secondary` |
//! | `secondaryContainer` | `ColorScheme::secondary_container` |
//! | `onSecondaryContainer` | `ColorScheme::on_secondary_container` |
//! | `tertiary` | `ColorScheme::tertiary` |
//! | `onTertiary` | `ColorScheme::on_tertiary` |
//! | `tertiaryContainer` | `ColorScheme::tertiary_container` |
//! | `onTertiaryContainer` | `ColorScheme::on_tertiary_container` |
//! | `error` | `ColorScheme::error` |
//! | `onError` | `ColorScheme::on_error` |
//! | `errorContainer` | `ColorScheme::error_container` |
//! | `onErrorContainer` | `ColorScheme::on_error_container` |
//! | `surface` | `ColorScheme::surface` |
//! | `onSurface` | `ColorScheme::on_surface` |
//! | `onSurfaceVariant` | `ColorScheme::on_surface_variant` |
//! | `surfaceContainerLowest` | `ColorScheme::surface_container_lowest` |
//! | `surfaceContainerLow` | `ColorScheme::surface_container_low` |
//! | `surfaceContainer` | `ColorScheme::surface_container` |
//! | `surfaceContainerHigh` | `ColorScheme::surface_container_high` |
//! | `surfaceContainerHighest` | `ColorScheme::surface_container_highest` |
//! | `surfaceDim` | `ColorScheme::surface_dim` |
//! | `surfaceBright` | `ColorScheme::surface_bright` |
//! | `inverseSurface` | `ColorScheme::inverse_surface` |
//! | `onInverseSurface` | `ColorScheme::inverse_on_surface` |
//! | `inversePrimary` | `ColorScheme::inverse_primary` |
//! | `outline` | `ColorScheme::outline` |
//! | `outlineVariant` | `ColorScheme::outline_variant` |
//! | `shadow` | `ColorScheme::shadow` |
//! | `scrim` | `ColorScheme::scrim` |
//! | `surfaceTint` | `ColorScheme::surface_tint` |
//! | `emphasis` | `MaterialTokens.{light,dark}.emphasis` |
//! | `onEmphasis` | `MaterialTokens.{light,dark}.on_emphasis` |
//! | `info` | `MaterialTokens.{light,dark}.info` |
//! | `success` | `MaterialTokens.{light,dark}.success` |
//! | `warning` | `MaterialTokens.{light,dark}.warning` |
//! | `danger` | `MaterialTokens.{light,dark}.danger` |
//! | `surfaceStrong` | `MaterialTokens.{light,dark}.surface_strong` |
//! | `onSurfaceStrong` | `MaterialTokens.{light,dark}.on_surface_strong` |
//! | `outlineStrong` | `MaterialTokens.{light,dark}.outline_strong` |
//!
//! (`brightness` is the reference's 44th constructor parameter, but it's a
//! `Brightness` enum, not a `Color` — not part of the 43-role count.)

mod color;
mod extension;
mod fonts;
mod hct;
mod metrics;
mod motion;
mod type_scale;

pub use color::{MaterialSemanticColors, color_scheme_dark, color_scheme_light};
pub use extension::MaterialTokens;
pub use fonts::font_data;
pub use hct::{CorePalette, Hct, TonalPalette, from_seed, theme_from_seed};
pub use metrics::{MaterialDimensions, MaterialSpacing, elevation};
pub use motion::{MaterialMotion, MaterialSpring};
pub use type_scale::type_scale;

use frust::authoring::text::TextStyle;
use frust::{
    Brightness, CosmeticLoopRate, DesignLanguage, EasingSet, FontFace, GlassScale, MotionDurations,
    MotionScheme, MotionSpring, NativeTypefaces, ShapeScale, StatusColors, StatusPalette, Theme,
    ThemeExtensions,
};
use peniko::Color;

/// The family name Roboto Flex's own `name` table reports (`name` ID 1) — what
/// a font stack must name for the bundled face to resolve. Note it is
/// `"Roboto Flex"`, matching the variable release.
pub const ROBOTO_FLEX_FAMILY: &str = "Roboto Flex";

/// The family name Roboto Mono's own `name` table reports (`name` ID 1).
#[allow(dead_code)]
pub const ROBOTO_MONO_FAMILY: &str = "Roboto Mono";

/// The Material 3 baseline theme: baseline light/dark color schemes, the M3
/// type scale (built from `TextStyle::default()`), the M3 shape scale, the M3
/// elevation table, and the M3 Expressive motion scheme. Starts in
/// [`Brightness::Light`]. Attaches [`status_palette`] and
/// [`MaterialTokens::material`] as pre-populated extensions (see
/// `Theme::extension`), so `extension::<StatusPalette>()` and
/// `extension::<MaterialTokens>()` are always `Some` on this baseline.
pub fn baseline() -> Theme {
    let mut extensions = ThemeExtensions::new();
    extensions.insert(status_palette());
    extensions.insert(MaterialTokens::material());
    extensions.insert(native_typefaces());
    Theme {
        light: color_scheme_light(),
        dark: color_scheme_dark(),
        type_scale: type_scale(&TextStyle::default()),
        shape: shape_scale(),
        elevation: elevation(),
        motion: motion_scheme(),
        glass: GlassScale::opaque_material(),
        brightness: Brightness::Light,
        design_language: DesignLanguage::Material3,
        extensions,
    }
}

/// The Material 3 baseline [`StatusPalette`] default: success/warning/info
/// color roles Material 3's baseline 46-role `ColorScheme` has no fixed slot
/// for.
///
/// **Community-approximate**: Material 3 has no single official token source
/// for a `success`/`warning`/`info` role table (Google's own Material Theme
/// Builder covers the gap via a "custom color" extension, not a fixed role
/// table) — the values below apply the same tone-relationship
/// [`color_scheme_light`]/[`color_scheme_dark`]'s `error` roles use (light:
/// base/on/container/on-container ≈ tone 40/100/90/10; dark: ≈ tone
/// 80/20/30/90) to green (success), amber (warning), and blue (info) seed
/// hues, chosen for conventional semantic association and AA contrast
/// against `surface`/`on_surface`.
pub fn status_palette() -> StatusPalette {
    StatusPalette {
        light: StatusColors {
            // Green seed, ~tone 40/100/90/10 (mirrors `error`'s light
            // tone relationship: base/on/container/on-container).
            success: Color::from_rgb8(0x2E, 0x7D, 0x32),
            on_success: Color::from_rgb8(0xFF, 0xFF, 0xFF),
            success_container: Color::from_rgb8(0xC8, 0xE6, 0xC9),
            on_success_container: Color::from_rgb8(0x1B, 0x5E, 0x20),

            // Amber seed, tuned dark enough for AA-on-white at the base
            // tone (a literal amber-400 like `#FFC107` fails AA on white).
            warning: Color::from_rgb8(0x8A, 0x53, 0x00),
            on_warning: Color::from_rgb8(0xFF, 0xFF, 0xFF),
            warning_container: Color::from_rgb8(0xFF, 0xDD, 0xB0),
            on_warning_container: Color::from_rgb8(0x2B, 0x17, 0x00),

            // Blue seed, echoing M3's own `primary`-adjacent "info" blue
            // used elsewhere in Google's Material guidance.
            info: Color::from_rgb8(0x00, 0x61, 0xA4),
            on_info: Color::from_rgb8(0xFF, 0xFF, 0xFF),
            info_container: Color::from_rgb8(0xD1, 0xE4, 0xFF),
            on_info_container: Color::from_rgb8(0x00, 0x1D, 0x36),
        },
        dark: StatusColors {
            // Dark tone relationship (~80/20/30/90), mirroring `error`'s
            // dark tones (`F2B8B5`/`601410`/`8C1D18`/`F9DEDC`).
            success: Color::from_rgb8(0xA6, 0xF1, 0xA1),
            on_success: Color::from_rgb8(0x00, 0x39, 0x0F),
            success_container: Color::from_rgb8(0x20, 0x57, 0x23),
            on_success_container: Color::from_rgb8(0xC8, 0xE6, 0xC9),

            warning: Color::from_rgb8(0xFF, 0xC4, 0x6B),
            on_warning: Color::from_rgb8(0x45, 0x2B, 0x00),
            warning_container: Color::from_rgb8(0x6F, 0x49, 0x00),
            on_warning_container: Color::from_rgb8(0xFF, 0xDD, 0xB0),

            info: Color::from_rgb8(0x9F, 0xCA, 0xFF),
            on_info: Color::from_rgb8(0x00, 0x32, 0x50),
            info_container: Color::from_rgb8(0x00, 0x4A, 0x76),
            on_info_container: Color::from_rgb8(0xD1, 0xE4, 0xFF),
        },
    }
}

// ---- Shape scale --------------------------------------------------------

/// The Material 3 shape scale: 10 corner-radius tokens (post-Expressive
/// scale). Source: <https://m3.material.io/styles/shape/corner-radius-scale>
/// (verified 2026-07-17; the pre-Expressive scale had 7 tokens, not 10).
/// Radii are dp, treated 1:1 as logical px.
pub const fn shape_scale() -> ShapeScale {
    ShapeScale {
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

// ---- Motion ---------------------------------------------------------------

/// The Material 3 Expressive baseline motion scheme: six spring presets
/// (of [`MaterialMotion`]/[`MaterialSpring`]'s full 34-token M3E table — see
/// their own module doc for the source citation and the Dart → Rust naming
/// map).
///
/// Spring slots take their values from six of [`MaterialSpring`]'s 11
/// presets: `fast_spatial`/`default_spatial`/`slow_spatial` from
/// [`MaterialSpring::SPATIAL_FAST`]/[`SPATIAL_DEFAULT`](MaterialSpring::SPATIAL_DEFAULT)/[`SPATIAL_SLOW`](MaterialSpring::SPATIAL_SLOW),
/// `fast_effects`/`default_effects`/`slow_effects` from
/// [`MaterialSpring::EFFECTS_FAST`]/[`EFFECTS_DEFAULT`](MaterialSpring::EFFECTS_DEFAULT)/[`EFFECTS_SLOW`](MaterialSpring::EFFECTS_SLOW).
/// "Effects" springs (opacity/color) are critically damped
/// (`damping_ratio: 1.0`) by design — no bounce; "spatial" springs
/// (position/size) are `0.9`, allowing a small overshoot. The remaining five
/// [`MaterialSpring`] presets have no slot here and stay reachable directly.
///
/// Glyph's five-slot `MotionDurations` vocabulary maps onto
/// [`MaterialMotion`]'s 16-value duration scale as: `instant` →
/// [`MaterialMotion::SHORT_1`] (50ms), `fast` → [`MaterialMotion::SHORT_3`]
/// (150ms), `base` → [`MaterialMotion::MEDIUM_2`] (300ms, the most
/// commonly-cited M3 "default" transition duration), `slow` →
/// [`MaterialMotion::LONG_2`] (500ms), `deliberate` →
/// [`MaterialMotion::EXTRA_LONG_1`] (700ms, the Extra Long tier's floor).
///
/// `EasingSet`'s three slots map onto three of [`MaterialMotion`]'s 7
/// easings: `spatial` → [`MaterialMotion::EMPHASIZED_DECELERATE`] (M3's
/// curve for entering/spatial transitions — its steep initial deceleration
/// reads as a slight overshoot-adjacent settle), `effects` →
/// [`MaterialMotion::STANDARD`] (M3's curve for opacity/color fades — never
/// overshoots), `exit` → [`MaterialMotion::STANDARD_ACCELERATE`] (M3's curve
/// for elements leaving the screen).
///
/// `cosmetic_loop_rate` is this design system's own authored value — 30Hz,
/// the same cap every built-in baseline declares.
pub const fn motion_scheme() -> MotionScheme {
    MotionScheme {
        fast_spatial: MotionSpring {
            damping_ratio: MaterialSpring::SPATIAL_FAST.damping_ratio,
            stiffness: MaterialSpring::SPATIAL_FAST.stiffness,
        },
        fast_effects: MotionSpring {
            damping_ratio: MaterialSpring::EFFECTS_FAST.damping_ratio,
            stiffness: MaterialSpring::EFFECTS_FAST.stiffness,
        },
        default_spatial: MotionSpring {
            damping_ratio: MaterialSpring::SPATIAL_DEFAULT.damping_ratio,
            stiffness: MaterialSpring::SPATIAL_DEFAULT.stiffness,
        },
        default_effects: MotionSpring {
            damping_ratio: MaterialSpring::EFFECTS_DEFAULT.damping_ratio,
            stiffness: MaterialSpring::EFFECTS_DEFAULT.stiffness,
        },
        slow_spatial: MotionSpring {
            damping_ratio: MaterialSpring::SPATIAL_SLOW.damping_ratio,
            stiffness: MaterialSpring::SPATIAL_SLOW.stiffness,
        },
        slow_effects: MotionSpring {
            damping_ratio: MaterialSpring::EFFECTS_SLOW.damping_ratio,
            stiffness: MaterialSpring::EFFECTS_SLOW.stiffness,
        },
        durations: MotionDurations {
            instant: MaterialMotion::SHORT_1.as_millis() as f64,
            fast: MaterialMotion::SHORT_3.as_millis() as f64,
            base: MaterialMotion::MEDIUM_2.as_millis() as f64,
            slow: MaterialMotion::LONG_2.as_millis() as f64,
            deliberate: MaterialMotion::EXTRA_LONG_1.as_millis() as f64,
        },
        easing: EasingSet {
            spatial: MaterialMotion::EMPHASIZED_DECELERATE,
            effects: MaterialMotion::STANDARD,
            exit: MaterialMotion::STANDARD_ACCELERATE,
        },
        reduce_motion: false,
        cosmetic_loop_rate: CosmeticLoopRate::new(30.0),
    }
}

/// The native-control typeface binding [`baseline`] attaches: the bundled
/// Roboto Flex face in both button and body slots.
///
/// Material 3 names one sans family for all text (display/heading/body alike),
/// so both native control slots carry the same face. Both are taken
/// **out of [`fonts::font_data`]'s own array** rather than re-referenced from
/// the underlying constants: a native host de-duplicates published payloads by
/// byte identity (address + length), so a slot's face and the bytes a shell
/// registers through [`frust::register_app_fonts`] must be the *same*
/// `&'static [u8]`, not merely equal ones.
pub fn native_typefaces() -> NativeTypefaces {
    match fonts::font_data()
        .get(fonts::ROBOTO_FLEX_VARIABLE_INDEX)
        .copied()
    {
        Some(bytes) => NativeTypefaces::uniform(FontFace::new(ROBOTO_FLEX_FAMILY, bytes)),
        None => NativeTypefaces::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_scale_matches_table() {
        let s = shape_scale();
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
    fn elevation_dp_matches_table() {
        let e = elevation();
        assert_eq!(e.level0.dp, 0.0);
        assert_eq!(e.level1.dp, 1.0);
        assert_eq!(e.level2.dp, 3.0);
        assert_eq!(e.level3.dp, 6.0);
        assert_eq!(e.level4.dp, 8.0);
        assert_eq!(e.level5.dp, 12.0);
    }

    #[test]
    fn elevation_shadow_is_identical_on_both_brightnesses() {
        let e = elevation();
        assert_eq!(e.level3.shadow_light, e.level3.shadow_dark);
    }

    #[test]
    fn motion_scheme_spring_presets_are_exact() {
        let m = motion_scheme();
        assert_eq!(
            m.fast_spatial,
            MotionSpring {
                damping_ratio: 0.9,
                stiffness: 1400.0
            }
        );
        assert_eq!(
            m.fast_effects,
            MotionSpring {
                damping_ratio: 1.0,
                stiffness: 3800.0
            }
        );
    }

    #[test]
    fn motion_scheme_effects_springs_are_critically_damped() {
        let m = motion_scheme();
        assert_eq!(m.fast_effects.damping_ratio, 1.0);
        assert_eq!(m.default_effects.damping_ratio, 1.0);
        assert_eq!(m.slow_effects.damping_ratio, 1.0);
    }

    #[test]
    fn motion_scheme_cosmetic_loop_rate_is_30hz() {
        assert_eq!(motion_scheme().cosmetic_loop_rate.hz(), 30.0);
    }

    #[test]
    fn baseline_composes_the_m3_scales() {
        let theme = baseline();
        assert_eq!(theme.shape, shape_scale());
        assert_eq!(theme.elevation, elevation());
        assert_eq!(theme.motion, motion_scheme());
    }

    #[test]
    fn baseline_attaches_both_extensions() {
        let theme = baseline();
        assert_eq!(theme.extension::<StatusPalette>(), Some(&status_palette()));
        assert_eq!(
            theme.extension::<MaterialTokens>(),
            Some(&MaterialTokens::material())
        );
    }
}
