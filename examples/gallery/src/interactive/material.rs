//! Stateful constructors for the Material design system's cases (`material/*`
//! slugs) — see the [module docs](super) for what this table is and why it
//! sits beside the registry rather than inside it.
//!
//! Empty for now: every Material case still builds from its own
//! [`crate::case::Case::build`], which is what a host falls back to when
//! [`super::find`] returns `None`. Adding a row here is the whole
//! registration — nothing in [`super`] changes.

use super::Entry;

/// This catalog's slice of the side table [`super::entries`] concatenates.
pub const INTERACTIVE: &[Entry] = &[];
