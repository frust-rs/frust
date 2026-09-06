//! The Sample design language's token layer: a [`Theme`] built from
//! [`Theme::neutral`] plus one typed extension, [`SampleAccents`].
//!
//! # Why `Theme::neutral()` is the baseline
//!
//! `Theme::m3_baseline()`/`cupertino_baseline()`/`glyph_baseline()` are the
//! built-in design languages' own starting points; a third-party system that
//! seeded one of those would inherit that language's shape/motion opinions and
//! then have to overwrite them. [`Theme::neutral`] is the design-language-free
//! bundle every shell already falls back to, so building from it means every
//! token this module does *not* set is a deliberate "no opinion", not an
//! inherited Material one. (`Theme::glyph_baseline` is additionally
//! unreachable here by construction — it lives behind `frust`'s `glyph`
//! feature, which this workspace has off.)
//!
//! # The palette
//!
//! Two hues, deliberately not Material's: a desaturated **teal** accent and a
//! warm **clay** highlight, over near-neutral surfaces. The values are
//! hand-authored for this sample — there is no external design source to cite,
//! and nothing here is trying to be a shippable palette.
//!
//! # `SampleAccents`: the typed extension
//!
//! `ColorScheme` has no role for "the hue a badge/chip uses to mark itself as
//! *this* design system's" — `primary` is the accent *ink*,
//! `primary_container` the bright fill (conflating the two is the built-in
//! Glyph catalog's most common accent bug). Rather than smuggle a third
//! meaning into an existing role, the highlight hue and the hairline-rule ink
//! ride a typed [`ThemeExtensions`](frust::ThemeExtensions) attachment, the
//! same no-lock-in slot `StatusPalette` and Glyph's `GlyphInk` use.
//!
//! Consumers resolve it with the framework's documented precedence — **explicit
//! builder value > theme extension > fallback constant** — see
//! [`SampleAccents::resolve_highlight`], which is the one place that ladder is
//! written down for this system.

use frust::{
    Brightness, Color, ColorScheme, DesignLanguage, MotionScheme, ShapeScale, Theme, ThemeBuilder,
};

/// This design system's stable identity tag, carried on
/// [`Theme::design_language`] as [`DesignLanguage::Custom`].
///
/// A host or widget that branches on design language sees this id rather than
/// mistaking the system for Material; the built-in `==` branch sites treat any
/// `Custom` as the neutral/System path, which is exactly what a system with no
/// platform-native counterpart wants.
pub const SAMPLE_DESIGN_LANGUAGE: &str = "sample";

// ---- The two hues ---------------------------------------------------------

/// The accent ink, light mode: a desaturated teal.
const TEAL_INK_LIGHT: Color = Color::from_rgb8(0x0F, 0x6B, 0x63);
/// The accent ink, dark mode (lifted for contrast against a dark surface).
const TEAL_INK_DARK: Color = Color::from_rgb8(0x5E, 0xC8, 0xBD);
/// The bright accent fill, light mode.
const TEAL_FILL_LIGHT: Color = Color::from_rgb8(0x0B, 0x4F, 0x4A);
/// The bright accent fill, dark mode.
const TEAL_FILL_DARK: Color = Color::from_rgb8(0x11, 0x3B, 0x38);
/// The second hue: a warm clay, used only through [`SampleAccents`].
const CLAY_LIGHT: Color = Color::from_rgb8(0xB5, 0x53, 0x2E);
/// The second hue, dark mode.
const CLAY_DARK: Color = Color::from_rgb8(0xE8, 0x8B, 0x63);

/// Surface, light mode: warm off-white rather than Material's pure-ish white.
const SURFACE_LIGHT: Color = Color::from_rgb8(0xFA, 0xF7, 0xF2);
/// Raised surface, light mode.
const SURFACE_RAISED_LIGHT: Color = Color::from_rgb8(0xF1, 0xEC, 0xE3);
/// Body ink, light mode.
const ON_SURFACE_LIGHT: Color = Color::from_rgb8(0x1E, 0x1C, 0x19);
/// Muted ink, light mode.
const ON_SURFACE_MUTED_LIGHT: Color = Color::from_rgb8(0x60, 0x5B, 0x53);
/// Hairline rule, light mode.
const OUTLINE_LIGHT: Color = Color::from_rgb8(0xD8, 0xD1, 0xC5);

