//! Icon buttons: the reference's `IconButtonsPlayground`.
//!
//! Mirrors every `frust_material::icon_button` prop the reference's
//! `M3EIconButton` exercises here: variant, size, shape, width, enabled,
//! selected (a toggle, since this preview always passes a `selected_icon` the
//! way the reference always passes `selectedIcon`), and a numeric badge.
//! **Two props the reference exercises have no port to mirror, per
//! [`mod@frust_material::icon_button`]'s own "Not ported in v1" section**:
//! `tooltip` (no tooltip host in this catalog — [`IconButtonView::semantic_label`]
//! is used instead, the accessible-name analog, not a hover popup) and the
//! reference's "Gradient fill" preview (`M3EIconButtonDecoration`'s
//! gradient seam is not ported for this family at all — [`mod@crate::pages::playground::do_::buttons`]'s
//! own gradient preview is the button family's equivalent, which *does* have
//! the seam). Both omissions are the port's, not this page's.
//!
//! Knobs live in a page-local [`Knobs`], owned by the nested
//! [`IconButtonsPlayground`] `Component` (never [`AppState`]) per the page
//! contract in [`crate::pages::playground`]. `Variant`/`Size`/`Width` use the
//! two-piece [`play_enum_menu_field`]/[`play_enum_menu_panel`] control (each
//! needs its own [`OverlayAnchor`], all three held in `Knobs`, with the
//! panels mounted at this page's own outer [`Stack`]); `Shape` fits a plain
//! [`play_enum_segmented`] since it is a 2-way choice.

use frust::{AnyView, Component, View, any, component, icon, stack};
use frust_material::{
    BadgeValue, IconButtonShape, IconButtonSize, IconButtonVariant, IconButtonWidth, OverlayAnchor,
    icon_button, icons,
};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    PlaySnippet, control_panel, play_enum_menu_field, play_enum_menu_panel, play_enum_segmented,
    play_preview_card, play_snippet, play_switch, playground_body,
};

/// Every [`IconButtonVariant`] the "Variant" menu offers — the crate's own
/// declaration order, which happens to match the reference's `.values` order
/// (`standard`/`filled`/`tonal`/`outlined`).
const VARIANTS: [IconButtonVariant; 4] = [
    IconButtonVariant::Standard,
    IconButtonVariant::Filled,
    IconButtonVariant::Tonal,
    IconButtonVariant::Outlined,
];

/// Every [`IconButtonSize`] the "Size" menu offers.
const SIZES: [IconButtonSize; 5] = [
    IconButtonSize::Xs,
    IconButtonSize::Sm,
    IconButtonSize::Md,
    IconButtonSize::Lg,
    IconButtonSize::Xl,
];

/// Every [`IconButtonShape`] the "Shape" segmented control offers.
const SHAPES: [IconButtonShape; 2] = [IconButtonShape::Round, IconButtonShape::Square];

/// Every [`IconButtonWidth`] the "Width" menu offers.
const WIDTHS: [IconButtonWidth; 3] = [
    IconButtonWidth::Standard,
    IconButtonWidth::Narrow,
    IconButtonWidth::Wide,
];

/// This page's own knob state — held by [`IconButtonsPlayground`], never
/// [`AppState`] (the page contract in [`crate::pages::playground`]).
struct Knobs {
    variant: IconButtonVariant,
    size: IconButtonSize,
    shape: IconButtonShape,
    width: IconButtonWidth,
    enabled: bool,
    selected: bool,
    badge: bool,
    /// Shared with [`variant_menu_panel`].
    variant_anchor: OverlayAnchor,
    variant_open: bool,
    /// Shared with [`size_menu_panel`].
    size_anchor: OverlayAnchor,
    size_open: bool,
    /// Shared with [`width_menu_panel`].
    width_anchor: OverlayAnchor,
    width_open: bool,
}

impl Default for Knobs {
    /// The reference's own `_IconButtonsPlaygroundState` field initializers.
    fn default() -> Self {
        Self {
            variant: IconButtonVariant::Standard,
            size: IconButtonSize::Sm,
            shape: IconButtonShape::Round,
            width: IconButtonWidth::Standard,
            enabled: true,
            selected: false,
            badge: false,
            variant_anchor: OverlayAnchor::new(),
            variant_open: false,
            size_anchor: OverlayAnchor::new(),
            size_open: false,
            width_anchor: OverlayAnchor::new(),
            width_open: false,
        }
    }
}

