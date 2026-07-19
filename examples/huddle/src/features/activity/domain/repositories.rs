//! The activity feature's domain repository contract (huddle
//! clean-architecture refactor, task 04).
//!
//! [`ActivityRepository`] is the seam between the presentation controller /
//! its use cases and the data layer: [`LoadActivity`](super::use_cases::LoadActivity)
//! reads the mentions feed through this trait's one fallible async method,
//! and the data layer's `StoreActivityRepository` implements it over the
//! shared in-memory store, owning the feature's single failure-mapping site.
//! Consumed as `Arc<dyn ActivityRepository + Send + Sync>` (the team-demo DI
//! convention, mirroring `ChannelRepository`/`MessageRepository`).
//!
//! [`user`](ActivityRepository::user)/[`channel`](ActivityRepository::channel)
//! are synchronous, infallible reference reads a feed row needs to render its
//! headline (the actor's name, the mentioned channel's name) — cross-feature
//! reference reads routed through this feature's own repository seam, the
//! same shape `MessageRepository`'s `user`/`channel`/`dms`/`users` methods
//! establish (task 03's completion-summary refinement 3).

use crate::failure::HuddleFailure;
use crate::features::activity::domain::entities::ActivityItem;
use crate::features::channels::domain::Channel;
use crate::features::profile::domain::User;

/// The same dual `cfg_attr` every async trait in this codebase uses (per
/// `docs/CODE_STANDARDS.md`) — `wasm32`'s single-threaded event loop can't
/// require `Send` futures.
#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
pub trait ActivityRepository {
    /// Load the mentions feed. Returns cleanly today — the simulated latency
    /// and the (currently branch-free) failure boundary live in the
    /// implementation.
    async fn activity_items(&self) -> Result<Vec<ActivityItem>, HuddleFailure>;

    /// Look up the actor who authored a mention — a row's headline needs
    /// their name/initials; [`ActivityItem`] carries only the raw
    /// `author_id`.
    fn user(&self, id: u32) -> Option<User>;

    /// Look up the mentioned channel's display name (a row falls back to the
    /// raw id if this misses — should never happen against the mock data).
    fn channel(&self, id: &str) -> Option<Channel>;
}
