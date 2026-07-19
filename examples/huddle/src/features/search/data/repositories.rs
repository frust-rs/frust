//! [`StoreSearchRepository`] — the [`SearchRepository`] implementation this
//! app wires up (huddle clean-architecture refactor, task 04).
//!
//! Adapts the shared in-memory [`crate::data::store`] to the search domain
//! trait. Every method is a direct, infallible passthrough to a store
//! accessor — search has no failure-mapping site (PLAN Design Decision 8;
//! see [`crate::features::search::domain::repositories`]'s module docs).
//!
//! This is the **only** search file that mentions [`crate::data::store`]
//! (the inward dependency rule, PLAN Design Decision 3).

use crate::data::store;
use crate::features::channels::domain::Channel;
use crate::features::messages::domain::Message;
use crate::features::profile::domain::User;
use crate::features::search::domain::repositories::SearchRepository;

/// The store-backed [`SearchRepository`]: reads straight through to the
/// shared immutable dataset.
#[derive(Clone, Copy, Default)]
pub struct StoreSearchRepository;

impl StoreSearchRepository {
    /// A fresh repository handle (the dataset it reads is process-global
    /// static state, so the handle carries no state of its own).
    pub fn new() -> Self {
        Self
    }
}

impl SearchRepository for StoreSearchRepository {
    fn channels(&self) -> Vec<Channel> {
        store::channels()
    }

    fn users(&self) -> Vec<User> {
        store::users()
    }

    fn user(&self, id: u32) -> Option<User> {
        store::user(id)
    }

    fn messages(&self) -> Vec<Message> {
        store::messages()
    }

    fn firehose_messages(&self) -> Vec<Message> {
        store::firehose_messages()
    }
}
