//! Search feature — live filter over channels, people, and messages (huddle
//! clean-architecture refactor, task 04: sliced into
//! `domain`/`data`/`presentation`). **Sync, infallible** (PLAN Design
//! Decision 8): see [`domain::repositories::SearchRepository`]'s module docs
//! for why this slice carries no `HuddleFailure`/`ControllerCore`/async.
//!
//! See [`data::repositories::StoreSearchRepository`] for the store-backed
//! repository implementation and
//! [`presentation::controllers::SearchController`] for the view model
//! driving [`presentation::pages::search`].

pub mod data;
pub mod domain;
pub mod presentation;

pub use domain::{MAX_MESSAGE_HITS, MessageHit, SearchResults};
pub use presentation::SearchController;
