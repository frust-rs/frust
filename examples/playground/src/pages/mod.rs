//! The thirteen playground sections, one module per section: `platform_views`,
//! `camera`, `responsive`, `terminal`, `keys`, `i18n`, `video_player`,
//! `url_launcher`, `auth_session`, `database`, `graph_canvas`,
//! `scroll_control`, and `drag_drop` — each one an OS-facing capability (or,
//! for `graph_canvas`/`scroll_control`/`drag_drop`, a widget demo:
//! `CanvasView`/`PanZoomView` for the first, a keyed `ListView` bound to a
//! `ScrollController` for the second, `DragCoordinator`/`draggable`/
//! `drag_target`/`reorderable_list` for the third) exercised end to end
//! rather than a widget gallery.
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
//!   - `state.brightness` / `state.reduce_motion` — the live theme flags.
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

pub mod auth_session;
pub mod camera;
pub mod database;
pub mod drag_drop;
pub mod graph_canvas;
pub mod i18n;
pub mod keys;
pub mod platform_views;
pub mod responsive;
pub mod scroll_control;
pub mod terminal;
pub mod url_launcher;
pub mod video_player;

use frust::{AnyView, any};

use crate::PlaygroundState;

/// The thirteen section labels, in order. Indexed by `PlaygroundState::section`
/// and dispatched by [`current`].
///
/// Kept short on purpose: at phone width the shell's narrow-width bottom nav
/// bar (`crate::bottom_nav_bar`) shows only the first [`PRIMARY_NAV_COUNT`]
/// of these plus a trailing "More" destination (see that function's own
/// doc), and the wide-width side rail (`crate::side_rail`) shows every one of
/// them in a single column — either way a label that wraps to two lines
/// overflows its row's declared height. Every *visible-at-phone-width* label
/// is verified (see `tests/smoke.rs`'s
/// `full_shell_nav_labels_fit_single_line_at_phone_width`) to shape on a
/// single line at that width. A page wanting a longer heading for itself
/// (e.g. `platform_views`'s own on-page title) uses its own string literal
/// rather than this array — see `pages/platform_views.rs`.
pub const SECTION_LABELS: [&str; 13] = [
    "Platform", "Camera", "Layout", "Terminal", "Keys", "i18n", "Video", "URL", "Auth", "DB",
    "Graph", "Scroll", "Drag",
];

/// How many of [`SECTION_LABELS`], in order, count as "primary" — shown
/// directly in the narrow-width bottom nav bar (`crate::bottom_nav_bar`)
/// alongside a trailing "More" destination that opens the rest. Five matches
/// the M3 navigation-bar convention of showing at most five destinations
/// directly; every section beyond this cutoff is still reachable (through
/// "More" at phone width, or directly in the side rail at desktop width —
/// see `crate::side_rail`), so growing [`SECTION_LABELS`] never drops a
/// section off the shell, only off the bar's own direct row.
pub const PRIMARY_NAV_COUNT: usize = 5;

/// Dispatch to the section page for `section` (0..13), falling back to
/// platform views for any out-of-range index (defensive — the navigation bar
/// and side rail only ever yield a valid index).
// erasure: keep the 14-arm section dispatch erases each page at this one boundary
pub fn current(section: usize, state: &PlaygroundState) -> AnyView<PlaygroundState> {
    match section {
        0 => any(platform_views::page(state)),
        1 => any(camera::page(state)),
        2 => any(responsive::page(state)),
        3 => any(terminal::page(state)),
        4 => any(keys::page(state)),
        5 => any(i18n::page(state)),
        6 => any(video_player::page(state)),
        7 => any(url_launcher::page(state)),
        8 => any(auth_session::page(state)),
        9 => any(database::page(state)),
        10 => any(graph_canvas::page(state)),
        11 => any(scroll_control::page(state)),
        12 => any(drag_drop::page(state)),
        _ => any(platform_views::page(state)),
    }
}

/// Resolve a section index from its label via `frustplay://section/<label>`
/// deep links. Matches case-insensitively against SECTION_LABELS (e.g. "db"/
/// "DB" resolve to index 9, the database section; "keys"/"Keys" resolve to
/// index 4, the on-screen-keyboard/IME section).
///
/// Returns `Some(index)` for a recognized label, or `None` for an unknown one.
pub fn section_index_for(label: &str) -> Option<usize> {
    SECTION_LABELS
        .iter()
        .position(|&s| s.eq_ignore_ascii_case(label))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_section_index_for() {
        // Test each label in its exact case
        for (expected_idx, label) in SECTION_LABELS.iter().enumerate() {
            assert_eq!(
                section_index_for(label),
                Some(expected_idx),
                "label {} at index {}",
                label,
                expected_idx
            );
        }

        // Test case-insensitive matching
        assert_eq!(section_index_for("db"), Some(9), "lowercase 'db'");
        assert_eq!(section_index_for("DB"), Some(9), "uppercase 'DB'");
        assert_eq!(
            section_index_for("platform"),
            Some(0),
            "lowercase 'platform'"
        );
        assert_eq!(
            section_index_for("PLATFORM"),
            Some(0),
            "uppercase 'PLATFORM'"
        );
        assert_eq!(section_index_for("camera"), Some(1), "lowercase 'camera'");

        // Test unknown label
        assert_eq!(section_index_for("unknown"), None, "unknown label");
        assert_eq!(section_index_for(""), None, "empty string");
    }
}
