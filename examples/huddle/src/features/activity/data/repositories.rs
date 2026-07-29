//! [`StoreActivityRepository`] — the [`ActivityRepository`] implementation
//! this app wires up (huddle clean-architecture refactor).
//!
//! Adapts the shared in-memory [`crate::data::store`] to the activity domain
//! trait: it materializes the mentions feed from the immutable dataset and
//! owns this feature's single failure-mapping boundary. The store read is
//! infallible today, so the boundary is currently branch-free — the method
//! preserves the exact behavior of the pre-refactor `LoadActivity` (a
//! ~400ms simulated latency then a clean feed; no fail-once-then-retry). If a
//! future real source can fail, the `map_*_error -> HuddleFailure`
//! conversion lands here, at this one site, and nothing above the data layer
//! changes.
//!
//! This is the **only** activity file that mentions [`crate::data::store`]
//! (the inward dependency rule: only the data layer may reach into the
//! shared store; domain/presentation never do).

use std::time::Duration;

use crate::data::store;
use crate::failure::HuddleFailure;
use crate::features::activity::domain::entities::ActivityItem;
use crate::features::activity::domain::repositories::ActivityRepository;
use crate::features::channels::domain::Channel;
use crate::features::profile::domain::User;

/// Mock load latency ("~400ms → skeletons") — long enough that the screen's
/// loading skeletons are actually visible before the feed resolves. Fixed
/// (not per-caller-tunable, unlike `messages`' latency), so it lives here
/// with the store read rather than in the use case.
pub const LOAD_LATENCY: Duration = Duration::from_millis(400);

/// The store-backed [`ActivityRepository`]: materializes the mentions feed
/// from the shared immutable dataset.
#[derive(Clone, Copy, Default)]
pub struct StoreActivityRepository;

impl StoreActivityRepository {
    /// A fresh repository handle (the dataset it reads is process-global
    /// static state, so the handle carries no state of its own).
    pub fn new() -> Self {
        Self
    }
}

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl ActivityRepository for StoreActivityRepository {
    async fn activity_items(&self) -> Result<Vec<ActivityItem>, HuddleFailure> {
        clean_signals::time::sleep(LOAD_LATENCY).await;
        Ok(store::activity())
    }

    fn user(&self, id: u32) -> Option<User> {
        store::user(id)
    }

    fn channel(&self, id: &str) -> Option<Channel> {
        store::channel(id)
    }
}
