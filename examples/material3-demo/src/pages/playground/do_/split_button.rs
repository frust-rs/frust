//! Split button: the reference's `SplitButtonPlayground`.
//!
//! # Three deltas from upstream's own preview/knob surface
//!
//! - **Only one preview.** [`mod@frust_material::split_button`]'s own module
//!   docs list what a v1 does not carry: `M3ESplitButtonDecoration`'s
//!   gradient layers (so the reference's "Gradient fill" preview has nothing
//!   to drive) and `menuBuilder`/`m3eMenuBuilder` (so the reference's "Custom
//!   M3E menu" preview — a hand-built, nested `M3EMenuSubmenu` tree — has no
//!   port to mirror; [`frust_material::SplitButtonItem`] is a flat list, no
//!   submenu). Both omitted, not stubbed.
//! - **"Menu style" is a 2-way segmented control, not a 3-way dropdown.**
//!   [`frust_material::SplitButtonMenuStyle`] ports only `Popup`/
//!   `BottomSheet` — the reference's third style, `native` (Flutter's
//!   `showMenu`), is descoped crate-wide (this framework hosts no platform
//!   menu route). A 2-way choice fits [`play_enum_segmented`] better than the
//!   reference's [`crate::widgets::playground::play_enum_menu_field`], the
//!   same swap this section's `Shape` rows already make.
//! - **"Size" offers all five tiers, not the reference's restricted three.**
//!   The reference's own `_sizes` picker only offers `xs`/`sm`/`md`;
//!   [`frust_material::SplitButtonSize::ALL`] is the crate's own full
//!   five-tier axis ("every size, in table order — the axis a matrix test
//!   walks"), and nothing in the port narrows it, so this page shows the
//!   whole thing rather than reproducing an upstream UI convenience.
//!
//! # The two menu routes, one page
//!
//! [`frust_material::SplitButtonView::popup_menu`] is a second, app-mounted
//! view (kept mounted at this page's outer `frust::Stack`, toggled via
//! [`Knobs::open`]) — mounted only while [`Knobs::menu_style`] is
//! [`SplitButtonMenuStyle::Popup`]. `BottomSheet` instead needs a
//! [`frust::NavigatorController`] a leaf widget cannot reach, so — per this
//! section's own established split (see [`crate::pages::playground::view::bottom_sheet`]) —
//! [`SplitButtonPlayground::build`] mounts a thin [`frust::navigator`] and
//! [`content`] holds everything this file's tests exercise. Both routes
//! report a selection through the same [`on_select`], mirroring the
//! reference's single `onSelected` regardless of `menuStyle`; the
//! bottom-sheet route is composed by hand from
//! [`frust_material::menu_selectable`] rather than reached through
//! [`frust_material::SplitButtonView::sheet_menu`], which needs `&self` on a
//! live view instance an event handler never holds.
//!
//! Knobs live in a page-local [`Knobs`], owned by the nested
//! [`SplitButtonPlayground`] `Component` (never [`AppState`]) per the page
//! contract in [`crate::pages::playground`]. `Knobs` derives `Clone` (every
//! field — `NavigatorController`, `OverlayAnchor`, plain data — already is)
//! so [`SplitButtonPlayground::build`] can hand [`content`] an owned
//! snapshot inside the navigator's build closure, which cannot borrow the
//! live component state.

use frust::{
    AnyView, Component, NavigatorController, PopResult, Stack, any, component, icon, navigator,
};
use frust_material::{
    MenuNode, MenuSelection, OverlayAnchor, SplitButtonItem, SplitButtonMenuStyle,
    SplitButtonShape, SplitButtonSize, SplitButtonVariant, bottom_sheet, icons, menu_panel,
    menu_selectable, show_bottom_sheet, split_button, split_button_item,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    PlaySnippet, control_panel, play_enum_menu_field, play_enum_menu_panel, play_enum_segmented,
    play_preview_card, play_snippet, play_switch, play_text_field, playground_body,
};

/// Every [`SplitButtonShape`] the "Shape" segmented control offers.
const SHAPES: [SplitButtonShape; 2] = [SplitButtonShape::Round, SplitButtonShape::Square];

/// Every ported [`SplitButtonMenuStyle`] the "Menu style" segmented control
/// offers — see the [module docs](self) for why `native` has no third row
/// here.
const MENU_STYLES: [SplitButtonMenuStyle; 2] = [
    SplitButtonMenuStyle::Popup,
    SplitButtonMenuStyle::BottomSheet,
];

