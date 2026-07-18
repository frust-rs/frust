//! User profile card (`/user/:id`).
//!
//! **Placeholder** (Phase C task: Profile — the Huddle user card an avatar tap
//! anywhere navigates to, the shared-element hero destination). Today it
//! renders the user's name/status from the [`mock`](crate::mock) dataset.

use forgekit::AnyView;

use crate::HuddleState;
use crate::mock;
use crate::screens::{placeholder_body, scaffold};

/// The profile card for `user_id`.
pub fn profile_screen(user_id: String) -> AnyView<HuddleState> {
    let (title, note) = match user_id.parse::<u32>().ok().and_then(mock::user) {
        Some(u) => (
            u.name.to_string(),
            format!(
                "{} — {} (Phase C: avatar hero, shared status, actions).",
                u.name,
                u.status.label()
            ),
        ),
        None => (
            "Profile".to_string(),
            "Profile: the user card an avatar tap navigates to (Phase C).".to_string(),
        ),
    };
    scaffold(&title, placeholder_body(&note))
}
