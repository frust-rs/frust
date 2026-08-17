//! A free-standing copy of the iOS-27-kit "Liquid Glass" [`GlassScale`]
//! recipe [`crate::baseline`] installs, kept in its own module (rather than
//! folded into `tokens.rs`) because [`crate::switch`]'s reflective-knob
//! treatment also reads [`ios27`] directly at paint time for its unthemed
//! fallback — a widget, not just a token constructor, depends on this
//! specific recipe.
//!
//! Copied from `frust-theme`'s own (ungated) `GlassScale::ios27` — see that
//! constructor's doc comment (`crates/frust-theme/src/glass.rs`) for the
//! recipe's mined source and the load-bearing rationale for each tier's
//! chosen wash/hairline/shadow values. This crate owns its own copy rather
//! than calling that inherent constructor so it stays self-sufficient once
//! the in-tree Cupertino catalog is removed.

use frust::{GlassFill, GlassMaterial, GlassScale, ShadowSpec};

/// The iOS-27-kit glass scale — the recipes mined into `glass-recipes.json`
/// (see the module docs). Blur intents 75/45/15 (`chrome`/`bar`/`control`).
pub fn ios27() -> GlassScale {
    GlassScale {
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
            // Derived from the kit's dark bar records (see the module docs):
            // their `[gray(0.6) a=0.17]` translucent tint, minus the opaque
            // background base a live-content glass bar omits.
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
            // Pure lens — no fill (see the module docs).
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
