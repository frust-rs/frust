//! Stub: Motion · Navigation — this scaffold's placeholder (see
//! `crate::pages` for the convention every stub leaf follows).

use frust::AnyView;

use crate::AppState;

/// b-20 fills this page in with the navigation components (animated-sidebar,
/// bounce-sidebar, dock, tabs, expandable-control, adaptive-stepper).
pub fn page() -> AnyView<AppState> {
    crate::pages::stub_page(
        "Motion \u{b7} Navigation",
        "b-20 fills this page in with the navigation components (animated-sidebar, bounce-sidebar, dock, tabs, expandable-control, adaptive-stepper).",
    )
}
