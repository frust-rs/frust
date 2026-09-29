//! The ten playground sections, one module per section: `platform_views`,
//! `camera`, `responsive`, `terminal`, `keys`, `i18n`, `video_player`,
//! `url_launcher`, `auth_session`, and `database` — each one an OS-facing
//! capability exercised end to end rather than a widget gallery.
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
pub mod i18n;
pub mod keys;
pub mod platform_views;
pub mod responsive;
pub mod terminal;
pub mod url_launcher;
pub mod video_player;

use frust::AnyView;

use crate::PlaygroundState;

/// The ten section labels, in order. Indexed by `PlaygroundState::section`
/// and dispatched by [`current`], and used verbatim as the shell's bottom
/// navigation-bar destinations.
///
/// Kept short on purpose: [`NavigationBarWidget`](frust_material::navigation_bar)
/// divides the bar's width evenly across all ten destinations, so at
/// phone width (390px logical) each slot is only ~39px wide. The longer
/// human names (e.g. "Platform Views") wrapped to two lines
/// there and overflowed the bar's declared 64dp height — every
/// label here is verified (see `tests/smoke.rs`'s
/// `full_shell_nav_labels_fit_single_line_at_phone_width`) to shape on a
/// single line at this slot width. A page wanting a longer heading for
/// itself (e.g. `platform_views`'s own on-page title) uses its own string
/// literal rather than this array — see `pages/platform_views.rs`.
pub const SECTION_LABELS: [&str; 10] = [
    "Platform", "Camera", "Layout", "Terminal", "Keys", "i18n", "Video", "URL", "Auth", "DB",
];

/// Dispatch to the section page for `section` (0..10), falling back to
/// platform views for any out-of-range index (defensive — the navigation bar
/// only ever yields a valid index).
pub fn current(section: usize, state: &PlaygroundState) -> AnyView<PlaygroundState> {
    match section {
        0 => platform_views::page(state),
        1 => camera::page(state),
        2 => responsive::page(state),
        3 => terminal::page(state),
        4 => keys::page(state),
        5 => i18n::page(state),
        6 => video_player::page(state),
        7 => url_launcher::page(state),
        8 => auth_session::page(state),
        9 => database::page(state),
        _ => platform_views::page(state),
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
