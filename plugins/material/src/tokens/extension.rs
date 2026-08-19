//! [`MaterialTokens`]: the typed `ThemeExtensions` payload carrying the nine
//! M3E color roles `ColorScheme` has no field for — `emphasis`/`onEmphasis`,
//! `info`, `success`, `warning`, `danger`, `surfaceStrong`/`onSurfaceStrong`,
//! and `outlineStrong`. See [`crate::tokens`]'s module docs for the full
//! 43-role partition table and [`crate::tokens::color`]'s module docs for how
//! each field here is sourced.
//!
//! Mirrors `frust-shadcn`'s `ShadcnTokens` shape: a `Clone + Debug +
//! PartialEq` payload carrying both brightnesses at once (a design-system
//! theme may flip brightness live without rebuilding), attached by
//! [`super::baseline`] via the framework's `ThemeExtensions` bus, with a
//! brightness-aware getter ([`MaterialTokens::colors`]) and the framework's
//! documented resolution ladder — **explicit builder value > theme
//! extension > fallback constant** — spelled out here for
//! [`resolve_emphasis`](MaterialTokens::resolve_emphasis) and
//! [`resolve_danger`](MaterialTokens::resolve_danger), the two roles a
//! component is most likely to need a direct override for (an accent-style
//! role and a semantic-alert role, respectively). A future component wanting
//! the ladder for another field follows the same two-rung shape these two
//! establish.

use frust::{Brightness, Color, Theme};

use super::color::{MaterialSemanticColors, semantic_dark, semantic_light};

/// The unthemed fallback for [`MaterialTokens::resolve_emphasis`] — the
/// light baseline's `emphasis` role (== `primary`, seed `#6750A4`).
///
/// The bottom rung of that method's ladder — used when no theme is threaded
/// into the pass *and* no explicit value was given.
pub const FALLBACK_EMPHASIS: Color = Color::from_rgb8(0x67, 0x50, 0xA4);

/// The unthemed fallback for [`MaterialTokens::resolve_danger`] — the light
/// baseline's `danger` role (== `error`, the M3 baseline light error tone).
pub const FALLBACK_DANGER: Color = Color::from_rgb8(0xB3, 0x26, 0x1E);

/// The Material 3 Expressive semantic-role theme extension:
/// [`MaterialSemanticColors`] for both brightnesses at once, exactly like
/// [`frust::Theme`]'s own `light`/`dark` pair. Recovered from a themed app
/// with `theme.extension::<MaterialTokens>()`.
///
/// A `Theme` is cloned across the framework's two delivery paths, so an
/// extension must be `Any + Send + Sync` — this is plain `Color`-only data
/// and satisfies that for free.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialTokens {
    /// The nine semantic roles, light mode.
    pub light: MaterialSemanticColors,
    /// The nine semantic roles, dark mode.
    pub dark: MaterialSemanticColors,
}

impl MaterialTokens {
    /// The Material 3 Expressive baseline extension (seed `#6750A4`) — what
    /// [`super::baseline`] attaches.
    pub fn material() -> Self {
        Self {
            light: semantic_light(),
            dark: semantic_dark(),
        }
    }

    /// The semantic-role table for `brightness` — mirrors
    /// [`frust::Theme::scheme`]'s light/dark selector.
    pub fn colors(&self, brightness: Brightness) -> &MaterialSemanticColors {
        match brightness {
            Brightness::Light => &self.light,
            Brightness::Dark => &self.dark,
        }
    }

    /// Resolve the `emphasis` role under the framework's documented
    /// precedence: **explicit builder value > theme extension > fallback
    /// constant**. The theme rung reads the extension *and* the theme's own
    /// brightness, so `emphasis` follows a live light/dark flip with no
    /// component involvement — the same shape `ShadcnTokens::resolve_ring`
    /// documents for its own catalog.
    pub fn resolve_emphasis(explicit: Option<Color>, theme: Option<&Theme>) -> Color {
        if let Some(color) = explicit {
            return color;
        }
        match theme.and_then(|t| t.extension::<MaterialTokens>().map(|x| (x, t.brightness))) {
            Some((tokens, brightness)) => tokens.colors(brightness).emphasis,
            None => FALLBACK_EMPHASIS,
        }
    }

