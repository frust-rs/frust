//! Toolbar: the reference's `ToolbarPlayground`.
//!
//! # Descoped: the `Axis` control
//!
//! The reference's `PlayEnumMenu<Axis>` (shown only for the floating
//! placement) picks between `Axis.horizontal`/`Axis.vertical`.
//! `plugins/material/src/toolbar.rs`'s module docs list **the vertical axis**
//! among its *Not ported* items — "a horizontal bar is the whole of this
//! module's v1" — so [`ToolbarView`] carries no axis knob to drive; this page
//! shows the one supported (horizontal) geometry and omits the control
//! rather than wiring it to nothing.
//!
//! # No scroll-hide demo: the reference has none
//!
//! `plugins/material/src/toolbar.rs`'s own scroll-hide contract
//! (`ToolbarScrollHide` + an app-owned `on_scroll`, the shape
//! `nav/app_bars.rs`'s sliver preview exercises for the app-bar family) has
//! no counterpart here: `toolbar_playground.dart` builds no `ScrollView`, no
//! visibility controller and no `on_scroll` wiring anywhere in its 191
//! lines — its whole demo is the static placement/color/expansion/FAB/label
//! knob set below. This page mirrors that source rather than inventing a
//! scroll-hide preview the reference itself does not show.

use frust::{Align, Alignment, AnyView, Component, View, any, component, icon};
use frust_material::{
    FabSize, ToolbarAction, ToolbarColorStyle, ToolbarVariant, docked_toolbar, fab,
    floating_toolbar, icons, toolbar_action,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    control_panel, play_enum_segmented, play_preview_card, play_snippet, play_switch,
    playground_body,
};

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(ToolbarPlayground))
}

const PLACEMENTS: [ToolbarVariant; 2] = [ToolbarVariant::Floating, ToolbarVariant::Docked];
const COLOR_STYLES: [ToolbarColorStyle; 2] =
    [ToolbarColorStyle::Standard, ToolbarColorStyle::Vibrant];

fn placement_label(v: ToolbarVariant) -> &'static str {
    match v {
        ToolbarVariant::Floating => "floating",
        ToolbarVariant::Docked => "docked",
    }
}

fn color_style_label(v: ToolbarColorStyle) -> &'static str {
    match v {
        ToolbarColorStyle::Standard => "standard",
        ToolbarColorStyle::Vibrant => "vibrant",
    }
}

/// This playground's own knobs — the reference's `_ToolbarPlaygroundState`
/// fields, minus `_axis` (see this module's own docs).
struct ToolbarState {
    placement: ToolbarVariant,
    color_style: ToolbarColorStyle,
    expanded: bool,
    show_fab: bool,
    labeled: bool,
    active_index: usize,
}

struct ToolbarPlayground;

impl Component for ToolbarPlayground {
    type State = ToolbarState;

    fn init(&self) -> ToolbarState {
        ToolbarState {
            placement: ToolbarVariant::Floating,
            color_style: ToolbarColorStyle::Standard,
            expanded: true,
            show_fab: false,
            labeled: false,
            active_index: 0,
        }
    }

    fn build(&self, state: &mut ToolbarState) -> impl View<ToolbarState> {
        body(state)
    }
}

/// The whole page body: [`playground_body`]'s preview/snippet/controls
/// arrangement. No overlay is mounted here — unlike its `nav` siblings that
/// pick from an enum menu, every control on this page is a segmented button
/// or a switch.
fn body(state: &ToolbarState) -> AnyView<ToolbarState> {
    playground_body(
        vec![play_preview_card(
            "Toolbar",
            Align(Alignment::CENTER, toolbar_view(state)),
        )],
        vec![play_snippet("Toolbar", snippet_code(state))],
        vec![appearance_panel(state), behavior_panel(state)],
    )
}

/// The typed action list — the reference's `_actions` getter: three labeled
/// selectable actions, or three icon-only ones with a share expand-trigger.
fn actions(state: &ToolbarState) -> Vec<ToolbarAction<ToolbarState>> {
    if state.labeled {
        [
            (icons::HOME, "Home"),
            (icons::SEARCH, "Search"),
            (icons::FAVORITE, "Favorites"),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (source, label))| {
            toolbar_action(source, move |s: &mut ToolbarState| s.active_index = index)
                .label(label)
                .active(state.active_index == index)
        })
        .collect()
    } else {
        vec![
            toolbar_action(icons::EDIT, |_: &mut ToolbarState| {}),
            toolbar_action(icons::SHARE, |_: &mut ToolbarState| {}).expand_trigger(true),
            toolbar_action(icons::FAVORITE, |_: &mut ToolbarState| {}),
        ]
    }
}

