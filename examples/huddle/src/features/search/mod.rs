//! Search feature domain — live filter over channels, people, and messages.
//!
//! [`SearchController`] owns the query `RwSignal<String>` and a `Memo`-derived
//! [`SearchResults`] (`PLAN.md`'s "Search live filter" row: `signals + Memo`,
//! the same "roster pattern" it names). There is no async use case here: the
//! whole dataset is already resident (`mock::*`), so filtering is a pure,
//! synchronous function of the query string — no `ControllerCore`/`UseCase`
//! composition is needed the way `features::settings` uses one for its
//! (also synchronous) theme application; this feature's spec item is
//! explicit that search "is instant/local".
//!
//! # Why a thread-local instance, not a `Component`
//!
//! The `/search` route (`src/routes.rs`, a frozen hub file per
//! `src/README-phase-c.md`) calls [`crate::screens::search::search_screen`]
//! with **no arguments**, and the navigator re-runs that zero-arg page
//! builder on every app-wide rebuild while the page is retained
//! (`docs/ARCHITECTURE.md`'s Navigation flow: "2. Reconcile every retained
//! page ... by re-running its builder against live state") — so a plain
//! local variable (or a signal created fresh inside the function body) would
//! be discarded every frame, losing whatever the user just typed.
//!
//! Wrapping the screen in a `Component` would fix *that* (a `Component`'s
//! `State` is retained across rebuilds — the pattern
//! `screens::settings_appearance` already uses) — but it isn't usable here:
//! a `Component`'s inner event handlers only ever see the component's own
//! local `State`, never the outer `HuddleState`
//! (`docs/ARCHITECTURE.md`'s Component state boundary — a deliberate
//! isolation, not an oversight), and a result row's `on_press` needs
//! `&mut HuddleState` to reach `state.nav.router().push(..)` — the frozen
//! route gives search no other way to reach the navigator (no
//! `NavigatorController` parameter like `screens::home`/`channel_feed` get,
//! and nothing provides one via `use_context` either). Keeping
//! `search_screen` a **plain** (non-`Component`) function is what lets its
//! row callbacks close over `&mut HuddleState` directly, exactly like
//! `screens::home`/`screens::you`/`screens::channel_feed`'s `Button`
//! callbacks do — so the query/results state instead lives behind
//! [`SearchController::instance`], a lazily-created, thread-local singleton.
//! This is correctly scoped: this app's `RenderRoot`/reactive runtime pump
//! always runs on one thread, and a real process only ever has one; each
//! `#[test]` in `tests/search.rs` gets its own fresh OS thread (verified
//! empirically — Rust's default test harness spawns per-test, never reuses
//! one for a later test), so the thread-local never leaks across tests
//! either.

use std::cell::RefCell;

use frust::{Get, GetUntracked, Memo, RwSignal};

use crate::mock;

/// A text-matched message: the fields a search result row needs, without
/// re-deriving them from [`mock::Message`] at render time.
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
    pub channels: Vec<mock::Channel>,
    /// Members whose name or derived handle matches.
    pub users: Vec<mock::User>,
    /// Text messages whose body matches, capped at [`MAX_MESSAGE_HITS`].
    pub messages: Vec<MessageHit>,
}

/// Id-based equality (the `Memo` recompute gate). Mock rows are static data
/// keyed by id, so section-wise id equality is exact result equality; the
/// frozen `mock` types themselves don't derive `PartialEq`.
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
    /// via [`filter`], but the screen shows its own hint for that case rather
    /// than "no results").
    pub fn is_empty(&self) -> bool {
        self.channels.is_empty() && self.users.is_empty() && self.messages.is_empty()
    }
}

/// Message-hit cap (spec: "cap message hits at ~20") — the dataset's 160
/// messages (40 authored + the 120-message `#firehose`) could otherwise flood
/// a broad query's result list.
pub const MAX_MESSAGE_HITS: usize = 20;

/// A member's searchable handle, derived from their display name (the mock
/// dataset — a frozen hub file, see `src/README-phase-c.md` — has no
/// `handle` field of its own): lowercased with spaces stripped, e.g. "Ada
/// Lovelace" → "adalovelace".
fn handle_for(name: &str) -> String {
    name.to_lowercase().split_whitespace().collect()
}