    /// Resolve the `danger` role under the same ladder as
    /// [`resolve_emphasis`](Self::resolve_emphasis).
    pub fn resolve_danger(explicit: Option<Color>, theme: Option<&Theme>) -> Color {
        if let Some(color) = explicit {
            return color;
        }
        match theme.and_then(|t| t.extension::<MaterialTokens>().map(|x| (x, t.brightness))) {
            Some((tokens, brightness)) => tokens.colors(brightness).danger,
            None => FALLBACK_DANGER,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_tokens_attach_round_trips_from_baseline() {
        let theme = super::super::baseline();
        let tokens = theme
            .extension::<MaterialTokens>()
            .expect("baseline() attaches MaterialTokens");
        assert_eq!(
            tokens.colors(Brightness::Light).emphasis,
            tokens.light.emphasis
        );
        assert_ne!(
            tokens.colors(Brightness::Light).emphasis,
            tokens.colors(Brightness::Dark).emphasis
        );
    }

    #[test]
    fn material_matches_the_hand_baked_semantic_tables() {
        assert_eq!(MaterialTokens::material().light, semantic_light());
        assert_eq!(MaterialTokens::material().dark, semantic_dark());
    }

    #[test]
    fn colors_selects_by_brightness() {
        let tokens = MaterialTokens::material();
        assert_eq!(tokens.colors(Brightness::Light), &tokens.light);
        assert_eq!(tokens.colors(Brightness::Dark), &tokens.dark);
    }

    #[test]
    fn emphasis_precedence_is_explicit_then_extension_then_fallback() {
        let explicit = Color::from_rgb8(0x00, 0x00, 0xFF);
        let theme = super::super::baseline();

        // 1. explicit wins over everything.
        assert_eq!(
            MaterialTokens::resolve_emphasis(Some(explicit), Some(&theme)),
            explicit
        );
        // 2. no explicit value: the theme extension wins over the fallback,
        //    at the theme's own brightness.
        assert_eq!(
            MaterialTokens::resolve_emphasis(None, Some(&theme)),
            MaterialTokens::material().colors(theme.brightness).emphasis
        );
        // 3. no theme at all: the fallback constant.
        assert_eq!(
            MaterialTokens::resolve_emphasis(None, None),
            FALLBACK_EMPHASIS
        );
    }

    #[test]
    fn emphasis_follows_the_theme_s_brightness() {
        let light = super::super::baseline().with_brightness(Brightness::Light);
        let dark = super::super::baseline().with_brightness(Brightness::Dark);
        assert_eq!(
            MaterialTokens::resolve_emphasis(None, Some(&light)),
            MaterialTokens::material().light.emphasis
        );
        assert_eq!(
            MaterialTokens::resolve_emphasis(None, Some(&dark)),
            MaterialTokens::material().dark.emphasis
        );
    }

    #[test]
    fn danger_precedence_is_explicit_then_extension_then_fallback() {
        let explicit = Color::from_rgb8(0x00, 0xFF, 0x00);
        let theme = super::super::baseline();
        assert_eq!(
            MaterialTokens::resolve_danger(Some(explicit), Some(&theme)),
            explicit
        );
        assert_eq!(
            MaterialTokens::resolve_danger(None, Some(&theme)),
            MaterialTokens::material().colors(theme.brightness).danger
        );
        assert_eq!(MaterialTokens::resolve_danger(None, None), FALLBACK_DANGER);
    }

    #[test]
    fn a_theme_with_the_extension_cleared_falls_through_to_the_fallbacks() {
        // An app is free to build a theme carrying no MaterialTokens; both
        // resolvers must degrade, not panic.
        let bare = Theme::neutral();
        assert!(bare.extension::<MaterialTokens>().is_none());
        assert_eq!(
            MaterialTokens::resolve_emphasis(None, Some(&bare)),
            FALLBACK_EMPHASIS,
            "a non-Material theme resolves the fallback emphasis color"
        );
        assert_eq!(
            MaterialTokens::resolve_danger(None, Some(&bare)),
            FALLBACK_DANGER,
            "a non-Material theme resolves the fallback danger color"
        );
    }
}
