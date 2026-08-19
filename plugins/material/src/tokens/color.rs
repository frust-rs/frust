//! Material's baseline light/dark [`ColorScheme`]s, plus the nine M3E
//! "semantic" color roles frust-theme's `ColorScheme` has no baseline slot
//! for.
//!
//! [`color_scheme_light`]/[`color_scheme_dark`] carry the 34 M3E roles that
//! *do* map onto `ColorScheme`'s own field set — unchanged from this crate's
//! original hand-baked Material Web v0.192 tokens (seed `#6750A4`). The
//! remaining 9 — [`MaterialSemanticColors`]'s fields, built by
//! [`semantic_light`]/[`semantic_dark`] — round the baseline out to the
//! Material 3 Expressive reference's full 43-role `M3EColorScheme`. See
//! [`super`]'s module docs for the complete 43-row role-partition table.
//!
//! Source: `paadevelopments/material_3_expressive` v1.0.8 (pub.dev;
//! `github.com/paadevelopments/material_3_expressive`),
//! `lib/foundations/m3e_color_scheme.dart`'s `M3EColorScheme.fromColorScheme`
//! factory, retrieved 2026-08-19.
//!
//! **Porting decisions** (the nine semantic roles):
//! - `emphasis`/`onEmphasis`, `info`, `danger`, `onSurfaceStrong`, and
//!   `outlineStrong` are the reference's own *derived* roles — it defines
//!   them as `scheme.primary`/`scheme.onPrimary`, `scheme.tertiary`,
//!   `scheme.error`, `scheme.onSurface`, and `scheme.outline` respectively,
//!   never as independent values — so [`semantic_light`]/[`semantic_dark`]
//!   read them straight off [`color_scheme_light`]/[`color_scheme_dark`]'s
//!   matching fields rather than re-baking a second copy of the same hex.
//! - `success`/`warning` are the reference's only two genuinely independent
//!   semantic constants (`#FF2E7D32`/`#FFEF6C00` light, `#FF81C784`/
//!   `#FFFFB74D` dark) — transcribed verbatim.
//! - `surfaceStrong` is the one *computed* role:
//!   `Color.alphaBlend(primary.withValues(alpha: 0.06), surface)`, a 6%
//!   alpha-wash of `primary` flattened over `surface` to opaque. This module
//!   applies the same blend formula (and rounding) as
//!   `frust_theme::color::blend_accent_over_surface` — this crate's existing
//!   precedent for exactly this kind of accent-over-surface wash, just at a
//!   6% mix instead of that helper's 10% — and bakes the result as a
//!   hand-computed constant rather than blending at runtime (this task's
//!   values are hand-baked throughout; a later HCT task must reproduce
//!   them).

use frust::ColorScheme;
use peniko::Color;

/// The Material 3 baseline light [`ColorScheme`] (seed `#6750A4`).
///
/// Source: material-components/material-web tokens v0.192
/// `_md-sys-color.scss` (light), resolved 2026-07-17.
///
/// Carries 34 of the M3E reference's 43 color roles — see [module
/// docs](self) for the porting notes and [`super`]'s module docs for the
/// full role-partition table.
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
///
/// Carries 34 of the M3E reference's 43 color roles — see [module
/// docs](self) for the porting notes and [`super`]'s module docs for the
/// full role-partition table.
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

