//! The Glyph design language: a terminal-native, **dark-first**,
//! monospace-led token set — "inspired by Material 3 Expressive, Liquid
//! Glass, and Yaru research · original tokens".
//!
//! Every token here is source-derived from the two vendored Glyph reference
//! builds beside this crate's research doc — `glyph-design-system.html`
//! (dark, the canonical brightness) and `glyph-design-system-light.html`
//! (light, "same tokens · accent re-tuned for AA on paper surfaces") — plus
//! `glyph-motion.html` for the motion vocabulary. Retrieval date **2026-07-21**
//! (the date both HTML sources were vendored; see
//! `workflow/plans/features/glyph-design-system/research/RESEARCH.md` §1.1,
//! §1.1b, §1.2, §1.3, §1.4). Where the flat Glyph token set has no clean
//! Material-3-role home, this module documents the nearest-role mapping
//! decision inline and lists the leftovers in the task's completion summary.
//!
//! Glyph maps onto the **same** fixed token scales every other baseline uses
//! ([`crate::color::ColorScheme`]'s 46 roles, [`crate::typography::TypeScale`]'s
//! 30 slots, [`crate::shape::ShapeScale`]'s 10 radii, [`crate::elevation::Elevation`]'s
//! per-brightness shadow table, [`crate::motion::MotionScheme`], and a
//! [`crate::glass::GlassScale`]) so every existing widget keeps working under a
//! Glyph theme with zero widget edits — only genuinely new Glyph components need
//! a new catalog. Three light-mode structural consequences drive the design
//! (RESEARCH §1.1b): the accent splits into a text role (`primary`) and a
//! bright-fill role (`primary_container`), uniform across brightnesses; the
//! elevation table's shadows differ by brightness (light warm-ink, dark black);
//! and the terminal block + tooltip stay dark in light mode as
//! brightness-invariant "ink" ([`GlyphInk`]), not brightness-swapped roles.
//!
//! - The [`color`] module — [`crate::color::ColorScheme::glyph_dark`]/
//!   [`crate::color::ColorScheme::glyph_light`], [`GlyphInk`], and
//!   [`crate::status::StatusPalette::glyph`].
//! - The [`scales`] module — [`crate::typography::TypeScale::glyph`],
//!   [`crate::shape::ShapeScale::glyph`], [`crate::elevation::Elevation::glyph`],
//!   [`crate::motion::MotionScheme::glyph`], and [`crate::glass::GlassScale::glyph`].
//! - The [`baseline`] module — [`crate::theme::Theme::glyph_baseline`].

pub mod baseline;
pub mod color;
#[cfg(feature = "glyph-fonts")]
pub mod fonts;
pub mod scales;

pub use color::GlyphInk;
