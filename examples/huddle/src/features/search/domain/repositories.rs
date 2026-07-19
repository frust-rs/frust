//! The search feature's domain repository contract (huddle
//! clean-architecture refactor, task 04) — **sync, infallible** (PLAN Design
//! Decision 8: search has no `ControllerCore`/`UseCase` spine and nothing
//! that can fail — see [`super`]'s module docs, preserved here).
//!
//! [`SearchRepository`] carries only plain synchronous reads shaped by what
//! [`filter`](super::filter::filter)/the search page actually read today
//! (`channels`/`users`/`messages`/`firehose_messages` for the filter itself,
//! `user` for a message-hit row's author lookup) — **no `async`, no
//! `HuddleFailure`**. If a future real data source makes search fallible,
//! this trait widens then (PLAN Design Decision 8's own closing note).

use crate::features::channels::domain::Channel;
use crate::features::messages::domain::Message;
use crate::features::profile::domain::User;

/// The seam between the search presentation layer and the data layer:
/// `StoreSearchRepository` implements this over the shared in-memory store.
/// Consumed as `Arc<dyn SearchRepository + Send + Sync>` (the same DI
/// convention every other feature repository uses), even though every method
/// is synchronous — search's controller still needs to move the repo into a
/// `'static` `Memo` closure.
pub trait SearchRepository {
    /// All channels (the filter's channel-name match source).
    fn channels(&self) -> Vec<Channel>;
    /// All workspace members (the filter's name/handle match source).
    fn users(&self) -> Vec<User>;
    /// Look up a member by id (a message-hit row's author lookup).
    fn user(&self, id: u32) -> Option<User>;
    /// The ~40 authored channel/DM messages (the filter's text-match source).
    fn messages(&self) -> Vec<Message>;
    /// The 120-message `#firehose` channel (also filtered for text matches).
    fn firehose_messages(&self) -> Vec<Message>;
}
