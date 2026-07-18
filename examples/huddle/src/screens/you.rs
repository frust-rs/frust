//! You tab — the current user's card and the entry into the settings stack.
//!
//! **Placeholder** (Phase C task: You screen — the current user's profile card,
//! status, and account actions). Today it renders the scaffold and pushes into
//! the settings stack so the `/you` → `/you/settings` flow is clickable.

use forgekit::{AnyView, Button, Column, NavigatorController, any};

use crate::HuddleState;
use crate::screens::{placeholder_body, scaffold, settings};

/// The You tab root. `controller` lets the demo push the settings stack.
pub fn you_screen(controller: NavigatorController<HuddleState>) -> AnyView<HuddleState> {
    let body = any(Column(vec![
        placeholder_body("You: your profile card, status, and account actions (Phase C)."),
        any(Button("Settings", move |_s: &mut HuddleState| {
            let c = controller.clone();
            controller.push(move || settings::settings_screen(c.clone()));
        })),
    ]));
    scaffold("You", body)
}