/// The nine M3E roles [`ColorScheme`] has no field for — one brightness's
/// worth. Carried by [`super::extension::MaterialTokens`] as its
/// `light`/`dark` fields; see [module docs](self) for how each is sourced.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialSemanticColors {
    /// `emphasis` — the reference's high-emphasis accent (`scheme.primary`).
    pub emphasis: Color,
    /// `onEmphasis` — content color on [`Self::emphasis`] (`scheme.onPrimary`).
    pub on_emphasis: Color,
    /// `info` — informational role (`scheme.tertiary`).
    pub info: Color,
    /// `success` — independent semantic constant, not derived from `scheme`.
    pub success: Color,
    /// `warning` — independent semantic constant, not derived from `scheme`.
    pub warning: Color,
    /// `danger` — destructive/error role (`scheme.error`).
    pub danger: Color,
    /// `surfaceStrong` — a slightly emphasized grouped-container surface;
    /// `primary` alpha-blended over `surface` at 6%.
    pub surface_strong: Color,
    /// `onSurfaceStrong` — content color on [`Self::surface_strong`]
    /// (`scheme.onSurface`).
    pub on_surface_strong: Color,
    /// `outlineStrong` — a stronger outline for high-contrast separators
    /// (`scheme.outline`).
    pub outline_strong: Color,
}

/// [`MaterialSemanticColors`] for [`color_scheme_light`] — see [module
/// docs](self)'s porting decisions for how each field is sourced.
pub fn semantic_light() -> MaterialSemanticColors {
    let scheme = color_scheme_light();
    MaterialSemanticColors {
        emphasis: scheme.primary,
        on_emphasis: scheme.on_primary,
        info: scheme.tertiary,
        // `Color(0xFF2E7D32)` in the reference's light branch.
        success: Color::from_rgb8(0x2E, 0x7D, 0x32),
        // `Color(0xFFEF6C00)` in the reference's light branch.
        warning: Color::from_rgb8(0xEF, 0x6C, 0x00),
        danger: scheme.error,
        // `Color.alphaBlend(primary.withValues(alpha: 0.06), surface)`,
        // hand-computed — see [module docs](self).
        surface_strong: Color::from_rgb8(0xF5, 0xED, 0xFA),
        on_surface_strong: scheme.on_surface,
        outline_strong: scheme.outline,
    }
}

