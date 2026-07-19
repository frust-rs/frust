//! User entities — moved verbatim from the former `mock` module (huddle
//! clean-architecture refactor, task 01). The shared dataset that
//! materializes these types now lives in [`crate::data::store`].

/// A member's presence state (the status dot in avatars and roster rows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserStatus {
    /// Available.
    Online,
    /// Idle / away.
    Away,
    /// Do not disturb.
    Dnd,
}

impl UserStatus {
    /// A short human label for the status.
    pub fn label(self) -> &'static str {
        match self {
            UserStatus::Online => "Online",
            UserStatus::Away => "Away",
            UserStatus::Dnd => "Do not disturb",
        }
    }
}

/// A workspace member.
#[derive(Clone, Copy, Debug)]
pub struct User {
    /// Stable numeric id (used in `/user/:id` deep links).
    pub id: u32,
    /// Display name.
    pub name: &'static str,
    /// Two-letter initials for the avatar escape-hatch paint.
    pub initials: &'static str,
    /// Presence.
    pub status: UserStatus,
}

/// The "me" user whose mentions drive the [`activity`] feed.
pub const CURRENT_USER_ID: u32 = 1;
