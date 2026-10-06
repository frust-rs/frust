//! Dropdown menu: the reference's `DropdownMenuPlayground`.
//!
//! The flagship consumer of both `frust_material::dropdown` itself and the
//! kit's two-piece [`play_enum_menu_field`]/[`play_enum_menu_panel`] control —
//! the preview mounts a real [`dropdown_field`]/[`dropdown`] pair sharing one
//! [`OverlayAnchor`] (not the kit control, which exists for the *Controls*
//! panel and is used here only for the "Expand" knob), while "Expand" itself
//! is a two-piece menu with its own, second anchor — the `do_::buttons`
//! precedent for more than one anchored dropdown on one page.
//!
//! Knobs live in a page-local [`Knobs`] (never [`AppState`]), per the page
//! contract in [`crate::pages::playground`]. No navigator is mounted: unlike
//! the picker dialogs in this section, `frust_material::dropdown` opens
//! through the anchored-overlay seam, not the modal/navigator one, so this
//! page's `Component::build` needs no [`frust::navigator`] split (the
//! `do_::buttons` precedent again).
//!
//! # Descoped: form-field validator and restoration
//!
//! `frust_material::dropdown`'s own module docs descope the reference's
//! `FormField` wrapper (`validator`/`autovalidateMode`/restoration) arc-wide —
//! there is no prop here to drive a control for, so it is omitted rather than
//! faked.

use frust::{
    AnyView, Column, Component, CrossAxisAlignment, SizedBox, Stack, View, any, component, text,
};
use frust_material::{
    DropdownItem, OverlayAnchor, OverlaySide, dropdown, dropdown_field, dropdown_item,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    PlaySnippet, ambient_theme, control_panel, play_enum_menu_field, play_enum_menu_panel,
    play_preview_card, play_snippet, play_switch, play_text_field, playground_body,
};

/// The preview's option list: `(label, value)` pairs, the reference's own
/// `_items` — used both to build the item list and to resolve a reported
/// value back to its label for the selection summary.
const ITEM_DEFS: [(&str, &str); 5] = [
    ("Flutter", "flutter"),
    ("Dart", "dart"),
    ("Material 3", "m3"),
    ("Theming", "theming"),
    ("Animation", "animation"),
];

fn items() -> Vec<DropdownItem> {
    ITEM_DEFS
        .iter()
        .map(|(label, value)| dropdown_item(*label, *value))
        .collect()
}

/// The label `value` names among [`ITEM_DEFS`], or `value` itself if it names
/// nothing (never reachable through this page's own controlled round trip,
/// but total rather than panicking).
fn label_of_value(value: &str) -> String {
    ITEM_DEFS
        .iter()
        .find(|(_, v)| *v == value)
        .map(|(label, _)| (*label).to_string())
        .unwrap_or_else(|| value.to_string())
}

/// The preview's "Selected: …" line — the reference's own
/// `items.map((i) => i.label).join(', ')`, joined by resolved label rather
/// than raw value.
fn selection_summary(selected: &[String]) -> Option<String> {
    if selected.is_empty() {
        None
    } else {
        Some(
            selected
                .iter()
                .map(|v| label_of_value(v))
                .collect::<Vec<_>>()
                .join(", "),
        )
    }
}

/// Which side the panel opens on — the reference's
/// `M3EDropdownExpandDirection`. `frust_material::dropdown`'s own module docs
/// name the mapping this control applies: `up`/`down` are explicit
/// [`OverlaySide`]s, `auto` is the anchored host's own collision flip
/// (already on by default), so `Auto` and `Down` share the same default side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExpandDirection {
    Auto,
    Up,
    Down,
}

/// Every [`ExpandDirection`] the "Expand" menu offers, the reference's own
/// `M3EDropdownExpandDirection.values` order.
const EXPAND_DIRECTIONS: [ExpandDirection; 3] = [
    ExpandDirection::Auto,
    ExpandDirection::Up,
    ExpandDirection::Down,
];

/// Expand-direction label for the menu/snippet — the reference's
/// `M3EDropdownExpandDirection.name`.
fn expand_label(direction: ExpandDirection) -> &'static str {
    match direction {
        ExpandDirection::Auto => "auto",
        ExpandDirection::Up => "up",
        ExpandDirection::Down => "down",
    }
}

