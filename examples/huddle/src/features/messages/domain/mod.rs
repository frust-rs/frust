//! `messages` domain — entities, the owned feed models, the repository
//! contract, and the use cases (huddle clean-architecture refactor: entities
//! landed in task 01; models/repositories/use_cases in task 03).

pub mod entities;
pub mod models;
pub mod repositories;
pub mod use_cases;

pub use entities::{Message, MessageBody, Reaction};
pub use models::{FeedBody, FeedMessage, FeedReaction, FeedReply, LoadMessagesParams, PAGE_SIZE};
pub use repositories::MessageRepository;
pub use use_cases::LoadMessages;
