//! [`StatusPalette`]: success/warning/info color roles as a
//! [`crate::extensions::ThemeExtensions`] consumer.
//!
//! [`crate::color::ColorScheme`]'s 46 roles carry no
//! `success`/`warning`/`info` at all — only `error`. This extension is that
//! gap's typed filler: [`crate::theme::Theme::neutral`] attaches
//! [`StatusPalette::neutral`], and a design system attaches its own table the
//! same way.
//!
//! **Community-approximate** (`neutral`'s values): they apply the same
//! tone-relationship Material 3's `error` roles use (light:
//! base/on/container/on-container ≈ tone 40/100/90/10; dark: ≈ tone
//! 80/20/30/90) to green (success), amber (warning), and blue (info) seed
//! hues, chosen for conventional semantic association and AA contrast against
//! `surface`/`on_surface`, not measured against a specific published export —
//! Material 3 has no fixed status-role table to cite (its Theme Builder
//! covers the gap with per-seed HCT "custom colors" instead).

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
/// [`crate::theme::Theme::neutral`] attaches [`StatusPalette::neutral`], so
/// `extension::<StatusPalette>()` is always `Some` on the framework's own
/// baseline; an app or design system attaches its own table with
/// `theme.extensions.insert(StatusPalette { .. })` (or
/// [`ThemeBuilder::extension`](crate::builder::ThemeBuilder::extension)) to
/// override it wholesale.
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

    /// The language-free palette [`crate::theme::Theme::neutral`] composes.
    ///
    /// Success/warning/info are a *functional* signal, not a design-language
    /// "look" — the same reasoning `ColorScheme::neutral_light`/`_dark` use to
    /// keep `error` real red instead of grayscaling it too — so the neutral
    /// baseline carries a real three-status table rather than going unset.
    /// The values are the module docs' community-approximate tone
    /// relationships, stated here in full rather than delegated, so this
    /// constructor stands on its own with every design language out of tree.
    pub const fn neutral() -> Self {
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

                // Blue seed, the conventional "info" blue.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_colors_selects_by_brightness() {
        let palette = StatusPalette::neutral();
        assert_eq!(palette.colors(Brightness::Light), &palette.light);
        assert_eq!(palette.colors(Brightness::Dark), &palette.dark);
    }

    #[test]
    fn neutral_light_and_dark_are_distinct() {
        let palette = StatusPalette::neutral();
        assert_ne!(palette.light.success, palette.dark.success);
        assert_ne!(palette.light.warning, palette.dark.warning);
        assert_ne!(palette.light.info, palette.dark.info);
    }

    #[test]
    fn neutral_spot_check() {
        let palette = StatusPalette::neutral();
        assert_eq!(palette.light.success, Color::from_rgb8(0x2E, 0x7D, 0x32));
        assert_eq!(palette.light.on_success, Color::from_rgb8(0xFF, 0xFF, 0xFF));
        assert_eq!(palette.dark.warning, Color::from_rgb8(0xFF, 0xC4, 0x6B));
        assert_eq!(palette.dark.info, Color::from_rgb8(0x9F, 0xCA, 0xFF));
    }

    #[test]
    fn neutral_is_const_constructible() {
        // Regression anchor: `StatusPalette::neutral()` must stay a `const fn`
        // so a design system can name it in a const context; this binds it to
        // a `const` and just uses it, which fails to compile if constness
        // regresses.
        const PALETTE: StatusPalette = StatusPalette::neutral();
        assert_eq!(PALETTE.light.success, Color::from_rgb8(0x2E, 0x7D, 0x32));
    }
}
