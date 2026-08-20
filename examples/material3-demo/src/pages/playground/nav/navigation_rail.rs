//! Navigation rail: the reference's `NavigationRailPlayground`.
//!
//! # Descoped: the `Modality` control
//!
//! The reference seeds `_modality` from `M3ENavigationRailModality.standard`
//! as a `final` field — never wired to a control, never flipped by
//! `setState` anywhere in the page. This port matches the source rather than
//! adding a knob upstream itself never exposes: [`NavigationRailModality`]
//! stays fixed at [`NavigationRailModality::Standard`] here, and the modal
//! (scrim-over-content) presentation this widget also supports goes
//! undemoed on this page, exactly as it does upstream.

use frust::{AnyView, Component, SizedBox, Stack, Theme, View, any, component, container, icon};
use frust_material::{
    MaterialDimensions, NavigationRailModality, NavigationRailType, OverlayAnchor,
    RailLabelBehavior, RailSection, icons, navigation_rail, rail_destination, rail_fab,
    rail_section,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    ambient_theme, control_panel, play_enum_menu_field, play_enum_menu_panel, play_preview_card,
    play_snippet, play_switch, playground_body,
};

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(NavigationRailPlayground))
}

/// Fixed preview height, in logical px — the reference's own
/// `SizedBox(height: 320)`.
const PREVIEW_HEIGHT: f64 = 320.0;

const RAIL_TYPES: [NavigationRailType; 4] = [
    NavigationRailType::Collapsed,
    NavigationRailType::AlwaysCollapse,
    NavigationRailType::Expanded,
    NavigationRailType::AlwaysExpand,
];
const LABEL_BEHAVIORS: [RailLabelBehavior; 3] = [
    RailLabelBehavior::AlwaysShow,
    RailLabelBehavior::OnlySelected,
    RailLabelBehavior::AlwaysHide,
];

fn rail_type_label(v: NavigationRailType) -> &'static str {
    match v {
        NavigationRailType::Collapsed => "collapsed",
        NavigationRailType::AlwaysCollapse => "always collapse",
        NavigationRailType::Expanded => "expanded",
        NavigationRailType::AlwaysExpand => "always expand",
    }
}

fn label_behavior_label(v: RailLabelBehavior) -> &'static str {
    match v {
        RailLabelBehavior::AlwaysShow => "always show",
        RailLabelBehavior::OnlySelected => "only selected",
        RailLabelBehavior::AlwaysHide => "always hide",
    }
}

/// This playground's own knobs — the reference's
/// `_NavigationRailPlaygroundState` fields (minus `_modality`, see this
/// module's own docs), plus the two independent [`OverlayAnchor`]/open-flag
/// pairs the Type and Labels pickers each need.
struct RailState {
    selected: usize,
    rail_type: NavigationRailType,
    label_behavior: RailLabelBehavior,
    show_fab: bool,
    type_anchor: OverlayAnchor,
    type_open: bool,
    label_anchor: OverlayAnchor,
    label_open: bool,
}

struct NavigationRailPlayground;

impl Component for NavigationRailPlayground {
    type State = RailState;

    fn init(&self) -> RailState {
        RailState {
            selected: 0,
            rail_type: NavigationRailType::Expanded,
            label_behavior: RailLabelBehavior::AlwaysShow,
            show_fab: true,
            type_anchor: OverlayAnchor::new(),
            type_open: false,
            label_anchor: OverlayAnchor::new(),
            label_open: false,
        }
    }

    fn build(&self, state: &mut RailState) -> AnyView<RailState> {
        body(state)
    }
}

/// The whole page body: preview/snippet/controls, plus both enum-menu
/// popups mounted at this page's own outer [`Stack`] — the kept-mounted
/// pattern the kit's own module docs document.
fn body(state: &RailState) -> AnyView<RailState> {
    let theme = ambient_theme();
    let content = playground_body(
        vec![play_preview_card("Navigation rail", preview(&theme, state))],
        vec![play_snippet("Navigation rail", snippet_code(state))],
        vec![appearance_panel(state)],
    );
    any(Stack(vec![
        content,
        type_menu_panel(state),
        label_menu_panel(state),
    ]))
}

/// Frame `child` in an outlined, rounded box — the reference's `_framed`
/// helper (duplicated per playground page upstream, so this port does the
/// same rather than sharing one across files).
fn framed(theme: &Theme, child: impl View<RailState>) -> AnyView<RailState> {
    let outline = theme.scheme().outline_variant;
    any(container(child)
        .radius(MaterialDimensions::RADIUS_LARGE)
        .border(outline, 1.0))
}

