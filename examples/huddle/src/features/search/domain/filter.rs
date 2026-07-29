//! [`filter`] — the pure, synchronous search-query filter. Moved verbatim
//! from the former flat `features/search/mod.rs` apart from reading through
//! the injected [`SearchRepository`] instead of the shared dataset
//! ([`crate::data::store`]) directly.

use crate::features::messages::domain::MessageBody;

use super::models::{MAX_MESSAGE_HITS, MessageHit, SearchResults};
use super::repositories::SearchRepository;

/// A member's searchable handle, derived from their display name (the mock
/// dataset has no `handle` field of its own): lowercased with spaces
/// stripped, e.g. "Ada Lovelace" → "adalovelace".
fn handle_for(name: &str) -> String {
    name.to_lowercase().split_whitespace().collect()
}

/// The pure filter: case-insensitive substring match against channel names,
/// user names/derived handles, and message text bodies. An empty (or
/// whitespace-only) `query` yields the empty [`SearchResults`] — showing the
/// "type to search" hint for that case is the screen's job, not this
/// function's.
pub fn filter(repo: &(dyn SearchRepository + Send + Sync), query: &str) -> SearchResults {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return SearchResults::default();
    }

    let channels = repo
        .channels()
        .into_iter()
        .filter(|c| c.name.to_lowercase().contains(&q))
        .collect();

    let users = repo
        .users()
        .into_iter()
        .filter(|u| u.name.to_lowercase().contains(&q) || handle_for(u.name).contains(&q))
        .collect();

    let messages = repo
        .messages()
        .into_iter()
        .chain(repo.firehose_messages())
        .filter_map(|m| match m.body {
            MessageBody::Text(text) if text.to_lowercase().contains(&q) => Some(MessageHit {
                message_id: m.id,
                channel_id: m.channel_id,
                author_id: m.author_id,
                text,
            }),
            _ => None,
        })
        .take(MAX_MESSAGE_HITS)
        .collect();

    SearchResults {
        channels,
        users,
        messages,
    }
}
