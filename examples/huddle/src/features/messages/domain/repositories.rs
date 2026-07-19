//! The messages feature's domain repository contract (huddle
//! clean-architecture refactor, task 03).
//!
//! [`MessageRepository`] is the seam between the feed/thread presentation and
//! the data layer: the [`LoadMessages`](super::use_cases::LoadMessages) use case
//! (and the controller's pagination / total-count reads) load a channel's
//! messages through [`messages_for`](MessageRepository::messages_for), and the
//! feed/thread pages read the author/channel reference metadata they render
//! (avatar initials, app-bar titles, the thread's root-message lookup) through
//! the sync accessors below. The data layer's `StoreMessageRepository`
//! implements it over the shared in-memory store, owning the feature's single
//! failure-mapping site. Consumed as `Arc<dyn MessageRepository + Send + Sync>`
//! (the team-demo DI convention).
//!
//! The signature is shaped by exactly what the controller / use cases / pages
//! read today (no speculative methods). Only [`messages_for`](MessageRepository::messages_for)
//! is async + fallible (the loader that a real transport would await); the
//! reference reads are synchronous, infallible store lookups (PLAN Design
//! Decision 8 — no `Result`/`async` ceremony for infallible reads). Returning
//! another feature's domain type ([`Channel`]/[`Dm`]/[`User`]) is a
//! domain→domain reuse (Design Decision 2), not a layer break.

use crate::failure::HuddleFailure;
use crate::features::channels::domain::{Channel, Dm};
use crate::features::messages::domain::entities::Message;
use crate::features::profile::domain::User;

/// The same dual `cfg_attr` every async trait in this codebase uses (per
/// `docs/CODE_STANDARDS.md`) — `wasm32`'s single-threaded event loop can't
/// require `Send` futures.
#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
pub trait MessageRepository {
    /// All messages in a channel or DM, oldest-first (the firehose generates its
    /// 120). Returns cleanly today — the (currently branch-free) failure
    /// boundary lives in the implementation; the simulated load latency lives in
    /// the caller (the use case / controller), which is per-controller and
    /// test-tunable via `MessagesController::with_latency`.
    async fn messages_for(&self, channel_id: &str) -> Result<Vec<Message>, HuddleFailure>;

    /// Whether `channel_id` is the paginated `#firehose` channel (drives the
    /// newest-page load cap + the "has older" flag).
    fn is_firehose(&self, channel_id: &str) -> bool;

    /// Look up a channel by id (the feed/thread app-bar title).
    fn channel(&self, id: &str) -> Option<Channel>;

    /// All direct-message conversations (the app-bar's DM-peer derivation).
    fn dms(&self) -> Vec<Dm>;

    /// All workspace members (the app-bar's member count).
    fn users(&self) -> Vec<User>;

    /// Look up a member by id (avatar initials / author names).
    fn user(&self, id: u32) -> Option<User>;

    /// Every authored channel/DM message (the thread screen's root-message
    /// lookup, chained with [`firehose_messages`](MessageRepository::firehose_messages)).
    fn messages(&self) -> Vec<Message>;

    /// The generated `#firehose` messages (the other half of the thread screen's
    /// root-message lookup).
    fn firehose_messages(&self) -> Vec<Message>;
}
