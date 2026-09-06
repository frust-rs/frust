//! The Glyph design language: a terminal-native, **dark-first**,
//! monospace-led token set — "inspired by Material 3 Expressive, Liquid
//! Glass, and Yaru research · original tokens".
//!
//! Every token here is source-derived from the two vendored Glyph reference
//! builds — `glyph-design-system.html`
//! (dark, the canonical brightness) and `glyph-design-system-light.html`
//! (light, "same tokens · accent re-tuned for AA on paper surfaces") — plus
//! `glyph-motion.html` for the motion vocabulary. Retrieval date **2026-07-21**
//! (the date both HTML sources were vendored). Where the flat Glyph token set
//! has no clean Material-3-role home, this module documents the nearest-role
//! mapping decision inline.
//!
//! Glyph maps onto the **same** fixed token scales every other baseline uses
//! (`ColorScheme`'s 46 roles, `TypeScale`'s 30 slots, `ShapeScale`'s 10 radii,
//! `Elevation`'s per-brightness shadow table, `MotionScheme`, and a
//! `GlassScale`) so every widget in the baseline set keeps working under a
//! Glyph theme with zero widget edits — only genuinely new Glyph components
//! need this crate's catalog. Three light-mode structural consequences drive
//! the design: the accent splits into a text role (`primary`) and a
//! bright-fill role (`primary_container`), uniform across brightnesses; the
//! elevation table's shadows differ by brightness (light warm-ink, dark black);
//! and the terminal block + tooltip stay dark in light mode as
//! brightness-invariant "ink" ([`GlyphInk`]), not brightness-swapped roles.
//!
//! - The [`color`] module — [`color::dark`]/[`color::light`], [`GlyphInk`],
//!   and [`color::status`].
//! - The [`scales`] module — [`scales::type_scale`], [`scales::shape`],
//!   [`scales::elevation`], [`scales::motion`], and [`scales::glass`].
//! - The [`baseline`] module — [`baseline::baseline`], the assembled
//!   `frust::Theme` [`install`](crate::install) seeds.
//! - The [`fonts`] module — the bundled face bytes [`install`](crate::install)
//!   registers.

pub mod baseline;
pub mod color;
pub mod fonts;
pub mod scales;

pub use baseline::{baseline, native_typefaces};
pub use color::GlyphInk;
pub use fonts::font_data;
