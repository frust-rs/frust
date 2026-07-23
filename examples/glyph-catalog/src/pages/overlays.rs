//! Overlays section — **filled by `c07`**: `show_glyph_dialog`,
//! `show_command_palette` (live filter), and a `SlideUp` bottom-sheet push —
//! all driven through `state.nav`, the shared navigator controller.
//!
//! Stub scaffold (`c01`): a placeholder honoring the page-fn contract in
//! `pages/mod.rs`.

use frust::{AnyView, any, text};

use crate::CatalogState;

/// See the page-fn contract in [`crate::pages`].
pub fn page(_state: &CatalogState) -> AnyView<CatalogState> {
    any(text("Overlays — filled by c07"))
}
