//! iOS-27-kit "Liquid Glass" material tokens: a three-tier [`GlassScale`]
//! (`chrome`/`bar`/`control`), each carrying a [`GlassMaterial`] recipe —
//! background-blur intent, translucent fill washes for over-light and
//! over-dark content, a specular hairline alpha, and a drop [`ShadowSpec`].
//!
//! This is **pure data**: no widget consumes it yet (a future update paints
//! from it), and no blur is rendered here. A [`Theme`](crate::theme::Theme)
//! carries one `GlassScale` regardless of design language so a widget can
//! read a single API on either — [`GlassScale::ios27`] encodes the kit
//! recipes, [`GlassScale::opaque_material`] the Material-language equivalent
//! (zero blur intent, opaque surface — see that constructor's docs).
//!
//! # Source
//!
//! Recipes mined from the iOS 27 "Liquid Glass" kit
//! (584 records, retrieved 2026-07-17). The three canonical tiers below are
//! the json's dominant fill stacks per corner-radius family (fill `rgb`
//! values are 0-1 floats, `a` the wash alpha):
//!
//! - **chrome** (popovers/sheets/menus), `radius 75`: over-dark fill stack
//!   `[black a=0.41]` (25/108 records); over-light `[white a=0.34, white
//!   a=0.84]` (25/108). A `[white a=0.34, white a=0.78]` variant (24/108)
//!   exists at near-equal frequency — we pick the `0.84` top wash (slightly
//!   more opaque chrome); the `0.78` variant is noted here as the alternative.
//! - **bar** (tab/nav/toolbars), `radius 45`: over-light `[white a=0.07,
//!   white a=0.03]` (all 11 `radius 45` records). The kit's dark bar records
//!   (`Tab Bars/Dark`, `Toolbars/…/Dark`) composite `[gray(0.6) a=0.17]` over
//!   an opaque background base; we mirror that translucent tint as the
//!   over-dark wash `[gray(0.6) a=0.17]`, dropping the opaque base because a
//!   glass bar composites over live content, not an opaque plate.
//! - **control** (buttons/toggles), `radius 15`: no fill — a pure lens, so
//!   both fill stacks are empty. (The kit's other `radius 15` records are
//!   menus, which are semantically *chrome* despite the smaller corner; their
//!   fills/shadow belong to the chrome tier, not to plain controls.)
//!
//! The `hairline_alpha` and `shadow` per tier are **tuned Frust policy**
//! (documented, not load-bearing), in the same spirit as [`crate::elevation`]'s
//! shadow math: the kit stores hairlines/shadows as multi-layer stacks that do
//! not reduce cleanly to a single [`ShadowSpec`], so we pick a defensible
//! per-tier value (chrome floats highest, bars sit flush with no shadow,
//! controls get a subtle lift). [`GlassScale::opaque_material`]'s shadows reuse
//! the [`Elevation::m3`] table so the opaque path matches Material elevation.

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
    /// Background-blur intent in the kit's blur units (75 chrome / 45 bar / 15
    /// control for [`GlassScale::ios27`]; `0.0` for the opaque path).
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
    /// The iOS-27-kit glass scale — the recipes mined into
    /// `glass-recipes.json` (see module docs). Blur intents 75/45/15.
    pub fn ios27() -> Self {
        Self {
            chrome: GlassMaterial {
                blur_radius_intent: 75.0,
                fills_light: vec![
                    GlassFill::new(1.0, 1.0, 1.0, 0.34),
                    GlassFill::new(1.0, 1.0, 1.0, 0.84),
                ],
                fills_dark: vec![GlassFill::new(0.0, 0.0, 0.0, 0.41)],
                hairline_alpha: 0.5,
                shadow: ShadowSpec {
                    y_offset: 18.0,
                    blur_std_dev: 24.0,
                    color_alpha: 0.30,
                },
            },
            bar: GlassMaterial {
                blur_radius_intent: 45.0,
                fills_light: vec![
                    GlassFill::new(1.0, 1.0, 1.0, 0.07),
                    GlassFill::new(1.0, 1.0, 1.0, 0.03),
                ],
                // Derived from the kit's dark bar records (see module docs):
                // their `[gray(0.6) a=0.17]` translucent tint, minus the
                // opaque background base a live-content glass bar omits.
                fills_dark: vec![GlassFill::new(0.6, 0.6, 0.6, 0.17)],
                hairline_alpha: 0.5,
                // Bars sit flush with content edges — the kit's `radius 45`
                // records carry no shadow.
                shadow: ShadowSpec {
                    y_offset: 0.0,
                    blur_std_dev: 0.0,
                    color_alpha: 0.0,
                },
            },
            control: GlassMaterial {
                blur_radius_intent: 15.0,
                // Pure lens — no fill (see module docs).
                fills_light: Vec::new(),
                fills_dark: Vec::new(),
                hairline_alpha: 0.3,
                shadow: ShadowSpec {
                    y_offset: 2.0,
                    blur_std_dev: 4.0,
                    color_alpha: 0.12,
                },
            },
        }
    }

    /// The Material-language equivalent scale: every tier is **opaque** —
    /// `blur_radius_intent == 0.0` and empty fill stacks — so a widget reading
    /// [`GlassMaterial::is_opaque`] paints its surface-container role (the
    /// opaque Material surface) instead of a translucent lens. This is the
    /// "one API on either design language" seam: the same widget code reads
    /// `theme.glass.<tier>` and branches on `is_opaque()`.
    ///
    /// Shadows reuse the [`Elevation::m3`] table so the opaque path lines up
    /// with Material elevation: chrome = level 3 (menus/dialogs), bar = level
    /// 2 (nav bar), control = level 1.
    pub fn opaque_material() -> Self {
        let m3 = Elevation::m3();
        let opaque = |shadow: ShadowSpec| GlassMaterial {
            blur_radius_intent: 0.0,
            fills_light: Vec::new(),
            fills_dark: Vec::new(),
            hairline_alpha: 0.0,
            shadow,
        };
        Self {
            // M3's v1 shadow mapping doesn't branch by brightness (see
            // `elevation`'s module docs), so `shadow_light` and
            // `shadow_dark` are identical here — pick either.
            chrome: opaque(m3.level3.shadow_light),
            bar: opaque(m3.level2.shadow_light),
            control: opaque(m3.level1.shadow_light),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ios27_blur_intents_match_canonical_tiers() {
        let g = GlassScale::ios27();
        assert_eq!(g.chrome.blur_radius_intent, 75.0);
        assert_eq!(g.bar.blur_radius_intent, 45.0);
        assert_eq!(g.control.blur_radius_intent, 15.0);
    }

    #[test]
    fn ios27_chrome_fills_match_recipe() {
        let g = GlassScale::ios27();
        // over-dark: [black a=0.41]
        assert_eq!(
            g.chrome.fills_dark,
            vec![GlassFill::new(0.0, 0.0, 0.0, 0.41)]
        );
        // over-light: [white a=0.34, white a=0.84] (0.84 variant picked)
        assert_eq!(
            g.chrome.fills_light,
            vec![
                GlassFill::new(1.0, 1.0, 1.0, 0.34),
                GlassFill::new(1.0, 1.0, 1.0, 0.84),
            ]
        );
    }

    #[test]
    fn ios27_bar_fills_match_recipe_and_derived_dark_mirror() {
        let g = GlassScale::ios27();
        // over-light: [white a=0.07, white a=0.03]
        assert_eq!(
            g.bar.fills_light,
            vec![
                GlassFill::new(1.0, 1.0, 1.0, 0.07),
                GlassFill::new(1.0, 1.0, 1.0, 0.03),
            ]
        );
        // over-dark: derived mirror from the kit's dark bar tint [gray(0.6) a=0.17]
        assert_eq!(g.bar.fills_dark, vec![GlassFill::new(0.6, 0.6, 0.6, 0.17)]);
    }

    #[test]
    fn ios27_control_is_a_pure_lens_with_no_fill() {
        let g = GlassScale::ios27();
        assert!(g.control.fills_light.is_empty());
        assert!(g.control.fills_dark.is_empty());
        assert!(!g.control.is_opaque()); // still a lens, just fill-less
    }

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
    fn opaque_material_shadows_track_the_m3_elevation_table() {
        let g = GlassScale::opaque_material();
        let m3 = Elevation::m3();
        assert_eq!(g.chrome.shadow, m3.level3.shadow_light);
        assert_eq!(g.bar.shadow, m3.level2.shadow_light);
        assert_eq!(g.control.shadow, m3.level1.shadow_light);
    }

    #[test]
    fn both_constructors_populate_all_three_tiers() {
        // ios27 carries the kit blur intents; opaque_material zeroes them —
        // either way all three tier fields exist and are distinct materials.
        let ios = GlassScale::ios27();
        assert!(ios.chrome.blur_radius_intent > 0.0);
        assert!(ios.bar.blur_radius_intent > 0.0);
        assert!(ios.control.blur_radius_intent > 0.0);

        let opaque = GlassScale::opaque_material();
        assert!(opaque.chrome.is_opaque());
        assert!(opaque.bar.is_opaque());
        assert!(opaque.control.is_opaque());
    }
}
