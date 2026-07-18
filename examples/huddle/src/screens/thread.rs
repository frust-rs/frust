//! Thread view (`/thread/:id`).
//!
//! **Placeholder** (Phase C task: Thread — a focused reply view over a parent
//! message, swipe-to-reply, and the thread composer). Today it renders the
//! scaffold only.

use forgekit::AnyView;

use crate::HuddleState;
use crate::screens::{placeholder_body, scaffold};

/// The thread view for `thread_id`.
pub fn thread_screen(thread_id: String) -> AnyView<HuddleState> {
    scaffold(
        &format!("Thread {thread_id}"),
        placeholder_body("Thread: focused replies, swipe-to-reply, composer (Phase C)."),
    )
}
