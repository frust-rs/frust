//! Stub: Motion · Controls — this scaffold's placeholder (see `crate::pages`
//! for the convention every stub leaf follows).

use frust::AnyView;

use crate::AppState;

/// b-20 fills this page in with the form controls (checkbox, radio, switch,
/// number, range-slider, wheel-picker).
pub fn page() -> AnyView<AppState> {
    crate::pages::stub_page(
        "Motion \u{b7} Controls",
        "b-20 fills this page in with the form controls (checkbox, radio, switch, number, range-slider, wheel-picker).",
    )
}