/// Surface, dark mode.
const SURFACE_DARK: Color = Color::from_rgb8(0x14, 0x16, 0x16);
/// Raised surface, dark mode.
const SURFACE_RAISED_DARK: Color = Color::from_rgb8(0x1D, 0x21, 0x21);
/// Body ink, dark mode.
const ON_SURFACE_DARK: Color = Color::from_rgb8(0xEC, 0xE7, 0xDF);
/// Muted ink, dark mode.
const ON_SURFACE_MUTED_DARK: Color = Color::from_rgb8(0x9B, 0x99, 0x92);
/// Hairline rule, dark mode.
const OUTLINE_DARK: Color = Color::from_rgb8(0x33, 0x38, 0x38);

// ---- The shape opinion ----------------------------------------------------

/// The system's one shape opinion: a small, uniform corner radius across the
/// whole scale (a "flat, squared-off" look), in logical px. `full` is left at
/// its baseline `f64::INFINITY` — it is the pill sentinel every consumer
/// resolves against a box, not a radius.
const SAMPLE_RADIUS: f64 = 3.0;

// ---- The typed extension --------------------------------------------------

/// Sample's typed theme extension: the tokens `ColorScheme` has no role for.
///
/// Attached by [`sample_theme`]; recovered by a widget with
/// `theme.extension::<SampleAccents>()`. A `Theme` is cloned across the
/// framework's two delivery paths, so the extension slot requires
/// `Any + Send + Sync` — this struct is plain `Copy` data and satisfies that
/// for free.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampleAccents {
    /// The highlight hue (the second of the system's two hues), light mode.
    pub highlight_light: Color,
    /// The highlight hue, dark mode.
    pub highlight_dark: Color,
}

/// The highlight's unthemed fallback, used when no theme is threaded into the
/// pass at all *and* no explicit value was given — the bottom rung of the
/// precedence ladder ([`SampleAccents::resolve_highlight`]).
pub const FALLBACK_HIGHLIGHT: Color = CLAY_LIGHT;

impl SampleAccents {
    /// This system's own accents.
    pub const fn sample() -> Self {
        Self {
            highlight_light: CLAY_LIGHT,
            highlight_dark: CLAY_DARK,
        }
    }

    /// The highlight hue for `brightness`.
    pub fn highlight(&self, brightness: Brightness) -> Color {
        match brightness {
            Brightness::Light => self.highlight_light,
            Brightness::Dark => self.highlight_dark,
        }
    }

    /// Resolve a highlight color under the framework's documented precedence:
    /// **explicit builder value > theme extension > fallback constant**.
    ///
    /// This is the one place the ladder is spelled out for this design system;
    /// every widget that needs a highlight calls through here rather than
    /// re-deriving it, so a consumer reading two widgets never finds two
    /// different precedence orders.
    ///
    /// The middle rung is deliberately *not* "theme, always": a theme with the
    /// extension cleared (an app is free to build one) falls straight through
    /// to [`FALLBACK_HIGHLIGHT`] rather than panicking or painting nothing.
    pub fn resolve_highlight(explicit: Option<Color>, theme: Option<&Theme>) -> Color {
        if let Some(color) = explicit {
            return color;
        }
        match theme.and_then(|t| t.extension::<SampleAccents>().map(|a| (a, t.brightness))) {
            Some((accents, brightness)) => accents.highlight(brightness),
            None => FALLBACK_HIGHLIGHT,
        }
    }
}

// ---- The theme ------------------------------------------------------------

/// Light-mode color roles.
fn sample_light() -> ColorScheme {
    ColorScheme {
        primary: TEAL_INK_LIGHT,
        on_primary: Color::from_rgb8(0xFF, 0xFF, 0xFF),
        primary_container: TEAL_FILL_LIGHT,
        on_primary_container: Color::from_rgb8(0xE6, 0xF4, 0xF2),
        surface: SURFACE_LIGHT,
        surface_container: SURFACE_RAISED_LIGHT,
        surface_container_low: SURFACE_LIGHT,
        surface_container_high: SURFACE_RAISED_LIGHT,
        on_surface: ON_SURFACE_LIGHT,
        on_surface_variant: ON_SURFACE_MUTED_LIGHT,
        outline: OUTLINE_LIGHT,
        outline_variant: OUTLINE_LIGHT,
        ..ColorScheme::neutral_light()
    }
}

/// Dark-mode color roles.
fn sample_dark() -> ColorScheme {
    ColorScheme {
        primary: TEAL_INK_DARK,
        on_primary: Color::from_rgb8(0x04, 0x1B, 0x19),
        primary_container: TEAL_FILL_DARK,
        on_primary_container: TEAL_INK_DARK,
        surface: SURFACE_DARK,
        surface_container: SURFACE_RAISED_DARK,
        surface_container_low: SURFACE_DARK,
        surface_container_high: SURFACE_RAISED_DARK,
        on_surface: ON_SURFACE_DARK,
        on_surface_variant: ON_SURFACE_MUTED_DARK,
        outline: OUTLINE_DARK,
        outline_variant: OUTLINE_DARK,
        ..ColorScheme::neutral_dark()
    }
}

