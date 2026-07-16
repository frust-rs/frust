//! Material 3 baseline color roles: [`ColorScheme`] (46 non-deprecated
//! roles) and the [`Brightness`] a [`crate::theme::Theme`] selects one by.
//!
//! Values sourced from Google's Material 3 design-system tokens v0.192
//! (`material-components/material-web`
//! `tokens/versions/v0_192/_md-sys-color.scss` +
//! `_md-ref-palette.scss`), resolved 2026-07-17, seed color `#6750A4`. See
//! `workflow/plans/features/forgekit-phase-6a-foundations/research/RESEARCH.md`
//! §Q5 for the role-count verification (47 non-deprecated roles counting
//! `brightness` on [`crate::theme::Theme`]; 46 of those are `Color` fields
//! here).
//!
//! `background`/`onBackground`/`surfaceVariant` are **not implemented** —
//! Material 3 deprecated all three in favor of `surface`/`onSurface` and the
//! `surfaceContainer*` ladder respectively; carrying them forward would just
//! duplicate an existing role under a legacy name.

use peniko::Color;

/// Which half of a [`crate::theme::Theme`]'s paired light/dark
/// [`ColorScheme`]s is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Brightness {
    /// The light `ColorScheme`. Default.
    #[default]
    Light,
    /// The dark `ColorScheme`.
    Dark,
}

/// A full set of Material 3 color roles for one brightness (light or dark).
///
/// 46 non-deprecated roles (see module docs for the deprecated 3). Construct
/// the Material 3 baseline palette (seed `#6750A4`) via
/// [`ColorScheme::m3_baseline_light`] / [`ColorScheme::m3_baseline_dark`];
/// every field is a plain opaque [`Color`] (alpha channel-appropriate
/// translucency, e.g. for scrims, is applied by the widget/scene layer, not
/// stored here).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorScheme {
    pub primary: Color,
    pub on_primary: Color,
    pub primary_container: Color,
    pub on_primary_container: Color,
    pub primary_fixed: Color,
    pub primary_fixed_dim: Color,
    pub on_primary_fixed: Color,
    pub on_primary_fixed_variant: Color,

    pub secondary: Color,
    pub on_secondary: Color,
    pub secondary_container: Color,
    pub on_secondary_container: Color,
    pub secondary_fixed: Color,
    pub secondary_fixed_dim: Color,
    pub on_secondary_fixed: Color,
    pub on_secondary_fixed_variant: Color,

    pub tertiary: Color,
    pub on_tertiary: Color,
    pub tertiary_container: Color,
    pub on_tertiary_container: Color,
    pub tertiary_fixed: Color,
    pub tertiary_fixed_dim: Color,
    pub on_tertiary_fixed: Color,
    pub on_tertiary_fixed_variant: Color,

    pub error: Color,
    pub on_error: Color,
    pub error_container: Color,
    pub on_error_container: Color,

    pub surface: Color,
    pub on_surface: Color,
    pub on_surface_variant: Color,
    pub surface_dim: Color,
    pub surface_bright: Color,
    pub surface_container_lowest: Color,
    pub surface_container_low: Color,
    pub surface_container: Color,
    pub surface_container_high: Color,
    pub surface_container_highest: Color,

    pub outline: Color,
    pub outline_variant: Color,
    pub shadow: Color,
    pub scrim: Color,
    pub inverse_surface: Color,
    pub inverse_on_surface: Color,
    pub inverse_primary: Color,
    pub surface_tint: Color,
}

impl ColorScheme {
    /// The Material 3 baseline light `ColorScheme` (seed `#6750A4`).
    ///
    /// Source: material-components/material-web tokens v0.192
    /// `_md-sys-color.scss` (light), resolved 2026-07-17.
    pub const fn m3_baseline_light() -> Self {
        Self {
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

    /// The Material 3 baseline dark `ColorScheme` (seed `#6750A4`).
    ///
    /// Source: material-components/material-web tokens v0.192
    /// `_md-sys-color.scss` (dark), resolved 2026-07-17.
    pub const fn m3_baseline_dark() -> Self {
        Self {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip a known ARGB hex value through `Color::from_rgb8` the same
    /// way every scheme role above is constructed.
    #[test]
    fn hex_round_trip() {
        let c = Color::from_rgb8(0x67, 0x50, 0xA4);
        let [r, g, b, a] = c.to_rgba8().to_u8_array();
        assert_eq!((r, g, b, a), (0x67, 0x50, 0xA4, 0xFF));
    }

    #[test]
    fn light_scheme_spot_check() {
        let s = ColorScheme::m3_baseline_light();
        assert_eq!(s.primary, Color::from_rgb8(0x67, 0x50, 0xA4));
        assert_eq!(s.on_primary, Color::from_rgb8(0xFF, 0xFF, 0xFF));
        assert_eq!(s.primary_container, Color::from_rgb8(0xEA, 0xDD, 0xFF));
        assert_eq!(s.error, Color::from_rgb8(0xB3, 0x26, 0x1E));
        assert_eq!(s.surface, Color::from_rgb8(0xFE, 0xF7, 0xFF));
        assert_eq!(
            s.surface_container_lowest,
            Color::from_rgb8(0xFF, 0xFF, 0xFF)
        );
        assert_eq!(s.surface_container, Color::from_rgb8(0xF3, 0xED, 0xF7));
        assert_eq!(
            s.surface_container_highest,
            Color::from_rgb8(0xE6, 0xE0, 0xE9)
        );
        assert_eq!(s.outline, Color::from_rgb8(0x79, 0x74, 0x7E));
        assert_eq!(s.surface_tint, Color::from_rgb8(0x67, 0x50, 0xA4));
    }

    #[test]
    fn dark_scheme_spot_check() {
        let s = ColorScheme::m3_baseline_dark();
        assert_eq!(s.primary, Color::from_rgb8(0xD0, 0xBC, 0xFF));
        assert_eq!(s.on_primary, Color::from_rgb8(0x38, 0x1E, 0x72));
        assert_eq!(s.primary_container, Color::from_rgb8(0x4F, 0x37, 0x8B));
        assert_eq!(s.error, Color::from_rgb8(0xF2, 0xB8, 0xB5));
        assert_eq!(s.surface, Color::from_rgb8(0x14, 0x12, 0x18));
        assert_eq!(
            s.surface_container_lowest,
            Color::from_rgb8(0x0F, 0x0D, 0x13)
        );
        assert_eq!(s.surface_container, Color::from_rgb8(0x21, 0x1F, 0x26));
        assert_eq!(
            s.surface_container_highest,
            Color::from_rgb8(0x36, 0x34, 0x3B)
        );
        assert_eq!(s.outline, Color::from_rgb8(0x93, 0x8F, 0x99));
        assert_eq!(s.surface_tint, Color::from_rgb8(0xD0, 0xBC, 0xFF));
    }

    #[test]
    fn deprecated_roles_absent() {
        // background/onBackground/surfaceVariant are not fields on
        // ColorScheme at all — this test exists as a documentation anchor,
        // not a runtime check (a missing field is a compile error, not a
        // test failure). See module docs for why they're omitted.
        let _ = ColorScheme::m3_baseline_light();
    }
}
