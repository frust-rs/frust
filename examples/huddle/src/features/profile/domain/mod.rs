//! `profile` domain — entities + the repository seam (huddle
//! clean-architecture refactor, tasks 01 and 05).

pub mod entities;
pub mod repositories;

pub use entities::{CURRENT_USER_ID, User, UserStatus};
pub use repositories::ProfileRepository;
