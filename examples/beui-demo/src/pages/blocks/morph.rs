//! Stub: Blocks · Morph — this scaffold's placeholder (see `crate::pages`
//! for the convention every stub leaf follows).

use frust::AnyView;

use crate::AppState;

/// b-22 fills this page in with the morph-modal block.
pub fn page() -> AnyView<AppState> {
    crate::pages::stub_page(
        "Blocks \u{b7} Morph",
        "b-22 fills this page in with the morph-modal block.",
    )
}
