//! `messages` data layer (huddle clean-architecture refactor, task 03): the
//! store-backed [`MessageRepository`](super::domain::repositories::MessageRepository)
//! implementation. The only messages layer that reads [`crate::data::store`].

pub mod repositories;
