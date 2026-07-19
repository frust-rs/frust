//! Activity feature — the mentions feed's async load + local read-state
//! controller (huddle clean-architecture refactor, task 04: sliced into
//! `domain`/`data`/`presentation` following task 02's established pattern).
//!
//! See [`domain::repositories::ActivityRepository`] for the data seam,
//! [`data::repositories::StoreActivityRepository`] for its store-backed
//! implementation, and [`presentation::controllers::ActivityController`] for
//! the view model driving [`presentation::pages::activity`].

pub mod data;
pub mod domain;
pub mod presentation;

pub use domain::{ActivityItem, ActivityRow};
pub use presentation::ActivityController;
