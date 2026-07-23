//! Buttons + Forms section — **filled by `c03`**: every `ButtonStyle` across
//! {default, sm, loading, disabled}, plus the baseline form controls
//! (`text_input`, `checkbox`, `radio`, `switch`, `slider`) under the Glyph
//! theme, shown honestly as baseline-themed widgets.
//!
//! Stub scaffold (`c01`): a placeholder honoring the page-fn contract in
//! `pages/mod.rs`.

use frust::{AnyView, any, text};

use crate::CatalogState;

/// See the page-fn contract in [`crate::pages`].
pub fn page(_state: &CatalogState) -> AnyView<CatalogState> {
    any(text("Buttons + Forms — filled by c03"))
}
