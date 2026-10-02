//! Navigation bar: the reference's `NavigationBarPlayground`.
//!
//! **Descoped**: the reference's Shape and Density controls
//! (`M3ENavBarShapeFamily`/`M3ENavBarDensity`). `NavigationBarView` carries
//! neither knob — `plugins/material/src/navbar.rs`'s module docs list both
//! among the "Unported upstream props" (container/decoration knobs with no
//! axis in this port's scope); this page shows the one supported surface
//! (size, label behavior, indicator style, badges) and omits the two
//! controls rather than wiring them to nothing.

use frust::{AnyView, Component, Stack, Theme, View, any, component, container, icon};
use frust_material::{
    MaterialDimensions, NavBarIndicatorStyle, NavBarLabelBehavior, NavBarSize, NavItem,
    OverlayAnchor, icons, nav_item, navigation_bar,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    ambient_theme, control_panel, play_enum_menu_field, play_enum_menu_panel, play_enum_segmented,
    play_preview_card, play_snippet, play_switch, playground_body,
};

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(NavigationBarPlayground))
}

const LABEL_BEHAVIORS: [NavBarLabelBehavior; 3] = [
    NavBarLabelBehavior::AlwaysShow,
    NavBarLabelBehavior::OnlySelected,
    NavBarLabelBehavior::AlwaysHide,
];
const NAV_BAR_SIZES: [NavBarSize; 2] = [NavBarSize::Small, NavBarSize::Medium];
const INDICATOR_STYLES: [NavBarIndicatorStyle; 3] = [
    NavBarIndicatorStyle::Pill,
    NavBarIndicatorStyle::Underline,
    NavBarIndicatorStyle::None,
];

fn label_behavior_label(v: NavBarLabelBehavior) -> &'static str {
    match v {
        NavBarLabelBehavior::AlwaysShow => "always show",
        NavBarLabelBehavior::OnlySelected => "only selected",
        NavBarLabelBehavior::AlwaysHide => "always hide",
    }
}

fn size_label(v: NavBarSize) -> &'static str {
    match v {
        NavBarSize::Small => "small",
        NavBarSize::Medium => "medium",
    }
}

fn indicator_label(v: NavBarIndicatorStyle) -> &'static str {
    match v {
        NavBarIndicatorStyle::Pill => "pill",
        NavBarIndicatorStyle::Underline => "underline",
        NavBarIndicatorStyle::None => "none",
    }
}

/// This playground's own knobs — the reference's `_NavigationBarPlaygroundState`
/// fields, plus the two independent [`OverlayAnchor`]/open-flag pairs the
/// Labels and Indicator pickers each need (see
/// [`crate::widgets::playground`]'s module docs on
/// `play_enum_menu_field`/`play_enum_menu_panel`).
struct NavBarState {
    selected: usize,
    label_behavior: NavBarLabelBehavior,
    size: NavBarSize,
    indicator: NavBarIndicatorStyle,
    badges: bool,
    label_anchor: OverlayAnchor,
    label_open: bool,
    indicator_anchor: OverlayAnchor,
    indicator_open: bool,
}

struct NavigationBarPlayground;

impl Component for NavigationBarPlayground {
    type State = NavBarState;

    fn init(&self) -> NavBarState {
        NavBarState {
            selected: 0,
            label_behavior: NavBarLabelBehavior::AlwaysShow,
            size: NavBarSize::Medium,
            indicator: NavBarIndicatorStyle::Pill,
            badges: true,
            label_anchor: OverlayAnchor::new(),
            label_open: false,
            indicator_anchor: OverlayAnchor::new(),
            indicator_open: false,
        }
    }

    fn build(&self, state: &mut NavBarState) -> AnyView<NavBarState> {
        body(state)
    }
}

/// The whole page body: preview/snippet/controls, plus both enum-menu
/// popups mounted at this page's own outer [`Stack`] — the kept-mounted
/// pattern the kit's own module docs document.
fn body(state: &NavBarState) -> AnyView<NavBarState> {
    let theme = ambient_theme();
    let content = playground_body(
        vec![play_preview_card("Navigation bar", preview(&theme, state))],
        vec![play_snippet("Navigation bar", snippet_code(state))],
        vec![appearance_panel(state)],
    );
    any(Stack(vec![
        content,
        label_menu_panel(state),
        indicator_menu_panel(state),
    ]))
}

/// Frame `child` in an outlined, rounded box — the reference's `_framed`
/// helper (duplicated per playground page upstream, so this port does the
/// same rather than sharing one across files).
fn framed(theme: &Theme, child: impl View<NavBarState>) -> AnyView<NavBarState> {
    let outline = theme.scheme().outline_variant;
    any(container(child)
        .radius(MaterialDimensions::RADIUS_LARGE)
        .border(outline, 1.0))
}

