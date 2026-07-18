//! Notification settings (`/you/settings/notifications`).
//!
//! **Placeholder** (Phase C task: Notifications — a `radio` group for the
//! notification frequency, plus per-channel mute switches). Today it renders
//! the scaffold only.

use forgekit::AnyView;

use crate::HuddleState;
use crate::screens::{placeholder_body, scaffold};

/// The notification-settings page.
pub fn notifications_screen() -> AnyView<HuddleState> {
    scaffold(
        "Notifications",
        placeholder_body(
            "Notifications: a radio group for frequency + per-channel mutes (Phase C).",
        ),
    )
}
