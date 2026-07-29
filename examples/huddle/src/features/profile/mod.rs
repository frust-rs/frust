//! `profile` feature — a **synchronous** lookup against the shared dataset,
//! no `clean-signals` controller/use-case spine needed: there is no async
//! work here and nothing
//! that can fail, so [`ProfileController::load`] is a plain function taking
//! the injected [`ProfileRepository`](domain::ProfileRepository), not a
//! `clean_signals_frust::use_controller`-hosted `ControllerCore` (contrast
//! [`crate::features::settings::SettingsController`], which genuinely needs
//! one because `SetTheme` is a real, if infallible, use case run through the
//! spine).

pub mod data;
pub mod domain;
pub mod presentation;

pub use domain::{CURRENT_USER_ID, ProfileRepository, User, UserStatus};
pub use presentation::ProfileController;
