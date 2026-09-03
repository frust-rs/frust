//! Stub: Motion · Text — this scaffold's placeholder (see `crate::pages` for
//! the convention every stub leaf follows).

use frust::AnyView;

use crate::AppState;

/// b-20 fills this page in with the text-effect components (marquee,
/// text_animation, and friends).
pub fn page() -> AnyView<AppState> {
    crate::pages::stub_page(
        "Motion \u{b7} Text",
        "b-20 fills this page in with the text-effect components (marquee, text_animation, and friends).",
    )
}
