//! `Base`-design cases — the framework's own baseline widgets, with no
//! design-system plugin involved (see [`crate::case::Design::Base`]).
//!
//! Each catalog category gets its own submodule. Two shapes coexist by
//! design: `basics`/`text`/`layout`/`styling` expose one
//! `pub(super) const <NAME>: Case` per case, while the other modules expose a
//! `pub const CASES: &[Case]` slice. [`PARTS`] lists every module's slice and
//! [`cases`] concatenates them once at runtime (`Case` is `Copy`, so the
//! concatenation is a plain copy into a leaked, process-lifetime `Vec`) — a
//! `const` array literal cannot splice a slice, and a `const fn` concat
//! would need every length spelled out by hand. To add a category, add its
//! `mod` line and its slice to [`PARTS`]; `tests/registry.rs` enforces slug
//! uniqueness across the whole registry.

use std::sync::OnceLock;

use crate::case::Case;

mod animation;
mod assets;
mod basics;
mod input;
mod interaction;
mod layout;
mod navigation;
mod painting;
mod scrolling;
mod styling;
mod text;

/// The per-case constants of the modules that expose cases one by one.
const SINGLES: &[Case] = &[
    // basics (g1-03)
    basics::ANY_VIEW,
    basics::CONTAINER,
    basics::SAFE_AREA,
    basics::SCAFFOLD,
    // text (g1-03)
    text::TEXT,
    // layout (g1-03)
    layout::ALIGN,
    layout::DIVIDER,
    layout::FLEX,
    layout::PADDING,
    layout::SIZED_BOX,
    layout::STACK,
    // styling (g1-03)
    styling::ALIGNMENT,
    styling::COLOR,
    styling::EDGE_INSETS,
    styling::TEXT_STYLE,
];

/// Every module's contribution, in catalog order. [`cases`] flattens this.
const PARTS: &[&[Case]] = &[
    SINGLES,
    input::CASES,
    interaction::CASES,
    navigation::CASES,
    scrolling::CASES,
    animation::CASES,
    assets::CASES,
    painting::CASES,
];

/// This module's slice of the registry [`crate::cases`]: every `Base` case
/// across every submodule, concatenated once and cached for the process.
pub fn cases() -> &'static [Case] {
    static ALL: OnceLock<Vec<Case>> = OnceLock::new();
    ALL.get_or_init(|| PARTS.concat())
}
