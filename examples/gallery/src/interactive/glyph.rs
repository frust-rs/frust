//! Stateful constructors for the Glyph design system's cases (`glyph/*`
//! slugs) — see the [module docs](super) for what this table is and why it
//! sits beside the registry rather than inside it.
//!
//! Empty for now: every Glyph case still builds from its own
//! [`crate::case::Case::build`], which is what a host falls back to when
//! [`super::find`] returns `None`. Adding a row here is the whole
//! registration — nothing in [`super`] changes.

use super::Entry;

/// This catalog's slice of the side table [`super::entries`] concatenates.
pub const INTERACTIVE: &[Entry] = &[];
