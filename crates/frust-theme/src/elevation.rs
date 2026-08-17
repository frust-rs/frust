//! [`Elevation::neutral`]: the design-language-free 6-level (0-5) elevation
//! table, each level a dp value, a v1 shadow mapping, and the
//! surface-container role a surface at that level should paint with.
//!
//! This crate constructs no other `Elevation` — a design system builds its
//! own from its own plugin crate (`frust-material`'s `tokens` module carries
//! the Material 3 table these numbers originate from, source cited there;
//! `frust-cupertino`'s carries the subtler iOS-idiom shadow mapping over the
//! same dp ladder). The dp values and shadow math below are the M3 numbers
//! reused verbatim (see `crate::theme::Theme::neutral`'s module docs).
//!
//! dp source: <https://m3.material.io/styles/elevation> (verified
//! 2026-07-17): L0 0dp, L1 1dp, L2 3dp, L3 6dp, L4 8dp, L5 12dp. Component
//! mapping per the same source: L1 = elevated cards/bottom sheets, L2 = nav
//! bar/menus, L3 = FAB/dialogs.
//!
//! **Shadow math is TUNABLE, not an M3-published spec.** Material 3's 2023
//! direction replaced tonal-overlay tinting with *static surface-container
//! roles* for most elevated surfaces — the opposite of "primarily tonal
//! overlay post-2023" — but shadows still exist alongside; M3 does not
//! publish exact shadow blur/offset math, so this module defines a
//! documented v1 mapping:
//! `y_offset = dp / 2.0 + 1.0`, `blur_std_dev = dp`, shadow color =
//! `ColorScheme::shadow` at `color_alpha` ~0.3. A gallery example
//! is the visual check for this mapping; treat it as adjustable, not load-
//! bearing, Frust-specific policy.
//!
//! # Per-brightness shadows
//!
//! [`ElevationLevel`] carries **separate** light/dark [`ShadowSpec`]s
//! (`shadow_light`/`shadow_dark`), selected via [`ElevationLevel::shadow`].
//! [`Elevation::neutral`] duplicates the same v1 mapping into both slots —
//! behavior-preserving, byte-identical rendered output on either brightness.
//! A design language whose shadow recipe actually differs by brightness
//! (e.g. Glyph) fills the two slots independently.

use crate::color::Brightness;

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

/// One elevation level: its dp value, per-brightness v1 shadow specs, and
/// surface-container role.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ElevationLevel {
    pub dp: f64,
    /// Shadow rendered on [`Brightness::Light`].
    pub shadow_light: ShadowSpec,
    /// Shadow rendered on [`Brightness::Dark`].
    pub shadow_dark: ShadowSpec,
    pub surface_role: SurfaceRole,
}

impl ElevationLevel {
    /// The `ShadowSpec` to render for the given brightness.
    pub fn shadow(&self, brightness: Brightness) -> &ShadowSpec {
        match brightness {
            Brightness::Light => &self.shadow_light,
            Brightness::Dark => &self.shadow_dark,
        }
    }
}

const fn level(dp: f64, surface_role: SurfaceRole) -> ElevationLevel {
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

/// The 6 elevation levels (0-5).
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
    /// The neutral, design-language-free elevation table (dp values
    /// verified; shadow math and surface-role assignment are this crate's
    /// documented v1 mapping — see module docs).
    pub const fn neutral() -> Self {
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
        let e = Elevation::neutral();
        assert_eq!(e.level0.dp, 0.0);
        assert_eq!(e.level1.dp, 1.0);
        assert_eq!(e.level2.dp, 3.0);
        assert_eq!(e.level3.dp, 6.0);
        assert_eq!(e.level4.dp, 8.0);
        assert_eq!(e.level5.dp, 12.0);
    }

    #[test]
    fn shadow_mapping_is_consistent_with_dp() {
        let e = Elevation::neutral();
        assert_eq!(e.level3.shadow_light.y_offset, 4.0);
        assert_eq!(e.level3.shadow_light.blur_std_dev, 6.0);
        assert_eq!(e.level3.shadow_light.color_alpha, 0.3);
    }

    #[test]
    fn neutral_shadow_is_identical_on_both_brightnesses() {
        // Behavior-preserving: the v1 mapping doesn't branch by brightness,
        // so both slots hold the same value and the accessor returns it
        // either way.
        let e = Elevation::neutral();
        assert_eq!(e.level3.shadow_light, e.level3.shadow_dark);
        assert_eq!(
            e.level3.shadow(Brightness::Light),
            e.level3.shadow(Brightness::Dark)
        );
        assert_eq!(*e.level3.shadow(Brightness::Light), e.level3.shadow_light);
    }

    #[test]
    fn surface_roles_match_static_container_direction() {
        let e = Elevation::neutral();
        assert_eq!(e.level0.surface_role, SurfaceRole::Surface);
        assert_eq!(e.level3.surface_role, SurfaceRole::SurfaceContainerHigh);
        assert_eq!(e.level5.surface_role, SurfaceRole::SurfaceContainerHighest);
    }
}