/// The [`OverlaySide`] a direction resolves to.
fn expand_side(direction: ExpandDirection) -> OverlaySide {
    match direction {
        ExpandDirection::Auto | ExpandDirection::Down => OverlaySide::Bottom,
        ExpandDirection::Up => OverlaySide::Top,
    }
}

/// This page's own knob state — held by [`DropdownMenuPlayground`], never
/// [`AppState`] (the page contract in [`crate::pages::playground`]).
struct Knobs {
    single_select: bool,
    search_enabled: bool,
    enabled: bool,
    show_clear: bool,
    expand: ExpandDirection,
    hint: String,
    selected: Vec<String>,
    open: bool,
    query: String,
    /// Shared by the preview's [`dropdown_field`]/[`dropdown`] pair.
    anchor: OverlayAnchor,
    /// Shared with [`expand_menu_panel`] — the "Expand" dropdown's own,
    /// second anchor.
    expand_anchor: OverlayAnchor,
    expand_open: bool,
}

impl Default for Knobs {
    /// The reference's own `_DropdownMenuPlaygroundState` field initializers.
    fn default() -> Self {
        Self {
            single_select: true,
            search_enabled: false,
            enabled: true,
            show_clear: true,
            expand: ExpandDirection::Auto,
            hint: "Choose an option".to_string(),
            selected: Vec::new(),
            open: false,
            query: String::new(),
            anchor: OverlayAnchor::new(),
            expand_anchor: OverlayAnchor::new(),
            expand_open: false,
        }
    }
}

/// The preview's trigger half, sized to the reference's own `SizedBox(width:
/// 320)`.
fn preview_field(state: &Knobs) -> AnyView<Knobs> {
    any(SizedBox::<Knobs>(Some(320.0), None).child(
        dropdown_field(items())
            .anchor(&state.anchor)
            .selected(state.selected.clone())
            .hint(state.hint.clone())
            .multi(!state.single_select)
            .open(state.open)
            .enabled(state.enabled)
            .show_clear(state.show_clear)
            .on_open(|s: &mut Knobs, next: bool| s.open = next)
            .on_change(|s: &mut Knobs, values: Vec<String>| s.selected = values),
    ))
}

/// The "Dropdown menu" preview card's content: the trigger, plus the
/// selection summary line once something is picked.
fn preview(state: &Knobs) -> AnyView<Knobs> {
    let mut rows: Vec<AnyView<Knobs>> = vec![preview_field(state)];
    if let Some(summary) = selection_summary(&state.selected) {
        let theme = ambient_theme();
        let mut style = theme.type_scale.body_medium.clone();
        style.color = theme.scheme().on_surface_variant;
        rows.push(any(SizedBox::<Knobs>(None, Some(12.0))));
        rows.push(any(text(format!("Selected: {summary}")).style(style)));
    }
    any(Column(rows).cross_axis(CrossAxisAlignment::Start))
}

/// The preview's panel half — mounted at this page's outer [`Stack`], pointed
/// at the same [`OverlayAnchor`] the trigger captures into.
fn preview_panel(state: &Knobs) -> AnyView<Knobs> {
    any(dropdown(items(), |s: &mut Knobs, values: Vec<String>| {
        s.selected = values
    })
    .anchor(&state.anchor)
    .selected(state.selected.clone())
    .multi(!state.single_select)
    .open(state.open)
    .searchable(state.search_enabled)
    .query(state.query.clone())
    .side(expand_side(state.expand))
    .on_query(|s: &mut Knobs, next: String| s.query = next)
    .on_open(|s: &mut Knobs, next: bool| s.open = next))
}

/// The paste-ready snippet for the current knob state — the reference's
/// `_snippets`, in Frust rather than Dart. Mirrors the reference's own
/// hardcoded three-item subset rather than the full five-item list.
fn snippet(state: &Knobs) -> PlaySnippet {
    let code = format!(
        "let items = vec![\n    \
             dropdown_item(\"Flutter\", \"flutter\"),\n    \
             dropdown_item(\"Dart\", \"dart\"),\n    \
             dropdown_item(\"Material 3\", \"m3\"),\n\
         ];\n\n\
         dropdown_field(items.clone())\n    \
             .multi({multi})\n    \
             .hint({hint:?})\n    \
             .show_clear({show_clear})\n    \
             .enabled({enabled})\n    \
             .on_change(|state, values| {{ /* ... */ }});\n\n\
         dropdown(items, |state, values| {{ /* ... */ }})\n    \
             .multi({multi})\n    \
             .searchable({searchable})\n    \
             .side(OverlaySide::{side:?});",
        multi = !state.single_select,
        hint = state.hint,
        show_clear = state.show_clear,
        enabled = state.enabled,
        searchable = state.search_enabled,
        side = expand_side(state.expand),
    );
    play_snippet("Dropdown menu", code)
}