/// The one section of four destinations — the reference's own `_sections`.
fn sections() -> Vec<RailSection<RailState>> {
    vec![rail_section(vec![
        rail_destination(any(icon(icons::HOME)), "Home"),
        rail_destination(any(icon(icons::SEARCH)), "Search"),
        rail_destination(any(icon(icons::CALENDAR_TODAY)), "Agenda").badge_count(3),
        rail_destination(any(icon(icons::EDIT)), "Drafts"),
    ])]
}

fn preview(theme: &Theme, state: &RailState) -> AnyView<RailState> {
    let mut rail = navigation_rail(sections(), state.selected, |s: &mut RailState, index| {
        s.selected = index
    })
    .rail_type(state.rail_type)
    .modality(NavigationRailModality::Standard)
    .label_behavior(state.label_behavior);
    if state.show_fab {
        rail = rail.fab(rail_fab(icons::ADD, "Compose", |_: &mut RailState| {}));
    }
    framed(
        theme,
        SizedBox::<RailState>(None, Some(PREVIEW_HEIGHT)).child(rail),
    )
}

fn appearance_panel(state: &RailState) -> AnyView<RailState> {
    control_panel(
        "Appearance",
        vec![
            play_enum_menu_field::<RailState, NavigationRailType>(
                "Type",
                state.rail_type,
                &RAIL_TYPES,
                rail_type_label,
                &state.type_anchor,
                state.type_open,
                |s: &mut RailState, open| s.type_open = open,
            ),
            play_enum_menu_field::<RailState, RailLabelBehavior>(
                "Labels",
                state.label_behavior,
                &LABEL_BEHAVIORS,
                label_behavior_label,
                &state.label_anchor,
                state.label_open,
                |s: &mut RailState, open| s.label_open = open,
            ),
            play_switch::<RailState>("Show FAB", state.show_fab, |s, v| s.show_fab = v),
        ],
    )
}

fn type_menu_panel(state: &RailState) -> AnyView<RailState> {
    play_enum_menu_panel::<RailState, NavigationRailType>(
        state.rail_type,
        &RAIL_TYPES,
        rail_type_label,
        &state.type_anchor,
        state.type_open,
        |s: &mut RailState, open| s.type_open = open,
        |s: &mut RailState, v| s.rail_type = v,
    )
}

fn label_menu_panel(state: &RailState) -> AnyView<RailState> {
    play_enum_menu_panel::<RailState, RailLabelBehavior>(
        state.label_behavior,
        &LABEL_BEHAVIORS,
        label_behavior_label,
        &state.label_anchor,
        state.label_open,
        |s: &mut RailState, open| s.label_open = open,
        |s: &mut RailState, v| s.label_behavior = v,
    )
}

/// The FRUST-equivalent snippet for the current knob state.
fn snippet_code(state: &RailState) -> String {
    let fab_line = if state.show_fab {
        "\n    .fab(rail_fab(icons::ADD, \"Compose\", on_press));"
    } else {
        ";"
    };
    format!(
        "navigation_rail(\n    vec![rail_section(vec![\n        rail_destination(home_icon, \"Home\"),\n        rail_destination(search_icon, \"Search\"),\n        rail_destination(agenda_icon, \"Agenda\").badge_count(3),\n        rail_destination(edit_icon, \"Drafts\"),\n    ])],\n    selected,\n    on_select,\n)\n    .rail_type(NavigationRailType::{rail_type:?})\n    .label_behavior(RailLabelBehavior::{label_behavior:?}){fab_line}",
        rail_type = state.rail_type,
        label_behavior = state.label_behavior,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_state() -> RailState {
        NavigationRailPlayground.init()
    }

    /// Every state this file's own pickers can reach still builds a page —
    /// selection, every rail type, every label behavior, and the FAB toggle.
    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let mut state = base_state();
        // Four destinations in the one section — see `sections()`.
        for index in 0..4 {
            state.selected = index;
            let _view = body(&state);
        }
        for rail_type in RAIL_TYPES {
            state.rail_type = rail_type;
            let _view = body(&state);
        }
        for behavior in LABEL_BEHAVIORS {
            state.label_behavior = behavior;
            let _view = body(&state);
        }
        for flag in [true, false] {
            state.show_fab = flag;
            let _view = body(&state);
        }
        state.type_open = true;
        state.label_open = true;
        let _view = body(&state);
    }
}
