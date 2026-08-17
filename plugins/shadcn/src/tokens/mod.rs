//! The shadcn design language: the vendored token tables, their fold onto
//! frust's fixed token scales, the typed extension for what has no role, and the
//! bundled font bytes.
//!
//! Everything here is **source-derived**, not authored: the values come from
//! shadcn/ui v4's own registry (`apps/v4/registry/themes.ts`, rev
//! `d4fc45b1fbabfccb7a6a4333d8004cf19481caa9`, retrieved 2026-08-17) and its
//! `@theme inline` radius derivations (`apps/v4/app/globals.css`, same rev).
//! Where a shadcn token has no clean frust-role home, the decision is recorded
//! at the field that decides it.
//!
//! - The [`palette`] module — the vendored per-preset/per-brightness token
//!   tables ([`ShadcnPalette`], [`ShadcnBase`], [`ShadcnSidebar`]) and the
//!   authoring-time OKLCH → sRGB conversion method.
//! - The [`theme`](mod@theme) module — [`theme()`](fn@theme::theme) and the six
//!   per-preset constructors, the 46-role [`theme::color_scheme`] mapping, the
//!   [`theme::type_scale`]/[`theme::shape_scale`] folds, and the
//!   [`theme::native_typefaces`] binding.
//! - The [`extension`] module — [`ShadcnTokens`], the typed payload carrying
//!   ring/chart/sidebar/radius, and its resolution ladder.
//! - The [`fonts`] module — the bundled face bytes
//!   [`install`](crate::install) registers.

pub mod extension;
pub mod fonts;
pub mod palette;
pub mod theme;

pub use extension::{FALLBACK_RING, RADIUS_BASE, ShadcnRadius, ShadcnTokens};
pub use fonts::font_data;
pub use palette::{ShadcnBase, ShadcnPalette, ShadcnSidebar};
pub use theme::{
    INTER_FAMILY, JETBRAINS_MONO_FAMILY, SHADCN_DESIGN_LANGUAGE, color_scheme, color_scheme_dark,
    color_scheme_light, mono_family, native_typefaces, sans_family, shape_scale, theme, theme_for,
    theme_mauve, theme_mist, theme_olive, theme_stone, theme_taupe, theme_zinc, type_scale,
};
