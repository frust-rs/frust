//! [`StoreMessageRepository`] — the [`MessageRepository`] implementation this
//! app wires up (huddle clean-architecture refactor, task 03).
//!
//! Adapts the shared in-memory [`crate::data::store`] to the messages domain
//! trait and owns this feature's single failure-mapping boundary. The store
//! reads are infallible today, so the boundary is currently branch-free — the
//! [`messages_for`](MessageRepository::messages_for) method preserves the exact
//! behavior of the pre-refactor `mock::messages_for` read (a clean channel load;
//! the simulated latency lives in the caller — the use case / controller — since
//! it is per-controller and test-tunable). If a future real source can fail, the
//! `map_*_error -> HuddleFailure` conversion lands here, at this one site, and
//! nothing above the data layer changes.
//!
//! This is the **only** messages file that mentions [`crate::data::store`] (the
//! inward dependency rule, PLAN Design Decision 3).

use crate::failure::HuddleFailure;
use crate::features::channels::domain::{Channel, Dm};
use crate::features::messages::domain::entities::Message;
use crate::features::messages::domain::repositories::MessageRepository;
use crate::features::profile::domain::User;

/// The store-backed [`MessageRepository`]: materializes messages and the
/// author/channel reference metadata from the shared immutable dataset.
#[derive(Clone, Copy, Default)]
pub struct StoreMessageRepository;

impl StoreMessageRepository {
    /// A fresh repository handle (the dataset it reads is process-global static
    /// state, so the handle carries no state of its own).
    pub fn new() -> Self {
        Self
    }
}

#[cfg_attr(not(target_arch = "wasm32"), clean_signals::async_trait)]
#[cfg_attr(target_arch = "wasm32", clean_signals::async_trait(?Send))]
impl MessageRepository for StoreMessageRepository {
    async fn messages_for(&self, channel_id: &str) -> Result<Vec<Message>, HuddleFailure> {
        Ok(crate::data::store::messages_for(channel_id))
    }

    fn is_firehose(&self, channel_id: &str) -> bool {
        channel_id == crate::data::store::FIREHOSE_ID
    }

    fn channel(&self, id: &str) -> Option<Channel> {
        crate::data::store::channel(id)
    }

    fn dms(&self) -> Vec<Dm> {
        crate::data::store::dms()
    }

    fn users(&self) -> Vec<User> {
        crate::data::store::users()
    }

    fn user(&self, id: u32) -> Option<User> {
        crate::data::store::user(id)
    }

    fn messages(&self) -> Vec<Message> {
        crate::data::store::messages()
    }

    fn firehose_messages(&self) -> Vec<Message> {
        crate::data::store::firehose_messages()
    }
}
