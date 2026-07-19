//! `channels` domain — entities, derived roster models, the repository
//! contract, and the use cases (huddle clean-architecture refactor: entities
//! landed in task 01; models/repositories/use_cases in task 02).

pub mod entities;
pub mod models;
pub mod repositories;
pub mod use_cases;

pub use entities::{Channel, Dm};
pub use models::{ChannelItem, ChannelsData, DmItem, FlagParams};
pub use repositories::ChannelRepository;
