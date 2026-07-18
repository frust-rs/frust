//! Search tab — live filter across channels, messages, and people.
//!
//! **Placeholder** (Phase C task: Search screen — a `TextInput` driving a
//! `Memo` filter over the mock dataset, with empty/no-results states). Today it
//! renders the scaffold only.

use forgekit::AnyView;

use crate::HuddleState;
use crate::screens::{placeholder_body, scaffold};

/// The Search tab root.
pub fn search_screen() -> AnyView<HuddleState> {
    scaffold(
        "Search",
        placeholder_body("Search: live filter over channels, messages, and people (Phase C)."),
    )
}
