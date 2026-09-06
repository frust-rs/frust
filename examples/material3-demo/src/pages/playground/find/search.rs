//! Search: the reference's `SearchPlayground`.
//!
//! `frust_material::search`'s own module docs document two divergences from
//! the reference this page inherits:
//!
//! - **The presentation split is width-driven, not platform-driven**
//!   ([`frust_material::SearchViewMode`]'s own docs) — full-screen (a
//!   navigator route) below the compact breakpoint, docked (an anchored
//!   panel) at or above it. This playground demonstrates the **docked**
//!   presentation only: the full-screen route needs its own
//!   `NavigatorController` mounted as a page's *outermost* view (the page
//!   contract in [`crate::pages::playground`]), which a single preview
//!   page has no natural width-driven trigger for — a live window resize is
//!   what upstream itself re-evaluates the split on. Both presentations
//!   share [`frust_material`]'s one content widget either way, so nothing
//!   about the split itself goes untested at the crate level.
//! - **No "expand on focus" knob.** The reference's plain `M3ESearchBar`
//!   takes `expandOnFocus`; [`frust_material::search_bar`] has no such
//!   builder — its pill always runs the same focus-driven expand spring
//!   internally ([`frust_material::search`]'s *expand spring* section), so
//!   this playground's plain-bar mode omits the control rather than adding
//!   a prop the wrapped widget cannot honor.
//!
//! The docked panel is an anchored overlay in window space, so — like
//! [`crate::widgets::playground`]'s [`play_enum_menu_field`]/
//! [`play_enum_menu_panel`] pair — it mounts at this page's own outer
//! `Stack`, not nested inside [`playground_body`]'s `scroll_view`.
//!
//! [`play_enum_menu_field`]: crate::widgets::playground::play_enum_menu_field
//! [`play_enum_menu_panel`]: crate::widgets::playground::play_enum_menu_panel

use frust::{AnyView, Column, Component, Stack, any, component};
use frust_material::{OverlayAnchor, list_item, overlay_anchor, search_bar, search_view};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    control_panel, play_preview_card, play_snippet, play_switch, play_text_field, playground_body,
};

/// The suggestion names this playground searches over — the reference's
/// `_SearchPlaygroundState._names`.
const NAMES: [&str; 10] = [
    "Buttons",
    "Cards",
    "Carousel",
    "Navigation bar",
    "Progress",
    "Search bar",
    "Snackbar",
    "Tabs",
    "Text field",
    "Tooltip",
];

/// This playground's knobs — the reference's `_SearchPlaygroundState`.
struct Knobs {
    use_anchor: bool,
    enabled: bool,
    hint: String,
    /// The bar's/view's shared live query.
    query: String,
    /// The docked view's placement source — the bar's own captured rect.
    anchor: OverlayAnchor,
    /// Whether the docked view is open — the kept-mounted contract's own
    /// flag (see [`frust_material::SearchView::docked`]).
    open: bool,
}

impl Default for Knobs {
    fn default() -> Self {
        Self {
            use_anchor: true,
            enabled: true,
            hint: "Search components".to_string(),
            query: String::new(),
            anchor: OverlayAnchor::new(),
            open: false,
        }
    }
}

/// `NAMES` matching `query`, case-insensitively — the reference's
/// `_suggestions`.
fn matches(query: &str) -> Vec<&'static str> {
    let q = query.trim().to_lowercase();
    NAMES
        .iter()
        .copied()
        .filter(|name| q.is_empty() || name.to_lowercase().contains(&q))
        .collect()
}

/// The suggestion list a docked/open view shows — tapping a row picks it and
/// closes the view, the reference's `controller.closeView(name)`.
fn suggestions(query: &str) -> AnyView<Knobs> {
    let rows: Vec<AnyView<Knobs>> = matches(query)
        .into_iter()
        .map(|name| {
            any(list_item::<Knobs>(name).on_press(move |s: &mut Knobs| {
                s.query = name.to_string();
                s.open = false;
            }))
        })
        .collect();
    any(Column(rows))
}

