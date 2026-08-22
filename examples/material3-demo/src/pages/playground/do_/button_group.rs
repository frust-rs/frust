//! Button group: the reference's `ButtonGroupPlayground`.
//!
//! # Three deltas from upstream's own knob surface
//!
//! - **No "Shape" control.** The reference's `M3EButtonShape` knob toggles a
//!   *static* round/square family, independent of checked state. This port
//!   has no such seam at all —
//!   [`mod@frust_material::toggle_button`]'s own module docs: the checked
//!   shape morph (round unchecked, square checked, one spring) is fully
//!   intrinsic, with no builder overriding the family the way the plain
//!   [`mod@frust_material::button`]'s `ButtonShape` does. Omitted, not
//!   stubbed.
//! - **[`ToggleButtonSize`] has no `xs` row** — scoped to `sm`/`md`/`lg` per
//!   that module's own docs. The reference's "Compact xs (scroll overflow)"
//!   preview substitutes [`ToggleButtonSize::Sm`], the smallest size this
//!   port ships; everything else about that preview (compact density, the
//!   explicit 8px spacing, three short-label actions inside a 32px-tall box)
//!   still demonstrates the same scroll-overflow behaviour.
//! - **No "Toggle gradient fill" preview.**
//!   [`mod@frust_material::toggle_button`]'s own module docs: neither a focus
//!   ring nor the `M3EToggleButtonDecoration` gradient/override seam is
//!   ported in this v1 (the plain button's own `ButtonDecoration` seam is
//!   where a future one would land). Omitted, not stubbed.
//!
//! Knobs live in a page-local [`Knobs`], owned by the nested
//! [`ButtonGroupPlayground`] `Component` (never [`AppState`]) per the page
//! contract in [`crate::pages::playground`]. `Size`/`Style` use the
//! two-piece [`play_enum_menu_field`]/[`play_enum_menu_panel`] control (each
//! needs its own [`OverlayAnchor`], both held in `Knobs`, with the panels
//! mounted at this page's own outer [`Stack`]); `Type` fits a plain
//! [`play_enum_segmented`] since it is a 2-way choice.

use frust::{AnyView, Component, SizedBox, Stack, any, component, icon};
use frust_material::{
    ButtonGroupDensity, ButtonGroupType, ButtonVariant, OverlayAnchor, ToggleButtonSize,
    button_group_action, button_group_actions, icons, toggle_button,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    PlaySnippet, control_panel, play_enum_menu_field, play_enum_menu_panel, play_enum_segmented,
    play_preview_card, play_snippet, play_switch, playground_body,
};

/// Every [`ButtonGroupType`] the "Type" segmented control offers — the
/// crate's own declaration order.
const TYPES: [ButtonGroupType; 2] = [ButtonGroupType::Standard, ButtonGroupType::Connected];

/// Every [`ToggleButtonSize`] the "Size" menu offers — see the [module
/// docs](self) for why there is no `xs` row.
const SIZES: [ToggleButtonSize; 3] = [
    ToggleButtonSize::Sm,
    ToggleButtonSize::Md,
    ToggleButtonSize::Lg,
];

/// Every [`ButtonVariant`] the "Style" menu offers.
const STYLES: [ButtonVariant; 5] = [
    ButtonVariant::Filled,
    ButtonVariant::Outlined,
    ButtonVariant::Tonal,
    ButtonVariant::Elevated,
    ButtonVariant::Text,
];

/// This page's own knob state — held by [`ButtonGroupPlayground`], never
/// [`AppState`] (the page contract in [`crate::pages::playground`]).
struct Knobs {
    group_type: ButtonGroupType,
    size: ToggleButtonSize,
    style: ButtonVariant,
    neighbor_squish: bool,
    selected: usize,
    toggle_checked: bool,
    /// Shared with [`size_menu_panel`] — the "Size" dropdown's anchor.
    size_anchor: OverlayAnchor,
    size_open: bool,
    /// Shared with [`style_menu_panel`] — the "Style" dropdown's anchor.
    style_anchor: OverlayAnchor,
    style_open: bool,
}

impl Default for Knobs {
    /// The reference's own `_ButtonGroupPlaygroundState` field initializers.
    fn default() -> Self {
        Self {
            group_type: ButtonGroupType::Standard,
            size: ToggleButtonSize::Sm,
            style: ButtonVariant::Filled,
            neighbor_squish: true,
            selected: 0,
            toggle_checked: false,
            size_anchor: OverlayAnchor::new(),
            size_open: false,
            style_anchor: OverlayAnchor::new(),
            style_open: false,
        }
    }
}

/// Type label for the segmented control — the reference's
/// `M3EButtonGroupType.name`.
fn type_label(group_type: ButtonGroupType) -> &'static str {
    match group_type {
        ButtonGroupType::Standard => "standard",
        ButtonGroupType::Connected => "connected",
    }
}

