//! `channels` data layer (huddle clean-architecture refactor, task 02): the
//! store-backed [`ChannelRepository`](super::domain::repositories::ChannelRepository)
//! implementation. The only channels layer that reads [`crate::data::store`].

pub mod repositories;
