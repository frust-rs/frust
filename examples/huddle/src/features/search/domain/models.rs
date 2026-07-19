//! Search domain — the sectioned filter-result model (huddle
//! clean-architecture refactor, task 04). Moved verbatim from the former flat
//! `features/search/mod.rs`.

use crate::features::channels::domain::Channel;
use crate::features::profile::domain::User;

/// A text-matched message: the fields a search result row needs, without
/// re-deriving them from `Message` at render time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MessageHit {
    /// The matched message's id.
    pub message_id: u32,
    /// The channel or DM it belongs to (a result row navigates here).
    pub channel_id: &'static str,
    /// The message's author.
    pub author_id: u32,
    /// The matched text body.
    pub text: &'static str,
}

/// The sectioned filter result: channels/people/messages matching the current
/// query. `Default` is the empty result (empty query, or no matches) — every
/// section empty.
#[derive(Clone, Debug, Default)]
pub struct SearchResults {
    /// Channels whose name matches.
    pub channels: Vec<Channel>,
    /// Members whose name or derived handle matches.
    pub users: Vec<User>,
    /// Text messages whose body matches, capped at [`MAX_MESSAGE_HITS`].
    pub messages: Vec<MessageHit>,
}

/// Id-based equality (the `Memo` recompute gate). Mock rows are static data
/// keyed by id, so section-wise id equality is exact result equality; the
/// entity types themselves don't derive `PartialEq`.
impl PartialEq for SearchResults {
    fn eq(&self, other: &Self) -> bool {
        self.channels.len() == other.channels.len()
            && self.users.len() == other.users.len()
            && self.messages == other.messages
            && self
                .channels
                .iter()
                .zip(&other.channels)
                .all(|(a, b)| a.id == b.id)
            && self
                .users
                .iter()
                .zip(&other.users)
                .all(|(a, b)| a.id == b.id)
    }
}

impl SearchResults {
    /// True when every section is empty (the screen's "no results" state —
    /// only meaningful for a non-empty query; an empty query also yields this
    /// via [`super::filter::filter`], but the screen shows its own hint for
    /// that case rather than "no results").
    pub fn is_empty(&self) -> bool {
        self.channels.is_empty() && self.users.is_empty() && self.messages.is_empty()
    }
}

/// Message-hit cap (spec: "cap message hits at ~20") — the dataset's 160
/// messages (40 authored + the 120-message `#firehose`) could otherwise flood
/// a broad query's result list.
pub const MAX_MESSAGE_HITS: usize = 20;
