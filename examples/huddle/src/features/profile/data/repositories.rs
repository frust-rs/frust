//! `profile` data — [`StoreProfileRepository`], the sole store reader for the
//! profile feature (huddle clean-architecture refactor, task 05).
//!
//! Synchronous and infallible (PLAN Design Decision 8): every method is a
//! direct passthrough to a [`crate::data::store`] accessor, so — like
//! `search`'s data impl (task 04) — there is no failure-mapping site.

use crate::features::channels::domain::Dm;
use crate::features::profile::domain::User;
use crate::features::profile::domain::repositories::ProfileRepository;

/// The store-backed [`ProfileRepository`] impl.
#[derive(Default)]
pub struct StoreProfileRepository;

impl StoreProfileRepository {
    /// A fresh repository over the shared store.
    pub fn new() -> Self {
        Self
    }
}

impl ProfileRepository for StoreProfileRepository {
    fn user(&self, id: u32) -> Option<User> {
        crate::data::store::user(id)
    }

    fn dms(&self) -> Vec<Dm> {
        crate::data::store::dms()
    }
}
