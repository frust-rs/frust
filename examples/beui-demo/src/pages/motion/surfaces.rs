//! Stub: Motion · Surfaces — this scaffold's placeholder (see
//! `crate::pages` for the convention every stub leaf follows).

use frust::AnyView;

use crate::AppState;

/// b-20 fills this page in with the surface components (tilt-card,
/// cylinder-carousel, preview-rail, shared-layout-bg, marquee, file-tree).
pub fn page() -> AnyView<AppState> {
    crate::pages::stub_page(
        "Motion \u{b7} Surfaces",
        "b-20 fills this page in with the surface components (tilt-card, cylinder-carousel, preview-rail, shared-layout-bg, marquee, file-tree).",
    )
}
