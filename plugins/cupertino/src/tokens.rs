//! The Cupertino baseline design tokens this crate installs: a [`Theme`]
//! aggregate plus its constituent light/dark [`ColorScheme`]s and the
//! success/warning/info [`StatusPalette`] extension.
//!
//! These tables were lifted out of `frust-theme` when the Cupertino catalog
//! moved out of tree; nothing in the framework constructs them any more, so
//! this crate owns them outright. The Liquid Glass recipe lives in
//! [`crate::glass`] instead (a separate module — see that module's own docs
//! for why).

use frust::authoring::text::TextStyle;
use frust::{
    Brightness, ColorScheme, DesignLanguage, Elevation, MotionScheme, ShapeScale, StatusColors,
    StatusPalette, Theme, ThemeExtensions, TypeScale,
};
use peniko::Color;

/// The Cupertino baseline theme: baseline light/dark Cupertino color
/// schemes, the Cupertino type scale (built from `TextStyle::default()`),
/// the Cupertino shape scale, the Cupertino elevation table, the Cupertino
/// motion scheme, and the [`crate::glass::ios27`] Liquid Glass scale. Starts
/// in [`Brightness::Light`]. Attaches [`status_palette`] as a pre-populated
/// extension (Cupertino has no published success/warning/info equivalent,
/// so it shares the Material 3 default rather than going unset — see
/// `Theme::extension`).
pub fn baseline() -> Theme {
    let mut extensions = ThemeExtensions::new();
    extensions.insert(status_palette());
    Theme {
        light: color_scheme_light(),
        dark: color_scheme_dark(),
        type_scale: TypeScale::cupertino(&TextStyle::default()),
        shape: ShapeScale::cupertino(),
        elevation: Elevation::cupertino(),
        motion: MotionScheme::cupertino(),
        glass: crate::glass::ios27(),
        brightness: Brightness::Light,
        design_language: DesignLanguage::Cupertino,
        extensions,
    }
}

