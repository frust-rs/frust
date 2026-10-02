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
//!
//! # The variant has to reach the pixels
//!
//! Every case is recorded twice — once per [`crate::Variant`] — and the
//! recorder clears each pass to that variant's own theme `surface`
//! (`crates/frust-testing/src/snapshot.rs`'s `base_color`). A case that
//! paints a fixed, opaque colour across its whole frame overwrites that clear
//! and makes its two recordings byte-identical, which is exactly the
//! light/dark split the website's previews exist to show. So: build a case
//! through [`framed`]/[`framed_in`], which size the frame but never fill it,
//! leave body/label text at its themed default (`text(..)` resolves
//! `on_surface`) instead of hard-coding a glyph colour, and where a widget
//! genuinely needs a contrasting swatch, size that swatch smaller than the
//! frame so the cleared surface still shows around it.

use std::sync::OnceLock;

use frust_core::{AnyView, View, any};
use frust_widgets::container;
use kurbo::Size;

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

/// Wrap `child`, centered, in a fixed [`Case::DEFAULT_SIZE`] frame rather than
/// letting it hug its content and sit in the frame's top-left:
/// `RenderRoot::layout_with_text` hands the root LOOSE constraints (zero up to
/// the window size), so an unsized case would shrink-wrap and leave the rest
/// of the preview blank.
///
/// Deliberately no `.fill(..)` — see this module's "The variant has to reach
/// the pixels" section for why a case must never paint a fixed full-frame
/// backdrop.
pub(super) fn framed<V: View<()>>(child: V) -> AnyView<()> {
    framed_in(Case::DEFAULT_SIZE, child)
}

/// [`framed`] at an explicit frame size, for a module that records at its own
/// overridden viewport rather than [`Case::DEFAULT_SIZE`] (today: [`layout`]).
/// Fills nothing, for the same reason.
pub(super) fn framed_in<V: View<()>>(frame: Size, child: V) -> AnyView<()> {
    any(container(child).size_centered(frame.width, frame.height))
}

/// The per-case constants of the modules that expose cases one by one.
const SINGLES: &[Case] = &[
    // basics
    basics::ANY_VIEW,
    basics::CONTAINER,
    basics::SAFE_AREA,
    basics::SCAFFOLD,
    // text
    text::TEXT,
    // layout
    layout::ALIGN,
    layout::DIVIDER,
    layout::FLEX,
    layout::PADDING,
    layout::SIZED_BOX,
    layout::STACK,
    // styling
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
