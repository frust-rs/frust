//! `search` domain — live filter over channels, people, and messages.
//! **Sync, infallible**: no `async`, no `HuddleFailure`, no
//! `ControllerCore`/`UseCase` — see [`repositories`]'s module docs.

pub mod filter;
pub mod models;
pub mod repositories;

pub use filter::filter;
pub use models::{MAX_MESSAGE_HITS, MessageHit, SearchResults};
pub use repositories::SearchRepository;
