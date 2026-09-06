//! Menu: the reference's `MenuPlayground`.
//!
//! The demoed surface is [`frust_material::menu`] — a [`MenuPanelView`] inside
//! [`frust_material::overlay::anchored`] — not the panel alone; see the menu
//! module's own docs for why the host owns placement, light dismiss, Escape
//! and the enter/exit ramp instead of this catalog hand-rolling them again.
//! The trigger and the popup are therefore two pieces sharing one
//! [`OverlayAnchor`], the same kept-mounted split
//! [`crate::widgets::playground`]'s `play_enum_menu_field`/
//! `play_enum_menu_panel` document and `nav/app_bars.rs`'s Kind picker
//! exercises — mounted here as [`trigger_view`] (inside the preview card) and
//! [`menu_panel_view`] (at the page's own outer [`Stack`]).

use frust::{AnyView, Component, Stack, any, component, icon};
use frust_material::{
    MenuAction, MenuColorStyle, MenuNode, MenuSelection, OverlayAlign, OverlayAnchor, OverlaySide,
    icons, menu, menu_entry, menu_group, menu_selectable, menu_submenu, menu_toggleable,
    overlay_anchor, tonal_button,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    control_panel, play_enum_menu_field, play_enum_menu_panel, play_enum_segmented,
    play_preview_card, play_snippet, play_switch, playground_body,
};

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(MenuPlayground))
}

const COLOR_STYLES: [MenuColorStyle; 2] = [MenuColorStyle::Standard, MenuColorStyle::Vibrant];

fn color_style_label(v: MenuColorStyle) -> &'static str {
    match v {
        MenuColorStyle::Standard => "standard",
        MenuColorStyle::Vibrant => "vibrant",
    }
}

/// The reference's `M3EMenuAnchorPosition` — a (side, cross-axis align) pair
/// this catalog's [`OverlaySide`]/[`OverlayAlign`] already carry separately;
/// this enum is the kit's picker vocabulary over that pair, not a new prop on
/// the widget itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuPosition {
    BottomStart,
    BottomEnd,
    TopStart,
    TopEnd,
}

impl MenuPosition {
    const ALL: [MenuPosition; 4] = [
        MenuPosition::BottomStart,
        MenuPosition::BottomEnd,
        MenuPosition::TopStart,
        MenuPosition::TopEnd,
    ];

    fn label(self) -> &'static str {
        match self {
            MenuPosition::BottomStart => "bottom start",
            MenuPosition::BottomEnd => "bottom end",
            MenuPosition::TopStart => "top start",
            MenuPosition::TopEnd => "top end",
        }
    }

    fn side(self) -> OverlaySide {
        match self {
            MenuPosition::BottomStart | MenuPosition::BottomEnd => OverlaySide::Bottom,
            MenuPosition::TopStart | MenuPosition::TopEnd => OverlaySide::Top,
        }
    }

    fn align(self) -> OverlayAlign {
        match self {
            MenuPosition::BottomStart | MenuPosition::TopStart => OverlayAlign::Start,
            MenuPosition::BottomEnd | MenuPosition::TopEnd => OverlayAlign::End,
        }
    }
}

/// This playground's own knobs — the reference's `_MenuPlaygroundState`
/// fields, plus the trigger's own [`OverlayAnchor`]/open flag (the menu being
/// demoed) and the Position picker's independent pair (the kit's control,
/// see [`crate::widgets::playground`]'s module docs).
struct MenuState {
    color_style: MenuColorStyle,
    position: MenuPosition,
    close_on_select: bool,
    selected: String,
    starred: bool,
    trigger_anchor: OverlayAnchor,
    trigger_open: bool,
    position_anchor: OverlayAnchor,
    position_open: bool,
}

struct MenuPlayground;

impl Component for MenuPlayground {
    type State = MenuState;

    fn init(&self) -> MenuState {
        MenuState {
            color_style: MenuColorStyle::Standard,
            position: MenuPosition::BottomStart,
            close_on_select: true,
            selected: "Inbox".to_string(),
            starred: true,
            trigger_anchor: OverlayAnchor::new(),
            trigger_open: false,
            position_anchor: OverlayAnchor::new(),
            position_open: false,
        }
    }

    fn build(&self, state: &mut MenuState) -> AnyView<MenuState> {
        body(state)
    }
}

/// The whole page body: preview/snippet/controls, plus the demoed menu's own
/// popup and the Position picker's popup, both mounted at this page's own
/// outer [`Stack`] — the kept-mounted pattern the kit's own module docs
/// document.
fn body(state: &MenuState) -> AnyView<MenuState> {
    let content = playground_body(
        vec![play_preview_card("Anchored menu", trigger_view(state))],
        vec![play_snippet("Anchored menu", snippet_code(state))],
        vec![appearance_panel(state)],
    );
    any(Stack(vec![
        content,
        menu_panel_view(state),
        position_menu_panel(state),
    ]))
}

/// The mailbox/starred/more-actions node tree — the reference's `_children`
/// getter.
fn nodes(state: &MenuState) -> Vec<MenuNode> {
    vec![
        menu_group(vec![
            menu_selectable("Inbox", "Inbox")
                .leading(icons::INBOX)
                .selected(state.selected == "Inbox")
                .into(),
            menu_selectable("Sent", "Sent")
                .leading(icons::SEND)
                .selected(state.selected == "Sent")
                .into(),
        ])
        .label("Mailbox")
        .into(),
        menu_group(vec![
            menu_toggleable("Starred", state.starred).into(),
            menu_submenu(
                "More actions",
                vec![
                    menu_entry("Archive").leading(icons::ARCHIVE).into(),
                    menu_entry("Report").leading(icons::REPORT).into(),
                ],
            )
            .leading(icons::MORE_HORIZ)
            .into(),
        ])
        .into(),
    ]
}

