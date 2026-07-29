//! `activity` domain — entities, the derived row model, the repository
//! contract, and the use cases (huddle clean-architecture refactor).

pub mod entities;
pub mod models;
pub mod repositories;
pub mod use_cases;

pub use entities::ActivityItem;
pub use models::ActivityRow;
pub use repositories::ActivityRepository;
pub use use_cases::{LoadActivity, MarkAllRead, compose_mark_all_read};
