//! Foundations section — **filled by `c02`**: live color swatches
//! (`theme.scheme()` + `StatusPalette` + `GlyphInk`), the type-scale
//! specimens, and the radius-scale chips.
//!
//! Stub scaffold (`c01`): a placeholder honoring the page-fn contract in
//! `pages/mod.rs`.

use frust::{AnyView, any, text};

use crate::CatalogState;

/// See the page-fn contract in [`crate::pages`].
pub fn page(_state: &CatalogState) -> AnyView<CatalogState> {
    any(text("Foundations — filled by c02"))
}