/// Variant label — the reference's `M3EIconButtonVariant.name`.
fn variant_label(variant: IconButtonVariant) -> &'static str {
    match variant {
        IconButtonVariant::Standard => "standard",
        IconButtonVariant::Filled => "filled",
        IconButtonVariant::Tonal => "tonal",
        IconButtonVariant::Outlined => "outlined",
    }
}

/// Size label — the reference's `M3EIconButtonSize.name`.
fn size_label(size: IconButtonSize) -> &'static str {
    match size {
        IconButtonSize::Xs => "xs",
        IconButtonSize::Sm => "sm",
        IconButtonSize::Md => "md",
        IconButtonSize::Lg => "lg",
        IconButtonSize::Xl => "xl",
    }
}

/// Shape label — the reference's `M3EIconButtonShapeVariant.name`.
fn shape_label(shape: IconButtonShape) -> &'static str {
    match shape {
        IconButtonShape::Round => "round",
        IconButtonShape::Square => "square",
    }
}

/// Width label — this port's own [`IconButtonWidth::Standard`] naming (the
/// reference's `defaultWidth`, renamed to avoid colliding with Rust's
/// `Default` trait; see [`mod@frust_material::icon_button`]'s module docs).
fn width_label(width: IconButtonWidth) -> &'static str {
    match width {
        IconButtonWidth::Standard => "standard",
        IconButtonWidth::Narrow => "narrow",
        IconButtonWidth::Wide => "wide",
    }
}

/// The "Icon button" preview: always a toggle (a `selected_icon` is always
/// supplied, mirroring the reference always passing `selectedIcon`).
fn preview(state: &Knobs) -> impl View<Knobs> {
    let mut view = icon_button(icon(icons::FAVORITE), |_: &mut Knobs| {})
        .selected_icon(icon(icons::FAVORITE))
        .variant(state.variant)
        .size(state.size)
        .shape(state.shape)
        .width(state.width)
        .selected(state.selected)
        .enabled(state.enabled)
        .semantic_label("Favorite");
    if state.badge {
        view = view.badge(BadgeValue::Count(3));
    }
    view
}

/// The paste-ready snippet for the current knob state — the reference's
/// `_snippets`, in Frust rather than Dart.
fn snippet(state: &Knobs) -> PlaySnippet {
    let badge = if state.badge {
        ".badge(BadgeValue::Count(3))\n    "
    } else {
        ""
    };
    let code = format!(
        "icon_button(any(icon(icons::FAVORITE)), on_press)\n    .selected_icon(any(icon(icons::FAVORITE)))\n    .variant(IconButtonVariant::{variant:?})\n    .size(IconButtonSize::{size:?})\n    .shape(IconButtonShape::{shape:?})\n    .width(IconButtonWidth::{width:?})\n    .selected({selected})\n    .enabled({enabled})\n    {badge}.semantic_label(\"Favorite\");",
        variant = state.variant,
        size = state.size,
        shape = state.shape,
        width = state.width,
        selected = state.selected,
        enabled = state.enabled,
        badge = badge,
    );
    play_snippet("Icon button", code)
}

