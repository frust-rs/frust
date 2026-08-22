//! The gallery's catalog: what a section lists, and what a row opens.
//!
//! Pure data — no view code. `entries` holds the table itself;
//! [`DemoSection`] and [`DemoEntry`] are the two types it is expressed in,
//! and every consumer
//! (the navigation bar, the section list pane, the `/playground/:id` route)
//! reads it from here rather than keeping a list of its own.

mod entries;
mod entry;
mod section;

pub use entries::{find_by_id, for_section};
pub use entry::DemoEntry;
pub use section::{DemoSection, SPLIT_BREAKPOINT_PX};
