//! Color roles: [`ColorScheme`] (46 roles) and the [`Brightness`] a
//! [`crate::theme::Theme`] selects one by.
//!
//! The role SET is Material 3's — 46 non-deprecated roles, the vocabulary
//! every design system's table fills — but no Material or Cupertino *values*
//! live here any more: those tables moved out with their design systems
//! (`frust-material`, `frust-cupertino`), which fill these same fields from
//! their own sources. What remains is the role struct, the
//! [`ColorScheme::with_accent`] recolor helper, and
//! [`ColorScheme::neutral_light`]/[`ColorScheme::neutral_dark`] — the
//! language-free ramp [`crate::theme::Theme::neutral`] composes.
//!
//! `background`/`onBackground`/`surfaceVariant` are **not implemented** —
//! Material 3 deprecated all three in favor of `surface`/`onSurface` and the
//! `surfaceContainer*` ladder respectively; carrying them forward would just
//! duplicate an existing role under a legacy name.
//!
//! A role whose real token is translucent may carry a non-opaque `Color`: the
//! struct stores whatever a design system's source value is, and flattening a
//! genuinely translucent token to opaque would misrepresent it.

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

/// A full set of color roles for one brightness (light or dark), on Material
/// 3's role vocabulary.
///
/// 46 non-deprecated roles (see module docs for the deprecated 3). This crate
/// constructs one table, the language-free
/// [`ColorScheme::neutral_light`]/[`ColorScheme::neutral_dark`] pair; a
/// design system fills the same fields from its own source values in its own
/// crate. Fields are normally opaque [`Color`]s — channel-appropriate
/// translucency (e.g. for scrims) is applied by the widget/scene layer, not
/// stored here — but a design system whose real token is itself translucent
/// may store the non-opaque value rather than misrepresent it.
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
    ///   primary).
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
    /// This method works on any `ColorScheme` — the neutral ramp below or any
    /// design system's own table — without design-language branching.
    ///
    /// # Example
    ///
    /// ```
    /// use peniko::Color;
    /// use frust_theme::color::ColorScheme;
    ///
    /// let base = ColorScheme::neutral_dark();
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

    /// The neutral, design-language-free light `ColorScheme`: a plain
    /// achromatic surface/on-surface ramp, plus **one** restrained
    /// slate-blue accent filling the `primary` family — every other role
    /// (`secondary`/`tertiary`) stays gray too, since "neutral" means one
    /// accent, not a second or third hue. See
    /// [`crate::theme::Theme::neutral`]'s module docs for the design intent
    /// ("deliberately plain, not half-finished").
    ///
    /// **Frust-authored, not sourced from any third-party design system** —
    /// there is no external spec to
    /// cite here; values are chosen for legible contrast against their
    /// paired ink role (see [`ColorScheme::neutral_dark`]'s doc comment for
    /// the contrast rationale, verified by
    /// `neutral_surface_on_surface_contrast_is_legible` in this module's
    /// tests). `error`/`on_error`/`error_container`/`on_error_container`
    /// are the one exception kept in real red — a functional/semantic signal
    /// (danger/destructive state), not a "look", the same reasoning
    /// [`crate::theme::Theme::neutral`] uses to still attach
    /// [`crate::status::StatusPalette::neutral`] rather than a grayscale
    /// status set.
    pub const fn neutral_light() -> Self {
        // A single restrained slate-blue accent (`primary` family only).
        const ACCENT_LIGHT: Color = Color::from_rgb8(0x3D, 0x5A, 0x73);
        const ACCENT_DARK: Color = Color::from_rgb8(0x8F, 0xB4, 0xD1);

        Self {
            primary: ACCENT_LIGHT,
            on_primary: Color::from_rgb8(0xFF, 0xFF, 0xFF),
            primary_container: Color::from_rgb8(0xE3, 0xE9, 0xEE),
            on_primary_container: Color::from_rgb8(0x1B, 0x2A, 0x35),
            // "Fixed" roles are brightness-invariant by M3 definition —
            // the same literal in both `neutral_light`/`neutral_dark`.
            primary_fixed: ACCENT_LIGHT,
            primary_fixed_dim: ACCENT_DARK,
            on_primary_fixed: Color::from_rgb8(0xFF, 0xFF, 0xFF),
            on_primary_fixed_variant: Color::from_rgb8(0x2E, 0x47, 0x59),

            // Plain gray — "one restrained accent" means the primary family
            // above, not a second hue.
            secondary: Color::from_rgb8(0x6B, 0x6B, 0x6B),
            on_secondary: Color::from_rgb8(0xFF, 0xFF, 0xFF),
            secondary_container: Color::from_rgb8(0xE4, 0xE4, 0xE4),
            on_secondary_container: Color::from_rgb8(0x2B, 0x2B, 0x2B),
            secondary_fixed: Color::from_rgb8(0xE4, 0xE4, 0xE4),
            secondary_fixed_dim: Color::from_rgb8(0xC7, 0xC7, 0xC7),
            on_secondary_fixed: Color::from_rgb8(0x2B, 0x2B, 0x2B),
            on_secondary_fixed_variant: Color::from_rgb8(0x4A, 0x4A, 0x4A),

            // Plain gray — a third hue would defeat the "one accent" floor.
            tertiary: Color::from_rgb8(0x4A, 0x4A, 0x4A),
            on_tertiary: Color::from_rgb8(0xFF, 0xFF, 0xFF),
            tertiary_container: Color::from_rgb8(0xD6, 0xD6, 0xD6),
            on_tertiary_container: Color::from_rgb8(0x26, 0x26, 0x26),
            tertiary_fixed: Color::from_rgb8(0xD6, 0xD6, 0xD6),
            tertiary_fixed_dim: Color::from_rgb8(0xB8, 0xB8, 0xB8),
            on_tertiary_fixed: Color::from_rgb8(0x26, 0x26, 0x26),
            on_tertiary_fixed_variant: Color::from_rgb8(0x3D, 0x3D, 0x3D),

            // Kept real red — see this constructor's doc comment. The
            // Material 3 baseline light `error` family, verbatim.
            error: Color::from_rgb8(0xB3, 0x26, 0x1E),
            on_error: Color::from_rgb8(0xFF, 0xFF, 0xFF),
            error_container: Color::from_rgb8(0xF9, 0xDE, 0xDC),
            on_error_container: Color::from_rgb8(0x41, 0x0E, 0x0B),

            surface: Color::from_rgb8(0xFA, 0xFA, 0xFA),
            on_surface: Color::from_rgb8(0x1A, 0x1A, 0x1A),
            on_surface_variant: Color::from_rgb8(0x5C, 0x5C, 0x5C),
            surface_dim: Color::from_rgb8(0xE8, 0xE8, 0xE8),
            surface_bright: Color::from_rgb8(0xFF, 0xFF, 0xFF),
            surface_container_lowest: Color::from_rgb8(0xFF, 0xFF, 0xFF),
            surface_container_low: Color::from_rgb8(0xF5, 0xF5, 0xF5),
            surface_container: Color::from_rgb8(0xEF, 0xEF, 0xEF),
            surface_container_high: Color::from_rgb8(0xE7, 0xE7, 0xE7),
            surface_container_highest: Color::from_rgb8(0xDF, 0xDF, 0xDF),

            outline: Color::from_rgb8(0x8A, 0x8A, 0x8A),
            outline_variant: Color::from_rgb8(0xD0, 0xD0, 0xD0),
            shadow: Color::from_rgb8(0x00, 0x00, 0x00),
            scrim: Color::from_rgb8(0x00, 0x00, 0x00),
            inverse_surface: Color::from_rgb8(0x2B, 0x2B, 0x2B),
            inverse_on_surface: Color::from_rgb8(0xF5, 0xF5, 0xF5),
            inverse_primary: ACCENT_DARK,
            // Mirrors M3's own convention: surface_tint == primary.
            surface_tint: ACCENT_LIGHT,
        }
    }

    /// The neutral, design-language-free dark `ColorScheme` — the dark-mode
    /// mirror of [`ColorScheme::neutral_light`]; see that constructor's doc
    /// comment for the design rationale.
    ///
    /// Contrast check (verified in this module's tests): `surface`
    /// (`#121212`) against `on_surface` (`#F2F2F2`) is ~16.7:1, and
    /// [`ColorScheme::neutral_light`]'s equivalent pair is ~16.7:1 too —
    /// both far past the WCAG AA 4.5:1 normal-text floor.
    pub const fn neutral_dark() -> Self {
        const ACCENT_LIGHT: Color = Color::from_rgb8(0x3D, 0x5A, 0x73);
        const ACCENT_DARK: Color = Color::from_rgb8(0x8F, 0xB4, 0xD1);

        Self {
            primary: ACCENT_DARK,
            on_primary: Color::from_rgb8(0x16, 0x23, 0x2C),
            primary_container: Color::from_rgb8(0x22, 0x34, 0x41),
            on_primary_container: Color::from_rgb8(0xC7, 0xDC, 0xEA),
            primary_fixed: ACCENT_LIGHT,
            primary_fixed_dim: ACCENT_DARK,
            on_primary_fixed: Color::from_rgb8(0xFF, 0xFF, 0xFF),
            on_primary_fixed_variant: Color::from_rgb8(0x2E, 0x47, 0x59),

            secondary: Color::from_rgb8(0xB0, 0xB0, 0xB0),
            on_secondary: Color::from_rgb8(0x1E, 0x1E, 0x1E),
            secondary_container: Color::from_rgb8(0x3A, 0x3A, 0x3A),
            on_secondary_container: Color::from_rgb8(0xE4, 0xE4, 0xE4),
            secondary_fixed: Color::from_rgb8(0xE4, 0xE4, 0xE4),
            secondary_fixed_dim: Color::from_rgb8(0xC7, 0xC7, 0xC7),
            on_secondary_fixed: Color::from_rgb8(0x2B, 0x2B, 0x2B),
            on_secondary_fixed_variant: Color::from_rgb8(0x4A, 0x4A, 0x4A),

            tertiary: Color::from_rgb8(0xB8, 0xB8, 0xB8),
            on_tertiary: Color::from_rgb8(0x1E, 0x1E, 0x1E),
            tertiary_container: Color::from_rgb8(0x38, 0x38, 0x38),
            on_tertiary_container: Color::from_rgb8(0xD6, 0xD6, 0xD6),
            tertiary_fixed: Color::from_rgb8(0xD6, 0xD6, 0xD6),
            tertiary_fixed_dim: Color::from_rgb8(0xB8, 0xB8, 0xB8),
            on_tertiary_fixed: Color::from_rgb8(0x26, 0x26, 0x26),
            on_tertiary_fixed_variant: Color::from_rgb8(0x3D, 0x3D, 0x3D),

            // Kept real red — see `neutral_light`'s doc comment. The
            // Material 3 baseline dark `error` family, verbatim.
            error: Color::from_rgb8(0xF2, 0xB8, 0xB5),
            on_error: Color::from_rgb8(0x60, 0x14, 0x10),
            error_container: Color::from_rgb8(0x8C, 0x1D, 0x18),
            on_error_container: Color::from_rgb8(0xF9, 0xDE, 0xDC),

            surface: Color::from_rgb8(0x12, 0x12, 0x12),
            on_surface: Color::from_rgb8(0xF2, 0xF2, 0xF2),
            on_surface_variant: Color::from_rgb8(0xB0, 0xB0, 0xB0),
            surface_dim: Color::from_rgb8(0x12, 0x12, 0x12),
            surface_bright: Color::from_rgb8(0x38, 0x38, 0x38),
            surface_container_lowest: Color::from_rgb8(0x0B, 0x0B, 0x0B),
            surface_container_low: Color::from_rgb8(0x1C, 0x1C, 0x1C),
            surface_container: Color::from_rgb8(0x20, 0x20, 0x20),
            surface_container_high: Color::from_rgb8(0x2A, 0x2A, 0x2A),
            surface_container_highest: Color::from_rgb8(0x35, 0x35, 0x35),

            outline: Color::from_rgb8(0x8A, 0x8A, 0x8A),
            outline_variant: Color::from_rgb8(0x47, 0x47, 0x47),
            shadow: Color::from_rgb8(0x00, 0x00, 0x00),
            scrim: Color::from_rgb8(0x00, 0x00, 0x00),
            inverse_surface: Color::from_rgb8(0xE6, 0xE6, 0xE6),
            inverse_on_surface: Color::from_rgb8(0x2B, 0x2B, 0x2B),
            inverse_primary: ACCENT_LIGHT,
            surface_tint: ACCENT_DARK,
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
    fn deprecated_roles_absent() {
        // background/onBackground/surfaceVariant are not fields on
        // ColorScheme at all — this test exists as a documentation anchor,
        // not a runtime check (a missing field is a compile error, not a
        // test failure). See module docs for why they're omitted.
        let _ = ColorScheme::neutral_light();
    }

    #[test]
    fn with_accent_changes_exactly_four_roles() {
        // Verify that with_accent changes exactly the 4 accent roles and
        // leaves all other 42 fields byte-equal.
        let base = ColorScheme::neutral_dark();
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
        let base = ColorScheme::neutral_dark();

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

    // ---- Neutral baseline -------------------------------------------------

    /// WCAG contrast ratio between two colors, per the standard `(L1 +
    /// 0.05) / (L2 + 0.05)` formula (`L1` the lighter of the pair).
    fn contrast_ratio(a: Color, b: Color) -> f64 {
        let la = relative_luminance(a);
        let lb = relative_luminance(b);
        let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn neutral_surface_on_surface_contrast_is_legible() {
        // Both brightnesses clear the WCAG AA normal-text floor (4.5:1) for
        // the surface/on-surface pair.
        const AA_NORMAL_TEXT: f64 = 4.5;

        let light = ColorScheme::neutral_light();
        assert!(
            contrast_ratio(light.surface, light.on_surface) >= AA_NORMAL_TEXT,
            "neutral_light surface/on_surface contrast too low"
        );

        let dark = ColorScheme::neutral_dark();
        assert!(
            contrast_ratio(dark.surface, dark.on_surface) >= AA_NORMAL_TEXT,
            "neutral_dark surface/on_surface contrast too low"
        );
    }

    #[test]
    fn neutral_light_and_dark_are_distinct() {
        let light = ColorScheme::neutral_light();
        let dark = ColorScheme::neutral_dark();
        assert_ne!(light.surface, dark.surface);
        assert_ne!(light.on_surface, dark.on_surface);
        assert_ne!(light.primary, dark.primary);
    }

    #[test]
    fn neutral_carries_exactly_one_accent_family() {
        // "One restrained accent": secondary/tertiary stay achromatic
        // (r == g == b) in both brightnesses, unlike primary.
        fn is_achromatic(c: Color) -> bool {
            let [r, g, b, _] = c.to_rgba8().to_u8_array();
            r == g && g == b
        }

        for scheme in [ColorScheme::neutral_light(), ColorScheme::neutral_dark()] {
            assert!(!is_achromatic(scheme.primary), "primary must be the accent");
            assert!(is_achromatic(scheme.secondary));
            assert!(is_achromatic(scheme.tertiary));
            assert!(is_achromatic(scheme.surface));
            assert!(is_achromatic(scheme.on_surface));
        }
    }

    #[test]
    fn neutral_fixed_roles_are_brightness_invariant() {
        let light = ColorScheme::neutral_light();
        let dark = ColorScheme::neutral_dark();
        assert_eq!(light.primary_fixed, dark.primary_fixed);
        assert_eq!(light.primary_fixed_dim, dark.primary_fixed_dim);
        assert_eq!(
            light.on_primary_fixed_variant,
            dark.on_primary_fixed_variant
        );
        assert_eq!(light.secondary_fixed, dark.secondary_fixed);
        assert_eq!(light.tertiary_fixed, dark.tertiary_fixed);
    }

    #[test]
    fn neutral_is_const_constructible() {
        const LIGHT: ColorScheme = ColorScheme::neutral_light();
        const DARK: ColorScheme = ColorScheme::neutral_dark();
        assert_ne!(LIGHT.surface, DARK.surface);
    }
}
