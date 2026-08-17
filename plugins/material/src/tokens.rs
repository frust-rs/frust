//! The Material 3 baseline design tokens this crate installs: a [`Theme`]
//! aggregate plus its constituent light/dark [`ColorScheme`]s and the
//! success/warning/info [`StatusPalette`] extension.
//!
//! Values sourced from Google's Material 3 design-system tokens v0.192
//! (`material-components/material-web` `tokens/versions/v0_192/
//! _md-sys-color.scss` + `_md-ref-palette.scss`), resolved 2026-07-17, seed
//! color `#6750A4`. These tables lived in `frust-theme` until the Material
//! catalog moved out of tree; nothing in the framework constructs them any
//! more, so this crate owns them outright.

use frust::authoring::text::TextStyle;
use frust::{
    Brightness, ColorScheme, DesignLanguage, Elevation, GlassScale, MotionScheme, ShapeScale,
    StatusColors, StatusPalette, Theme, ThemeExtensions, TypeScale,
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
        type_scale: TypeScale::m3(&TextStyle::default()),
        shape: ShapeScale::m3(),
        elevation: Elevation::m3(),
        motion: MotionScheme::m3_expressive(),
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
