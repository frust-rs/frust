//! Home tab — channels + DMs.
//!
//! **Placeholder** (Phase C task: Home screen — channel/DM list with loading
//! skeletons, swipeable archive/mute rows, unread badges, pull-to-refresh).
//! Today it renders the scaffold and offers a single push into a channel feed
//! so the `/` → `/channel/:id` flow is clickable end-to-end.

use forgekit::{AnyView, Button, Column, NavigatorController, any};

use crate::HuddleState;
use crate::mock;
use crate::screens::{channel_feed, placeholder_body, scaffold};

/// The Home tab root. `controller` lets the demo push into a channel feed.
pub fn home_screen(controller: NavigatorController<HuddleState>) -> AnyView<HuddleState> {
    let body = any(Column(vec![
        placeholder_body(
            "Home: channels + DMs with skeletons, swipeable rows, unread badges (Phase C).",
        ),
        any(Button("Open #general", move |_s: &mut HuddleState| {
            let c = controller.clone();
            controller.push(move || {
                channel_feed::channel_feed(
                    c.clone(),
                    mock::channel("general").unwrap().id.to_string(),
                )
            });
        })),
    ]));
    scaffold("Huddle", body)
}
