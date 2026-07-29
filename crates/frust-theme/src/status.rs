//! [`StatusPalette`]: success/warning/info color roles as a
//! [`crate::extensions::ThemeExtensions`] consumer.
//!
//! Material 3's baseline 46-role [`crate::color::ColorScheme`] has no
//! `success`/`warning`/`info` roles at all — only `error`. Google's own
//! Material Theme Builder covers this gap via its "custom color" extension
//! feature (apply the same HCT tonal-palette derivation `error`/
//! `error_container` uses to an arbitrary seed hue), rather than a second
//! fixed role table — so there is no single official M3 token source to cite
//! here. **Community-approximate**: the values below apply that same
//! tone-relationship `ColorScheme::m3_baseline_light`/`_dark`'s `error` roles
//! use (light: base/on/container/on-container ≈ tone 40/100/90/10; dark: ≈
//! tone 80/20/30/90) to green (success), amber (warning), and blue (info)
//! seed hues, chosen for conventional semantic association and AA contrast
//! against `surface`/`on_surface`, not measured against a specific Google
//! export.
//!
//! Glyph's own values (`StatusPalette::glyph()`) land in a later task (15) —
//! this module ships the extension type plus the M3 default only.

use crate::color::Brightness;
use peniko::Color;

/// One brightness's worth of success/warning/info color roles, mirroring
/// `ColorScheme`'s `error`/`on_error`/`error_container`/`on_error_container`
/// shape for each of the three statuses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatusColors {
    pub success: Color,
    pub on_success: Color,
    pub success_container: Color,
    pub on_success_container: Color,

    pub warning: Color,
    pub on_warning: Color,
    pub warning_container: Color,
    pub on_warning_container: Color,

    pub info: Color,
    pub on_info: Color,
    pub info_container: Color,
    pub on_info_container: Color,
}

/// The success/warning/info extension a [`crate::theme::Theme`] carries via
/// [`crate::extensions::ThemeExtensions`] — recovered with
/// `theme.extension::<StatusPalette>()`.
///
/// Every built-in baseline (`Theme::m3_baseline`, `Theme::cupertino_baseline`)
/// attaches [`StatusPalette::m3`] by default, so `extension::<StatusPalette>()`
/// is always `Some` on a Frust-constructed `Theme` — an app or third-party
/// design system can still `theme.extensions.insert(StatusPalette { .. })`
/// to override it wholesale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatusPalette {
    pub light: StatusColors,
    pub dark: StatusColors,
}

impl StatusPalette {
    /// The `StatusColors` for `brightness` — mirrors
    /// [`crate::theme::Theme::scheme`]'s light/dark selector.
    pub fn colors(&self, brightness: Brightness) -> &StatusColors {
        match brightness {
            Brightness::Light => &self.light,
            Brightness::Dark => &self.dark,
        }
    }

    /// The Material 3 baseline default (see module docs for derivation
    /// rationale) — attached to both `Theme::m3_baseline()` and
    /// `Theme::cupertino_baseline()` (Cupertino has no published equivalent
    /// either, so it shares this default rather than going unset).
    pub const fn m3() -> Self {
        Self {
            light: StatusColors {
                // Green seed, ~tone 40/100/90/10 (mirrors `error`'s light
                // tone relationship: base/on/container/on-container).
                success: Color::from_rgb8(0x2E, 0x7D, 0x32),
                on_success: Color::from_rgb8(0xFF, 0xFF, 0xFF),
                success_container: Color::from_rgb8(0xC8, 0xE6, 0xC9),
                on_success_container: Color::from_rgb8(0x1B, 0x5E, 0x20),

                // Amber seed, tuned dark enough for AA-on-white at the base
                // tone (a literal amber-400 like `#FFC107` fails AA on
                // white).
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
                // Dark tone relationship (~80/20/30/90), mirroring
                // `error`'s dark tones (`F2B8B5`/`601410`/`8C1D18`/`F9DEDC`).
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m3_colors_selects_by_brightness() {
        let palette = StatusPalette::m3();
        assert_eq!(palette.colors(Brightness::Light), &palette.light);
        assert_eq!(palette.colors(Brightness::Dark), &palette.dark);
    }

    #[test]
    fn m3_light_and_dark_are_distinct() {
        let palette = StatusPalette::m3();
        assert_ne!(palette.light.success, palette.dark.success);
        assert_ne!(palette.light.warning, palette.dark.warning);
        assert_ne!(palette.light.info, palette.dark.info);
    }

    #[test]
    fn m3_spot_check() {
        let palette = StatusPalette::m3();
        assert_eq!(palette.light.success, Color::from_rgb8(0x2E, 0x7D, 0x32));
        assert_eq!(palette.light.on_success, Color::from_rgb8(0xFF, 0xFF, 0xFF));
        assert_eq!(palette.dark.warning, Color::from_rgb8(0xFF, 0xC4, 0x6B));
        assert_eq!(palette.dark.info, Color::from_rgb8(0x9F, 0xCA, 0xFF));
    }

    #[test]
    fn m3_is_const_constructible() {
        // Regression anchor: `StatusPalette::m3()` must stay a `const fn` so
        // it can be used in const contexts if a future caller wants that;
        // this binds it to a `const` and just uses it, which fails to
        // compile if constness regresses.
        const PALETTE: StatusPalette = StatusPalette::m3();
        assert_eq!(PALETTE.light.success, Color::from_rgb8(0x2E, 0x7D, 0x32));
    }
}
