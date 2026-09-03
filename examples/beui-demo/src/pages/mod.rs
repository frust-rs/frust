//! One module per gallery page.
//!
//! [`home`] and [`theming`] carry real interactive state today (both fields
//! live on [`crate::AppState`]); every page under [`motion`], [`agents`], and
//! [`blocks`] is a [`stub_page`] placeholder — `b-20`/`b-21`/`b-22` replace
//! each one with the section's real components and, where a page ends up
//! needing it, its own `State` field on [`crate::AppState`].

pub mod agents;
pub mod blocks;
pub mod home;
pub mod motion;
pub mod theming;

use frust::{AnyView, Column, SizedBox, any};

use crate::AppState;
use crate::nav::{caption, heading};

/// A titled placeholder page: the section's own title plus a one-line note on
/// which task fills it in. Every stub leaf under [`motion`], [`agents`], and
/// [`blocks`] renders through this, so the whole nav tree compiles and shows
/// a title from day one.
pub fn stub_page(title: &str, note: &str) -> AnyView<AppState> {
    any(Column(vec![
        any(heading(title)),
        any(SizedBox(None, Some(12.0))),
        any(caption(note.to_string())),
    ]))
}
