//! The six playground sections, one module per section: `platform_views`,
//! `camera`, `native_widgets`, `responsive`, `terminal`, and `keys` — each one
//! an OS-facing capability exercised end to end rather than a widget gallery.
//!
//! # Page-fn contract (fixed across every section)
//!
//! Each section module exposes exactly:
//!
//! ```ignore
//! pub fn page(state: &PlaygroundState) -> AnyView<PlaygroundState>;
//! ```
//!
//! - **Argument**: `state: &PlaygroundState` — the shared reactive handle the
//!   shell owns ([`crate::PlaygroundState`]). A page reads whatever it needs
//!   from it:
//!   - `state.brightness` / `state.reduce_motion` — the live theme flags (the
//!     native-widgets page reads brightness to drive its own theme-ladder
//!     toggle).
//!   - `state.toasts` — the FIFO toast queue: a page raises a toast by
//!     appending a message **inside an event handler** (e.g. a button's
//!     `on_press`), never during `build`.
//!   - `state.nav` — the shared [`NavigatorController`](frust::NavigatorController):
//!     an overlay page pushes a dialog / sheet onto the same root stack the
//!     shell mounts.
//! - **Return**: a fully type-erased
//!   [`AnyView<PlaygroundState>`](frust::AnyView). The shell wraps it in a
//!   [`scroll_view`](frust::scroll_view), so a page returns its content column
//!   directly (no outer scroll of its own).
//!
//! **Reactivity rule** (see `docs/CODE_STANDARDS.md`): a page's `build` may
//! only *read* signals (`.get()` to subscribe). Every *write* to a signal —
//! a toast append, a local demo-state flip — happens in an event handler
//! (`on_press`/`on_toggle`/…), which carries `&mut PlaygroundState`, never in
//! the `build` body.

pub mod camera;
pub mod keys;
pub mod native_widgets;
pub mod platform_views;
pub mod responsive;
pub mod terminal;

use frust::AnyView;

use crate::PlaygroundState;

/// The six section labels, in order. Indexed by `PlaygroundState::section`
/// and dispatched by [`current`], and used verbatim as the shell's bottom
/// navigation-bar destinations.
///
/// Kept short on purpose: [`NavigationBarWidget`](frust::navigation_bar)
/// divides the bar's width evenly across all six destinations, so at phone
/// width (390px logical) each slot is only 65px wide. The longer human names
/// ("Platform Views", "Native Widgets") wrapped to two lines there and
/// overflowed the bar's declared 64dp height — every
/// label here is verified (see `tests/smoke.rs`'s
/// `full_shell_nav_labels_fit_single_line_at_phone_width`) to shape on a
/// single line at a 65px slot width. A page wanting a longer heading for
/// itself (e.g. `native_widgets`'s own on-page title) uses its own string
/// literal rather than this array — see `pages/native_widgets.rs`.
pub const SECTION_LABELS: [&str; 6] =
    ["Platform", "Camera", "Native", "Layout", "Terminal", "Keys"];

/// Dispatch to the section page for `section` (0..6), falling back to
/// platform views for any out-of-range index (defensive — the navigation bar
/// only ever yields a valid index).
pub fn current(section: usize, state: &PlaygroundState) -> AnyView<PlaygroundState> {
    match section {
        0 => platform_views::page(state),
        1 => camera::page(state),
        2 => native_widgets::page(state),
        3 => responsive::page(state),
        4 => terminal::page(state),
        5 => keys::page(state),
        _ => platform_views::page(state),
    }
}