/// This page's own knob state — held by [`SplitButtonPlayground`], never
/// [`AppState`] (the page contract in [`crate::pages::playground`]). See the
/// [module docs](self) for why this derives `Clone`.
#[derive(Clone)]
struct Knobs {
    nav: NavigatorController<Knobs>,
    style: SplitButtonVariant,
    size: SplitButtonSize,
    shape: SplitButtonShape,
    menu_style: SplitButtonMenuStyle,
    enabled: bool,
    label: String,
    /// The app-confirmed selected item value (`"draft"`/`"copy"`), reported
    /// through [`on_select`] from either menu route.
    selected: Option<String>,
    /// Whether the trailing menu is currently presented — drives the
    /// chevron rotation and the trailing segment's open shape for both
    /// routes, and the anchored popup's own visibility for
    /// [`SplitButtonMenuStyle::Popup`].
    open: bool,
    /// The trailing segment's own window rect — captures where
    /// [`popup_menu`] places against.
    split_anchor: OverlayAnchor,
    /// Shared with [`style_menu_panel`] — the "Style" dropdown's anchor.
    style_anchor: OverlayAnchor,
    style_open: bool,
    /// Shared with [`size_menu_panel`] — the "Size" dropdown's anchor.
    size_anchor: OverlayAnchor,
    size_open: bool,
}

impl Default for Knobs {
    /// The reference's own `_SplitButtonPlaygroundState` field initializers.
    fn default() -> Self {
        Self {
            nav: NavigatorController::new(),
            style: SplitButtonVariant::Filled,
            size: SplitButtonSize::Sm,
            shape: SplitButtonShape::Round,
            menu_style: SplitButtonMenuStyle::Popup,
            enabled: true,
            label: "Save".to_string(),
            selected: None,
            open: false,
            split_anchor: OverlayAnchor::new(),
            style_anchor: OverlayAnchor::new(),
            style_open: false,
            size_anchor: OverlayAnchor::new(),
            size_open: false,
        }
    }
}

/// Style label for the menu/snippet — the reference's `M3EButtonStyle.name`.
fn style_label(style: SplitButtonVariant) -> &'static str {
    match style {
        SplitButtonVariant::Filled => "filled",
        SplitButtonVariant::Tonal => "tonal",
        SplitButtonVariant::Elevated => "elevated",
        SplitButtonVariant::Outlined => "outlined",
    }
}

/// Size label for the menu/snippet — the reference's `M3EButtonSize.name`.
fn size_label(size: SplitButtonSize) -> &'static str {
    match size {
        SplitButtonSize::Xs => "xs",
        SplitButtonSize::Sm => "sm",
        SplitButtonSize::Md => "md",
        SplitButtonSize::Lg => "lg",
        SplitButtonSize::Xl => "xl",
    }
}

/// Shape label for the segmented control — the reference's
/// `M3EButtonShape.name`.
fn shape_label(shape: SplitButtonShape) -> &'static str {
    match shape {
        SplitButtonShape::Round => "round",
        SplitButtonShape::Square => "square",
    }
}

/// Menu-style label for the segmented control — the reference's
/// `M3ESplitButtonMenuStyle.name`, restricted to this port's two ported
/// rows.
fn menu_style_label(style: SplitButtonMenuStyle) -> &'static str {
    match style {
        SplitButtonMenuStyle::Popup => "popup",
        SplitButtonMenuStyle::BottomSheet => "bottomSheet",
    }
}

/// The two menu items every preview offers — the reference's own
/// `M3ESplitButtonItem` pair (draft/copy).
fn split_items() -> Vec<SplitButtonItem> {
    vec![
        split_button_item("Save draft").value("draft"),
        split_button_item("Save a copy").value("copy"),
    ]
}

/// The same two items as menu nodes, for the bottom-sheet route this page
/// composes by hand — see the [module docs](self) for why
/// [`frust_material::SplitButtonView::sheet_menu`] isn't reachable from an
/// event handler.
fn sheet_nodes() -> Vec<MenuNode> {
    vec![
        menu_selectable("Save draft", "draft").into(),
        menu_selectable("Save a copy", "copy").into(),
    ]
}

/// Report a menu row's requested value into `state.selected` — the one
/// `on_select` callback both menu routes share, mirroring the reference's
/// own single `onSelected` regardless of `menuStyle`.
fn on_select(state: &mut Knobs, selection: MenuSelection) {
    if let Some(value) = selection.value() {
        state.selected = Some(value.to_string());
    }
}

/// The trailing segment's tap handler: toggle the anchored popup directly
/// under [`SplitButtonMenuStyle::Popup`], or push the bottom sheet once per
/// open request under [`SplitButtonMenuStyle::BottomSheet`]. See the [module
/// docs](self) for why the two routes are composed differently.
fn on_open(state: &mut Knobs) {
    match state.menu_style {
        SplitButtonMenuStyle::Popup => state.open = !state.open,
        SplitButtonMenuStyle::BottomSheet => {
            if state.open {
                return;
            }
            state.open = true;
            let nav = state.nav.clone();
            let selected = state.selected.clone();
            show_bottom_sheet(
                &nav,
                move || {
                    bottom_sheet(menu_panel(sheet_nodes(), on_select).selected(selected.clone()))
                },
                |state: &mut Knobs, _result: PopResult| state.open = false,
            );
        }
    }
}