/// The FRUST snippet text for the current knob state — the reference's
/// `_snippets` getter, ported to real `frust_material` code rather than a
/// Dart string.
fn snippet_code(state: &Knobs) -> String {
    if state.use_anchor {
        format!(
            "let anchor = OverlayAnchor::new();\noverlay_anchor(&anchor,\n    search_bar(query, on_query_changed)\n        .hint({:?})\n        .on_tap(|s| s.open = true),\n);\nsearch_view(query, on_query_changed)\n    .hint({:?})\n    .suggestions(suggestion_list())\n    .on_dismiss(|s| s.open = false)\n    .docked(&anchor)\n    .open(open);",
            state.hint, state.hint
        )
    } else {
        format!(
            "search_bar(query, on_query_changed)\n    .hint({:?})\n    .enabled({});",
            state.hint, state.enabled
        )
    }
}

/// The preview card: the bar alone, or the bar wrapped as the docked view's
/// anchor.
fn preview(state: &Knobs) -> AnyView<Knobs> {
    let query = state.query.clone();
    let hint = state.hint.clone();
    if state.use_anchor {
        let bar = search_bar(query, |s: &mut Knobs, v: String| s.query = v)
            .hint(hint)
            .on_tap(|s: &mut Knobs| s.open = true);
        play_preview_card("Search anchor", overlay_anchor(&state.anchor, bar))
    } else {
        let bar = search_bar(query, |s: &mut Knobs, v: String| s.query = v)
            .hint(hint)
            .enabled(state.enabled);
        play_preview_card("Search bar", bar)
    }
}

/// The docked view itself, mounted at this page's own outer `Stack` — see
/// the module docs.
fn docked_panel(state: &Knobs) -> AnyView<Knobs> {
    let query = state.query.clone();
    let hint = state.hint.clone();
    any(
        search_view(query.clone(), |s: &mut Knobs, v: String| s.query = v)
            .hint(hint)
            .suggestions(suggestions(&query))
            .on_dismiss(|s: &mut Knobs| s.open = false)
            .docked(&state.anchor)
            .open(state.open),
    )
}

/// The playground body for the current knob state.
fn body(state: &mut Knobs) -> AnyView<Knobs> {
    let preview_view = preview(state);
    let snippet_label = if state.use_anchor {
        "Search anchor"
    } else {
        "Search bar"
    };
    let snippet = play_snippet(snippet_label, snippet_code(state));

    let mut rows: Vec<AnyView<Knobs>> = vec![play_switch(
        "Use search anchor",
        state.use_anchor,
        |s: &mut Knobs, v| s.use_anchor = v,
    )];
    if !state.use_anchor {
        rows.push(play_switch("Enabled", state.enabled, |s: &mut Knobs, v| {
            s.enabled = v
        }));
    }
    rows.push(play_text_field(
        "Hint",
        state.hint.clone(),
        |s: &mut Knobs, v| s.hint = v,
    ));
    let controls = control_panel("Mode", rows);

    let content = playground_body(vec![preview_view], vec![snippet], vec![controls]);
    if state.use_anchor {
        any(Stack(vec![content, docked_panel(state)]))
    } else {
        content
    }
}

/// The nested [`Component`] this page owns its knobs in. See the page
/// contract in [`crate::pages::playground`].
#[derive(Default)]
struct SearchPlayground;

impl Component for SearchPlayground {
    type State = Knobs;

    fn init(&self) -> Knobs {
        Knobs::default()
    }

    fn build(&self, state: &mut Knobs) -> AnyView<Knobs> {
        body(state)
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(SearchPlayground))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every state this file's own controls can reach still builds a body —
    /// both presentations, the docked view open and closed, and a non-empty
    /// query.
    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let mut knobs = Knobs::default();
        let _view = body(&mut knobs);

        knobs.use_anchor = false;
        let _view = body(&mut knobs);
        knobs.enabled = false;
        let _view = body(&mut knobs);

        knobs.use_anchor = true;
        knobs.open = true;
        let _view = body(&mut knobs);
        knobs.query = "Search".to_string();
        let _view = body(&mut knobs);
    }

    #[test]
    fn suggestions_filter_case_insensitively() {
        assert_eq!(matches("").len(), NAMES.len());
        assert_eq!(matches("search"), vec!["Search bar"]);
        assert_eq!(matches("SNACK"), vec!["Snackbar"]);
        assert!(matches("zzz").is_empty());
    }
}
