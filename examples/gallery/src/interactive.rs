//! The INTERACTIVE side table: an optional, slug-keyed stateful constructor
//! that a *live* host may build a case from instead of [`Case::build`].
//!
//! # Why a side table and not a field on `Case`
//!
//! Every widget in `frust-widgets` is **controlled** — `checkbox`/`slider`/
//! `text_input` report a requested value through `on_toggle`/`on_change` and
//! leave their own `checked`/`value` untouched until the next rebuild feeds
//! the app-confirmed value back down (`docs/CODE_STANDARDS.md`'s Interaction
//! Semantics). A [`Case::build`] is a `fn() -> AnyView<()>`, so its callbacks
//! have nowhere to write: the registry's own cases pass `|_: &mut (), _| {}`
//! and a click therefore changes nothing. Giving a case somewhere to write
//! means giving it retained state.
//!
//! That state must not reach the snapshot oracle. `crates/frust-testing`'s
//! `record_scene` hands `case.build` — the function pointer, by that name —
//! to `record_view`, and the website's poster gate byte-compares whole PNGs
//! (`widget-snapshots --check`). Keeping the stateful constructor in a table
//! *beside* the registry rather than in a field *on* it makes the oracle
//! provably unaffected: there is no path from this module to `case.build`, so
//! no argument about init purity or first-frame rest state is needed, and no
//! baseline has to be re-recorded. A field on [`Case`] would also have to be
//! spelled out in all ~107 case literals, every one of which names every
//! field explicitly.
//!
//! # Shape
//!
//! One `pub const INTERACTIVE: &[Entry]` per catalog submodule, listed once in
//! [`PARTS`]. A catalog registers its own cases by editing **only its own
//! file** — nothing here has to change to add an entry, so the six catalog
//! modules can be filled in independently.
//!
//! A slug that names no case in the registry silently registers nothing, which
//! is the one real hazard of pairing by string; `tests/registry.rs` asserts
//! every slug here resolves through [`crate::find`].
//!
//! # What a constructor may do
//!
//! It returns the same `AnyView<()>` a `Case::build` does and is built by the
//! same `View` machinery, so it is still bound by the crate's pure-`View`
//! contract at *its own* root: no `use_signal`, no `use_context`, no wall
//! clock. Retained state comes from [`frust_core::Component`] instead — plain
//! data the retained `ComponentWidget` owns across rebuilds, seeded once by
//! `init` and handed to `build` as `&mut State`, which is exactly what a
//! controlled widget's callback needs to write to. `ComponentView<C>`
//! implements `View<Outer>` for **every** `Outer`, so a component with its own
//! state is a legal `AnyView<()>` and needs no change to `Case` or to a host.
//!
//! Because a component's subtree is bound to `C::State` and not `()`,
//! [`crate::base::framed`] (which is `View<()>`-shaped) cannot frame it — use
//! [`framed`]/[`framed_in`] below, this module's state-generic mirrors of it.

use frust_core::{AnyView, View};
use frust_widgets::container;
use kurbo::Size;

use crate::case::Case;

pub mod base;
pub mod beui;
pub mod cupertino;
pub mod glyph;
pub mod material;
pub mod shadcn;

/// A stateful case constructor — the same signature as [`Case::build`], so a
/// host can substitute one for the other without knowing which it holds.
pub type Build = fn() -> AnyView<()>;

/// One side-table row: the [`Case::slug`] it shadows, and the constructor to
/// build that case from when the host is live rather than recording.
pub type Entry = (&'static str, Build);

/// Every catalog submodule's own table. A submodule appears here whether or
/// not it has entries yet, so filling one in touches only that one file.
const PARTS: &[&[Entry]] = &[
    base::INTERACTIVE,
    material::INTERACTIVE,
    cupertino::INTERACTIVE,
    glyph::INTERACTIVE,
    shadcn::INTERACTIVE,
    beui::INTERACTIVE,
];

/// The whole side table: every catalog submodule's [`Entry`] rows,
/// concatenated once and cached for the process — the same shape (and the
/// same reason) as [`crate::cases`].
pub fn entries() -> &'static [Entry] {
    static ALL: std::sync::OnceLock<Vec<Entry>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| PARTS.iter().flat_map(|part| part.iter().copied()).collect())
}

/// The stateful constructor registered for `slug`, if any. `None` — the case
/// for all but a handful of slugs — means a host should fall back to that
/// case's own [`Case::build`].
pub fn find(slug: &str) -> Option<Build> {
    entries()
        .iter()
        .find(|(entry_slug, _)| *entry_slug == slug)
        .map(|(_, build)| *build)
}

/// [`crate::base::framed`]'s state-generic mirror: wrap `child`, centered, in
/// a fixed [`Case::DEFAULT_SIZE`] frame, at whatever state type the child is
/// bound to.
///
/// A [`frust_core::Component`]'s subtree is an `AnyView<C::State>`, so the
/// `View<()>`-shaped original cannot frame it. Deliberately no `.fill(..)`,
/// for the reason `crate::base`'s "the variant has to reach the pixels"
/// section gives: a full-frame opaque backdrop would overwrite the recorder's
/// per-variant clear.
pub fn framed<State: 'static, V: View<State>>(child: V) -> impl View<State> {
    framed_in(Case::DEFAULT_SIZE, child)
}

/// [`framed`] at an explicit frame size, for a case recorded at its own
/// overridden viewport rather than [`Case::DEFAULT_SIZE`]. Fills nothing, for
/// the same reason.
pub fn framed_in<State: 'static, V: View<State>>(frame: Size, child: V) -> impl View<State> {
    container(child).size_centered(frame.width, frame.height)
}