/// The Cupertino (iOS) light [`ColorScheme`] — semantic roles from Apple's
/// iOS semantic color palette, resolved against the iOS 27 UI Kit
/// (2026-07-18) where a real, citable kit record exists; a Frust-authored
/// fill-in otherwise (M3 concepts with no iOS equivalent — "container"
/// roles, the 5-step surface-container ladder).
pub fn color_scheme_light() -> ColorScheme {
    // systemBlue — community-measured (Apple doesn't publish exact
    // hex); iOS 27 UI Kit `System Colors/Light/8 Blue`, 2026-07-18.
    const SYSTEM_BLUE_LIGHT: Color = Color::from_rgb8(0x00, 0x87, 0xFF);
    const SYSTEM_BLUE_DARK: Color = Color::from_rgb8(0x00, 0x90, 0xFF);
    // systemPurple — community-measured, used as the "tertiary" accent;
    // iOS 27 UI Kit `System Colors/Light/10 Purple`, 2026-07-18.
    const SYSTEM_PURPLE_LIGHT: Color = Color::from_rgb8(0xCB, 0x2F, 0xE0);
    // systemGray — Apple documents the same base hex for light/dark;
    // iOS 27 UI Kit `Grays/Light|Dark/Gray`, 2026-07-18.
    const SYSTEM_GRAY: Color = Color::from_rgb8(0x8D, 0x8D, 0x92);
    // systemRed — community-measured; iOS 27 UI Kit
    // `System Colors/Light/1 Red`, 2026-07-18.
    const SYSTEM_RED_LIGHT: Color = Color::from_rgb8(0xFF, 0x38, 0x3C);
    const LABEL_LIGHT: Color = Color::from_rgb8(0x00, 0x00, 0x00);
    // opaqueSeparator (Apple-documented flattened hairline); iOS 27 UI
    // Kit `Separators/Light/Opaque`, 2026-07-18.
    const OPAQUE_SEPARATOR_LIGHT: Color = Color::from_rgb8(0xC5, 0xC5, 0xC7);

    ColorScheme {
        primary: SYSTEM_BLUE_LIGHT,
        on_primary: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        // Frust-derived tinted "container" (iOS has no container
        // role) — a pale tint of systemBlue over systemBackground.
        primary_container: Color::from_rgb8(0xD6, 0xE9, 0xFF),
        on_primary_container: SYSTEM_BLUE_LIGHT,
        // "Fixed" roles are brightness-invariant by M3 definition — the
        // light-mode tone reused verbatim in both color_scheme_light() and
        // color_scheme_dark().
        primary_fixed: SYSTEM_BLUE_LIGHT,
        primary_fixed_dim: SYSTEM_BLUE_DARK,
        on_primary_fixed: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        on_primary_fixed_variant: Color::from_rgb8(0x00, 0x4C, 0x99),

        secondary: SYSTEM_GRAY,
        on_secondary: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        secondary_container: Color::from_rgb8(0xF2, 0xF2, 0xF7),
        on_secondary_container: LABEL_LIGHT,
        secondary_fixed: Color::from_rgb8(0xF2, 0xF2, 0xF7),
        secondary_fixed_dim: Color::from_rgb8(0x2C, 0x2C, 0x2E),
        on_secondary_fixed: LABEL_LIGHT,
        on_secondary_fixed_variant: SYSTEM_GRAY,

        tertiary: SYSTEM_PURPLE_LIGHT,
        on_tertiary: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        tertiary_container: Color::from_rgb8(0xF3, 0xE1, 0xFB),
        on_tertiary_container: SYSTEM_PURPLE_LIGHT,
        tertiary_fixed: SYSTEM_PURPLE_LIGHT,
        // iOS 27 UI Kit `System Colors/Dark/10 Purple`, 2026-07-18.
        tertiary_fixed_dim: Color::from_rgb8(0xDB, 0x34, 0xF1),
        on_tertiary_fixed: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        on_tertiary_fixed_variant: Color::from_rgb8(0x6B, 0x2E, 0x86),

        error: SYSTEM_RED_LIGHT,
        on_error: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        error_container: Color::from_rgb8(0xFF, 0xD9, 0xD6),
        on_error_container: SYSTEM_RED_LIGHT,

        // systemBackground — iOS 27 UI Kit
        // `Backgrounds/Light - Base/Primary`, 2026-07-18 (confirmed
        // unchanged, opaque white).
        surface: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        on_surface: LABEL_LIGHT,
        // secondaryLabel — translucent per its real iOS token,
        // confirmed unchanged by `Labels/Light/2 Secondary`, 2026-07-18.
        on_surface_variant: Color::from_rgba8(0x3C, 0x3C, 0x43, 153),
        // secondarySystemBackground — iOS 27 UI Kit
        // `Backgrounds/Light - Base/Secondary`, 2026-07-18.
        surface_dim: Color::from_rgb8(0xF1, 0xF1, 0xF6),
        // tertiarySystemBackground — iOS 27 UI Kit
        // `Backgrounds/Light - Base/Tertiary`, 2026-07-18 (confirmed
        // unchanged, opaque white).
        surface_bright: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        // Frust-authored 5-step elevation ladder built from the iOS
        // background/gray tones — iOS has no native 5-step ladder concept.
        surface_container_lowest: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        surface_container_low: Color::from_rgb8(0xF7, 0xF7, 0xFA),
        surface_container: Color::from_rgb8(0xF2, 0xF2, 0xF7),
        surface_container_high: Color::from_rgb8(0xE5, 0xE5, 0xEA),
        surface_container_highest: Color::from_rgb8(0xD1, 0xD1, 0xD6),

        outline: OPAQUE_SEPARATOR_LIGHT,
        // separator — translucent per its real iOS token; iOS 27 UI Kit
        // `Separators/Light/Non-Opaque`, 2026-07-18.
        outline_variant: Color::from_rgba8(0x00, 0x00, 0x00, 31),
        shadow: Color::from_rgb8(0x00, 0x00, 0x00),
        scrim: Color::from_rgb8(0x00, 0x00, 0x00),
        // "Inverse" roles reuse the opposite brightness's base tones.
        inverse_surface: Color::from_rgb8(0x00, 0x00, 0x00),
        inverse_on_surface: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        inverse_primary: SYSTEM_BLUE_DARK,
        // Mirrors M3's own convention: surface_tint == primary.
        surface_tint: SYSTEM_BLUE_LIGHT,
    }
}

