//! Accent themes — the three prebuilt brand palettes the appearance screen
//! composes onto the active baseline (Huddle showcase, task 15).
//!
//! # Why hand-derived, not HCT-computed
//!
//! Material 3's real dynamic color derives a full tonal palette from a single
//! seed via the HCT color space. Frust ships no HCT engine (a non-goal for
//! the showcase), so each accent here is a **hand-authored role table**: a
//! small set of the M3 color roles ([`primary`](ColorScheme::primary),
//! `on_primary`, `primary_container`, `on_primary_container`, `secondary`,
//! `tertiary`) picked to read like a coherent tonal family, one table per
//! brightness. The values are chosen in the *spirit* of the M3 baseline scheme
//! ([`ColorScheme::m3_baseline_light`]/`_dark`): a saturated mid-tone `primary`
//! on white in light mode, a light `primary` on a dark `on_primary` in dark
//! mode, and a pale/deep `primary_container` pair — the same relationships the
//! baseline uses, just re-hued. They are *consistent*, not tonally exact, and
//! deliberately so: this demonstrates the compose-onto-a-baseline mechanism,
//! not a color-science pipeline.
//!
//! The unlisted roles (surfaces, outline, error, the `*_fixed`/`inverse_*`
//! families) are left at the active baseline's values — an accent re-tints the
//! primary/secondary/tertiary spine and lets the surface neutrals ride along,
//! which is enough for the swatch + live-swap demo.

use frust::{Brightness, Color, ColorScheme, Theme};

/// The selected accent palette. [`AccentChoice::Default`] is the seed state on a
/// fresh appearance screen: no accent override, so the active baseline's own
/// palette (M3 purple / Cupertino blue) shows through. The three named variants
/// are the pickable swatches ([`AccentChoice::SWATCHES`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AccentChoice {
    /// No accent override — follow the baseline palette. The fresh-entry seed.
    #[default]
    Default,
    /// Forge Red — the brand anvil red.
    ForgeRed,
    /// Ocean Blue.
    OceanBlue,
    /// Moss Green.
    MossGreen,
}

impl AccentChoice {
    /// The three pickable accent swatches, in selector order (excludes
    /// [`AccentChoice::Default`], which is reachable only by "Follow system").
    pub const SWATCHES: [AccentChoice; 3] = [
        AccentChoice::ForgeRed,
        AccentChoice::OceanBlue,
        AccentChoice::MossGreen,
    ];

    /// The swatch label.
    pub fn label(self) -> &'static str {
        match self {
            AccentChoice::Default => "Default",
            AccentChoice::ForgeRed => "Forge Red",
            AccentChoice::OceanBlue => "Ocean Blue",
            AccentChoice::MossGreen => "Moss Green",
        }
    }

    /// This accent's `primary` color for `brightness` — the swatch fill and the
    /// value [`apply`] writes into the composed scheme. [`AccentChoice::Default`]
    /// has no color of its own; it returns the baseline's `primary` so a swatch
    /// still has something to paint.
    pub fn primary(self, brightness: Brightness) -> Color {
        match self.roles(brightness) {
            Some(roles) => roles.primary,
            None => baseline_scheme(brightness).primary,
        }
    }

    /// The role table for this accent at `brightness`, or `None` for
    /// [`AccentChoice::Default`] (no override).
    fn roles(self, brightness: Brightness) -> Option<AccentRoles> {
        let light = matches!(brightness, Brightness::Light);
        Some(match (self, light) {
            (AccentChoice::Default, _) => return None,

            // Forge Red — the brand anvil red.
            (AccentChoice::ForgeRed, true) => AccentRoles {
                primary: rgb(0xA4, 0x30, 0x2A),
                on_primary: rgb(0xFF, 0xFF, 0xFF),
                primary_container: rgb(0xFF, 0xDA, 0xD4),
                on_primary_container: rgb(0x40, 0x00, 0x02),
                secondary: rgb(0x77, 0x56, 0x52),
                tertiary: rgb(0x70, 0x5C, 0x2E),
            },
            (AccentChoice::ForgeRed, false) => AccentRoles {
                primary: rgb(0xFF, 0xB4, 0xAB),
                on_primary: rgb(0x69, 0x00, 0x05),
                primary_container: rgb(0x93, 0x00, 0x0A),
                on_primary_container: rgb(0xFF, 0xDA, 0xD4),
                secondary: rgb(0xE7, 0xBD, 0xB6),
                tertiary: rgb(0xE1, 0xC3, 0x8C),
            },

            // Ocean Blue.
            (AccentChoice::OceanBlue, true) => AccentRoles {
                primary: rgb(0x00, 0x63, 0x9B),
                on_primary: rgb(0xFF, 0xFF, 0xFF),
                primary_container: rgb(0xCD, 0xE5, 0xFF),
                on_primary_container: rgb(0x00, 0x1D, 0x33),
                secondary: rgb(0x51, 0x60, 0x6F),
                tertiary: rgb(0x66, 0x58, 0x7B),
            },
            (AccentChoice::OceanBlue, false) => AccentRoles {
                primary: rgb(0x97, 0xCB, 0xFF),
                on_primary: rgb(0x00, 0x33, 0x54),
                primary_container: rgb(0x00, 0x4A, 0x77),
                on_primary_container: rgb(0xCD, 0xE5, 0xFF),
                secondary: rgb(0xB9, 0xC8, 0xDA),
                tertiary: rgb(0xD1, 0xBF, 0xE7),
            },

            // Moss Green.
            (AccentChoice::MossGreen, true) => AccentRoles {
                primary: rgb(0x3B, 0x69, 0x39),
                on_primary: rgb(0xFF, 0xFF, 0xFF),
                primary_container: rgb(0xBC, 0xF0, 0xB4),
                on_primary_container: rgb(0x00, 0x21, 0x06),
                secondary: rgb(0x52, 0x63, 0x4F),
                tertiary: rgb(0x38, 0x65, 0x6A),
            },
            (AccentChoice::MossGreen, false) => AccentRoles {
                primary: rgb(0xA1, 0xD3, 0x99),
                on_primary: rgb(0x0A, 0x39, 0x0F),
                primary_container: rgb(0x23, 0x50, 0x24),
                on_primary_container: rgb(0xBC, 0xF0, 0xB4),
                secondary: rgb(0xB9, 0xCC, 0xB4),
                tertiary: rgb(0xA0, 0xCF, 0xD4),
            },
        })
    }
}