/// The live split button for this pass, every knob applied — shared by
/// [`preview`] and [`split_popup_menu`] (the latter reaches only
/// [`frust_material::SplitButtonView::popup_menu`] off it).
fn split_view(state: &Knobs) -> frust_material::SplitButtonView<Knobs> {
    split_button(state.label.clone(), state.open, |_: &mut Knobs| {}, on_open)
        .leading_icon(|| any(icon(icons::SAVE)))
        .variant(state.style)
        .size(state.size)
        .shape(state.shape)
        .enabled(state.enabled)
        .items(split_items())
        .selected_value(state.selected.clone())
        .menu_anchor(&state.split_anchor)
        .on_select(on_select)
}

/// The "Split button" preview.
fn preview(state: &Knobs) -> AnyView<Knobs> {
    any(split_view(state))
}

/// The trailing trigger's own popup menu — mounted separately at the page's
/// outer [`frust::Stack`], kept mounted (per
/// [`mod@frust_material::split_button`]'s Menu styles section) so closing it
/// plays the host's exit ramp. Only relevant while [`Knobs::menu_style`] is
/// [`SplitButtonMenuStyle::Popup`] — the bottom-sheet route presents through
/// [`Knobs::nav`] instead, with no kept-mounted counterpart of its own.
fn split_popup_menu(state: &Knobs) -> AnyView<Knobs> {
    split_view(state).popup_menu()
}

/// "Appearance" controls: style, size, shape, menu style.
fn appearance_panel(state: &Knobs) -> AnyView<Knobs> {
    control_panel(
        "Appearance",
        vec![
            play_enum_menu_field(
                "Style",
                state.style,
                &SplitButtonVariant::ALL,
                style_label,
                &state.style_anchor,
                state.style_open,
                |state: &mut Knobs, open: bool| state.style_open = open,
            ),
            play_enum_menu_field(
                "Size",
                state.size,
                &SplitButtonSize::ALL,
                size_label,
                &state.size_anchor,
                state.size_open,
                |state: &mut Knobs, open: bool| state.size_open = open,
            ),
            play_enum_segmented(
                "Shape",
                state.shape,
                &SHAPES,
                shape_label,
                |state: &mut Knobs, next: SplitButtonShape| state.shape = next,
            ),
            play_enum_segmented(
                "Menu style",
                state.menu_style,
                &MENU_STYLES,
                menu_style_label,
                |state: &mut Knobs, next: SplitButtonMenuStyle| {
                    // This page's own scoping decision, not a reference
                    // behaviour (the Dart page never shows both routes at
                    // once): switching the route while a menu is showing
                    // would otherwise strand the popup open, so reset
                    // rather than mix the two routes' open state.
                    state.menu_style = next;
                    state.open = false;
                },
            ),
        ],
    )
}

/// "Content" controls: label, enabled.
fn content_panel(state: &Knobs) -> AnyView<Knobs> {
    control_panel(
        "Content",
        vec![
            play_text_field(
                "Label",
                state.label.clone(),
                |state: &mut Knobs, next: String| state.label = next,
            ),
            play_switch("Enabled", state.enabled, |state: &mut Knobs, next: bool| {
                state.enabled = next;
            }),
        ],
    )
}

/// The "Style" menu's popup half — mounted at this page's outer [`Stack`].
fn style_menu_panel(state: &Knobs) -> AnyView<Knobs> {
    play_enum_menu_panel(
        state.style,
        &SplitButtonVariant::ALL,
        style_label,
        &state.style_anchor,
        state.style_open,
        |state: &mut Knobs, open: bool| state.style_open = open,
        |state: &mut Knobs, next: SplitButtonVariant| state.style = next,
    )
}

/// The "Size" menu's popup half — mounted at this page's outer [`Stack`].
fn size_menu_panel(state: &Knobs) -> AnyView<Knobs> {
    play_enum_menu_panel(
        state.size,
        &SplitButtonSize::ALL,
        size_label,
        &state.size_anchor,
        state.size_open,
        |state: &mut Knobs, open: bool| state.size_open = open,
        |state: &mut Knobs, next: SplitButtonSize| state.size = next,
    )
}