/// Size label for the menu/snippet — the reference's `M3EButtonSize.name`
/// restricted to this port's own `sm`/`md`/`lg` rows.
fn size_label(size: ToggleButtonSize) -> &'static str {
    match size {
        ToggleButtonSize::Sm => "sm",
        ToggleButtonSize::Md => "md",
        ToggleButtonSize::Lg => "lg",
    }
}

/// Style label for the menu/snippet — the reference's `M3EButtonStyle.name`.
fn style_label(style: ButtonVariant) -> &'static str {
    match style {
        ButtonVariant::Filled => "filled",
        ButtonVariant::Outlined => "outlined",
        ButtonVariant::Tonal => "tonal",
        ButtonVariant::Elevated => "elevated",
        ButtonVariant::Text => "text",
    }
}

/// `onSelectedIndexChanged`'s own guard, shared by every preview below: a
/// deselecting tap (`next == None`) is ignored, matching the reference's
/// `if (index != null) setState(...)`.
fn on_selected_index_changed(state: &mut Knobs, next: Option<usize>) {
    if let Some(index) = next {
        state.selected = index;
    }
}

/// The "Compact xs (scroll overflow)" preview: a compact, 32px-tall group
/// whose three short-label actions pan under the default
/// [`frust_material::ButtonGroupOverflow::Scroll`] once they overflow the
/// card's width — the reference's own `SizedBox(height: 32)` wrapper.
fn compact_preview(state: &Knobs) -> AnyView<Knobs> {
    let group = button_group_actions(vec![
        button_group_action("Every day"),
        button_group_action("Days per week"),
        button_group_action("Selected days"),
    ])
    .group_type(state.group_type)
    .density(ButtonGroupDensity::Compact)
    .spacing(8.0)
    .variant(state.style)
    .size(ToggleButtonSize::Sm)
    .neighbor_squish(state.neighbor_squish)
    .selected_index(Some(state.selected))
    .on_selected_index_changed(on_selected_index_changed);
    any(SizedBox::<Knobs>(None, Some(32.0)).child(group))
}

/// The "Button group" preview: the full-size group at the current knob
/// state, with icon+label actions.
fn button_group_preview(state: &Knobs) -> AnyView<Knobs> {
    any(button_group_actions(vec![
        button_group_action("Left").icon(|| any(icon(icons::FORMAT_ALIGN_LEFT))),
        button_group_action("Center").icon(|| any(icon(icons::FORMAT_ALIGN_CENTER))),
        button_group_action("Right").icon(|| any(icon(icons::FORMAT_ALIGN_RIGHT))),
    ])
    .group_type(state.group_type)
    .variant(state.style)
    .size(state.size)
    .neighbor_squish(state.neighbor_squish)
    .selected_index(Some(state.selected))
    .on_selected_index_changed(on_selected_index_changed))
}

/// The "Toggle button" preview: a single [`mod@frust_material::toggle_button`]
/// sharing the same style/size knobs.
fn toggle_preview(state: &Knobs) -> AnyView<Knobs> {
    any(
        toggle_button(state.toggle_checked, |state: &mut Knobs, checked: bool| {
            state.toggle_checked = checked;
        })
        .icon(any(icon(icons::STAR)))
        .checked_icon(any(icon(icons::STAR)))
        .label("Star")
        .variant(state.style)
        .size(state.size),
    )
}

/// "Group" controls: type, size, style, neighbor squish. See the [module
/// docs](self) for why there is no "Shape" row.
fn group_panel(state: &Knobs) -> AnyView<Knobs> {
    control_panel(
        "Group",
        vec![
            play_enum_segmented(
                "Type",
                state.group_type,
                &TYPES,
                type_label,
                |state: &mut Knobs, next: ButtonGroupType| state.group_type = next,
            ),
            play_enum_menu_field(
                "Size",
                state.size,
                &SIZES,
                size_label,
                &state.size_anchor,
                state.size_open,
                |state: &mut Knobs, open: bool| state.size_open = open,
            ),
            play_enum_menu_field(
                "Style",
                state.style,
                &STYLES,
                style_label,
                &state.style_anchor,
                state.style_open,
                |state: &mut Knobs, open: bool| state.style_open = open,
            ),
            play_switch(
                "Neighbor squish",
                state.neighbor_squish,
                |state: &mut Knobs, next: bool| state.neighbor_squish = next,
            ),
        ],
    )
}

/// The "Size" menu's popup half — mounted at this page's outer [`Stack`].
fn size_menu_panel(state: &Knobs) -> AnyView<Knobs> {
    play_enum_menu_panel(
        state.size,
        &SIZES,
        size_label,
        &state.size_anchor,
        state.size_open,
        |state: &mut Knobs, open: bool| state.size_open = open,
        |state: &mut Knobs, next: ToggleButtonSize| state.size = next,
    )
}

/// The "Style" menu's popup half — mounted at this page's outer [`Stack`].
fn style_menu_panel(state: &Knobs) -> AnyView<Knobs> {
    play_enum_menu_panel(
        state.style,
        &STYLES,
        style_label,
        &state.style_anchor,
        state.style_open,
        |state: &mut Knobs, open: bool| state.style_open = open,
        |state: &mut Knobs, next: ButtonVariant| state.style = next,
    )
}

