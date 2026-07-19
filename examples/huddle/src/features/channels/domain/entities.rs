//! Channel and DM entities — moved verbatim from the former `mock` module
//! (huddle clean-architecture refactor, task 01). The shared dataset that
//! materializes these types now lives in [`crate::data::store`].

/// A channel (a named, possibly private, multi-member conversation).
#[derive(Clone, Copy, Debug)]
pub struct Channel {
    /// Stable string id (used in `/channel/:id`).
    pub id: &'static str,
    /// Display name (without the leading `#`).
    pub name: &'static str,
    /// Whether the channel is private (rendered with a lock icon).
    pub private: bool,
    /// One-line channel topic.
    pub topic: &'static str,
}

/// A direct message conversation (1:1 with a [`User`]).
#[derive(Clone, Copy, Debug)]
pub struct Dm {
    /// Stable string id (used in `/channel/:id`, DMs route through the same
    /// feed screen as channels).
    pub id: &'static str,
    /// The other participant.
    pub user_id: u32,
    /// A short preview of the latest message (roster subtitle).
    pub preview: &'static str,
}
