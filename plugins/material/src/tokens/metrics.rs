//! Material 3 Expressive elevation, spacing, and dimension tokens.
//!
//! Elevation, spacing, and radius values sourced from Material 3 Expressive design tokens:
//! - Elevation: <https://m3.material.io/styles/elevation> (verified 2026-07-17, reference
//!   v0.192 dp values with two-layer shadow model)
//! - Spacing: Material 3 Expressive 6-step scale (xs=4…xxl=32dp)
//! - Dimensions: Material 3 Expressive spacing (0–48dp) and radius tokens

use frust::{Elevation, ElevationLevel, ShadowSpec, SurfaceRole};

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
/// The reference's Dart implementation uses a two-layer shadow model:
/// - Key shadow: 30% alpha, offset = dp/2, blur = dp
/// - Ambient shadow: 15% alpha, offset = dp, blur = dp*2
///
/// This Rust mapping uses a single shadow per the `ElevationLevel` type contract.
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

// ---- Spacing ---------------------------------------------------------------

/// Material 3 Expressive spacing scale (6-step tokens). Values are in logical pixels (dp).
/// Reference: Material 3 Expressive design tokens (verified 2026-08-19, matching
/// `material_3_expressive/lib/foundations/m3e_spacing.dart`).
///
/// These are static token values; components import them directly without theme plumbing.
pub struct MaterialSpacing;

impl MaterialSpacing {
    /// Extra-small gap: 4dp.
    pub const XS: f64 = 4.0;

    /// Small gap: 8dp.
    pub const SM: f64 = 8.0;

    /// Medium gap: 12dp.
    pub const MD: f64 = 12.0;

    /// Large gap: 16dp.
    pub const LG: f64 = 16.0;

    /// Extra-large gap: 24dp.
    pub const XL: f64 = 24.0;

    /// Extra-extra-large gap: 32dp.
    pub const XXL: f64 = 32.0;
}

// ---- Dimensions ---------------------------------------------------------

/// Material 3 Expressive dimension tokens: spacing and radius scales.
/// Reference: Material 3 Expressive design tokens (verified 2026-08-19,
/// `material_3_expressive/lib/foundations/m3e_dimensions.dart`).
///
/// Spacing follows a 4dp grid; radius scales match shape tokens.
/// These are static token values; components import them directly.
pub struct MaterialDimensions;

impl MaterialDimensions {
    // ---- Spacing scale (0–48dp on 4dp grid) ----

    /// 0dp spacing.
    pub const SPACE_0: f64 = 0.0;

    /// 4dp spacing.
    pub const SPACE_4: f64 = 4.0;

    /// 8dp spacing.
    pub const SPACE_8: f64 = 8.0;

    /// 12dp spacing.
    pub const SPACE_12: f64 = 12.0;

    /// 16dp spacing.
    pub const SPACE_16: f64 = 16.0;

    /// 20dp spacing.
    pub const SPACE_20: f64 = 20.0;

    /// 24dp spacing.
    pub const SPACE_24: f64 = 24.0;

    /// 28dp spacing.
    pub const SPACE_28: f64 = 28.0;

    /// 32dp spacing.
    pub const SPACE_32: f64 = 32.0;

    /// 40dp spacing.
    pub const SPACE_40: f64 = 40.0;

    /// 48dp spacing.
    pub const SPACE_48: f64 = 48.0;

    // ---- Theme spacing aliases ----

    /// Theme extra-small gap (`MaterialSpacing::XS`).
    pub const SPACE_XS: f64 = Self::SPACE_4;

    /// Theme small gap (`MaterialSpacing::SM`).
    pub const SPACE_SM: f64 = Self::SPACE_8;

    /// Theme medium gap (`MaterialSpacing::MD`).
    pub const SPACE_MD: f64 = Self::SPACE_12;

    /// Theme large gap (`MaterialSpacing::LG`).
    pub const SPACE_LG: f64 = Self::SPACE_16;

    /// Theme extra-large gap (`MaterialSpacing::XL`).
    pub const SPACE_XL: f64 = Self::SPACE_24;

    /// Theme extra-extra-large gap (`MaterialSpacing::XXL`).
    pub const SPACE_XXL: f64 = Self::SPACE_32;

    // ---- Radius scale ----

    /// 0dp — sharp corners.
    pub const RADIUS_NONE: f64 = 0.0;

    /// 4dp corner radius.
    pub const RADIUS_EXTRA_SMALL: f64 = 4.0;

    /// 8dp corner radius.
    pub const RADIUS_SMALL: f64 = 8.0;

