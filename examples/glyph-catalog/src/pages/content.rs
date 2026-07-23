//! Content section — **filled by `c06`**: `glyph_card`, the stat-card grid,
//! `glyph_list` (as the data-table stand-in), an accordion trio, an
//! `empty_state`, and the `term_block` (with a staggered replay) plus a
//! tooltip.
//!
//! Stub scaffold (`c01`): a placeholder honoring the page-fn contract in
//! `pages/mod.rs`.

use frust::{AnyView, any, text};

use crate::CatalogState;

/// See the page-fn contract in [`crate::pages`].
pub fn page(_state: &CatalogState) -> AnyView<CatalogState> {
    any(text("Content — filled by c06"))
}
