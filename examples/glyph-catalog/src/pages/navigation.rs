//! Navigation section — **filled by `c05`**: a tabs demo, the segmented
//! control, a breadcrumb, `glyph_nav_bar` (char AND vector-icon items), and
//! avatars.
//!
//! Stub scaffold (`c01`): a placeholder honoring the page-fn contract in
//! `pages/mod.rs`.

use frust::{AnyView, any, text};

use crate::CatalogState;

/// See the page-fn contract in [`crate::pages`].
pub fn page(_state: &CatalogState) -> AnyView<CatalogState> {
    any(text("Navigation — filled by c05"))
}