/// The adjacent FAB slot — the reference's `fabExpandIcon`/`fabCollapseIcon`
/// pair, folded into one caller-owned icon since [`ToolbarView::fab`] takes a
/// single opaque view (see the toolbar module docs' "who owns the toggle").
/// Add while collapsed (press to expand), close while expanded (press to
/// collapse).
fn fab_view(state: &ToolbarState) -> AnyView<ToolbarState> {
    let (source, label) = if state.expanded {
        (icons::CLOSE, "Collapse")
    } else {
        (icons::ADD, "Expand")
    };
    any(fab(any(icon(source)), |s: &mut ToolbarState| {
        s.expanded = !s.expanded;
    })
    .size(FabSize::Medium)
    .label(label))
}

fn toolbar_view(state: &ToolbarState) -> AnyView<ToolbarState> {
    match state.placement {
        ToolbarVariant::Docked => any(docked_toolbar::<ToolbarState>()
            .color_style(state.color_style)
            .actions(actions(state))),
        ToolbarVariant::Floating => {
            let mut bar = floating_toolbar::<ToolbarState>()
                .color_style(state.color_style)
                .expanded(state.expanded)
                .actions(actions(state));
            if state.show_fab {
                bar = bar.fab(fab_view(state));
            }
            any(bar)
        }
    }
}

fn appearance_panel(state: &ToolbarState) -> AnyView<ToolbarState> {
    control_panel(
        "Appearance",
        vec![
            play_enum_segmented::<ToolbarState, ToolbarVariant>(
                "Placement",
                state.placement,
                &PLACEMENTS,
                placement_label,
                |s: &mut ToolbarState, v| s.placement = v,
            ),
            play_enum_segmented::<ToolbarState, ToolbarColorStyle>(
                "Color",
                state.color_style,
                &COLOR_STYLES,
                color_style_label,
                |s: &mut ToolbarState, v| s.color_style = v,
            ),
        ],
    )
}

/// `Behavior` control panel: Expanded/Show FAB only apply to the floating
/// placement — the reference's own conditional rows.
fn behavior_panel(state: &ToolbarState) -> AnyView<ToolbarState> {
    let mut rows = Vec::new();
    if state.placement == ToolbarVariant::Floating {
        rows.push(play_switch::<ToolbarState>(
            "Expanded",
            state.expanded,
            |s, v| s.expanded = v,
        ));
        rows.push(play_switch::<ToolbarState>(
            "Show FAB",
            state.show_fab,
            |s, v| s.show_fab = v,
        ));
    }
    rows.push(play_switch::<ToolbarState>(
        "Labeled selection",
        state.labeled,
        |s, v| s.labeled = v,
    ));
    control_panel("Behavior", rows)
}

/// The FRUST-equivalent snippet for the current knob state.
fn snippet_code(state: &ToolbarState) -> String {
    let actions_code = if state.labeled {
        "vec![\n        toolbar_action(icons::HOME, on_press).label(\"Home\"),\n        toolbar_action(icons::SEARCH, on_press).label(\"Search\"),\n        toolbar_action(icons::FAVORITE, on_press).label(\"Favorites\"),\n    ]"
    } else {
        "vec![\n        toolbar_action(icons::EDIT, on_press),\n        toolbar_action(icons::SHARE, on_press).expand_trigger(true),\n        toolbar_action(icons::FAVORITE, on_press),\n    ]"
    };
    match state.placement {
        ToolbarVariant::Docked => format!(
            "docked_toolbar()\n    .color_style(ToolbarColorStyle::{color_style:?})\n    .actions({actions_code});",
            color_style = state.color_style,
        ),
        ToolbarVariant::Floating => {
            let fab_line = if state.show_fab {
                "\n    .fab(fab_view);"
            } else {
                ";"
            };
            format!(
                "floating_toolbar()\n    .color_style(ToolbarColorStyle::{color_style:?})\n    .expanded({expanded})\n    .actions({actions_code}){fab_line}",
                color_style = state.color_style,
                expanded = state.expanded,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_state() -> ToolbarState {
        ToolbarPlayground.init()
    }

    /// Every state this file's own pickers can reach still builds a page —
    /// both placements, both color styles, every toggle combination, and a
    /// non-zero active index under labeled selection.
    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let mut state = base_state();
        for placement in PLACEMENTS {
            state.placement = placement;
            let _view = body(&state);
        }
        state.placement = ToolbarVariant::Floating;
        for color_style in COLOR_STYLES {
            state.color_style = color_style;
            let _view = body(&state);
        }
        for flag in [true, false] {
            state.expanded = flag;
            state.show_fab = flag;
            state.labeled = flag;
            let _view = body(&state);
        }
        state.labeled = true;
        state.active_index = 2;
        let _view = body(&state);

        // The docked placement drops the Expanded/Show FAB rows entirely.
        state.placement = ToolbarVariant::Docked;
        let _view = body(&state);
    }
}
