//! One module per gallery page.
//!
//! [`home`] and [`theming`] carry their interactive state on
//! [`crate::AppState`]; every section page under [`motion`], [`agents`], and
//! [`blocks`] hosts its own state in a page-local `frust::component`, so the
//! shell's page functions stay stateless and a page starts fresh each time it
//! is navigated to.

pub mod agents;
pub mod blocks;
pub mod home;
pub mod motion;
pub mod theming;
