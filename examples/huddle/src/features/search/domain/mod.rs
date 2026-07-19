//! `search` domain — live filter over channels, people, and messages
//! (huddle clean-architecture refactor, task 04). **Sync, infallible**
//! (PLAN Design Decision 8): no `async`, no `HuddleFailure`, no
//! `ControllerCore`/`UseCase` — see [`repositories`]'s module docs.

pub mod filter;
pub mod models;
pub mod repositories;

pub use filter::filter;
pub use models::{MAX_MESSAGE_HITS, MessageHit, SearchResults};
pub use repositories::SearchRepository;
