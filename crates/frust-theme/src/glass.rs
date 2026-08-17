//! Glass material tokens: a three-tier [`GlassScale`]
//! (`chrome`/`bar`/`control`), each carrying a [`GlassMaterial`] recipe —
//! background-blur intent, translucent fill washes for over-light and
//! over-dark content, a specular hairline alpha, and a drop [`ShadowSpec`].
//!
//! This is **pure data**: no blur is rendered here. A
//! [`Theme`](crate::theme::Theme) carries one `GlassScale` regardless of
//! design language so a widget can read a single API on either — a widget
//! branches on [`GlassMaterial::is_opaque`], never on a design-language tag.
//!
//! This crate constructs exactly one recipe,
//! [`GlassScale::opaque_material`]: zero blur intent, empty fill stacks,
//! shadows reusing the [`Elevation::neutral`] table. It is what
//! [`Theme::neutral`](crate::theme::Theme::neutral) carries, and the value a
//! translucency-free design system keeps. A design system with real glass
//! chrome (an iOS-style "Liquid Glass" recipe, say) authors its own
//! `GlassScale` from its own mined source values and installs it through
//! [`ThemeBuilder::glass`](crate::builder::ThemeBuilder::glass) — recipes are
//! design-system data, not framework data.
//!
//! `hairline_alpha` and `shadow` are **tuned Frust policy** on the opaque
//! path (documented, not load-bearing), in the same spirit as
//! [`crate::elevation`]'s shadow math.

use peniko::Color;

use crate::elevation::{Elevation, ShadowSpec};

/// One translucent wash in a glass fill stack. The `color`'s alpha channel is
/// the wash opacity; stacks are composited bottom-to-top (first element
/// painted first) over the blurred backdrop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassFill {
    /// The wash color, alpha included.
    pub color: Color,
}

impl GlassFill {
    /// A wash from straight-line-sRGB components `[r, g, b, a]` (0-1), matching
    /// the `glass-recipes.json` `rgb`/`a` encoding.
    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self {
            color: Color::new([r, g, b, a]),
        }
    }
}

/// One glass tier's recipe: how much background blur it intends, the
/// translucent washes to composite over light and over dark content, its
/// specular hairline alpha, and its drop shadow.
///
/// `blur_radius_intent` is an *intent* in the kit's blur units, not a rendered
/// pixel radius — a later painting task maps it to the platform's blur
/// primitive. `0.0` means "no lens": the opaque-material path (see
/// [`GlassMaterial::is_opaque`]).
#[derive(Clone, Debug, PartialEq)]
pub struct GlassMaterial {
    /// Background-blur intent in a design system's own blur units; `0.0` is
    /// "no lens", the opaque path [`GlassScale::opaque_material`] takes.
    pub blur_radius_intent: f64,
    /// Washes composited over light content, bottom-to-top.
    pub fills_light: Vec<GlassFill>,
    /// Washes composited over dark content, bottom-to-top.
    pub fills_dark: Vec<GlassFill>,
    /// Alpha of the specular hairline edge (white over light, drawn by the
    /// consuming widget). `0.0` on the opaque path.
    pub hairline_alpha: f32,
    /// The tier's drop shadow.
    pub shadow: ShadowSpec,
}

impl GlassMaterial {
    /// `true` when this material carries no background-blur intent — the
    /// opaque-material path a widget takes on the Material design language,
    /// where it paints its surface-container role instead of a lens. The
    /// single-API branch that lets one widget read either design language.
    pub fn is_opaque(&self) -> bool {
        self.blur_radius_intent == 0.0
    }
}

/// The three glass tiers a [`Theme`](crate::theme::Theme) carries:
/// `chrome` (popovers/sheets/menus), `bar` (tab/nav/toolbars), and `control`
/// (buttons/toggles). See the module docs for each tier's source recipe.
#[derive(Clone, Debug, PartialEq)]
pub struct GlassScale {
    /// Popovers, sheets, menus — the most opaque, highest-blur tier.
    pub chrome: GlassMaterial,
    /// Tab bars, nav bars, toolbars — a subtle mid-blur tier.
    pub bar: GlassMaterial,
    /// Buttons, toggles — a pure low-blur lens, no fill.
    pub control: GlassMaterial,
}

impl GlassScale {
    /// The opaque scale: every tier is **opaque** —
    /// `blur_radius_intent == 0.0` and empty fill stacks — so a widget reading
    /// [`GlassMaterial::is_opaque`] paints its surface-container role instead
    /// of a translucent lens. This is the "one API on either design language"
    /// seam: the same widget code reads `theme.glass.<tier>` and branches on
    /// `is_opaque()`.
    ///
    /// Shadows reuse the [`Elevation::neutral`] table so the opaque path
    /// lines up with that elevation ladder: chrome = level 3
    /// (menus/dialogs), bar = level 2 (nav bar), control = level 1.
    pub fn opaque_material() -> Self {
        let elevation = Elevation::neutral();
        let opaque = |shadow: ShadowSpec| GlassMaterial {
            blur_radius_intent: 0.0,
            fills_light: Vec::new(),
            fills_dark: Vec::new(),
            hairline_alpha: 0.0,
            shadow,
        };
        Self {
            // The v1 shadow mapping doesn't branch by brightness (see
            // `elevation`'s module docs), so `shadow_light` and
            // `shadow_dark` are identical here — pick either.
            chrome: opaque(elevation.level3.shadow_light),
            bar: opaque(elevation.level2.shadow_light),
            control: opaque(elevation.level1.shadow_light),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_material_is_opaque_in_every_tier() {
        let g = GlassScale::opaque_material();
        for m in [&g.chrome, &g.bar, &g.control] {
            assert_eq!(m.blur_radius_intent, 0.0);
            assert!(m.is_opaque());
            assert!(m.fills_light.is_empty());
            assert!(m.fills_dark.is_empty());
        }
    }

    #[test]
    fn opaque_material_shadows_track_the_neutral_elevation_table() {
        let g = GlassScale::opaque_material();
        let elevation = Elevation::neutral();
        assert_eq!(g.chrome.shadow, elevation.level3.shadow_light);
        assert_eq!(g.bar.shadow, elevation.level2.shadow_light);
        assert_eq!(g.control.shadow, elevation.level1.shadow_light);
    }

    #[test]
    fn a_design_system_scale_stays_a_lens_on_every_tier() {
        // The other half of the `is_opaque` branch, built the way a design
        // system with real glass chrome would build it (this crate ships no
        // translucent recipe of its own): non-zero blur intent per tier is
        // what makes a widget take the lens path instead of the opaque one.
        let lens = |blur: f64| GlassMaterial {
            blur_radius_intent: blur,
            fills_light: vec![GlassFill::new(1.0, 1.0, 1.0, 0.34)],
            fills_dark: vec![GlassFill::new(0.0, 0.0, 0.0, 0.41)],
            hairline_alpha: 0.5,
            shadow: ShadowSpec {
                y_offset: 18.0,
                blur_std_dev: 24.0,
                color_alpha: 0.30,
            },
        };
        let glass = GlassScale {
            chrome: lens(75.0),
            bar: lens(45.0),
            control: lens(15.0),
        };
        for m in [&glass.chrome, &glass.bar, &glass.control] {
            assert!(!m.is_opaque(), "a non-zero blur intent is never opaque");
        }
        assert!(GlassScale::opaque_material().chrome.is_opaque());
    }
}
