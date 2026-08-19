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
//! Split across three files:
//!
//! - This module — `baseline()`, the module's public re-export surface, and
//!   (for now) the type scale/shape scale/elevation table/motion
//!   scheme/`StatusPalette` bodies. A later task moves each of those into
//!   its own file; this one only splits out color.
//! - [`color`] — [`color_scheme_light`]/[`color_scheme_dark`] plus the M3E
//!   semantic-role expansion ([`MaterialSemanticColors`]).
//! - [`extension`] — [`MaterialTokens`], the `ThemeExtensions` payload
//!   wrapping [`color::MaterialSemanticColors`] for both brightnesses.
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
mod hct;

pub use color::{MaterialSemanticColors, color_scheme_dark, color_scheme_light};
pub use extension::MaterialTokens;
pub use hct::{CorePalette, Hct, TonalPalette, from_seed, theme_from_seed};

use frust::authoring::text::{FontWeight, LineHeight, TextStyle};
use frust::{
    Brightness, CosmeticLoopRate, Curve, DesignLanguage, EasingSet, Elevation, ElevationLevel,
    GlassScale, MotionDurations, MotionScheme, MotionSpring, ShadowSpec, ShapeScale, StatusColors,
    StatusPalette, SurfaceRole, Theme, ThemeExtensions, TypeScale,
};
use peniko::Color;

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

// ---- Type scale -------------------------------------------------------

/// Source: <https://m3.material.io/styles/typography/type-scale-tokens>
/// (verified 2026-07-17). Sizes are specified in sp; Frust treats sp and
/// logical px 1:1 (see `frust-text`'s scale). Line heights are absolute
/// logical pixels (`LineHeight::Absolute`, not a font-size-relative ratio) —
/// M3 publishes them as fixed px values per token, not a ratio. Letter
/// spacing is in logical pixels; `displayLarge`'s spacing is negative
/// (tighter tracking at very large sizes).
///
/// `titleLarge` is weight 400 (Regular) per m3.material.io; a secondary
/// source claims 500 — this module follows m3.material.io as the primary,
/// more authoritative source.
///
/// [`TypeScale`] additionally carries 15 `_emphasized` variants (one per
/// baseline role, 30 slots total) — the M3 Expressive emphasized scale.
/// **Role-count resolution:** an earlier belief that emphasized variants were
/// "15 baseline + 15 emphasized (30 total), applied to Display/Headline/Title
/// roles" was contested — the "30 total" count was right but the
/// "Display/Headline/Title only" scope was wrong. Verified directly against
/// the primary source: Jetpack Compose Material3's generated token file
/// (`androidx.compose.material3.tokens.TypographyTokens`/`TypeScaleTokens`,
/// `VERSION: v0_103`,
/// <https://github.com/androidx/androidx/blob/androidx-main/compose/material3/material3/src/commonMain/kotlin/androidx/compose/material3/tokens/TypeScaleTokens.kt>
/// — Compose's `Typography` class doc comments enumerate an
/// `*Emphasized` property for *all 15* baseline roles: `displayLarge`
/// through `labelSmall`, not a Display/Headline/Title-only subset;
/// retrieved/verified 2026-07-18). This module follows that: every one of
/// the 15 baseline roles gets an emphasized sibling.
///
/// **M3 deltas** (from the same source): size and line height are unchanged
/// between a role's baseline and emphasized token — only weight (and, for a
/// handful of roles, letter spacing) shift. Weight always steps up one rung
/// from the baseline token's own weight: Regular → Medium for every
/// Regular-weight baseline role (`display_*`, `headline_*`, `title_large`,
/// `body_*`), and Medium → Bold for every Medium-weight baseline role
/// (`title_medium`, `title_small`, `label_*`) — so every emphasized style is
/// guaranteed to differ from its base in weight. Letter spacing mostly
/// matches the baseline value already in this module's tables; `body_large`
/// is the one role whose emphasized tracking differs from its own baseline
/// (0.5px baseline → 0.15px emphasized, matching the source's
/// `BodyLargeEmphasizedTracking`).
///
/// One type-scale token's numeric shape: `(size_px, line_height_px,
/// letter_spacing_px, weight)`.
type TypeToken = (f32, f32, f32, FontWeight);

