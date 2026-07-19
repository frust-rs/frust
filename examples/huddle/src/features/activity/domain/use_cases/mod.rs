//! Activity use cases (huddle clean-architecture refactor, task 04).

pub mod load_activity;
pub mod mark_all_read;

pub use load_activity::LoadActivity;
pub use mark_all_read::{MarkAllRead, compose_mark_all_read};
