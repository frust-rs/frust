//! Feedback section — **filled by `c04`**: badges (5 variants + dot),
//! removable tags, the 4 alerts, toast triggers (per variant, appending to
//! `state.toasts`), and the progress / skeleton / dots loaders.
//!
//! Stub scaffold (`c01`): a placeholder honoring the page-fn contract in
//! `pages/mod.rs`.

use frust::{AnyView, any, text};

use crate::CatalogState;

/// See the page-fn contract in [`crate::pages`].
pub fn page(_state: &CatalogState) -> AnyView<CatalogState> {
    any(text("Feedback — filled by c04"))
}
