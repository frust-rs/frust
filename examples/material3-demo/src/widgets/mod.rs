//! Widgets shared across playgrounds.
//!
//! [`coming_soon()`] is what an un-ported entry renders; [`playground`] is the
//! reference's playground kit (preview cards, control panel, code snippets),
//! which lands with the first batch of real pages.

mod coming_soon;
pub mod playground;

pub use coming_soon::coming_soon;