/// "Appearance" controls: variant, size, shape, width.
fn appearance_panel(state: &Knobs) -> AnyView<Knobs> {
    control_panel(
        "Appearance",
        vec![
            play_enum_menu_field(
                "Variant",
                state.variant,
                &VARIANTS,
                variant_label,
                &state.variant_anchor,
                state.variant_open,
                |state: &mut Knobs, open: bool| state.variant_open = open,
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
            play_enum_segmented(
                "Shape",
                state.shape,
                &SHAPES,
                shape_label,
                |state: &mut Knobs, next: IconButtonShape| state.shape = next,
            ),
            play_enum_menu_field(
                "Width",
                state.width,
                &WIDTHS,
                width_label,
                &state.width_anchor,
                state.width_open,
                |state: &mut Knobs, open: bool| state.width_open = open,
            ),
        ],
    )
}

/// "State" controls: enabled, selected, badge.
fn state_panel(state: &Knobs) -> AnyView<Knobs> {
    control_panel(
        "State",
        vec![
            play_switch("Enabled", state.enabled, |state: &mut Knobs, next| {
                state.enabled = next;
            }),
            play_switch("Selected", state.selected, |state: &mut Knobs, next| {
                state.selected = next;
            }),
            play_switch("Badge", state.badge, |state: &mut Knobs, next| {
                state.badge = next;
            }),
        ],
    )
}

/// The "Variant" menu's popup half — mounted at this page's outer [`Stack`].
fn variant_menu_panel(state: &Knobs) -> impl View<Knobs> {
    play_enum_menu_panel(
        state.variant,
        &VARIANTS,
        variant_label,
        &state.variant_anchor,
        state.variant_open,
        |state: &mut Knobs, open: bool| state.variant_open = open,
        |state: &mut Knobs, next: IconButtonVariant| state.variant = next,
    )
}

/// The "Size" menu's popup half — mounted at this page's outer [`Stack`].
fn size_menu_panel(state: &Knobs) -> impl View<Knobs> {
    play_enum_menu_panel(
        state.size,
        &SIZES,
        size_label,
        &state.size_anchor,
        state.size_open,
        |state: &mut Knobs, open: bool| state.size_open = open,
        |state: &mut Knobs, next: IconButtonSize| state.size = next,
    )
}

/// The "Width" menu's popup half — mounted at this page's outer [`Stack`].
fn width_menu_panel(state: &Knobs) -> impl View<Knobs> {
    play_enum_menu_panel(
        state.width,
        &WIDTHS,
        width_label,
        &state.width_anchor,
        state.width_open,
        |state: &mut Knobs, open: bool| state.width_open = open,
        |state: &mut Knobs, next: IconButtonWidth| state.width = next,
    )
}

/// The page body: the playground content plus the three dropdown panels it
/// anchors, stacked so all can paint above the scrollable content.
fn body(state: &Knobs) -> impl View<Knobs> {
    let content = playground_body(
        vec![play_preview_card("Icon button", preview(state))],
        vec![snippet(state)],
        vec![appearance_panel(state), state_panel(state)],
    );
    stack()
        .child(content)
        .child(variant_menu_panel(state))
        .child(size_menu_panel(state))
        .child(width_menu_panel(state))
}

/// This page's knob component — see the [module docs](self).
struct IconButtonsPlayground;

impl Component for IconButtonsPlayground {
    type State = Knobs;

    fn init(&self) -> Self::State {
        Knobs::default()
    }

    fn build(&self, state: &mut Self::State) -> impl View<Self::State> {
        body(state)
    }
}

/// See the page contract in [`crate::pages::playground`].
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(IconButtonsPlayground))
}

#[cfg(test)]
mod tests {
    use super::{Knobs, SHAPES, SIZES, VARIANTS, WIDTHS, body, snippet};

    #[test]
    fn the_page_builds_across_every_variant() {
        let mut knobs = Knobs::default();
        for variant in VARIANTS {
            knobs.variant = variant;
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
    fn the_page_builds_across_every_shape() {
        let mut knobs = Knobs::default();
        for shape in SHAPES {
            knobs.shape = shape;
            let _view = body(&knobs);
        }
    }

    #[test]
    fn the_page_builds_across_every_width() {
        let mut knobs = Knobs::default();
        for width in WIDTHS {
            knobs.width = width;
            let _view = body(&knobs);
        }
    }

    #[test]
    fn the_page_builds_disabled_selected_and_badged() {
        let knobs = Knobs {
            enabled: false,
            selected: true,
            badge: true,
            ..Knobs::default()
        };
        let _view = body(&knobs);
    }

    #[test]
    fn the_page_builds_with_every_menu_open() {
        let knobs = Knobs {
            variant_open: true,
            size_open: true,
            width_open: true,
            ..Knobs::default()
        };
        let _view = body(&knobs);
    }

    #[test]
    fn the_snippet_carries_the_badge_call_only_when_badged() {
        let mut knobs = Knobs::default();
        assert!(!snippet(&knobs).code.contains("BadgeValue"));
        knobs.badge = true;
        assert!(snippet(&knobs).code.contains("BadgeValue::Count(3)"));
    }
}
