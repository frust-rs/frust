//! Channels domain models — the derived roster rows the Home screen renders
//! plus the flag-mutation params (huddle clean-architecture refactor, task 02).
//!
//! These are the *derived* domain types (per PLAN Design Decision 1's
//! entities-vs-derived-rows split): [`ChannelItem`]/[`DmItem`] enrich the raw
//! [`Channel`](super::entities::Channel)/[`Dm`](super::entities::Dm) entities
//! with per-row unread/archived/muted state, and [`ChannelsData`] is the loaded
//! roster the [`ChannelRepository`](super::repositories::ChannelRepository)
//! produces. Plain data — no `frust`/reactive/data-layer imports (the inward
//! dependency rule); the cross-feature [`UserStatus`] import is a domain→domain
//! reuse, which the convention allows (PLAN Design Decision 2).

use crate::features::profile::domain::UserStatus;

/// One channel roster row.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelItem {
    /// Stable channel id (`/channel/:id`).
    pub id: String,
    /// Display name (without the leading `#`).
    pub name: String,
    /// Whether the channel is private (rendered with a lock icon).
    pub private: bool,
    /// One-line preview (the channel topic).
    pub preview: String,
    /// Unread message count (drives the badge).
    pub unread: u32,
    /// Whether the row has been archived (swipe-right).
    pub archived: bool,
    /// Whether the row has been muted (swipe-left).
    pub muted: bool,
}

/// One direct-message roster row.
#[derive(Clone, Debug, PartialEq)]
pub struct DmItem {
    /// Stable DM channel id (`/channel/dm-:id`).
    pub id: String,
    /// The other participant's user id (`/user/:id`).
    pub user_id: u32,
    /// The participant's display name.
    pub name: String,
    /// Two-letter initials for the avatar.
    pub initials: String,
    /// The participant's presence (the avatar status dot).
    pub status: UserStatus,
    /// A short preview of the latest message.
    pub preview: String,
    /// Unread message count.
    pub unread: u32,
    /// Whether the row has been archived.
    pub archived: bool,
    /// Whether the row has been muted.
    pub muted: bool,
}

/// The loaded roster: channels + DMs.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ChannelsData {
    /// The channel section.
    pub channels: Vec<ChannelItem>,
    /// The direct-message section.
    pub dms: Vec<DmItem>,
}

/// Parameters for `ArchiveChannel`/`MuteChannel`: which row, and the new
/// flag value (`true` sets, `false` restores).
#[derive(Clone, Debug)]
pub struct FlagParams {
    /// The channel or DM id.
    pub id: String,
    /// The new flag value.
    pub value: bool,
}
