//! Channel / DM message feed (`/channel/:id`).
//!
//! **Placeholder** (Phase C task: Channel feed — message bubbles, reactions,
//! composer, typing indicator, entrance animations, and pagination against the
//! [`mock`](crate::mock) dataset, including the 120-message `#firehose`). Today
//! it renders the channel title and offers a push into a thread so the
//! `/channel/:id` → `/thread/:id` flow is clickable.

use forgekit::{AnyView, Button, Column, NavigatorController, any};

use crate::HuddleState;
use crate::mock;
use crate::screens::{placeholder_body, scaffold, thread};

/// The channel/DM feed for `channel_id`. `controller` lets the demo push a
/// thread.
pub fn channel_feed(
    controller: NavigatorController<HuddleState>,
    channel_id: String,
) -> AnyView<HuddleState> {
    let title = mock::channel(&channel_id)
        .map(|c| format!("#{}", c.name))
        .or_else(|| {
            mock::dms()
                .iter()
                .find(|d| d.id == channel_id)
                .and_then(|d| mock::user(d.user_id))
                .map(|u| u.name.to_string())
        })
        .unwrap_or_else(|| channel_id.clone());

    let count = mock::messages_for(&channel_id).len();
    let body = any(Column(vec![
        placeholder_body(&format!(
            "Channel feed ({count} messages): bubbles, reactions, composer, pagination (Phase C)."
        )),
        any(Button("Open a thread", move |_s: &mut HuddleState| {
            controller.push(|| thread::thread_screen("1".to_string()));
        })),
    ]));
    scaffold(&title, body)
}
