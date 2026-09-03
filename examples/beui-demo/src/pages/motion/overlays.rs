//! Stub: Motion · Overlays — this scaffold's placeholder (see `crate::pages`
//! for the convention every stub leaf follows).

use frust::AnyView;

use crate::AppState;

/// b-20 fills this page in with the overlay components (popover, tooltip,
/// context-menu, drawer, bottom-sheet, morphing-modal, center-morph-modal).
pub fn page() -> AnyView<AppState> {
    crate::pages::stub_page(
        "Motion \u{b7} Overlays",
        "b-20 fills this page in with the overlay components (popover, tooltip, context-menu, drawer, bottom-sheet, morphing-modal, center-morph-modal).",
    )
}