/// [`MaterialSemanticColors`] for [`color_scheme_dark`] — the dark-mode
/// mirror of [`semantic_light`]; see that function's doc comment.
pub fn semantic_dark() -> MaterialSemanticColors {
    let scheme = color_scheme_dark();
    MaterialSemanticColors {
        emphasis: scheme.primary,
        on_emphasis: scheme.on_primary,
        info: scheme.tertiary,
        // `Color(0xFF81C784)` in the reference's dark branch.
        success: Color::from_rgb8(0x81, 0xC7, 0x84),
        // `Color(0xFFFFB74D)` in the reference's dark branch.
        warning: Color::from_rgb8(0xFF, 0xB7, 0x4D),
        danger: scheme.error,
        surface_strong: Color::from_rgb8(0x1F, 0x1C, 0x26),
        on_surface_strong: scheme.on_surface,
        outline_strong: scheme.outline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A missing field on either struct is a compile error, not a test
    /// failure — this test exists as a documentation anchor for the
    /// "34 baseline-mapped + 9 semantic-only = 43" role-count claim (see
    /// [`super`]'s module docs' partition table), the same pattern
    /// `frust_theme::color`'s `deprecated_roles_absent` test uses.
    #[test]
    fn all_43_m3e_roles_are_reachable() {
        let scheme = color_scheme_light();
        let baseline_roles: [Color; 34] = [
            scheme.primary,
            scheme.on_primary,
            scheme.primary_container,
            scheme.on_primary_container,
            scheme.secondary,
            scheme.on_secondary,
            scheme.secondary_container,
            scheme.on_secondary_container,
            scheme.tertiary,
            scheme.on_tertiary,
            scheme.tertiary_container,
            scheme.on_tertiary_container,
            scheme.error,
            scheme.on_error,
            scheme.error_container,
            scheme.on_error_container,
            scheme.surface,
            scheme.on_surface,
            scheme.on_surface_variant,
            scheme.surface_container_lowest,
            scheme.surface_container_low,
            scheme.surface_container,
            scheme.surface_container_high,
            scheme.surface_container_highest,
            scheme.surface_dim,
            scheme.surface_bright,
            scheme.inverse_surface,
            scheme.inverse_on_surface,
            scheme.inverse_primary,
            scheme.outline,
            scheme.outline_variant,
            scheme.shadow,
            scheme.scrim,
            scheme.surface_tint,
        ];
        let semantic = semantic_light();
        let semantic_roles: [Color; 9] = [
            semantic.emphasis,
            semantic.on_emphasis,
            semantic.info,
            semantic.success,
            semantic.warning,
            semantic.danger,
            semantic.surface_strong,
            semantic.on_surface_strong,
            semantic.outline_strong,
        ];
        assert_eq!(baseline_roles.len() + semantic_roles.len(), 43);
    }

    #[test]
    fn semantic_light_and_dark_are_distinct() {
        assert_ne!(semantic_light(), semantic_dark());
        assert_ne!(semantic_light().emphasis, semantic_dark().emphasis);
        assert_ne!(
            semantic_light().surface_strong,
            semantic_dark().surface_strong
        );
    }

    #[test]
    fn semantic_derived_roles_mirror_the_baseline_scheme() {
        let scheme = color_scheme_light();
        let semantic = semantic_light();
        assert_eq!(semantic.emphasis, scheme.primary);
        assert_eq!(semantic.on_emphasis, scheme.on_primary);
        assert_eq!(semantic.info, scheme.tertiary);
        assert_eq!(semantic.danger, scheme.error);
        assert_eq!(semantic.on_surface_strong, scheme.on_surface);
        assert_eq!(semantic.outline_strong, scheme.outline);

        let dark_scheme = color_scheme_dark();
        let dark_semantic = semantic_dark();
        assert_eq!(dark_semantic.emphasis, dark_scheme.primary);
        assert_eq!(dark_semantic.on_emphasis, dark_scheme.on_primary);
        assert_eq!(dark_semantic.info, dark_scheme.tertiary);
        assert_eq!(dark_semantic.danger, dark_scheme.error);
        assert_eq!(dark_semantic.on_surface_strong, dark_scheme.on_surface);
        assert_eq!(dark_semantic.outline_strong, dark_scheme.outline);
    }

    #[test]
    fn semantic_independent_constants_match_the_reference() {
        assert_eq!(semantic_light().success, Color::from_rgb8(0x2E, 0x7D, 0x32));
        assert_eq!(semantic_light().warning, Color::from_rgb8(0xEF, 0x6C, 0x00));
        assert_eq!(semantic_dark().success, Color::from_rgb8(0x81, 0xC7, 0x84));
        assert_eq!(semantic_dark().warning, Color::from_rgb8(0xFF, 0xB7, 0x4D));
    }

    #[test]
    fn surface_strong_matches_the_alpha_blend_formula() {
        // Cross-check against `frust_theme::color`'s own blend precedent
        // (`ColorScheme::with_accent`'s `blend_accent_over_surface` helper)
        // at a 6% alpha instead of that helper's 10% — same formula, a
        // different mix, verifying the hand-baked constant above wasn't
        // mis-transcribed.
        fn blend(accent: Color, surface: Color, alpha: f64) -> Color {
            let [ar, ag, ab, _] = accent.to_rgba8().to_u8_array();
            let [sr, sg, sb, _] = surface.to_rgba8().to_u8_array();
            let mix = |a: u8, s: u8| -> u8 {
                ((a as f64 / 255.0 * alpha + s as f64 / 255.0 * (1.0 - alpha)) * 255.0).round()
                    as u8
            };
            Color::from_rgb8(mix(ar, sr), mix(ag, sg), mix(ab, sb))
        }

        let light = color_scheme_light();
        assert_eq!(
            semantic_light().surface_strong,
            blend(light.primary, light.surface, 0.06)
        );
        let dark = color_scheme_dark();
        assert_eq!(
            semantic_dark().surface_strong,
            blend(dark.primary, dark.surface, 0.06)
        );
    }
}
