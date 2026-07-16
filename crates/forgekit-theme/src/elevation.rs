//! Material 3 elevation: 6 levels (0-5), each a dp value, a v1 shadow
//! mapping, and the surface-container role a surface at that level should
//! paint with.
//!
//! dp source: <https://m3.material.io/styles/elevation> (verified
//! 2026-07-17): L0 0dp, L1 1dp, L2 3dp, L3 6dp, L4 8dp, L5 12dp. Component
//! mapping per the same source: L1 = elevated cards/bottom sheets, L2 = nav
//! bar/menus, L3 = FAB/dialogs.
//!
//! **Shadow math is TUNABLE, not an M3-published spec.** Material 3's 2023
//! direction replaced tonal-overlay tinting with *static surface-container
//! roles* for most elevated surfaces (see
//! `workflow/plans/features/forgekit-phase-6a-foundations/research/RESEARCH.md`'s
//! refuted-claims list — this is the opposite of "primarily tonal overlay
//! post-2023"), but shadows still exist alongside; M3 does not publish exact
//! shadow blur/offset math, so this module defines a documented v1 mapping:
//! `y_offset = dp / 2.0 + 1.0`, `blur_std_dev = dp`, shadow color =
//! `ColorScheme::shadow` at `color_alpha` ~0.3. The gallery example (task 09)
//! is the visual check for this mapping; treat it as adjustable, not load-
//! bearing, ForgeKit-specific policy.

/// Y-offset, Gaussian blur standard deviation, and shadow color alpha for
/// one elevation level's drop shadow. All lengths in logical px; `color_alpha`
/// multiplies against `ColorScheme::shadow`'s own alpha (usually opaque
/// black) to get the final translucency — see module docs for the v1
/// mapping this crate uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowSpec {
    pub y_offset: f64,
    pub blur_std_dev: f64,
    pub color_alpha: f32,
}

/// Which `ColorScheme` surface-container role an elevated surface at a given
/// level should paint with — the 2023 static-surface-container-role
/// direction (see module docs), not tonal-overlay tinting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceRole {
    Surface,
    SurfaceContainerLowest,
    SurfaceContainerLow,
    SurfaceContainer,
    SurfaceContainerHigh,
    SurfaceContainerHighest,
}

/// One elevation level: its dp value, v1 shadow spec, and surface-container
/// role.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ElevationLevel {
    pub dp: f64,
    pub shadow: ShadowSpec,
    pub surface_role: SurfaceRole,
}

const fn level(dp: f64, surface_role: SurfaceRole) -> ElevationLevel {
    ElevationLevel {
        dp,
        shadow: ShadowSpec {
            y_offset: dp / 2.0 + 1.0,
            blur_std_dev: dp,
            color_alpha: 0.3,
        },
        surface_role,
    }
}

/// The 6 Material 3 elevation levels (0-5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Elevation {
    pub level0: ElevationLevel,
    pub level1: ElevationLevel,
    pub level2: ElevationLevel,
    pub level3: ElevationLevel,
    pub level4: ElevationLevel,
    pub level5: ElevationLevel,
}

impl Elevation {
    /// The Material 3 baseline elevation table (dp values verified; shadow
    /// math and surface-role assignment are this crate's documented v1
    /// mapping — see module docs).
    pub const fn m3() -> Self {
        Self {
            level0: level(0.0, SurfaceRole::Surface),
            level1: level(1.0, SurfaceRole::SurfaceContainerLow),
            level2: level(3.0, SurfaceRole::SurfaceContainer),
            level3: level(6.0, SurfaceRole::SurfaceContainerHigh),
            level4: level(8.0, SurfaceRole::SurfaceContainerHigh),
            level5: level(12.0, SurfaceRole::SurfaceContainerHighest),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dp_matches_table() {
        let e = Elevation::m3();
        assert_eq!(e.level0.dp, 0.0);
        assert_eq!(e.level1.dp, 1.0);
        assert_eq!(e.level2.dp, 3.0);
        assert_eq!(e.level3.dp, 6.0);
        assert_eq!(e.level4.dp, 8.0);
        assert_eq!(e.level5.dp, 12.0);
    }

    #[test]
    fn shadow_mapping_is_consistent_with_dp() {
        let e = Elevation::m3();
        assert_eq!(e.level3.shadow.y_offset, 4.0);
        assert_eq!(e.level3.shadow.blur_std_dev, 6.0);
        assert_eq!(e.level3.shadow.color_alpha, 0.3);
    }

    #[test]
    fn surface_roles_match_static_container_direction() {
        let e = Elevation::m3();
        assert_eq!(e.level0.surface_role, SurfaceRole::Surface);
        assert_eq!(e.level3.surface_role, SurfaceRole::SurfaceContainerHigh);
        assert_eq!(e.level5.surface_role, SurfaceRole::SurfaceContainerHighest);
    }
}
