//! Material 3 baseline color roles: [`ColorScheme`] (46 non-deprecated
//! roles) and the [`Brightness`] a [`crate::theme::Theme`] selects one by.
//!
//! Values sourced from Google's Material 3 design-system tokens v0.192
//! (`material-components/material-web`
//! `tokens/versions/v0_192/_md-sys-color.scss` +
//! `_md-ref-palette.scss`), resolved 2026-07-17, seed color `#6750A4`.
//! Role-count verification: 47 non-deprecated roles counting
//! `brightness` on [`crate::theme::Theme`]; 46 of those are `Color` fields
//! here.
//!
//! `background`/`onBackground`/`surfaceVariant` are **not implemented** —
//! Material 3 deprecated all three in favor of `surface`/`onSurface` and the
//! `surfaceContainer*` ladder respectively; carrying them forward would just
//! duplicate an existing role under a legacy name.
//!
//! # Cupertino (iOS) mapping
//!
//! [`ColorScheme::cupertino_light`]/[`ColorScheme::cupertino_dark`] fill the
//! same 46 `Color` fields from Apple's iOS semantic color palette instead of
//! the M3 tokens above — `ColorScheme` itself stays a single, fixed-role,
//! design-language-agnostic struct; see
//! [`crate::theme::DesignLanguage`] for the tag a [`crate::theme::Theme`]
//! carries to say which baseline it was built from. Sources retrieved
//! 2026-07-17, **refreshed 2026-07-18** against mined swatches
//! (Apple iOS 27 UI Kit, 107 named swatches across System Colors/Labels/
//! Fills/Backgrounds/Separators/Grays/Liquid Glass/Fills-Vibrant) — every role
//! below notes whether the 2026-07-18 kit value confirmed or changed the
//! 2026-07-17 one:
//!
//! - `label`/`secondaryLabel`: Apple publishes `label` as opaque
//!   `#000000`/`#FFFFFF` (light/dark) — confirmed unchanged by the kit's
//!   `Labels/Light|Dark/1 Primary` swatches. `secondaryLabel` is a
//!   **translucent** token — `rgba(60,60,67,0.60)` light (unchanged,
//!   `Labels/Light/2 Secondary`) / `rgba(234,234,244,0.60)` dark (kit's
//!   `Labels/Dark/2 Secondary`, a 1-bit-per-channel refinement of the
//!   pre-refresh `rgba(235,235,245,0.60)`). Unlike the M3 constructors, the
//!   Cupertino constructors below **do** store non-opaque `Color`s for the
//!   handful of roles (`on_surface_variant`, `outline_variant`) whose real
//!   iOS token is itself translucent — flattening them to opaque would
//!   misrepresent the source value.
//! - System accent colors (`systemBlue`/`systemRed`/`systemPurple`/…) are
//!   **community-measured, not Apple-published** — Apple deliberately
//!   documents them as varying by trait environment. Values below now come
//!   from the iOS 27 UI Kit's `System Colors/Light|Dark/*` swatches
//!   (2026-07-18), which shifted noticeably from the pre-refresh
//!   colorsift.com/"Standard Colors" doc-page constants (e.g. `systemBlue`
//!   light `#007AFF` → `#0087FF`, dark `#0A84FF` → `#0090FF`; `systemRed`
//!   light `#FF3B30` → `#FF383C`, dark `#FF453A` → `#FF4245`; `systemPurple`
//!   light `#AF52DE` → `#CB2FE0`, dark `#BF5AF2` → `#DB34F1`) — still a
//!   design-tool snapshot rather than an Apple-published guarantee, so this
//!   remains community/tool-measured, just a newer measurement.
//! - `systemGray` (`secondary`): the kit's `Grays/Light|Dark/Gray` swatch is
//!   `#8D8D92` for both brightnesses (kit confirms Apple's "same base hex
//!   for light/dark" claim), a 1-bit-per-channel refinement of the
//!   pre-refresh `#8E8E93`.
//! - `separator`/`opaqueSeparator`: the 2026-07-18 kit refresh shows
//!   `separator` (`outline_variant`) as `rgba(0,0,0,0.12)` light /
//!   `rgba(255,255,255,0.12)` dark (`Separators/Light|Dark/Non-Opaque`) — a
//!   **changed token** from the pre-refresh `rgba(60,60,67,0.29)` light /
//!   `rgba(84,84,88,0.65)` dark values (Sarunw's cheat sheet), consistent
//!   with iOS 26/27 simplifying the hairline to a flat black/white overlay
//!   rather than a tinted gray; `opaqueSeparator` (`outline`) is the kit's
//!   `Separators/Light|Dark/Opaque` swatch (`#C5C5C7` light, confirmed
//!   `#38383A` dark unchanged) — Apple's flattened opaque variant for
//!   contexts that can't composite translucency.
//! - `systemBackground`/`secondarySystemBackground`/
//!   `tertiarySystemBackground` (mapped here to `surface`/`surface_dim`/
//!   `surface_bright`) are Apple-documented (UIColor system background
//!   docs); refreshed 2026-07-18 against the kit's `Backgrounds/Light|Dark -
//!   Base/Primary|Secondary|Tertiary` swatches (`secondarySystemBackground`
//!   light shifted `#F2F2F7` → `#F1F1F6`; dark's secondary/tertiary shifted
//!   `#1C1C1E`/`#2C2C2E` → `#1B1B1D`/`#2B2B2D`; `systemBackground` itself
//!   confirmed unchanged, opaque white/black). This module combines both the
//!   `*Background` and `*GroupedBackground` triads into a 5-step
//!   `surface_container*` **elevation ladder** to fill M3's
//!   `lowest/low/container/high/highest` slots — iOS itself has no such
//!   explicit 5-step ladder concept (unlike M3's `surfaceContainer*` scale),
//!   so this ordering (progressively more saturated gray moving up the
//!   ladder) is a documented **Frust policy**, not an Apple/community
//!   source, mirroring how [`crate::elevation`]'s shadow math is documented
//!   as tunable Frust policy rather than an Apple spec — the ladder's
//!   intermediate steps are untouched by this refresh (only the ladder's
//!   `lowest`/`bright` endpoints coincide with the literal
//!   `systemBackground`/`tertiarySystemBackground` roles above).
//! - The kit's `Labels - Liquid Glass/Light|Dark/*` swatch group (4 label
//!   tones tuned for legibility over a glass material) is **not** mapped
//!   into `ColorScheme` here, nor are the `Fills - Vibrant`/`Fills` groups —
//!   those are glass-token territory owned by
//!   `GlassScale`, not a `ColorScheme` role; `ColorScheme` stays a flat,
//!   opaque-surface role set as documented above.
//! - M3-only concepts with **no iOS equivalent at all** — the `*_container`
//!   tonal roles, the `*_fixed`/`*_fixed_dim`/`on_*_fixed_variant` roles
//!   (brightness-invariant tones — see M3's own baseline, where e.g.
//!   `primary_fixed` is the identical literal in both
//!   [`ColorScheme::m3_baseline_light`] and
//!   [`ColorScheme::m3_baseline_dark`]), and `inverse_*` — are filled with
//!   **Frust-authored derivations** documented per-field below (a light
//!   tint of the base accent for a "container", the base accent itself for
//!   `on_*_container`, the opposite brightness's base tone for `inverse_*`),
//!   not sourced from any Apple/community iOS reference, since none exists.
//!   `surface_tint` mirrors M3's own convention of reusing `primary` verbatim
//!   (true in both M3 baselines).

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
    /// Replace the accent family from one seed color, Glyph-style.
    ///
    /// Swaps exactly four roles: `primary`, `on_primary`, `primary_container`,
    /// and `on_primary_container`. All other 42 fields remain unchanged.
    ///
    /// - **primary**: set to the provided `accent` color.
    /// - **on_primary**: selected by relative luminance (WCAG contrast ratio ≥
    ///   4.5:1 preferred) — prefer white if the accent is dark enough, else
    ///   dark ink. A simple luminance threshold is used (see docs below).
    /// - **primary_container**: an alpha-wash of the accent at 10% opacity,
    ///   flattened over the scheme's `surface` color to opaque.
    /// - **on_primary_container**: set to the provided `accent` color (same as
    ///   primary, following Material 3 Glyph conventions).
    ///
    /// ## Luminance and Contrast
    ///
    /// `on_primary` is determined by picking the ink that provides better
    /// contrast against the accent. Luminance is calculated via the WCAG
    /// relative luminance formula: `L = 0.2126*R + 0.7152*G + 0.0722*B`
    /// (after linearizing each channel from sRGB). If the accent's luminance
    /// is below 0.5 (midpoint), white is preferred; otherwise, dark ink is
    /// preferred. This simple threshold provides acceptable contrast for most
    /// colors and avoids the overhead of computing the full contrast ratio
    /// (though a more precise contrast-ratio calculation could replace this later).
    ///
    /// ## Works with Any Scheme
    ///
    /// This method works on any `ColorScheme` — Material 3, Cupertino, or
    /// future Glyph baselines — without design-language branching.
    ///
    /// # Example
    ///
    /// ```
    /// use peniko::Color;
    /// use frust_theme::color::ColorScheme;
    ///
    /// let base = ColorScheme::m3_baseline_dark();
    /// let red = Color::from_rgb8(0xFF, 0x00, 0x00);
    /// let accent_scheme = base.with_accent(red);
    ///
    /// assert_eq!(accent_scheme.primary, red);
    /// assert_eq!(accent_scheme.on_primary_container, red);
    /// // All other roles remain unchanged from the base scheme.
    /// ```
    pub fn with_accent(self, accent: Color) -> Self {
        let on_primary = select_ink_by_luminance(accent);
        let primary_container = blend_accent_over_surface(accent, self.surface);

        Self {
            primary: accent,
            on_primary,
            primary_container,
            on_primary_container: accent,
            ..self
        }
    }

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

    /// The Cupertino (iOS) light `ColorScheme`, filling the same 46 M3
    /// roles from Apple's iOS semantic color palette. See the module docs'
    /// "Cupertino (iOS) mapping" section for the full role-by-role source
    /// table and the documented Frust-authored fill-ins for M3 concepts
    /// with no iOS equivalent.
    pub const fn cupertino_light() -> Self {
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

        Self {
            primary: SYSTEM_BLUE_LIGHT,
            on_primary: Color::from_rgb8(0xFF, 0xFF, 0xFF),
            // Frust-derived tinted "container" (iOS has no container
            // role) — a pale tint of systemBlue over systemBackground.
            primary_container: Color::from_rgb8(0xD6, 0xE9, 0xFF),
            on_primary_container: SYSTEM_BLUE_LIGHT,
            // "Fixed" roles are brightness-invariant by M3 definition (see
            // module docs) — the light-mode tone reused verbatim in both
            // cupertino_light() and cupertino_dark().
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
            // background/gray tones (see module docs) — iOS has no native
            // 5-step ladder concept.
            surface_container_lowest: Color::from_rgb8(0xFF, 0xFF, 0xFF),
            surface_container_low: Color::from_rgb8(0xF7, 0xF7, 0xFA),
            surface_container: Color::from_rgb8(0xF2, 0xF2, 0xF7),
            surface_container_high: Color::from_rgb8(0xE5, 0xE5, 0xEA),
            surface_container_highest: Color::from_rgb8(0xD1, 0xD1, 0xD6),

            outline: OPAQUE_SEPARATOR_LIGHT,
            // separator — translucent per its real iOS token; iOS 27 UI Kit
            // `Separators/Light/Non-Opaque`, 2026-07-18 (changed from the
            // pre-refresh `rgba(60,60,67,0.29)` — see module docs).
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

    /// The Cupertino (iOS) dark `ColorScheme` — the dark-mode mirror of
    /// [`ColorScheme::cupertino_light`]; see that constructor's doc comment
    /// and the module docs' "Cupertino (iOS) mapping" section for sources.
    pub const fn cupertino_dark() -> Self {
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
        // 27 UI Kit `Separators/Dark/Opaque`, 2026-07-18 (confirmed
        // unchanged).
        const OPAQUE_SEPARATOR_DARK: Color = Color::from_rgb8(0x38, 0x38, 0x3A);

        Self {
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
            // UI Kit `Separators/Dark/Non-Opaque`, 2026-07-18 (changed from
            // the pre-refresh `rgba(84,84,88,0.65)` — see module docs).
            outline_variant: Color::from_rgba8(0xFF, 0xFF, 0xFF, 31),
            shadow: Color::from_rgb8(0x00, 0x00, 0x00),
            scrim: Color::from_rgb8(0x00, 0x00, 0x00),
            inverse_surface: Color::from_rgb8(0xFF, 0xFF, 0xFF),
            inverse_on_surface: Color::from_rgb8(0x00, 0x00, 0x00),
            inverse_primary: SYSTEM_BLUE_LIGHT,
            surface_tint: SYSTEM_BLUE_DARK,
        }
    }
}

/// Calculate the relative luminance of a color per WCAG standards.
///
/// Luminance = 0.2126*R + 0.7152*G + 0.0722*B, where R, G, B are linearized
/// from sRGB.
fn relative_luminance(color: Color) -> f64 {
    let [r, g, b, _] = color.to_rgba8().to_u8_array();
    let r = linearize_srgb_channel(r as f64 / 255.0);
    let g = linearize_srgb_channel(g as f64 / 255.0);
    let b = linearize_srgb_channel(b as f64 / 255.0);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Linearize an sRGB channel value.
///
/// If the channel is ≤ 0.03928, divide by 12.92; otherwise, raise
/// ((channel + 0.055) / 1.055) to the power of 2.4.
fn linearize_srgb_channel(c: f64) -> f64 {
    if c <= 0.03928 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Select dark ink or white based on the luminance of a given color.
///
/// Returns white if the accent's relative luminance is below 0.5 (dark
/// accent), else returns dark ink. This provides a simple heuristic for
/// contrast without computing the full WCAG contrast ratio.
fn select_ink_by_luminance(color: Color) -> Color {
    if relative_luminance(color) < 0.5 {
        // Dark accent: use white
        Color::from_rgb8(0xFF, 0xFF, 0xFF)
    } else {
        // Light accent: use dark ink
        Color::from_rgb8(0x1D, 0x1B, 0x20)
    }
}

/// Blend an accent color at 10% opacity over a surface color, flattening to opaque.
///
/// Uses the standard alpha-blending formula: result = accent * alpha + surface * (1 - alpha).
/// The alpha is fixed at 0.10 (10%) per the Glyph accent-variant recipe.
fn blend_accent_over_surface(accent: Color, surface: Color) -> Color {
    const ACCENT_ALPHA: f64 = 0.10;

    let [a_r, a_g, a_b, _] = accent.to_rgba8().to_u8_array();
    let [s_r, s_g, s_b, _] = surface.to_rgba8().to_u8_array();

    let a_r = a_r as f64 / 255.0;
    let a_g = a_g as f64 / 255.0;
    let a_b = a_b as f64 / 255.0;
    let s_r = s_r as f64 / 255.0;
    let s_g = s_g as f64 / 255.0;
    let s_b = s_b as f64 / 255.0;

    let blend_channel = |a: f64, s: f64| -> u8 {
        let result = a * ACCENT_ALPHA + s * (1.0 - ACCENT_ALPHA);
        (result * 255.0).round() as u8
    };

    Color::from_rgb8(
        blend_channel(a_r, s_r),
        blend_channel(a_g, s_g),
        blend_channel(a_b, s_b),
    )
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

    #[test]
    fn cupertino_light_spot_check() {
        let s = ColorScheme::cupertino_light();
        // iOS 27 UI Kit `System Colors/Light/8 Blue`, 2026-07-18 refresh.
        assert_eq!(s.primary, Color::from_rgb8(0x00, 0x87, 0xFF));
        assert_eq!(s.surface, Color::from_rgb8(0xFF, 0xFF, 0xFF));
        assert_eq!(s.on_surface, Color::from_rgb8(0x00, 0x00, 0x00));
        // iOS 27 UI Kit `System Colors/Light/1 Red`, 2026-07-18 refresh.
        assert_eq!(s.error, Color::from_rgb8(0xFF, 0x38, 0x3C));
        // iOS 27 UI Kit `Separators/Light/Opaque`, 2026-07-18 refresh.
        assert_eq!(s.outline, Color::from_rgb8(0xC5, 0xC5, 0xC7));
        // iOS 27 UI Kit `System Colors/Light/10 Purple`, 2026-07-18 refresh.
        assert_eq!(s.tertiary, Color::from_rgb8(0xCB, 0x2F, 0xE0));
        // iOS 27 UI Kit `Grays/Light/Gray`, 2026-07-18 refresh.
        assert_eq!(s.secondary, Color::from_rgb8(0x8D, 0x8D, 0x92));
        // iOS 27 UI Kit `Backgrounds/Light - Base/Secondary`, 2026-07-18
        // refresh (secondarySystemBackground).
        assert_eq!(s.surface_dim, Color::from_rgb8(0xF1, 0xF1, 0xF6));
        assert_eq!(s.surface_tint, s.primary);
    }

    #[test]
    fn cupertino_dark_spot_check() {
        let s = ColorScheme::cupertino_dark();
        // iOS 27 UI Kit `System Colors/Dark/8 Blue`, 2026-07-18 refresh.
        assert_eq!(s.primary, Color::from_rgb8(0x00, 0x90, 0xFF));
        assert_eq!(s.surface, Color::from_rgb8(0x00, 0x00, 0x00));
        assert_eq!(s.on_surface, Color::from_rgb8(0xFF, 0xFF, 0xFF));
        // iOS 27 UI Kit `System Colors/Dark/1 Red`, 2026-07-18 refresh.
        assert_eq!(s.error, Color::from_rgb8(0xFF, 0x42, 0x45));
        // iOS 27 UI Kit `Separators/Dark/Opaque`, 2026-07-18 (confirmed
        // unchanged).
        assert_eq!(s.outline, Color::from_rgb8(0x38, 0x38, 0x3A));
        // iOS 27 UI Kit `System Colors/Dark/10 Purple`, 2026-07-18 refresh.
        assert_eq!(s.tertiary, Color::from_rgb8(0xDB, 0x34, 0xF1));
        // iOS 27 UI Kit `Backgrounds/Dark - Base/Tertiary`, 2026-07-18
        // refresh (tertiarySystemBackground, the "brightest" dark surface).
        assert_eq!(s.surface_bright, Color::from_rgb8(0x2B, 0x2B, 0x2D));
        assert_eq!(s.surface_tint, s.primary);
    }

    #[test]
    fn cupertino_translucent_roles_carry_real_alpha() {
        // secondaryLabel/separator are genuinely translucent iOS tokens —
        // unlike every M3 role, these two Cupertino
        // fields intentionally carry a non-1.0 alpha (see module docs). The
        // separator (outline_variant) alpha itself changed in the
        // 2026-07-18 kit refresh (`Separators/Light|Dark/Non-Opaque`:
        // 0.29/0.65 -> 0.12 for both brightnesses).
        let light = ColorScheme::cupertino_light();
        let [.., a] = light.on_surface_variant.to_rgba8().to_u8_array();
        assert_eq!(a, 153);
        let [r, g, b, a] = light.outline_variant.to_rgba8().to_u8_array();
        assert_eq!((r, g, b, a), (0x00, 0x00, 0x00, 31));

        let dark = ColorScheme::cupertino_dark();
        let [.., a] = dark.on_surface_variant.to_rgba8().to_u8_array();
        assert_eq!(a, 153);
        let [r, g, b, a] = dark.outline_variant.to_rgba8().to_u8_array();
        assert_eq!((r, g, b, a), (0xFF, 0xFF, 0xFF, 31));
    }

    #[test]
    fn cupertino_fixed_roles_are_brightness_invariant() {
        // "Fixed" roles must not change between light/dark, mirroring M3's
        // own convention (see module docs).
        let light = ColorScheme::cupertino_light();
        let dark = ColorScheme::cupertino_dark();
        assert_eq!(light.primary_fixed, dark.primary_fixed);
        assert_eq!(light.primary_fixed_dim, dark.primary_fixed_dim);
        assert_eq!(light.on_primary_fixed, dark.on_primary_fixed);
        assert_eq!(
            light.on_primary_fixed_variant,
            dark.on_primary_fixed_variant
        );
        assert_eq!(light.secondary_fixed, dark.secondary_fixed);
        assert_eq!(light.tertiary_fixed, dark.tertiary_fixed);
    }

    #[test]
    fn with_accent_changes_exactly_four_roles() {
        // Verify that with_accent changes exactly the 4 accent roles and
        // leaves all other 42 fields byte-equal.
        let base = ColorScheme::m3_baseline_dark();
        let red = Color::from_rgb8(0xFF, 0x00, 0x00);
        let modified = base.with_accent(red);

        // The four accent roles must change.
        assert_eq!(modified.primary, red);
        assert_eq!(modified.on_primary_container, red);
        // on_primary and primary_container are derived; check they're not the
        // same as the original.
        assert_ne!(modified.on_primary, base.on_primary);
        assert_ne!(modified.primary_container, base.primary_container);

        // All other 42 roles must remain unchanged.
        assert_eq!(modified.secondary, base.secondary);
        assert_eq!(modified.on_secondary, base.on_secondary);
        assert_eq!(modified.secondary_container, base.secondary_container);
        assert_eq!(modified.on_secondary_container, base.on_secondary_container);
        assert_eq!(modified.secondary_fixed, base.secondary_fixed);
        assert_eq!(modified.secondary_fixed_dim, base.secondary_fixed_dim);
        assert_eq!(modified.on_secondary_fixed, base.on_secondary_fixed);
        assert_eq!(
            modified.on_secondary_fixed_variant,
            base.on_secondary_fixed_variant
        );

        assert_eq!(modified.tertiary, base.tertiary);
        assert_eq!(modified.on_tertiary, base.on_tertiary);
        assert_eq!(modified.tertiary_container, base.tertiary_container);
        assert_eq!(modified.on_tertiary_container, base.on_tertiary_container);
        assert_eq!(modified.tertiary_fixed, base.tertiary_fixed);
        assert_eq!(modified.tertiary_fixed_dim, base.tertiary_fixed_dim);
        assert_eq!(modified.on_tertiary_fixed, base.on_tertiary_fixed);
        assert_eq!(
            modified.on_tertiary_fixed_variant,
            base.on_tertiary_fixed_variant
        );

        assert_eq!(modified.error, base.error);
        assert_eq!(modified.on_error, base.on_error);
        assert_eq!(modified.error_container, base.error_container);
        assert_eq!(modified.on_error_container, base.on_error_container);

        assert_eq!(modified.surface, base.surface);
        assert_eq!(modified.on_surface, base.on_surface);
        assert_eq!(modified.on_surface_variant, base.on_surface_variant);
        assert_eq!(modified.surface_dim, base.surface_dim);
        assert_eq!(modified.surface_bright, base.surface_bright);
        assert_eq!(
            modified.surface_container_lowest,
            base.surface_container_lowest
        );
        assert_eq!(modified.surface_container_low, base.surface_container_low);
        assert_eq!(modified.surface_container, base.surface_container);
        assert_eq!(modified.surface_container_high, base.surface_container_high);
        assert_eq!(
            modified.surface_container_highest,
            base.surface_container_highest
        );

        assert_eq!(modified.outline, base.outline);
        assert_eq!(modified.outline_variant, base.outline_variant);
        assert_eq!(modified.shadow, base.shadow);
        assert_eq!(modified.scrim, base.scrim);
        assert_eq!(modified.inverse_surface, base.inverse_surface);
        assert_eq!(modified.inverse_on_surface, base.inverse_on_surface);
        assert_eq!(modified.inverse_primary, base.inverse_primary);
        assert_eq!(modified.surface_tint, base.surface_tint);

        assert_eq!(modified.primary_fixed, base.primary_fixed);
        assert_eq!(modified.primary_fixed_dim, base.primary_fixed_dim);
        assert_eq!(modified.on_primary_fixed, base.on_primary_fixed);
        assert_eq!(
            modified.on_primary_fixed_variant,
            base.on_primary_fixed_variant
        );
    }

    #[test]
    fn on_primary_contrast_for_dark_and_light_accents() {
        // Verify on_primary is selected appropriately for both dark and light
        // accents.
        let base = ColorScheme::m3_baseline_dark();

        // Dark accent (red): should get white on_primary.
        let dark_accent = Color::from_rgb8(0x80, 0x00, 0x00);
        let dark_scheme = base.with_accent(dark_accent);
        assert_eq!(
            dark_scheme.on_primary,
            Color::from_rgb8(0xFF, 0xFF, 0xFF),
            "Dark accent should pair with white ink"
        );

        // Light accent (yellow): should get dark ink on_primary.
        let light_accent = Color::from_rgb8(0xFF, 0xFF, 0x00);
        let light_scheme = base.with_accent(light_accent);
        assert_eq!(
            light_scheme.on_primary,
            Color::from_rgb8(0x1D, 0x1B, 0x20),
            "Light accent should pair with dark ink"
        );
    }
}
