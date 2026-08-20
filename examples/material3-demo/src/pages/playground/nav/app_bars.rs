//! App bars: the reference's `AppBarsPlayground`.
//!
//! A placeholder until this section's batch ports the real playground —
//! everything around it (the catalog row, the routes, both host layouts) is
//! already live.

use frust::AnyView;

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::coming_soon;

/// See the page contract in [`crate::pages::playground`].
pub fn page(entry: DemoEntry) -> AnyView<AppState> {
    coming_soon(entry)
}