/// The paste-ready snippet for the current knob state — the reference's own
/// first `_snippets` entry, in Frust rather than Dart (see the [module
/// docs](self) for why there is no second "Custom M3E menu" snippet).
fn snippet(state: &Knobs) -> PlaySnippet {
    let selected = match &state.selected {
        Some(value) => format!("Some({value:?}.to_string())"),
        None => "None".to_string(),
    };
    let code = format!(
        "split_button({label:?}, {open}, on_press, on_open)\n    \
         .leading_icon(|| any(icon(icons::SAVE)))\n    \
         .variant(SplitButtonVariant::{style:?})\n    .size(SplitButtonSize::{size:?})\n    \
         .shape(SplitButtonShape::{shape:?})\n    .enabled({enabled})\n    .items(vec![\n        \
         split_button_item(\"Save draft\").value(\"draft\"),\n        \
         split_button_item(\"Save a copy\").value(\"copy\"),\n    ])\n    \
         .selected_value({selected})\n    .menu_style(SplitButtonMenuStyle::{menu_style:?})\n    \
         .on_select(on_select);",
        label = state.label,
        open = state.open,
        style = state.style,
        size = state.size,
        shape = state.shape,
        enabled = state.enabled,
        menu_style = state.menu_style,
    );
    play_snippet("Split button", code)
}

/// The playground body: the trigger preview, snippet, controls, and the
/// dropdown/popup panels they anchor — everything that varies with `state`,
/// built without touching the navigator (see the [module docs](self)).
fn content(state: &Knobs) -> AnyView<Knobs> {
    let body = playground_body(
        vec![play_preview_card("Split button", preview(state))],
        vec![snippet(state)],
        vec![appearance_panel(state), content_panel(state)],
    );
    let mut layers = vec![body, style_menu_panel(state), size_menu_panel(state)];
    if state.menu_style == SplitButtonMenuStyle::Popup {
        layers.push(split_popup_menu(state));
    }
    any(Stack(layers))
}

/// This page's knob component — see the [module docs](self).
struct SplitButtonPlayground;

impl Component for SplitButtonPlayground {
    type State = Knobs;

    fn init(&self) -> Knobs {
        Knobs::default()
    }

    fn build(&self, state: &mut Knobs) -> AnyView<Knobs> {
        let snapshot = state.clone();
        any(navigator(&state.nav, move || content(&snapshot)))
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(SplitButtonPlayground))
}

#[cfg(test)]
mod tests {
    use frust_material::{
        SplitButtonMenuStyle, SplitButtonShape, SplitButtonSize, SplitButtonVariant,
    };

    use super::{Knobs, content, on_open, snippet};

    #[test]
    fn the_page_builds_across_every_style() {
        let mut knobs = Knobs::default();
        for style in SplitButtonVariant::ALL {
            knobs.style = style;
            let _view = content(&knobs);
        }
    }

    #[test]
    fn the_page_builds_across_every_size() {
        let mut knobs = Knobs::default();
        for size in SplitButtonSize::ALL {
            knobs.size = size;
            let _view = content(&knobs);
        }
    }

    #[test]
    fn the_page_builds_across_every_shape() {
        let mut knobs = Knobs::default();
        for shape in [SplitButtonShape::Round, SplitButtonShape::Square] {
            knobs.shape = shape;
            let _view = content(&knobs);
        }
    }

    #[test]
    fn the_page_builds_in_the_bottom_sheet_menu_style_with_the_popup_layer_omitted() {
        let knobs = Knobs {
            menu_style: SplitButtonMenuStyle::BottomSheet,
            ..Knobs::default()
        };
        let _view = content(&knobs);
    }

    #[test]
    fn the_page_builds_with_the_popup_open_disabled_and_a_confirmed_selection() {
        let knobs = Knobs {
            open: true,
            enabled: false,
            selected: Some("copy".to_string()),
            label: String::new(),
            ..Knobs::default()
        };
        let _view = content(&knobs);
    }

    #[test]
    fn the_page_builds_with_both_style_and_size_dropdown_menus_open() {
        let knobs = Knobs {
            style_open: true,
            size_open: true,
            ..Knobs::default()
        };
        let _view = content(&knobs);
    }

    #[test]
    fn a_popup_tap_toggles_the_open_flag() {
        let mut knobs = Knobs::default();
        assert!(!knobs.open);
        on_open(&mut knobs);
        assert!(knobs.open);
        on_open(&mut knobs);
        assert!(!knobs.open);
    }

    #[test]
    fn the_snippet_reflects_the_confirmed_selection_and_menu_style() {
        let knobs = Knobs {
            selected: Some("draft".to_string()),
            ..Knobs::default()
        };
        let snippet = snippet(&knobs);
        assert!(snippet.code.contains("\"draft\""));
        assert!(snippet.code.contains("SplitButtonMenuStyle::Popup"));
    }
}
