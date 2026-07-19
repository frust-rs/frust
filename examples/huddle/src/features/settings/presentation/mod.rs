//! `settings` presentation — [`SettingsController`], [`NotificationsController`],
//! and the four settings pages (huddle clean-architecture refactor, task 05).

pub mod controllers;
pub mod pages;

pub use controllers::{NotificationsController, SettingsController};
