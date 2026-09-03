//! Stub: Motion · Data — this scaffold's placeholder (see `crate::pages`
//! for the convention every stub leaf follows).

use frust::AnyView;

use crate::AppState;

/// b-20 fills this page in with the data components (table, animated-badge,
/// animated-toast-stack, loader, pull-to-refresh, scroll-animation).
pub fn page() -> AnyView<AppState> {
    crate::pages::stub_page(
        "Motion \u{b7} Data",
        "b-20 fills this page in with the data components (table, animated-badge, animated-toast-stack, loader, pull-to-refresh, scroll-animation).",
    )
}
