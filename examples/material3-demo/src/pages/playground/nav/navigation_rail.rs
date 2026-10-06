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

use frust::{AnyView, Component, SizedBox, Theme, View, any, component, container, icon, stack};
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

/// Fixed preview height, in logical px.
///
/// The reference keeps its own `SizedBox(height: 320)` because upstream's
/// rail scrolls its destination column internally; this port's rail does
/// not (`plugins/material/src/navigation_rail.rs`'s module docs list that
/// column under *Not ported*), so at 320 the expanded rail — menu button +
/// FAB + all four destinations — overflow-paints past the frame instead of
/// scrolling. The frame is sized to the content instead, derived from
/// `navigation_rail.rs`'s own layout constants — as the **max** of the
/// expanded and collapsed derivations below (G8's device-gate fix: this
/// constant used to be sized off the expanded math alone, but the
/// **collapsed** rail is the taller of the two — its 66dp item height
/// outgrows the expanded row's 40dp minimum by more than the collapsed
/// state saves elsewhere — so "Drafts" spilled below the frame while
/// collapsed, screenshot-confirmed on the Xiaomi).
///
/// Both derivations share a fixed prefix: `TOP_GAP` (36) + the menu button
/// row (48 — the default `Sm`/`Standard` icon button's tap target,
/// `icon_button.rs`'s `theme_target_size`, not the ~40dp visual box) +
/// `SECTION_PADDING_BOTTOM` (12) + the FAB row (56 — `extended_fab`'s fixed
/// `EXTENDED_HEIGHT` while expanded, `FabSize::Medium`'s 56dp container
/// while collapsed; `rail_fab`'s own default size, both states agree) +
/// `SECTION_PADDING_BOTTOM` (12) = 164dp, plus 4 destinations at their
/// per-state height (`ITEM_VERTICAL_GAP` × 2 above/below each row):
///
/// * Expanded: 164 + 4 × (4 + `RAIL_ITEM_EXPANDED_HEIGHT` + 4) =
///   164 + 4×48 = 356dp.
/// * Collapsed: 164 + 4 × (4 + `RAIL_ITEM_COLLAPSED_HEIGHT` + 4) =
///   164 + 4×74 = 460dp.
///
/// Collapsed wins (460 > 356dp); rounded up to 500dp for headroom. A test
/// pins this constant against both derivations computed from the plugin's
/// own exported height constants, so a future geometry-table change can't
/// silently reopen this gap.
const PREVIEW_HEIGHT: f64 = 500.0;

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

    fn build(&self, state: &mut RailState) -> impl View<RailState> {
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
    any(stack()
        .child(content)
        .child(type_menu_panel(state))
        .child(label_menu_panel(state)))
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
        rail_destination(icon(icons::HOME), "Home"),
        rail_destination(icon(icons::SEARCH), "Search"),
        rail_destination(icon(icons::CALENDAR_TODAY), "Agenda").badge_count(3),
        rail_destination(icon(icons::EDIT), "Drafts"),
    ])]
}

/// The rail's own in-rail menu-button toggle — without this the button
/// renders and presses but nothing observes the request
/// ([`frust_material::navigation_rail`]'s own doc). Wires to the same
/// `rail_type` knob the Type dropdown edits (`appearance_panel`'s
/// `play_enum_menu_field`), so both controls stay a single source of truth.
fn apply_rail_type(state: &mut RailState, next: NavigationRailType) {
    state.rail_type = next;
}

fn preview(theme: &Theme, state: &RailState) -> AnyView<RailState> {
    let mut rail = navigation_rail(sections(), state.selected, |s: &mut RailState, index| {
        s.selected = index
    })
    .rail_type(state.rail_type)
    .modality(NavigationRailModality::Standard)
    .label_behavior(state.label_behavior)
    .on_type_changed(apply_rail_type);
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
    use frust_material::{RAIL_ITEM_COLLAPSED_HEIGHT, RAIL_ITEM_EXPANDED_HEIGHT};

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

    /// The in-rail menu button's callback (`preview`'s `.on_type_changed`)
    /// writes the same `rail_type` field the Type dropdown does — a plain fn,
    /// so it's cheap to drive directly rather than through a simulated press.
    #[test]
    fn the_rail_s_own_menu_button_drives_the_rail_type_knob() {
        let mut state = base_state();
        assert_eq!(state.rail_type, NavigationRailType::Expanded);

        apply_rail_type(&mut state, NavigationRailType::Collapsed);
        assert_eq!(state.rail_type, NavigationRailType::Collapsed);

        apply_rail_type(&mut state, NavigationRailType::AlwaysExpand);
        assert_eq!(state.rail_type, NavigationRailType::AlwaysExpand);
    }

    /// The regression guard for G8 (device-gate round 3): `PREVIEW_HEIGHT`
    /// must cover both the expanded *and* the collapsed rail's content
    /// height — computed here from the plugin's own exported item-height
    /// constants (plus the fixed prefix [`PREVIEW_HEIGHT`]'s own doc comment
    /// derives), so a future change to either geometry table fails this test
    /// instead of silently reopening the "Drafts spills below the frame"
    /// defect the fix closed.
    #[test]
    fn preview_height_covers_both_the_expanded_and_the_collapsed_rail() {
        // The fixed prefix shared by both states: TOP_GAP (36) + the menu
        // button row (48) + SECTION_PADDING_BOTTOM (12) + the FAB row (56)
        // + SECTION_PADDING_BOTTOM (12) — see `PREVIEW_HEIGHT`'s own doc
        // comment for the full citation of each term's source.
        const PREFIX: f64 = 36.0 + 48.0 + 12.0 + 56.0 + 12.0;
        // ITEM_VERTICAL_GAP (4dp), above and below every destination row —
        // `navigation_rail.rs`'s own private constant, restated here since
        // it isn't exported.
        const ITEM_VERTICAL_GAP: f64 = 4.0;
        const DESTINATIONS: f64 = 4.0;

        let expanded_height =
            PREFIX + DESTINATIONS * (ITEM_VERTICAL_GAP * 2.0 + RAIL_ITEM_EXPANDED_HEIGHT);
        let collapsed_height =
            PREFIX + DESTINATIONS * (ITEM_VERTICAL_GAP * 2.0 + RAIL_ITEM_COLLAPSED_HEIGHT);

        assert!(
            collapsed_height > expanded_height,
            "the collapsed rail is the taller of the two states: {collapsed_height} vs {expanded_height}"
        );
        assert!(
            PREVIEW_HEIGHT >= collapsed_height,
            "PREVIEW_HEIGHT ({PREVIEW_HEIGHT}) must cover the taller collapsed rail ({collapsed_height})"
        );
        assert!(
            PREVIEW_HEIGHT >= expanded_height,
            "PREVIEW_HEIGHT ({PREVIEW_HEIGHT}) must cover the expanded rail ({expanded_height})"
        );
    }
}
