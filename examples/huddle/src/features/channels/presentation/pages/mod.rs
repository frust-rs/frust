//! The channels feature's pages (huddle clean-architecture refactor, task 02;
//! moved verbatim from the former `src/screens/`):
//!
//! - [`home`] — the Home tab (channels + DMs roster).
//! - [`workspace_drawer`] — the workspace switcher, a documented controller-less
//!   page (its `WORKSPACES` are a screen-local mock, not shared-dataset state);
//!   it moves here because Home's app-bar tile is its one entry point.

pub mod home;
pub mod workspace_drawer;
