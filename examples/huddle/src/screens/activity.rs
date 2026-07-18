//! Activity tab — the mentions/activity feed.
//!
//! **Placeholder** (Phase C task: Activity screen — the mentions feed derived
//! from [`mock::activity`](crate::mock::activity), each row tappable through to
//! its source message). Today it renders the scaffold only.

use forgekit::AnyView;

use crate::HuddleState;
use crate::screens::{placeholder_body, scaffold};

/// The Activity tab root.
pub fn activity_screen() -> AnyView<HuddleState> {
    scaffold(
        "Activity",
        placeholder_body(
            "Activity: your mentions feed, derived from the message stream (Phase C).",
        ),
    )
}