/// The paste-ready "Button group" snippet — the reference's own
/// `_snippets[0]`, in Frust rather than Dart.
fn button_group_snippet(state: &Knobs) -> PlaySnippet {
    play_snippet(
        "Button group",
        format!(
            "button_group_actions(vec![\n    \
             button_group_action(\"Left\").icon(|| any(icon(icons::FORMAT_ALIGN_LEFT))),\n    \
             button_group_action(\"Center\").icon(|| any(icon(icons::FORMAT_ALIGN_CENTER))),\n    \
             button_group_action(\"Right\").icon(|| any(icon(icons::FORMAT_ALIGN_RIGHT))),\n\
             ])\n    .group_type(ButtonGroupType::{group_type:?})\n    \
             .variant(ButtonVariant::{style:?})\n    .size(ToggleButtonSize::{size:?})\n    \
             .neighbor_squish({neighbor_squish})\n    .selected_index(Some({selected}))\n    \
             .on_selected_index_changed(on_change);",
            group_type = state.group_type,
            style = state.style,
            size = state.size,
            neighbor_squish = state.neighbor_squish,
            selected = state.selected,
        ),
    )
}

/// The paste-ready "Toggle button" snippet — the reference's own
/// `_snippets[1]`, in Frust rather than Dart.
fn toggle_button_snippet(state: &Knobs) -> PlaySnippet {
    play_snippet(
        "Toggle button",
        format!(
            "toggle_button({checked}, on_checked_change)\n    .icon(any(icon(icons::STAR)))\n    \
             .checked_icon(any(icon(icons::STAR)))\n    .label(\"Star\")\n    \
             .variant(ButtonVariant::{style:?})\n    .size(ToggleButtonSize::{size:?});",
            checked = state.toggle_checked,
            style = state.style,
            size = state.size,
        ),
    )
}

/// The page body: the playground content plus the two dropdown panels it
/// anchors, stacked so both can paint above the scrollable content.
fn body(state: &Knobs) -> AnyView<Knobs> {
    let content = playground_body(
        vec![
            play_preview_card("Compact xs (scroll overflow)", compact_preview(state)),
            play_preview_card("Button group", button_group_preview(state)),
            play_preview_card("Toggle button", toggle_preview(state)),
        ],
        vec![button_group_snippet(state), toggle_button_snippet(state)],
        vec![group_panel(state)],
    );
    any(Stack(vec![
        content,
        size_menu_panel(state),
        style_menu_panel(state),
    ]))
}

/// This page's knob component — see the [module docs](self).
struct ButtonGroupPlayground;

impl Component for ButtonGroupPlayground {
    type State = Knobs;

    fn init(&self) -> Self::State {
        Knobs::default()
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        body(state)
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(ButtonGroupPlayground))
}

#[cfg(test)]
mod tests {
    use super::{
        ButtonVariant, Knobs, SIZES, STYLES, TYPES, body, button_group_snippet,
        toggle_button_snippet,
    };

    #[test]
    fn the_page_builds_across_every_group_type() {
        let mut knobs = Knobs::default();
        for group_type in TYPES {
            knobs.group_type = group_type;
            let _view = body(&knobs);
        }
    }

    #[test]
    fn the_page_builds_across_every_size() {
        let mut knobs = Knobs::default();
        for size in SIZES {
            knobs.size = size;
            let _view = body(&knobs);
        }
    }

    #[test]
    fn the_page_builds_across_every_style() {
        let mut knobs = Knobs::default();
        for style in STYLES {
            knobs.style = style;
            let _view = body(&knobs);
        }
    }

    #[test]
    fn the_page_builds_with_squish_off_and_a_non_zero_selection() {
        let knobs = Knobs {
            neighbor_squish: false,
            selected: 2,
            toggle_checked: true,
            ..Knobs::default()
        };
        let _view = body(&knobs);
    }

    #[test]
    fn the_page_builds_with_both_dropdown_menus_open() {
        let knobs = Knobs {
            size_open: true,
            style_open: true,
            ..Knobs::default()
        };
        let _view = body(&knobs);
    }

    #[test]
    fn the_button_group_snippet_reflects_the_current_style_and_squish() {
        let knobs = Knobs {
            style: ButtonVariant::Tonal,
            neighbor_squish: false,
            ..Knobs::default()
        };
        let snippet = button_group_snippet(&knobs);
        assert!(snippet.code.contains("Tonal"));
        assert!(snippet.code.contains("neighbor_squish(false)"));
    }

    #[test]
    fn the_toggle_button_snippet_reflects_the_checked_flag() {
        let knobs = Knobs {
            toggle_checked: true,
            ..Knobs::default()
        };
        let snippet = toggle_button_snippet(&knobs);
        assert!(snippet.code.starts_with("toggle_button(true"));
    }
}