/// The subset of M3 color roles an accent re-tints. See the [module docs](self)
/// for why only these six.
#[derive(Clone, Copy, Debug)]
struct AccentRoles {
    primary: Color,
    on_primary: Color,
    primary_container: Color,
    on_primary_container: Color,
    secondary: Color,
    tertiary: Color,
}

impl AccentRoles {
    /// Write this table's roles onto `scheme` in place (leaving every other role
    /// at the baseline value).
    fn write_onto(self, scheme: &mut ColorScheme) {
        scheme.primary = self.primary;
        scheme.on_primary = self.on_primary;
        scheme.primary_container = self.primary_container;
        scheme.on_primary_container = self.on_primary_container;
        scheme.secondary = self.secondary;
        scheme.tertiary = self.tertiary;
    }
}

/// Compose `accent` onto `theme` in place: re-tints both the light and dark
/// schemes (each from its own brightness table) so the accent stays correct if
/// the platform's light/dark preference flips under the override. A
/// [`AccentChoice::Default`] accent is a no-op — the baseline palette rides
/// through unchanged.
pub fn apply(theme: &mut Theme, accent: AccentChoice) {
    if let Some(light) = accent.roles(Brightness::Light) {
        light.write_onto(&mut theme.light);
    }
    if let Some(dark) = accent.roles(Brightness::Dark) {
        dark.write_onto(&mut theme.dark);
    }
}

/// The M3 baseline scheme for `brightness` — the fallback an
/// [`AccentChoice::Default`] swatch paints from.
fn baseline_scheme(brightness: Brightness) -> ColorScheme {
    match brightness {
        Brightness::Light => ColorScheme::m3_baseline_light(),
        Brightness::Dark => ColorScheme::m3_baseline_dark(),
    }
}

/// Opaque RGB shorthand ([`Color::from_rgb8`]).
const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb8(r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_a_no_op() {
        let mut theme = Theme::m3_baseline();
        let before = theme.clone();
        apply(&mut theme, AccentChoice::Default);
        assert_eq!(theme, before, "the default accent changes nothing");
    }

    #[test]
    fn applying_an_accent_retints_both_schemes() {
        let mut theme = Theme::m3_baseline();
        apply(&mut theme, AccentChoice::OceanBlue);
        assert_eq!(
            theme.light.primary,
            AccentChoice::OceanBlue.primary(Brightness::Light)
        );
        assert_eq!(
            theme.dark.primary,
            AccentChoice::OceanBlue.primary(Brightness::Dark)
        );
        // A non-primary neutral role is untouched.
        assert_eq!(
            theme.light.surface,
            ColorScheme::m3_baseline_light().surface
        );
    }

    #[test]
    fn swatches_are_the_three_named_accents() {
        assert_eq!(AccentChoice::SWATCHES.len(), 3);
        assert!(!AccentChoice::SWATCHES.contains(&AccentChoice::Default));
    }

    #[test]
    fn each_accent_has_a_distinct_primary() {
        let primaries: Vec<Color> = AccentChoice::SWATCHES
            .iter()
            .map(|a| a.primary(Brightness::Light))
            .collect();
        assert_ne!(primaries[0], primaries[1]);
        assert_ne!(primaries[1], primaries[2]);
        assert_ne!(primaries[0], primaries[2]);
    }
}
