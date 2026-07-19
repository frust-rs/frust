//! `messages` domain — entities (huddle clean-architecture refactor, task 01).
//! Repository traits and use cases land in a later task
//! (`workflow/plans/features/huddle-clean-architecture/`).

pub mod entities;

pub use entities::{Message, MessageBody, Reaction};
