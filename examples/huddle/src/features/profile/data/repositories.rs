//! `profile` data — [`StoreProfileRepository`], the sole store reader for the
//! profile feature.
//!
//! Synchronous and infallible: every method is a direct passthrough to a
//! [`crate::data::store`] accessor, so — like `search`'s data impl — there
//! is no failure-mapping site.

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
