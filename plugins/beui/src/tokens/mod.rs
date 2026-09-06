//! The beUI design language: the vendored token tables, their fold onto frust's
//! fixed token scales, the typed extension for what has no role, the motion
//! constants, and the bundled font bytes.
//!
//! Everything here is **source-derived**, not authored: the values come from
//! beUI's own stylesheet (`app/globals.css`) and motion module (`lib/ease.ts`),
//! rev `10c283e433a8f4f0ac0736684d4426ab612b9f55`, retrieved 2026-09-01. Where a
//! beUI token has no clean frust-role home, the decision is recorded at the
//! field that decides it.
//!
//! - The [`palette`] module — the vendored per-brightness token tables
//!   ([`BeuiPalette`], [`BeuiGlass`], [`BeuiGradient`]) and the authoring-time
//!   OKLCH → sRGB conversion record.
//! - The [`theme`](mod@theme) module — [`theme()`](fn@theme::theme), the 46-role
//!   [`theme::color_scheme`] mapping, the [`theme::type_scale`] /
//!   [`theme::shape_scale`] / [`theme::glass_scale`] folds, the
//!   [`theme::native_typefaces`] binding, and [`BeuiTokens`] — the typed payload
//!   carrying ring/glass/gradient/brand-hue values that have no `ColorScheme`
//!   role.
//! - The [`motion`](mod@motion) module — the three easing curves and six springs
//!   every animated component drives itself with.
//! - The [`fonts`] module — the bundled face bytes [`install`](crate::install)
//!   registers.

pub mod fonts;
pub mod motion;
pub mod palette;
pub mod theme;

pub use fonts::font_data;
pub use motion::{
    ALL_CURVES, ALL_SPRINGS, EASE_DRAWER, EASE_IN_OUT, EASE_OUT, SPRING_GLIDE, SPRING_LAYOUT,
    SPRING_MOUSE, SPRING_PANEL, SPRING_PRESS, SPRING_SWAP, TIMING_COLORS, TIMING_PRESS,
};
pub use palette::{BEUI_DARK, BEUI_LIGHT, BeuiGlass, BeuiGradient, BeuiPalette, palette};
pub use theme::{
    BEUI_DESIGN_LANGUAGE, BeuiTokens, FALLBACK_RING, GEIST_FAMILY, GEIST_MONO_FAMILY, color_scheme,
    color_scheme_dark, color_scheme_light, glass_scale, mono_family, native_typefaces, sans_family,
    shape_scale, theme, type_scale,
};