/// The Cupertino (iOS) dark [`ColorScheme`] — the dark-mode mirror of
/// [`color_scheme_light`]; see that constructor's doc comment for sources.
pub fn color_scheme_dark() -> ColorScheme {
    // iOS 27 UI Kit `System Colors/Light|Dark/8 Blue`, 2026-07-18.
    const SYSTEM_BLUE_LIGHT: Color = Color::from_rgb8(0x00, 0x87, 0xFF);
    const SYSTEM_BLUE_DARK: Color = Color::from_rgb8(0x00, 0x90, 0xFF);
    // iOS 27 UI Kit `System Colors/Dark/10 Purple`, 2026-07-18.
    const SYSTEM_PURPLE_DARK: Color = Color::from_rgb8(0xDB, 0x34, 0xF1);
    // iOS 27 UI Kit `Grays/Light|Dark/Gray`, 2026-07-18.
    const SYSTEM_GRAY: Color = Color::from_rgb8(0x8D, 0x8D, 0x92);
    // iOS 27 UI Kit `System Colors/Dark/1 Red`, 2026-07-18.
    const SYSTEM_RED_DARK: Color = Color::from_rgb8(0xFF, 0x42, 0x45);
    const LABEL_DARK: Color = Color::from_rgb8(0xFF, 0xFF, 0xFF);
    // opaqueSeparator (Apple-documented flattened hairline), dark; iOS
    // 27 UI Kit `Separators/Dark/Opaque`, 2026-07-18.
    const OPAQUE_SEPARATOR_DARK: Color = Color::from_rgb8(0x38, 0x38, 0x3A);

    ColorScheme {
        primary: SYSTEM_BLUE_DARK,
        on_primary: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        primary_container: Color::from_rgb8(0x16, 0x3A, 0x5C),
        on_primary_container: SYSTEM_BLUE_DARK,
        primary_fixed: SYSTEM_BLUE_LIGHT,
        primary_fixed_dim: SYSTEM_BLUE_DARK,
        on_primary_fixed: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        on_primary_fixed_variant: Color::from_rgb8(0x00, 0x4C, 0x99),

        secondary: SYSTEM_GRAY,
        on_secondary: Color::from_rgb8(0x00, 0x00, 0x00),
        secondary_container: Color::from_rgb8(0x1C, 0x1C, 0x1E),
        on_secondary_container: LABEL_DARK,
        secondary_fixed: Color::from_rgb8(0xF2, 0xF2, 0xF7),
        secondary_fixed_dim: Color::from_rgb8(0x2C, 0x2C, 0x2E),
        on_secondary_fixed: Color::from_rgb8(0x00, 0x00, 0x00),
        on_secondary_fixed_variant: SYSTEM_GRAY,

        tertiary: SYSTEM_PURPLE_DARK,
        on_tertiary: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        tertiary_container: Color::from_rgb8(0x3B, 0x1F, 0x49),
        on_tertiary_container: SYSTEM_PURPLE_DARK,
        // iOS 27 UI Kit `System Colors/Light/10 Purple`, 2026-07-18.
        tertiary_fixed: Color::from_rgb8(0xCB, 0x2F, 0xE0),
        tertiary_fixed_dim: SYSTEM_PURPLE_DARK,
        on_tertiary_fixed: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        on_tertiary_fixed_variant: Color::from_rgb8(0x6B, 0x2E, 0x86),

        error: SYSTEM_RED_DARK,
        on_error: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        error_container: Color::from_rgb8(0x4C, 0x16, 0x13),
        on_error_container: SYSTEM_RED_DARK,

        // systemBackground, dark — iOS 27 UI Kit
        // `Backgrounds/Dark - Base/Primary`, 2026-07-18 (confirmed
        // unchanged, opaque black).
        surface: Color::from_rgb8(0x00, 0x00, 0x00),
        on_surface: LABEL_DARK,
        // secondaryLabel, dark — translucent per its real iOS token;
        // iOS 27 UI Kit `Labels/Dark/2 Secondary`, 2026-07-18.
        on_surface_variant: Color::from_rgba8(0xEA, 0xEA, 0xF4, 153),
        // secondarySystemBackground, dark — iOS 27 UI Kit
        // `Backgrounds/Dark - Base/Secondary`, 2026-07-18.
        surface_dim: Color::from_rgb8(0x1B, 0x1B, 0x1D),
        // tertiarySystemBackground, dark — the "brightest" dark surface;
        // iOS 27 UI Kit `Backgrounds/Dark - Base/Tertiary`, 2026-07-18.
        surface_bright: Color::from_rgb8(0x2B, 0x2B, 0x2D),
        surface_container_lowest: Color::from_rgb8(0x00, 0x00, 0x00),
        surface_container_low: Color::from_rgb8(0x1C, 0x1C, 0x1E),
        surface_container: Color::from_rgb8(0x2C, 0x2C, 0x2E),
        surface_container_high: Color::from_rgb8(0x3A, 0x3A, 0x3C),
        surface_container_highest: Color::from_rgb8(0x48, 0x48, 0x4A),

        outline: OPAQUE_SEPARATOR_DARK,
        // separator, dark — translucent per its real iOS token; iOS 27
        // UI Kit `Separators/Dark/Non-Opaque`, 2026-07-18.
        outline_variant: Color::from_rgba8(0xFF, 0xFF, 0xFF, 31),
        shadow: Color::from_rgb8(0x00, 0x00, 0x00),
        scrim: Color::from_rgb8(0x00, 0x00, 0x00),
        inverse_surface: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        inverse_on_surface: Color::from_rgb8(0x00, 0x00, 0x00),
        inverse_primary: SYSTEM_BLUE_LIGHT,
        surface_tint: SYSTEM_BLUE_DARK,
    }
}

/// The Material 3 baseline [`StatusPalette`] default, shared by the
/// Cupertino baseline (see [`baseline`]'s doc comment for why): Cupertino
/// has no published success/warning/info equivalent either.
///
/// **Community-approximate**: Material 3 has no single official token source
/// for a `success`/`warning`/`info` role table — the values below apply the
/// same tone-relationship `color_scheme_light`/`color_scheme_dark`'s `error`
/// roles use (light: base/on/container/on-container ≈ tone 40/100/90/10;
/// dark: ≈ tone 80/20/30/90) to green (success), amber (warning), and blue
/// (info) seed hues, chosen for conventional semantic association and AA
/// contrast against `surface`/`on_surface`.
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