/// The four destinations — the reference's own `_destinations` getter.
fn destinations(state: &NavBarState) -> Vec<NavItem<NavBarState>> {
    let mut search = nav_item::<NavBarState>("Search").icon(any(icon(icons::SEARCH)));
    let mut agenda = nav_item::<NavBarState>("Agenda").icon(any(icon(icons::CALENDAR_TODAY)));
    if state.badges {
        search = search.badge_dot();
        agenda = agenda.badge_count(3);
    }
    vec![
        nav_item::<NavBarState>("Home").icon(any(icon(icons::HOME))),
        search,
        agenda,
        nav_item::<NavBarState>("Drafts").icon(any(icon(icons::EDIT))),
    ]
}

fn preview(theme: &Theme, state: &NavBarState) -> AnyView<NavBarState> {
    framed(
        theme,
        navigation_bar::<NavBarState, _>(
            destinations(state),
            state.selected,
            |s: &mut NavBarState, i| {
                s.selected = i;
            },
        )
        .size(state.size)
        .label_behavior(state.label_behavior)
        .indicator_style(state.indicator)
        .safe_area(false),
    )
}

fn appearance_panel(state: &NavBarState) -> AnyView<NavBarState> {
    control_panel(
        "Appearance",
        vec![
            play_enum_menu_field::<NavBarState, NavBarLabelBehavior>(
                "Labels",
                state.label_behavior,
                &LABEL_BEHAVIORS,
                label_behavior_label,
                &state.label_anchor,
                state.label_open,
                |s: &mut NavBarState, open| s.label_open = open,
            ),
            play_enum_segmented::<NavBarState, NavBarSize>(
                "Size",
                state.size,
                &NAV_BAR_SIZES,
                size_label,
                |s: &mut NavBarState, v| s.size = v,
            ),
            play_enum_menu_field::<NavBarState, NavBarIndicatorStyle>(
                "Indicator",
                state.indicator,
                &INDICATOR_STYLES,
                indicator_label,
                &state.indicator_anchor,
                state.indicator_open,
                |s: &mut NavBarState, open| s.indicator_open = open,
            ),
            play_switch::<NavBarState>("Badges", state.badges, |s, v| s.badges = v),
        ],
    )
}

fn label_menu_panel(state: &NavBarState) -> AnyView<NavBarState> {
    play_enum_menu_panel::<NavBarState, NavBarLabelBehavior>(
        state.label_behavior,
        &LABEL_BEHAVIORS,
        label_behavior_label,
        &state.label_anchor,
        state.label_open,
        |s: &mut NavBarState, open| s.label_open = open,
        |s: &mut NavBarState, v| s.label_behavior = v,
    )
}

fn indicator_menu_panel(state: &NavBarState) -> AnyView<NavBarState> {
    play_enum_menu_panel::<NavBarState, NavBarIndicatorStyle>(
        state.indicator,
        &INDICATOR_STYLES,
        indicator_label,
        &state.indicator_anchor,
        state.indicator_open,
        |s: &mut NavBarState, open| s.indicator_open = open,
        |s: &mut NavBarState, v| s.indicator = v,
    )
}

/// The FRUST-equivalent snippet for the current knob state.
fn snippet_code(state: &NavBarState) -> String {
    let rows = if state.badges {
        "destinations: vec![\n        nav_item(\"Home\").icon(any(icon(icons::HOME))),\n        nav_item(\"Search\").icon(any(icon(icons::SEARCH))).badge_dot(),\n        nav_item(\"Agenda\").icon(any(icon(icons::CALENDAR_TODAY))).badge_count(3),\n        nav_item(\"Drafts\").icon(any(icon(icons::EDIT))),\n    ],"
    } else {
        "destinations: vec![\n        nav_item(\"Home\").icon(any(icon(icons::HOME))),\n        nav_item(\"Search\").icon(any(icon(icons::SEARCH))),\n        nav_item(\"Agenda\").icon(any(icon(icons::CALENDAR_TODAY))),\n        nav_item(\"Drafts\").icon(any(icon(icons::EDIT))),\n    ],"
    };
    format!(
        "navigation_bar(\n    {rows}\n    selected,\n    on_select,\n)\n    .size(NavBarSize::{size:?})\n    .label_behavior(NavBarLabelBehavior::{behavior:?})\n    .indicator_style(NavBarIndicatorStyle::{indicator:?})\n    .safe_area(false);\n// preview is embedded mid-screen; a bar docked at the screen bottom omits .safe_area(false)",
        rows = rows,
        size = state.size,
        behavior = state.label_behavior,
        indicator = state.indicator,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_state() -> NavBarState {
        NavigationBarPlayground.init()
    }

    /// Every state this file's own pickers can reach still builds a page —
    /// selection, label behavior, size, indicator style, and badges.
    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let mut state = base_state();
        for index in 0..destinations(&state).len() {
            state.selected = index;
            let _view = body(&state);
        }
        for behavior in LABEL_BEHAVIORS {
            state.label_behavior = behavior;
            let _view = body(&state);
        }
        for size in NAV_BAR_SIZES {
            state.size = size;
            let _view = body(&state);
        }
        for indicator in INDICATOR_STYLES {
            state.indicator = indicator;
            let _view = body(&state);
        }
        for badges in [true, false] {
            state.badges = badges;
            let _view = body(&state);
        }
        state.label_open = true;
        state.indicator_open = true;
        let _view = body(&state);
    }
}
