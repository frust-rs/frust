//! The messages feature's use cases (huddle clean-architecture refactor,
//! task 03) — moved from the former flat `features/messages/mod.rs`, now behind
//! the [`MessageRepository`](super::repositories::MessageRepository) seam.
//!
//! - [`LoadMessages`] — the async message-load use case (takes the repo `Arc`,
//!   the team-demo `LoadTeam` shape). Sends / reactions / thread replies /
//!   pagination are synchronous controller-level signal patches (no data-layer
//!   round trip), so they stay methods on the controller rather than use cases.

pub mod load_messages;

pub use load_messages::LoadMessages;
