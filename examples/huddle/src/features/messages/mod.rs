//! The `messages` feature slice: `domain` ← `data` / `domain` ←
//! `presentation`, per the inward-only dependency rule. The channel/DM
//! message feed and its thread view — the app's largest slice.
//!
//! The former flat `features/messages/mod.rs` (controller + use case + feed
//! models) split across the layers:
//! - [`domain`] — entities, the owned [`FeedMessage`]/[`FeedBody`]/…
//!   models, the [`MessageRepository`](domain::MessageRepository) contract, and
//!   the `LoadMessages` use case.
//! - [`data`] — `StoreMessageRepository` (the trait impl over the shared store;
//!   the feature's single failure-mapping site + the reference-metadata reads).
//! - [`presentation`] — [`MessagesController`] and the `channel_feed`/`thread`
//!   pages.
//!
//! The re-exports below are the feature's public facade: consumers keep using
//! `messages::MessagesController`, `messages::FeedMessage`, `messages::PAGE_SIZE`,
//! etc. without reaching into layer paths.

pub mod data;
pub mod domain;
pub mod presentation;

pub use domain::{FeedBody, FeedMessage, FeedReaction, FeedReply, PAGE_SIZE};
pub use presentation::controllers::MessagesController;
