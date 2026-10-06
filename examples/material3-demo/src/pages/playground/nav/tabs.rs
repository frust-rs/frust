//! Tabs: the reference's `TabsPlayground`.
//!
//! No scrollable variant to omit — `frust_material::tabs`' own module docs
//! confirm the reference ships none either (`m3e_tabs.dart` lays every tab
//! out as an equal-share `Expanded` unconditionally); this page mirrors that
//! faithfully rather than inventing one.

use frust::{AnyView, Component, View, any, component};
use frust_material::{Tab, TabsVariant, icons, tab, tabs};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    control_panel, play_enum_segmented, play_preview_card, play_snippet, play_switch,
    playground_body,
};

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(TabsPlayground))
}

const VARIANTS: [TabsVariant; 2] = [TabsVariant::Primary, TabsVariant::Secondary];

fn variant_label(v: TabsVariant) -> &'static str {
    match v {
        TabsVariant::Primary => "primary",
        TabsVariant::Secondary => "secondary",
    }
}

/// This playground's own knobs — the reference's `_TabsPlaygroundState`
/// fields.
struct TabsState {
    variant: TabsVariant,
    show_icons: bool,
    selected: usize,
}

struct TabsPlayground;

impl Component for TabsPlayground {
    type State = TabsState;

    fn init(&self) -> TabsState {
        TabsState {
            variant: TabsVariant::Primary,
            show_icons: false,
            selected: 0,
        }
    }

    fn build(&self, state: &mut TabsState) -> impl View<TabsState> {
        body(state)
    }
}

fn body(state: &TabsState) -> AnyView<TabsState> {
    playground_body(
        vec![play_preview_card("Tabs", preview(state))],
        vec![play_snippet("Tabs", snippet_code(state))],
        vec![appearance_panel(state)],
    )
}

/// The three tabs' `(label, icon)` pairs — the reference's own `_tabs`
/// getter.
const TAB_SPECS: [(&str, frust::IconSource); 3] = [
    ("Overview", icons::HOME),
    ("Specs", icons::TUNE),
    ("Reviews", icons::STAR_OUTLINE),
];

fn build_tabs(state: &TabsState) -> Vec<Tab> {
    TAB_SPECS
        .into_iter()
        .map(|(label, icon_source)| {
            let t = tab().label(label);
            if state.show_icons {
                t.icon(icon_source)
            } else {
                t
            }
        })
        .collect()
}

fn preview(state: &TabsState) -> AnyView<TabsState> {
    any(
        tabs::<TabsState, _>(build_tabs(state), state.selected, |s: &mut TabsState, i| {
            s.selected = i;
        })
        .variant(state.variant),
    )
}

fn appearance_panel(state: &TabsState) -> AnyView<TabsState> {
    control_panel(
        "Appearance",
        vec![
            play_enum_segmented::<TabsState, TabsVariant>(
                "Variant",
                state.variant,
                &VARIANTS,
                variant_label,
                |s: &mut TabsState, v| s.variant = v,
            ),
            play_switch::<TabsState>("Show icons", state.show_icons, |s, v| s.show_icons = v),
        ],
    )
}

/// The FRUST-equivalent snippet for the current knob state.
fn snippet_code(state: &TabsState) -> String {
    let tabs_block = if state.show_icons {
        "tabs: vec![\n        tab().label(\"Overview\").icon(icons::HOME),\n        tab().label(\"Specs\").icon(icons::TUNE),\n        tab().label(\"Reviews\").icon(icons::STAR_OUTLINE),\n    ],"
    } else {
        "tabs: vec![\n        tab().label(\"Overview\"),\n        tab().label(\"Specs\"),\n        tab().label(\"Reviews\"),\n    ],"
    };
    format!(
        "tabs(\n    {tabs_block}\n    {selected},\n    on_selected,\n)\n    .variant(TabsVariant::{variant:?});",
        tabs_block = tabs_block,
        selected = state.selected,
        variant = state.variant,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_state() -> TabsState {
        TabsPlayground.init()
    }

    /// Every state this file's own pickers can reach still builds a page —
    /// variant, show-icons, and every selectable tab index.
    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let mut state = base_state();
        for variant in VARIANTS {
            state.variant = variant;
            let _view = body(&state);
        }
        for show_icons in [true, false] {
            state.show_icons = show_icons;
            let _view = body(&state);
        }
        for index in 0..TAB_SPECS.len() {
            state.selected = index;
            let _view = body(&state);
        }
    }
}