    /// 12dp corner radius.
    pub const RADIUS_MEDIUM: f64 = 12.0;

    /// 16dp corner radius.
    pub const RADIUS_LARGE: f64 = 16.0;

    /// 20dp corner radius (expressive large+).
    pub const RADIUS_LARGE_INCREASED: f64 = 20.0;

    /// 28dp corner radius.
    pub const RADIUS_EXTRA_LARGE: f64 = 28.0;

    /// 32dp corner radius (expressive XL+).
    pub const RADIUS_EXTRA_LARGE_INCREASED: f64 = 32.0;

    /// 48dp corner radius.
    pub const RADIUS_EXTRA_EXTRA_LARGE: f64 = 48.0;

    /// Sentinel radius that reads as a stadium for typical heights.
    pub const RADIUS_FULL: f64 = f64::INFINITY;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elevation_dp_matches_reference() {
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
        assert_eq!(e.level0.shadow_light, e.level0.shadow_dark);
        assert_eq!(e.level1.shadow_light, e.level1.shadow_dark);
        assert_eq!(e.level2.shadow_light, e.level2.shadow_dark);
        assert_eq!(e.level3.shadow_light, e.level3.shadow_dark);
        assert_eq!(e.level4.shadow_light, e.level4.shadow_dark);
        assert_eq!(e.level5.shadow_light, e.level5.shadow_dark);
    }

    #[test]
    fn spacing_scale_matches_reference() {
        assert_eq!(MaterialSpacing::XS, 4.0);
        assert_eq!(MaterialSpacing::SM, 8.0);
        assert_eq!(MaterialSpacing::MD, 12.0);
        assert_eq!(MaterialSpacing::LG, 16.0);
        assert_eq!(MaterialSpacing::XL, 24.0);
        assert_eq!(MaterialSpacing::XXL, 32.0);
    }

    #[test]
    fn spacing_tokens_match_reference() {
        assert_eq!(MaterialDimensions::SPACE_0, 0.0);
        assert_eq!(MaterialDimensions::SPACE_4, 4.0);
        assert_eq!(MaterialDimensions::SPACE_8, 8.0);
        assert_eq!(MaterialDimensions::SPACE_12, 12.0);
        assert_eq!(MaterialDimensions::SPACE_16, 16.0);
        assert_eq!(MaterialDimensions::SPACE_20, 20.0);
        assert_eq!(MaterialDimensions::SPACE_24, 24.0);
        assert_eq!(MaterialDimensions::SPACE_28, 28.0);
        assert_eq!(MaterialDimensions::SPACE_32, 32.0);
        assert_eq!(MaterialDimensions::SPACE_40, 40.0);
        assert_eq!(MaterialDimensions::SPACE_48, 48.0);
    }

    #[test]
    fn spacing_aliases_match_reference() {
        assert_eq!(MaterialDimensions::SPACE_XS, MaterialDimensions::SPACE_4);
        assert_eq!(MaterialDimensions::SPACE_SM, MaterialDimensions::SPACE_8);
        assert_eq!(MaterialDimensions::SPACE_MD, MaterialDimensions::SPACE_12);
        assert_eq!(MaterialDimensions::SPACE_LG, MaterialDimensions::SPACE_16);
        assert_eq!(MaterialDimensions::SPACE_XL, MaterialDimensions::SPACE_24);
        assert_eq!(MaterialDimensions::SPACE_XXL, MaterialDimensions::SPACE_32);
    }

    #[test]
    fn radius_tokens_match_reference() {
        assert_eq!(MaterialDimensions::RADIUS_NONE, 0.0);
        assert_eq!(MaterialDimensions::RADIUS_EXTRA_SMALL, 4.0);
        assert_eq!(MaterialDimensions::RADIUS_SMALL, 8.0);
        assert_eq!(MaterialDimensions::RADIUS_MEDIUM, 12.0);
        assert_eq!(MaterialDimensions::RADIUS_LARGE, 16.0);
        assert_eq!(MaterialDimensions::RADIUS_LARGE_INCREASED, 20.0);
        assert_eq!(MaterialDimensions::RADIUS_EXTRA_LARGE, 28.0);
        assert_eq!(MaterialDimensions::RADIUS_EXTRA_LARGE_INCREASED, 32.0);
        assert_eq!(MaterialDimensions::RADIUS_EXTRA_EXTRA_LARGE, 48.0);
        assert!(MaterialDimensions::RADIUS_FULL.is_infinite());
    }
}
