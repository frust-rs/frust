//! The Material 3 baseline design tokens this crate installs: a [`Theme`]
//! aggregate plus its constituent light/dark [`ColorScheme`]s, the M3 type
//! scale/shape scale/elevation table/Expressive motion scheme, and the
//! success/warning/info [`StatusPalette`] extension.
//!
//! Values sourced from Google's Material 3 design-system tokens v0.192
//! (`material-components/material-web` `tokens/versions/v0_192/
//! _md-sys-color.scss` + `_md-ref-palette.scss`), resolved 2026-07-17, seed
//! color `#6750A4`. These tables lived in `frust-theme` until the Material
//! catalog moved out of tree; nothing in the framework constructs them any
//! more, so this crate owns them outright.

use frust::authoring::text::{FontWeight, LineHeight, TextStyle};
use frust::{
    Brightness, ColorScheme, CosmeticLoopRate, Curve, DesignLanguage, EasingSet, Elevation,
    ElevationLevel, GlassScale, MotionDurations, MotionScheme, MotionSpring, ShadowSpec,
    ShapeScale, StatusColors, StatusPalette, SurfaceRole, Theme, ThemeExtensions, TypeScale,
};
use peniko::Color;

/// The Material 3 baseline theme: baseline light/dark color schemes, the M3
/// type scale (built from `TextStyle::default()`), the M3 shape scale, the M3
/// elevation table, and the M3 Expressive motion scheme. Starts in
/// [`Brightness::Light`]. Attaches [`status_palette`] as a pre-populated
/// extension (see `Theme::extension`) so `extension::<StatusPalette>()` is
/// always `Some` on this baseline.
pub fn baseline() -> Theme {
    let mut extensions = ThemeExtensions::new();
    extensions.insert(status_palette());
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

/// The Material 3 baseline light [`ColorScheme`] (seed `#6750A4`).
///
/// Source: material-components/material-web tokens v0.192
/// `_md-sys-color.scss` (light), resolved 2026-07-17.
pub fn color_scheme_light() -> ColorScheme {
    ColorScheme {
        primary: Color::from_rgb8(0x67, 0x50, 0xA4),
        on_primary: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        primary_container: Color::from_rgb8(0xEA, 0xDD, 0xFF),
        on_primary_container: Color::from_rgb8(0x21, 0x00, 0x5D),
        primary_fixed: Color::from_rgb8(0xEA, 0xDD, 0xFF),
        primary_fixed_dim: Color::from_rgb8(0xD0, 0xBC, 0xFF),
        on_primary_fixed: Color::from_rgb8(0x21, 0x00, 0x5D),
        on_primary_fixed_variant: Color::from_rgb8(0x4F, 0x37, 0x8B),

        secondary: Color::from_rgb8(0x62, 0x5B, 0x71),
        on_secondary: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        secondary_container: Color::from_rgb8(0xE8, 0xDE, 0xF8),
        on_secondary_container: Color::from_rgb8(0x1D, 0x19, 0x2B),
        secondary_fixed: Color::from_rgb8(0xE8, 0xDE, 0xF8),
        secondary_fixed_dim: Color::from_rgb8(0xCC, 0xC2, 0xDC),
        on_secondary_fixed: Color::from_rgb8(0x1D, 0x19, 0x2B),
        on_secondary_fixed_variant: Color::from_rgb8(0x4A, 0x44, 0x58),

        tertiary: Color::from_rgb8(0x7D, 0x52, 0x60),
        on_tertiary: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        tertiary_container: Color::from_rgb8(0xFF, 0xD8, 0xE4),
        on_tertiary_container: Color::from_rgb8(0x31, 0x11, 0x1D),
        tertiary_fixed: Color::from_rgb8(0xFF, 0xD8, 0xE4),
        tertiary_fixed_dim: Color::from_rgb8(0xEF, 0xB8, 0xC8),
        on_tertiary_fixed: Color::from_rgb8(0x31, 0x11, 0x1D),
        on_tertiary_fixed_variant: Color::from_rgb8(0x63, 0x3B, 0x48),

        error: Color::from_rgb8(0xB3, 0x26, 0x1E),
        on_error: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        error_container: Color::from_rgb8(0xF9, 0xDE, 0xDC),
        on_error_container: Color::from_rgb8(0x41, 0x0E, 0x0B),

        surface: Color::from_rgb8(0xFE, 0xF7, 0xFF),
        on_surface: Color::from_rgb8(0x1D, 0x1B, 0x20),
        on_surface_variant: Color::from_rgb8(0x49, 0x45, 0x4F),
        surface_dim: Color::from_rgb8(0xDE, 0xD8, 0xE1),
        surface_bright: Color::from_rgb8(0xFE, 0xF7, 0xFF),
        surface_container_lowest: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        surface_container_low: Color::from_rgb8(0xF7, 0xF2, 0xFA),
        surface_container: Color::from_rgb8(0xF3, 0xED, 0xF7),
        surface_container_high: Color::from_rgb8(0xEC, 0xE6, 0xF0),
        surface_container_highest: Color::from_rgb8(0xE6, 0xE0, 0xE9),

        outline: Color::from_rgb8(0x79, 0x74, 0x7E),
        outline_variant: Color::from_rgb8(0xCA, 0xC4, 0xD0),
        shadow: Color::from_rgb8(0x00, 0x00, 0x00),
        scrim: Color::from_rgb8(0x00, 0x00, 0x00),
        inverse_surface: Color::from_rgb8(0x32, 0x2F, 0x35),
        inverse_on_surface: Color::from_rgb8(0xF5, 0xEF, 0xF7),
        inverse_primary: Color::from_rgb8(0xD0, 0xBC, 0xFF),
        surface_tint: Color::from_rgb8(0x67, 0x50, 0xA4),
    }
}

/// The Material 3 baseline dark [`ColorScheme`] (seed `#6750A4`).
///
/// Source: material-components/material-web tokens v0.192
/// `_md-sys-color.scss` (dark), resolved 2026-07-17.
pub fn color_scheme_dark() -> ColorScheme {
    ColorScheme {
        primary: Color::from_rgb8(0xD0, 0xBC, 0xFF),
        on_primary: Color::from_rgb8(0x38, 0x1E, 0x72),
        primary_container: Color::from_rgb8(0x4F, 0x37, 0x8B),
        on_primary_container: Color::from_rgb8(0xEA, 0xDD, 0xFF),
        primary_fixed: Color::from_rgb8(0xEA, 0xDD, 0xFF),
        primary_fixed_dim: Color::from_rgb8(0xD0, 0xBC, 0xFF),
        on_primary_fixed: Color::from_rgb8(0x21, 0x00, 0x5D),
        on_primary_fixed_variant: Color::from_rgb8(0x4F, 0x37, 0x8B),

        secondary: Color::from_rgb8(0xCC, 0xC2, 0xDC),
        on_secondary: Color::from_rgb8(0x33, 0x2D, 0x41),
        secondary_container: Color::from_rgb8(0x4A, 0x44, 0x58),
        on_secondary_container: Color::from_rgb8(0xE8, 0xDE, 0xF8),
        secondary_fixed: Color::from_rgb8(0xE8, 0xDE, 0xF8),
        secondary_fixed_dim: Color::from_rgb8(0xCC, 0xC2, 0xDC),
        on_secondary_fixed: Color::from_rgb8(0x1D, 0x19, 0x2B),
        on_secondary_fixed_variant: Color::from_rgb8(0x4A, 0x44, 0x58),

        tertiary: Color::from_rgb8(0xEF, 0xB8, 0xC8),
        on_tertiary: Color::from_rgb8(0x49, 0x25, 0x32),
        tertiary_container: Color::from_rgb8(0x63, 0x3B, 0x48),
        on_tertiary_container: Color::from_rgb8(0xFF, 0xD8, 0xE4),
        tertiary_fixed: Color::from_rgb8(0xFF, 0xD8, 0xE4),
        tertiary_fixed_dim: Color::from_rgb8(0xEF, 0xB8, 0xC8),
        on_tertiary_fixed: Color::from_rgb8(0x31, 0x11, 0x1D),
        on_tertiary_fixed_variant: Color::from_rgb8(0x63, 0x3B, 0x48),

        error: Color::from_rgb8(0xF2, 0xB8, 0xB5),
        on_error: Color::from_rgb8(0x60, 0x14, 0x10),
        error_container: Color::from_rgb8(0x8C, 0x1D, 0x18),
        on_error_container: Color::from_rgb8(0xF9, 0xDE, 0xDC),

        surface: Color::from_rgb8(0x14, 0x12, 0x18),
        on_surface: Color::from_rgb8(0xE6, 0xE0, 0xE9),
        on_surface_variant: Color::from_rgb8(0xCA, 0xC4, 0xD0),
        surface_dim: Color::from_rgb8(0x14, 0x12, 0x18),
        surface_bright: Color::from_rgb8(0x3B, 0x38, 0x3E),
        surface_container_lowest: Color::from_rgb8(0x0F, 0x0D, 0x13),
        surface_container_low: Color::from_rgb8(0x1D, 0x1B, 0x20),
        surface_container: Color::from_rgb8(0x21, 0x1F, 0x26),
        surface_container_high: Color::from_rgb8(0x2B, 0x29, 0x30),
        surface_container_highest: Color::from_rgb8(0x36, 0x34, 0x3B),

        outline: Color::from_rgb8(0x93, 0x8F, 0x99),
        outline_variant: Color::from_rgb8(0x49, 0x45, 0x4F),
        shadow: Color::from_rgb8(0x00, 0x00, 0x00),
        scrim: Color::from_rgb8(0x00, 0x00, 0x00),
        inverse_surface: Color::from_rgb8(0xE6, 0xE0, 0xE9),
        inverse_on_surface: Color::from_rgb8(0x32, 0x2F, 0x35),
        inverse_primary: Color::from_rgb8(0x67, 0x50, 0xA4),
        surface_tint: Color::from_rgb8(0xD0, 0xBC, 0xFF),
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
}