/// Apply an activation to state — factored out of [`menu_panel_view`]'s
/// closure so it is unit-testable without a widget tree.
fn apply_selection(state: &mut MenuState, selection: MenuSelection) {
    match selection.action {
        MenuAction::Select(value) => state.selected = value,
        MenuAction::Toggle(checked) => state.starred = checked,
        MenuAction::Press => {}
    }
    if state.close_on_select {
        state.trigger_open = false;
    }
}

/// The trigger half: a tonal icon+label button reporting its own rect into
/// [`MenuState::trigger_anchor`] — the reference's `anchorBuilder`.
fn trigger_view(state: &MenuState) -> AnyView<MenuState> {
    any(overlay_anchor(
        &state.trigger_anchor,
        tonal_button(state.selected.clone(), |s: &mut MenuState| {
            s.trigger_open = true;
        })
        .icon(any(icon(icons::MORE_VERT))),
    ))
}

/// The popup half: the demoed menu itself, anchored to [`trigger_view`]'s
/// captured rect.
fn menu_panel_view(state: &MenuState) -> AnyView<MenuState> {
    any(menu(nodes(state), apply_selection)
        .anchor(&state.trigger_anchor)
        .side(state.position.side())
        .align(state.position.align())
        .color_style(state.color_style)
        .open(state.trigger_open)
        .on_dismiss(|s: &mut MenuState| s.trigger_open = false))
}

fn appearance_panel(state: &MenuState) -> AnyView<MenuState> {
    control_panel(
        "Appearance",
        vec![
            play_enum_segmented::<MenuState, MenuColorStyle>(
                "Color",
                state.color_style,
                &COLOR_STYLES,
                color_style_label,
                |s: &mut MenuState, v| s.color_style = v,
            ),
            play_enum_menu_field::<MenuState, MenuPosition>(
                "Position",
                state.position,
                &MenuPosition::ALL,
                MenuPosition::label,
                &state.position_anchor,
                state.position_open,
                |s: &mut MenuState, open| s.position_open = open,
            ),
            play_switch::<MenuState>("Close on select", state.close_on_select, |s, v| {
                s.close_on_select = v;
            }),
        ],
    )
}

fn position_menu_panel(state: &MenuState) -> AnyView<MenuState> {
    play_enum_menu_panel::<MenuState, MenuPosition>(
        state.position,
        &MenuPosition::ALL,
        MenuPosition::label,
        &state.position_anchor,
        state.position_open,
        |s: &mut MenuState, open| s.position_open = open,
        |s: &mut MenuState, v| s.position = v,
    )
}

/// The FRUST-equivalent snippet for the current knob state.
fn snippet_code(state: &MenuState) -> String {
    format!(
        "menu(\n    vec![\n        menu_group(vec![\n            menu_selectable(\"Inbox\", \"Inbox\").selected({inbox_selected}),\n            menu_selectable(\"Sent\", \"Sent\").selected({sent_selected}),\n        ]).label(\"Mailbox\"),\n        menu_group(vec![\n            menu_toggleable(\"Starred\", {starred}),\n            menu_submenu(\"More actions\", vec![\n                menu_entry(\"Archive\"),\n                menu_entry(\"Report\"),\n            ]),\n        ]),\n    ],\n    on_select,\n)\n    .anchor(&anchor)\n    .side(OverlaySide::{side:?})\n    .align(OverlayAlign::{align:?})\n    .color_style(MenuColorStyle::{color_style:?})\n    .open({open});",
        inbox_selected = state.selected == "Inbox",
        sent_selected = state.selected == "Sent",
        starred = state.starred,
        side = state.position.side(),
        align = state.position.align(),
        color_style = state.color_style,
        open = state.trigger_open,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_state() -> MenuState {
        MenuPlayground.init()
    }

    /// Every state this file's own pickers can reach still builds a page —
    /// every color style, every position, the close-on-select toggle, and
    /// both overlays open.
    #[test]
    fn the_page_builds_across_every_reachable_control_state() {
        let mut state = base_state();
        for color_style in COLOR_STYLES {
            state.color_style = color_style;
            let _view = body(&state);
        }
        for position in MenuPosition::ALL {
            state.position = position;
            let _view = body(&state);
        }
        for flag in [true, false] {
            state.close_on_select = flag;
            state.starred = flag;
            let _view = body(&state);
        }
        state.selected = "Sent".to_string();
        let _view = body(&state);
        state.trigger_open = true;
        state.position_open = true;
        let _view = body(&state);
    }

    /// A `Select` activation confirms the requested value and, when
    /// `close_on_select` is on, closes the trigger; a `Toggle` flips
    /// `starred` the same way; a plain `Press` (Archive/Report) touches
    /// neither.
    #[test]
    fn a_selection_applies_the_requested_change_and_the_close_on_select_rule() {
        let mut state = base_state();
        state.trigger_open = true;

        apply_selection(
            &mut state,
            MenuSelection {
                index: 0,
                label: "Sent".into(),
                action: MenuAction::Select("Sent".into()),
            },
        );
        assert_eq!(state.selected, "Sent");
        assert!(!state.trigger_open, "close_on_select defaults on");

        state.trigger_open = true;
        state.close_on_select = false;
        apply_selection(
            &mut state,
            MenuSelection {
                index: 2,
                label: "Starred".into(),
                action: MenuAction::Toggle(false),
            },
        );
        assert!(!state.starred);
        assert!(state.trigger_open, "close_on_select off keeps it open");

        apply_selection(
            &mut state,
            MenuSelection {
                index: 4,
                label: "Archive".into(),
                action: MenuAction::Press,
            },
        );
        assert_eq!(state.selected, "Sent", "a Press touches neither value");
    }
}