/// "Behavior" controls: single select, search, show clear, enabled, expand
/// direction, hint text.
fn controls_panel(state: &Knobs) -> AnyView<Knobs> {
    control_panel(
        "Behavior",
        vec![
            play_switch(
                "Single select",
                state.single_select,
                |s: &mut Knobs, v: bool| s.single_select = v,
            ),
            play_switch("Search", state.search_enabled, |s: &mut Knobs, v: bool| {
                s.search_enabled = v
            }),
            play_switch("Show clear", state.show_clear, |s: &mut Knobs, v: bool| {
                s.show_clear = v
            }),
            play_switch("Enabled", state.enabled, |s: &mut Knobs, v: bool| {
                s.enabled = v;
            }),
            play_enum_menu_field(
                "Expand",
                state.expand,
                &EXPAND_DIRECTIONS,
                expand_label,
                &state.expand_anchor,
                state.expand_open,
                |s: &mut Knobs, open: bool| s.expand_open = open,
            ),
            play_text_field("Hint", state.hint.clone(), |s: &mut Knobs, v: String| {
                s.hint = v;
            }),
        ],
    )
}

/// The "Expand" menu's popup half — mounted at this page's outer [`Stack`].
fn expand_menu_panel(state: &Knobs) -> AnyView<Knobs> {
    play_enum_menu_panel(
        state.expand,
        &EXPAND_DIRECTIONS,
        expand_label,
        &state.expand_anchor,
        state.expand_open,
        |s: &mut Knobs, open: bool| s.expand_open = open,
        |s: &mut Knobs, next: ExpandDirection| s.expand = next,
    )
}

/// The page body: the playground content plus both dropdown panels it
/// anchors (the live preview's own, and the "Expand" control's), stacked so
/// all three can paint above the scrollable content.
fn body(state: &Knobs) -> AnyView<Knobs> {
    let content = playground_body(
        vec![play_preview_card("Dropdown menu", preview(state))],
        vec![snippet(state)],
        vec![controls_panel(state)],
    );
    any(Stack(vec![
        content,
        preview_panel(state),
        expand_menu_panel(state),
    ]))
}

/// This page's knob component — see the [module docs](self).
struct DropdownMenuPlayground;

impl Component for DropdownMenuPlayground {
    type State = Knobs;

    fn init(&self) -> Self::State {
        Knobs::default()
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        body(state)
    }
}

/// See the page contract in [`crate::pages::playground`]. The reference's
/// `DropdownMenuPlayground` shows no page-level heading of its own, like
/// every other real playground in this catalog, so `entry` goes unused here.
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(DropdownMenuPlayground))
}

#[cfg(test)]
mod tests {
    use super::{EXPAND_DIRECTIONS, Knobs, body, snippet};

    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let mut knobs = Knobs::default();
        for single_select in [true, false] {
            knobs.single_select = single_select;
            for search_enabled in [true, false] {
                knobs.search_enabled = search_enabled;
                for enabled in [true, false] {
                    knobs.enabled = enabled;
                    for show_clear in [true, false] {
                        knobs.show_clear = show_clear;
                        for direction in EXPAND_DIRECTIONS {
                            knobs.expand = direction;
                            let _view = body(&knobs);
                            let _snippet = snippet(&knobs);
                        }
                    }
                }
            }
        }

        knobs.hint = String::new();
        let _view = body(&knobs);

        knobs.open = true;
        let _view = body(&knobs);

        knobs.query = "fl".to_string();
        let _view = body(&knobs);

        knobs.expand_open = true;
        let _view = body(&knobs);

        knobs.selected = vec!["flutter".to_string(), "dart".to_string()];
        let _view = body(&knobs);
        let _snippet = snippet(&knobs);
    }

    #[test]
    fn the_page_fn_builds_from_its_catalog_entry() {
        let entry = crate::catalog::find_by_id("dropdown_menu").expect("catalog entry exists");
        let _view = super::page(entry);
    }
}