/// Build the Sample design system's [`Theme`].
///
/// Composed the way a third-party system is meant to compose one — a
/// [`ThemeBuilder`] over a baseline, layering per-group edits — rather than by
/// filling a `Theme` literal field by field:
///
/// 1. baseline: [`Theme::neutral`] (see the module docs for why).
/// 2. `map_colors_light`/`map_colors_dark`: the two-hue palette.
/// 3. `map_shape`: one uniform small radius across the scale.
/// 4. `map_motion`: a slightly quicker-than-baseline duration set.
/// 5. `design_language`: the [`DesignLanguage::Custom`] identity tag.
/// 6. `extension`: [`SampleAccents`].
///
/// Brightness is left at the baseline's own value on purpose: the shell
/// re-derives light/dark from the platform against whatever base
/// [`crate::install`] seeded, and pinning it here would fight that.
pub fn sample_theme() -> Theme {
    ThemeBuilder::new(Theme::neutral())
        .map_colors_light(|_| sample_light())
        .map_colors_dark(|_| sample_dark())
        .map_shape(|s| ShapeScale {
            extra_small: SAMPLE_RADIUS,
            small: SAMPLE_RADIUS,
            medium: SAMPLE_RADIUS,
            large: SAMPLE_RADIUS,
            large_increased: SAMPLE_RADIUS,
            extra_large: SAMPLE_RADIUS,
            extra_large_increased: SAMPLE_RADIUS,
            extra_extra_large: SAMPLE_RADIUS,
            ..s
        })
        .map_motion(|m| MotionScheme {
            durations: frust::MotionDurations {
                instant: 60.0,
                fast: 120.0,
                base: 180.0,
                slow: 260.0,
                deliberate: 380.0,
            },
            ..m
        })
        .design_language(DesignLanguage::Custom(SAMPLE_DESIGN_LANGUAGE))
        .extension(SampleAccents::sample())
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_carries_the_custom_design_language_tag() {
        let theme = sample_theme();
        assert_eq!(
            theme.design_language,
            DesignLanguage::Custom(SAMPLE_DESIGN_LANGUAGE)
        );
        // Never mistakeable for a built-in language.
        assert_ne!(theme.design_language, DesignLanguage::Material3);
        assert_ne!(theme.design_language, DesignLanguage::Cupertino);
    }

    #[test]
    fn theme_carries_both_hues_in_both_brightnesses() {
        let theme = sample_theme();
        assert_eq!(theme.light.primary, TEAL_INK_LIGHT);
        assert_eq!(theme.dark.primary, TEAL_INK_DARK);
        let accents = theme
            .extension::<SampleAccents>()
            .expect("sample_theme attaches SampleAccents");
        assert_eq!(accents.highlight(Brightness::Light), CLAY_LIGHT);
        assert_eq!(accents.highlight(Brightness::Dark), CLAY_DARK);
    }

    #[test]
    fn shape_is_flattened_to_one_radius_but_full_stays_the_pill_sentinel() {
        let theme = sample_theme();
        assert_eq!(theme.shape.small, SAMPLE_RADIUS);
        assert_eq!(theme.shape.extra_large, SAMPLE_RADIUS);
        assert!(theme.shape.full.is_infinite());
    }

    #[test]
    fn highlight_precedence_is_explicit_then_extension_then_fallback() {
        let explicit = Color::from_rgb8(0x00, 0x00, 0xFF);
        let theme = sample_theme();

        // 1. explicit wins over everything.
        assert_eq!(
            SampleAccents::resolve_highlight(Some(explicit), Some(&theme)),
            explicit
        );
        // 2. no explicit value: the theme extension wins over the fallback.
        assert_eq!(
            SampleAccents::resolve_highlight(None, Some(&theme)),
            CLAY_LIGHT
        );
        // 3. no theme at all: the fallback constant.
        assert_eq!(
            SampleAccents::resolve_highlight(None, None),
            FALLBACK_HIGHLIGHT
        );
    }

    #[test]
    fn a_theme_with_the_extension_cleared_falls_through_to_the_fallback() {
        // An app is free to build a theme that carries no SampleAccents; the
        // resolver must degrade, not panic.
        let bare = Theme::neutral();
        assert!(bare.extension::<SampleAccents>().is_none());
        assert_eq!(
            SampleAccents::resolve_highlight(None, Some(&bare)),
            FALLBACK_HIGHLIGHT
        );
    }
}
