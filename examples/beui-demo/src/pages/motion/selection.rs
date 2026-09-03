//! Stub: Motion · Selection — this scaffold's placeholder (see
//! `crate::pages` for the convention every stub leaf follows).

use frust::AnyView;

use crate::AppState;

/// b-20 fills this page in with the selection components (select, combobox,
/// multi-select, action-swap).
pub fn page() -> AnyView<AppState> {
    crate::pages::stub_page(
        "Motion \u{b7} Selection",
        "b-20 fills this page in with the selection components (select, combobox, multi-select, action-swap).",
    )
}
