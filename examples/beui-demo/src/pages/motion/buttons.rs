//! Stub: Motion · Buttons — this scaffold's placeholder (see `crate::pages`
//! for the convention every stub leaf follows).

use frust::AnyView;

use crate::AppState;

/// b-20 fills this page in with the button family (base, metallic, stateful,
/// magnetic, expanding-arrow).
pub fn page() -> AnyView<AppState> {
    crate::pages::stub_page(
        "Motion \u{b7} Buttons",
        "b-20 fills this page in with the button family (base, metallic, stateful, magnetic, expanding-arrow).",
    )
}
