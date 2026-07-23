//! The seven catalog sections, one module per section, mirroring the reference
//! builds' section list (foundations, buttons+forms, feedback, navigation,
//! content, overlays, motion).
//!
//! # Page-fn contract (fixed by `c01`; every fill task keeps it exactly)
//!
//! Each section module exposes exactly:
//!
//! ```ignore
//! pub fn page(state: &CatalogState) -> AnyView<CatalogState>;
//! ```
//!
//! - **Argument**: `state: &CatalogState` — the shared reactive handle the
//!   shell owns ([`crate::CatalogState`]). A page reads whatever it needs from
//!   it:
//!   - `state.brightness` / `state.reduce_motion` — the live theme flags (a
//!     foundations page reads them to label swatches; a motion page ties its
//!     reduced-motion note to `state.reduce_motion`).
//!   - `state.toasts` — the FIFO toast queue: a page raises a toast by
//!     appending a message **inside an event handler** (e.g. a button's
//!     `on_press`), never during `build`.
//!   - `state.nav` — the shared [`NavigatorController`](frust::NavigatorController):
//!     an overlay page pushes a dialog / command palette / bottom sheet onto
//!     the same root stack the shell mounts.
//! - **Return**: a fully type-erased [`AnyView<CatalogState>`](frust::AnyView).
//!   The shell wraps it in a [`scroll_view`](frust::scroll_view), so a page
//!   returns its content column directly (no outer scroll of its own).
//!
//! **Reactivity rule** (see `docs/CODE_STANDARDS.md`): a page's `build` may
//! only *read* signals (`.get()` to subscribe). Every *write* to a signal —
//! a toast append, a local demo-state flip — happens in an event handler
//! (`on_press`/`on_toggle`/…), which carries `&mut CatalogState`, never in the
//! `build` body.

pub mod buttons_forms;
pub mod content;
pub mod feedback;
pub mod foundations;
pub mod motion;
pub mod navigation;
pub mod overlays;

use frust::AnyView;

use crate::CatalogState;

/// The seven section tab labels, in order. Indexed by `CatalogState::section`
/// and dispatched by [`current`].
pub const SECTION_LABELS: [&str; 7] = [
    "Foundations",
    "Buttons + Forms",
    "Feedback",
    "Navigation",
    "Content",
    "Overlays",
    "Motion",
];

/// Dispatch to the section page for `section` (0..7), falling back to
/// foundations for any out-of-range index (defensive — the tab strip only ever
/// yields a valid index).
pub fn current(section: usize, state: &CatalogState) -> AnyView<CatalogState> {
    match section {
        0 => foundations::page(state),
        1 => buttons_forms::page(state),
        2 => feedback::page(state),
        3 => navigation::page(state),
        4 => content::page(state),
        5 => overlays::page(state),
        6 => motion::page(state),
        _ => foundations::page(state),
    }
}