/// The pure filter: case-insensitive substring match against channel names,
/// user names/derived handles, and message text bodies. An empty (or
/// whitespace-only) `query` yields the empty [`SearchResults`] — showing the
/// "type to search" hint for that case is the screen's job, not this
/// function's.
pub fn filter(query: &str) -> SearchResults {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return SearchResults::default();
    }

    let channels = mock::channels()
        .into_iter()
        .filter(|c| c.name.to_lowercase().contains(&q))
        .collect();

    let users = mock::users()
        .into_iter()
        .filter(|u| u.name.to_lowercase().contains(&q) || handle_for(u.name).contains(&q))
        .collect();

    let messages = mock::messages()
        .into_iter()
        .chain(mock::firehose_messages())
        .filter_map(|m| match m.body {
            mock::MessageBody::Text(text) if text.to_lowercase().contains(&q) => Some(MessageHit {
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

/// The Search screen's view model: a query signal plus its `Memo`-derived
/// [`SearchResults`] (spec: "signals + `Memo`"). `Copy`/`Clone`: both fields
/// are cheap reactive-graph handles, not the underlying data.
#[derive(Clone, Copy)]
pub struct SearchController {
    /// The live query text (the search field's controlled value).
    pub query: RwSignal<String>,
    /// The query's live filtered results, recomputed on every `query` write.
    pub results: Memo<SearchResults>,
}

impl SearchController {
    fn new() -> Self {
        let query = RwSignal::new(String::new());
        let results = Memo::new(move |_prev: Option<&SearchResults>| filter(&query.get()));
        Self { query, results }
    }

    /// The single, lazily-created controller instance for this thread — see
    /// the [module docs](self)'s "Why a thread-local instance" section.
    pub fn instance() -> Self {
        thread_local! {
            static INSTANCE: RefCell<Option<SearchController>> = const { RefCell::new(None) };
        }
        INSTANCE.with(|cell| {
            // Self-heal (task 22 hardening): the cached controller's `query`
            // signal (and the `results` `Memo` derived from it) are owned by the
            // reactive `Owner` live when `new` ran; a headless test that disposes
            // its owner and re-enters (or a reused test thread) leaves a disposed
            // controller here whose next `query.get()` panics. Probing `query`
            // with `try_get_untracked` detects that and recreates — the same
            // guard `features::messages`'s registry and the screen composer
            // caches now carry.
            let cached = *cell.borrow();
            if let Some(existing) = cached
                && existing.query.try_get_untracked().is_some()
            {
                return existing;
            }
            let fresh = SearchController::new();
            *cell.borrow_mut() = Some(fresh);
            fresh
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_yields_the_empty_result() {
        let r = filter("");
        assert!(r.is_empty());
        let r = filter("   ");
        assert!(r.is_empty(), "whitespace-only query is treated as empty");
    }

    #[test]
    fn matches_channels_by_name_case_insensitively() {
        let r = filter("LEADERSHIP");
        assert_eq!(r.channels.len(), 1);
        assert_eq!(r.channels[0].id, "leadership");
        assert!(r.users.is_empty());
        assert!(r.messages.is_empty());
    }

    #[test]
    fn matches_users_by_name_or_derived_handle() {
        let r = filter("ada");
        assert_eq!(r.users.len(), 1);
        assert_eq!(r.users[0].id, mock::CURRENT_USER_ID);

        // "ada lovelace" (with a space) does NOT contain "adalovelace", so
        // this only matches through the derived-handle branch.
        let r = filter("adalovelace");
        assert_eq!(r.users.len(), 1, "the derived handle matches too");
    }

    #[test]
    fn message_hits_are_capped() {
        // "ada" matches 3 authored @-mentions plus 20 firehose recurrences of
        // the "@Ada Lovelace nightly build is green" body (one in every 6 of
        // the 120-message firehose) — 23 raw hits, capped to
        // MAX_MESSAGE_HITS.
        let r = filter("ada");
        assert_eq!(r.messages.len(), MAX_MESSAGE_HITS);
    }

    #[test]
    fn no_match_yields_all_empty_sections() {
        let r = filter("zzzznotarealquery");
        assert!(r.is_empty());
    }

    // `SearchController::instance()` needs an active `ReactiveRuntime`/`Owner`
    // (RwSignal::new/Memo::new) that this library-crate unit test module has
    // no harness for — its "same instance across calls" behavior is verified
    // in `tests/search.rs` instead, where `tests/support::setup()` installs
    // one.
}