const DISPLAY_LARGE: TypeToken = (57.0, 64.0, -0.25, FontWeight::REGULAR);
const DISPLAY_MEDIUM: TypeToken = (45.0, 52.0, 0.0, FontWeight::REGULAR);
const DISPLAY_SMALL: TypeToken = (36.0, 44.0, 0.0, FontWeight::REGULAR);
const HEADLINE_LARGE: TypeToken = (32.0, 40.0, 0.0, FontWeight::REGULAR);
const HEADLINE_MEDIUM: TypeToken = (28.0, 36.0, 0.0, FontWeight::REGULAR);
const HEADLINE_SMALL: TypeToken = (24.0, 32.0, 0.0, FontWeight::REGULAR);
const TITLE_LARGE: TypeToken = (22.0, 28.0, 0.0, FontWeight::REGULAR);
const TITLE_MEDIUM: TypeToken = (16.0, 24.0, 0.15, FontWeight::MEDIUM);
const TITLE_SMALL: TypeToken = (14.0, 20.0, 0.1, FontWeight::MEDIUM);
const BODY_LARGE: TypeToken = (16.0, 24.0, 0.5, FontWeight::REGULAR);
const BODY_MEDIUM: TypeToken = (14.0, 20.0, 0.25, FontWeight::REGULAR);
const BODY_SMALL: TypeToken = (12.0, 16.0, 0.4, FontWeight::REGULAR);
const LABEL_LARGE: TypeToken = (14.0, 20.0, 0.1, FontWeight::MEDIUM);
const LABEL_MEDIUM: TypeToken = (12.0, 16.0, 0.5, FontWeight::MEDIUM);
const LABEL_SMALL: TypeToken = (11.0, 16.0, 0.5, FontWeight::MEDIUM);

// M3-Expressive emphasized tokens: same size/line-height as the matching
// baseline `TypeToken` above in every case; weight steps up one rung from
// the baseline role's own weight (Regular -> Medium, Medium -> Bold) and
// letter spacing is the source's `*Emphasized*Tracking` value (see this
// section's doc comment for the primary-source citation and resolution of
// the contested Display/Headline/Title-only scope claim).
const DISPLAY_LARGE_EMPHASIZED: TypeToken = (57.0, 64.0, 0.0, FontWeight::MEDIUM);
const DISPLAY_MEDIUM_EMPHASIZED: TypeToken = (45.0, 52.0, 0.0, FontWeight::MEDIUM);
const DISPLAY_SMALL_EMPHASIZED: TypeToken = (36.0, 44.0, 0.0, FontWeight::MEDIUM);
const HEADLINE_LARGE_EMPHASIZED: TypeToken = (32.0, 40.0, 0.0, FontWeight::MEDIUM);
const HEADLINE_MEDIUM_EMPHASIZED: TypeToken = (28.0, 36.0, 0.0, FontWeight::MEDIUM);
const HEADLINE_SMALL_EMPHASIZED: TypeToken = (24.0, 32.0, 0.0, FontWeight::MEDIUM);
const TITLE_LARGE_EMPHASIZED: TypeToken = (22.0, 28.0, 0.0, FontWeight::MEDIUM);
const TITLE_MEDIUM_EMPHASIZED: TypeToken = (16.0, 24.0, 0.15, FontWeight::BOLD);
const TITLE_SMALL_EMPHASIZED: TypeToken = (14.0, 20.0, 0.1, FontWeight::BOLD);
const BODY_LARGE_EMPHASIZED: TypeToken = (16.0, 24.0, 0.15, FontWeight::MEDIUM);
const BODY_MEDIUM_EMPHASIZED: TypeToken = (14.0, 20.0, 0.25, FontWeight::MEDIUM);
const BODY_SMALL_EMPHASIZED: TypeToken = (12.0, 16.0, 0.4, FontWeight::MEDIUM);
const LABEL_LARGE_EMPHASIZED: TypeToken = (14.0, 20.0, 0.1, FontWeight::BOLD);
const LABEL_MEDIUM_EMPHASIZED: TypeToken = (12.0, 16.0, 0.5, FontWeight::BOLD);
const LABEL_SMALL_EMPHASIZED: TypeToken = (11.0, 16.0, 0.5, FontWeight::BOLD);

fn apply_type(base: &TextStyle, token: TypeToken) -> TextStyle {
    let (size, line_height_px, letter_spacing, weight) = token;
    TextStyle {
        size,
        weight,
        letter_spacing,
        line_height: LineHeight::Absolute(line_height_px),
        ..base.clone()
    }
}

