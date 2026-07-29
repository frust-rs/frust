//! Activity domain — the derived feed row (huddle clean-architecture
//! refactor). Moved verbatim from the former flat
//! `features/activity/mod.rs`.

use super::entities::ActivityItem;

/// One activity-feed row: the underlying mention plus its locally-tracked
/// read/unread flag. [`ActivityItem`] carries no read/unread field of its own
/// (the shared dataset — see [`crate::data::store`] — is immutable), so every
/// freshly loaded item starts unread until `MarkAllRead` clears it.
#[derive(Clone, Debug)]
pub struct ActivityRow {
    /// The underlying mention.
    pub item: ActivityItem,
    /// Whether this row is still unread.
    pub unread: bool,
}

/// Id-based equality: [`ActivityItem`] derives no `PartialEq`, and mock rows
/// are static data keyed by `message_id`, so message-id + unread-flag
/// equality is exact row equality.
impl PartialEq for ActivityRow {
    fn eq(&self, other: &Self) -> bool {
        self.item.message_id == other.item.message_id && self.unread == other.unread
    }
}
