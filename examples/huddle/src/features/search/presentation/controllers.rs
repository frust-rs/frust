//! [`SearchController`] — the Search tab's view model (huddle
//! clean-architecture refactor, task 04; moved verbatim apart from repo
//! threading from the former flat `features/search/mod.rs`).
//!
//! Owns the query `RwSignal<String>` and a `Memo`-derived
//! [`SearchResults`](crate::features::search::domain::SearchResults)
//! (`PLAN.md`'s "Search live filter" row: `signals + Memo`, the same
//! "roster pattern" it names). There is no async use case here: the whole
//! dataset is already resident (behind the injected
//! [`SearchRepository`](crate::features::search::domain::SearchRepository)),
//! so filtering is a pure, synchronous function of the query string — no
//! `ControllerCore`/`UseCase` composition is needed the way
//! `features::settings` uses one for its (also synchronous) theme
//! application; this feature's spec item is explicit that search "is
//! instant/local" (PLAN Design Decision 8).
//!
//! # Why a thread-local instance, not a `Component`
//!
//! The `/search` route (`src/routes.rs`, a shared hub file per
//! `src/README-phase-c.md`) calls
//! [`crate::features::search::presentation::pages::search::search_screen`]
//! with **no arguments**, and the navigator re-runs that zero-arg page
//! builder on every app-wide rebuild while the page is retained
//! (`docs/ARCHITECTURE.md`'s Navigation flow: "2. Reconcile every retained
//! page ... by re-running its builder against live state") — so a plain
//! local variable (or a signal created fresh inside the function body) would
//! be discarded every frame, losing whatever the user just typed.
//!
//! Wrapping the screen in a `Component` would fix *that* (a `Component`'s
//! `State` is retained across rebuilds — the pattern
//! `settings::presentation::pages::settings_appearance` already uses) — but
//! it isn't usable here:
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
//! `screens::home`/`profile::presentation::pages::you`/`screens::channel_feed`'s `Button`
//! callbacks do — so the query/results state instead lives behind
//! [`SearchController::instance`], a lazily-created, thread-local singleton.
//! This is correctly scoped: this app's `RenderRoot`/reactive runtime pump
//! always runs on one thread, and a real process only ever has one; each
//! `#[test]` in `tests/search.rs` gets its own fresh OS thread (verified
//! empirically — Rust's default test harness spawns per-test, never reuses
//! one for a later test), so the thread-local never leaks across tests
//! either.
//!
//! # Repository injection
//!
//! [`SearchController::instance`] recovers the injected `Arc<dyn
//! SearchRepository + Send + Sync>` via `use_context` the moment it lazily
//! constructs the controller (production's composition root,
//! `crate::HuddleApp::init`, publishes it once) — the same cleaner,
//! `cfg(test)`-free injection task 02 establishes (no external test crate
//! constructs `SearchController` directly — see the completion summary). The
//! repo is moved into [`Self::results`]'s `Memo` closure, so `filter` reads
//! through it on every recompute.

use std::cell::RefCell;
use std::sync::Arc;

use frust::{Get, GetUntracked, Memo, RwSignal, use_context};

use crate::features::search::domain::{SearchRepository, SearchResults, filter};

/// The Search screen's view model: a query signal plus its `Memo`-derived
/// [`SearchResults`]. `Copy`/`Clone`: both fields are cheap reactive-graph
/// handles, not the underlying data.
#[derive(Clone, Copy)]
pub struct SearchController {
    /// The live query text (the search field's controlled value).
    pub query: RwSignal<String>,
    /// The query's live filtered results, recomputed on every `query` write.
    pub results: Memo<SearchResults>,
}

impl SearchController {
    fn new(repo: Arc<dyn SearchRepository + Send + Sync>) -> Self {
        let query = RwSignal::new(String::new());
        let results =
            Memo::new(move |_prev: Option<&SearchResults>| filter(repo.as_ref(), &query.get()));
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
            let repo = use_context::<Arc<dyn SearchRepository + Send + Sync>>()
                .expect("the composition root provides a SearchRepository");
            let fresh = SearchController::new(repo);
            *cell.borrow_mut() = Some(fresh);
            fresh
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::search::data::repositories::StoreSearchRepository;

    /// The test-wiring composition root: the real store-backed repository
    /// behind the domain trait object. `#[cfg(test)]`-only wiring, so it is
    /// the sole place the presentation layer touches `data/` — exactly as
    /// the production composition root (`crate::HuddleApp::init`) is (task
    /// 02's completion-summary pattern note; production presentation
    /// imports no `data/`). Search's tests assert against the REAL dataset
    /// shape (a "leadership" channel, "ada"-matching users, capped firehose
    /// hits), so — unlike Activity's `FakeActivityRepository` — the real
    /// store-backed repo is what keeps every assertion verbatim.
    fn store_repo() -> Arc<dyn SearchRepository + Send + Sync> {
        Arc::new(StoreSearchRepository::new())
    }

    #[test]
    fn empty_query_yields_the_empty_result() {
        let repo = store_repo();
        let r = filter(repo.as_ref(), "");
        assert!(r.is_empty());
        let r = filter(repo.as_ref(), "   ");
        assert!(r.is_empty(), "whitespace-only query is treated as empty");
    }

    #[test]
    fn matches_channels_by_name_case_insensitively() {
        let repo = store_repo();
        let r = filter(repo.as_ref(), "LEADERSHIP");
        assert_eq!(r.channels.len(), 1);
        assert_eq!(r.channels[0].id, "leadership");
        assert!(r.users.is_empty());
        assert!(r.messages.is_empty());
    }

    #[test]
    fn matches_users_by_name_or_derived_handle() {
        let repo = store_repo();
        let r = filter(repo.as_ref(), "ada");
        assert_eq!(r.users.len(), 1);
        assert_eq!(
            r.users[0].id,
            crate::features::profile::domain::CURRENT_USER_ID
        );

        // "ada lovelace" (with a space) does NOT contain "adalovelace", so
        // this only matches through the derived-handle branch.
        let r = filter(repo.as_ref(), "adalovelace");
        assert_eq!(r.users.len(), 1, "the derived handle matches too");
    }

    #[test]
    fn message_hits_are_capped() {
        // "ada" matches 3 authored @-mentions plus 20 firehose recurrences of
        // the "@Ada Lovelace nightly build is green" body (one in every 6 of
        // the 120-message firehose) — 23 raw hits, capped to
        // MAX_MESSAGE_HITS.
        let repo = store_repo();
        let r = filter(repo.as_ref(), "ada");
        assert_eq!(
            r.messages.len(),
            crate::features::search::domain::MAX_MESSAGE_HITS
        );
    }

    #[test]
    fn no_match_yields_all_empty_sections() {
        let repo = store_repo();
        let r = filter(repo.as_ref(), "zzzznotarealquery");
        assert!(r.is_empty());
    }

    // `SearchController::instance()` needs an active `ReactiveRuntime`/`Owner`
    // (RwSignal::new/Memo::new) that this library-crate unit test module has
    // no harness for — its "same instance across calls" behavior is verified
    // in `tests/search.rs` instead, where `tests/support::setup()` installs
    // one.
}