/// Builds the Material 3 baseline type scale from `base` (its
/// `family`/`style`/`color` are preserved on every token; only
/// `size`/`weight`/`letter_spacing`/`line_height` are M3-specified).
pub fn type_scale(base: &TextStyle) -> TypeScale {
    TypeScale {
        display_large: apply_type(base, DISPLAY_LARGE),
        display_medium: apply_type(base, DISPLAY_MEDIUM),
        display_small: apply_type(base, DISPLAY_SMALL),
        headline_large: apply_type(base, HEADLINE_LARGE),
        headline_medium: apply_type(base, HEADLINE_MEDIUM),
        headline_small: apply_type(base, HEADLINE_SMALL),
        title_large: apply_type(base, TITLE_LARGE),
        title_medium: apply_type(base, TITLE_MEDIUM),
        title_small: apply_type(base, TITLE_SMALL),
        body_large: apply_type(base, BODY_LARGE),
        body_medium: apply_type(base, BODY_MEDIUM),
        body_small: apply_type(base, BODY_SMALL),
        label_large: apply_type(base, LABEL_LARGE),
        label_medium: apply_type(base, LABEL_MEDIUM),
        label_small: apply_type(base, LABEL_SMALL),
        display_large_emphasized: apply_type(base, DISPLAY_LARGE_EMPHASIZED),
        display_medium_emphasized: apply_type(base, DISPLAY_MEDIUM_EMPHASIZED),
        display_small_emphasized: apply_type(base, DISPLAY_SMALL_EMPHASIZED),
        headline_large_emphasized: apply_type(base, HEADLINE_LARGE_EMPHASIZED),
        headline_medium_emphasized: apply_type(base, HEADLINE_MEDIUM_EMPHASIZED),
        headline_small_emphasized: apply_type(base, HEADLINE_SMALL_EMPHASIZED),
        title_large_emphasized: apply_type(base, TITLE_LARGE_EMPHASIZED),
        title_medium_emphasized: apply_type(base, TITLE_MEDIUM_EMPHASIZED),
        title_small_emphasized: apply_type(base, TITLE_SMALL_EMPHASIZED),
        body_large_emphasized: apply_type(base, BODY_LARGE_EMPHASIZED),
        body_medium_emphasized: apply_type(base, BODY_MEDIUM_EMPHASIZED),
        body_small_emphasized: apply_type(base, BODY_SMALL_EMPHASIZED),
        label_large_emphasized: apply_type(base, LABEL_LARGE_EMPHASIZED),
        label_medium_emphasized: apply_type(base, LABEL_MEDIUM_EMPHASIZED),
        label_small_emphasized: apply_type(base, LABEL_SMALL_EMPHASIZED),
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

// ---- Elevation ----------------------------------------------------------

/// dp source: <https://m3.material.io/styles/elevation> (verified
/// 2026-07-17): L0 0dp, L1 1dp, L2 3dp, L3 6dp, L4 8dp, L5 12dp. Component
/// mapping per the same source: L1 = elevated cards/bottom sheets, L2 = nav
/// bar/menus, L3 = FAB/dialogs.
///
/// **Shadow math is TUNABLE, not an M3-published spec.** Material 3's 2023
/// direction replaced tonal-overlay tinting with *static surface-container
/// roles* for most elevated surfaces — the opposite of "primarily tonal
/// overlay post-2023" — but shadows still exist alongside; M3 does not
/// publish exact shadow blur/offset math, so this module defines a
/// documented v1 mapping: `y_offset = dp / 2.0 + 1.0`, `blur_std_dev = dp`,
/// shadow color = `ColorScheme::shadow` at `color_alpha` ~0.3. Treat it as
/// adjustable, not load-bearing, Frust-specific policy.
///
/// [`ElevationLevel`] carries **separate** light/dark shadow specs; this
/// mapping doesn't branch by brightness, so both slots hold the same value —
/// behavior-preserving, byte-identical rendered output on either brightness.
const fn elevation_level(dp: f64, surface_role: SurfaceRole) -> ElevationLevel {
    let shadow = ShadowSpec {
        y_offset: dp / 2.0 + 1.0,
        blur_std_dev: dp,
        color_alpha: 0.3,
    };
    ElevationLevel {
        dp,
        shadow_light: shadow,
        shadow_dark: shadow,
        surface_role,
    }
}

/// The Material 3 baseline elevation table (dp values verified; shadow math
/// and surface-role assignment are this crate's documented v1 mapping — see
/// [`elevation_level`]'s doc comment).
pub const fn elevation() -> Elevation {
    Elevation {
        level0: elevation_level(0.0, SurfaceRole::Surface),
        level1: elevation_level(1.0, SurfaceRole::SurfaceContainerLow),
        level2: elevation_level(3.0, SurfaceRole::SurfaceContainer),
        level3: elevation_level(6.0, SurfaceRole::SurfaceContainerHigh),
        level4: elevation_level(8.0, SurfaceRole::SurfaceContainerHigh),
        level5: elevation_level(12.0, SurfaceRole::SurfaceContainerHighest),
    }
}

// ---- Motion ---------------------------------------------------------------

/// The Material 3 Expressive baseline motion scheme: six spring presets.
///
/// Source: material-components-android `docs/theming/Motion.md` (M3
/// Expressive, verified 2026-07-17): fastSpatial (0.9, 1400), fastEffects
/// (1.0, 3800), defaultSpatial (0.9, 700), defaultEffects (1.0, 1600),
/// slowSpatial (0.9, 300), slowEffects (1.0, 800) — `(damping_ratio,
/// stiffness)`, mass 1 for every preset. "Effects" springs (opacity/color)
/// are critically damped (`damping_ratio: 1.0`) by design — no bounce;
/// "spatial" springs (position/size) are `0.9`, allowing a small overshoot.
///
/// Duration/easing tokens are sourced from the same
/// `material-components-android` `docs/theming/Motion.md`, which publishes a
/// 16-value duration scale in four tiers — Short (50, 100, 150, 200ms),
/// Medium (250, 300, 350, 400ms), Long (450, 500, 550, 600ms), Extra Long
/// (700–1000ms) — plus the easing curves below. Glyph's five-slot
/// `MotionDurations` vocabulary maps onto that scale as: `instant` → Short1
/// (50ms), `fast` → Short3 (150ms), `base` → Medium2 (300ms, the most
/// commonly-cited M3 "default" transition duration), `slow` → Long2 (500ms),
/// `deliberate` → the Extra Long tier's floor (700ms).
///
/// `EasingSet`'s three slots map onto M3's own easing-curve tokens (Jetpack
/// Compose's `androidx.compose.material3.tokens.MotionTokens` control
/// points, same source): `spatial` → Emphasized Decelerate
/// (`cubic-bezier(0.05, 0.7, 0.1, 1.0)`, M3's curve for entering/spatial
/// transitions — its steep initial deceleration reads as a slight
/// overshoot-adjacent settle), `effects` → Standard
/// (`cubic-bezier(0.2, 0.0, 0.0, 1.0)`, M3's curve for opacity/color fades —
/// never overshoots), `exit` → Standard Accelerate
/// (`cubic-bezier(0.3, 0.0, 1.0, 1.0)`, M3's curve for elements leaving the
/// screen).
///
/// `cosmetic_loop_rate` is this design system's own authored value — 30Hz,
/// the same cap every built-in baseline declares.
pub const fn motion_scheme() -> MotionScheme {
    MotionScheme {
        fast_spatial: MotionSpring {
            damping_ratio: 0.9,
            stiffness: 1400.0,
        },
        fast_effects: MotionSpring {
            damping_ratio: 1.0,
            stiffness: 3800.0,
        },
        default_spatial: MotionSpring {
            damping_ratio: 0.9,
            stiffness: 700.0,
        },
        default_effects: MotionSpring {
            damping_ratio: 1.0,
            stiffness: 1600.0,
        },
        slow_spatial: MotionSpring {
            damping_ratio: 0.9,
            stiffness: 300.0,
        },
        slow_effects: MotionSpring {
            damping_ratio: 1.0,
            stiffness: 800.0,
        },
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
        cosmetic_loop_rate: CosmeticLoopRate::new(30.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_scale_display_large_matches_table() {
        let scale = type_scale(&TextStyle::new(16.0, Color::BLACK));
        assert_eq!(scale.display_large.size, 57.0);
        assert_eq!(scale.display_large.line_height, LineHeight::Absolute(64.0));
        assert_eq!(scale.display_large.letter_spacing, -0.25);
        assert_eq!(scale.display_large.weight, FontWeight::REGULAR);
    }

    #[test]
    fn type_scale_preserves_base_family_and_color() {
        let base = TextStyle {
            family: frust::authoring::text::FontFamily::named("Roboto"),
            ..TextStyle::new(16.0, Color::from_rgb8(1, 2, 3))
        };
        let scale = type_scale(&base);
        assert_eq!(scale.body_large.family, base.family);
        assert_eq!(scale.body_large.color, base.color);
    }

    #[test]
    fn type_scale_every_emphasized_role_differs_in_weight_from_its_base() {
        let scale = type_scale(&TextStyle::new(16.0, Color::BLACK));
        let pairs: [(&TextStyle, &TextStyle); 15] = [
            (&scale.display_large, &scale.display_large_emphasized),
            (&scale.display_medium, &scale.display_medium_emphasized),
            (&scale.display_small, &scale.display_small_emphasized),
            (&scale.headline_large, &scale.headline_large_emphasized),
            (&scale.headline_medium, &scale.headline_medium_emphasized),
            (&scale.headline_small, &scale.headline_small_emphasized),
            (&scale.title_large, &scale.title_large_emphasized),
            (&scale.title_medium, &scale.title_medium_emphasized),
            (&scale.title_small, &scale.title_small_emphasized),
            (&scale.body_large, &scale.body_large_emphasized),
            (&scale.body_medium, &scale.body_medium_emphasized),
            (&scale.body_small, &scale.body_small_emphasized),
            (&scale.label_large, &scale.label_large_emphasized),
            (&scale.label_medium, &scale.label_medium_emphasized),
            (&scale.label_small, &scale.label_small_emphasized),
        ];
        for (b, emphasized) in pairs {
            assert_ne!(
                b.weight, emphasized.weight,
                "expected emphasized weight to differ from base weight"
            );
        }
    }

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
