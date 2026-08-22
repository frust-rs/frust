//! FABs: the reference's `FabsPlayground`.
//!
//! Mirrors every `frust_material::fab`/`extended_fab` prop the reference's
//! `M3EFab`/`M3EExtendedFab` exercises: size (`fab` only), color (both),
//! the extended label and its collapse/expand toggle. **The reference's
//! "Gradient fill" preview has no port to mirror**: unlike the button and
//! icon-button families, `frust_material::fab`/`extended_fab` carry no
//! decoration/gradient seam at all (there is no `FabDecoration` type and
//! `fab.rs` names no follow-up for one) — omitted here rather than faked.
//! `tooltip` likewise has no analogue ([`crate::pages::playground::do_::icon_buttons`]'s
//! own module docs record the same gap for that family); [`FabView::label`]
//! is used instead as the accessible-name substitute.
//!
//! Knobs live in a page-local [`Knobs`], owned by the nested
//! [`FabsPlayground`] `Component` (never [`AppState`]) per the page contract
//! in [`crate::pages::playground`]. Every control here is a plain
//! [`play_enum_segmented`]/[`play_switch`]/[`play_text_field`] — no dropdown
//! menu is needed, so unlike `buttons.rs`/`icon_buttons.rs` this page mounts
//! no outer [`frust::Stack`].

use frust::{AnyView, Component, any, component, icon};
use frust_material::{FabColor, FabSize, extended_fab, fab, icons};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    PlaySnippet, control_panel, play_enum_segmented, play_preview_card, play_snippet, play_switch,
    play_text_field, playground_body,
};

/// Every [`FabSize`] the "Size" segmented control offers.
const SIZES: [FabSize; 3] = [FabSize::Small, FabSize::Medium, FabSize::Large];

/// Every [`FabColor`] the "Color" segmented control offers.
const COLORS: [FabColor; 4] = [
    FabColor::Primary,
    FabColor::Secondary,
    FabColor::Tertiary,
    FabColor::Surface,
];

/// This page's own knob state — held by [`FabsPlayground`], never
/// [`AppState`] (the page contract in [`crate::pages::playground`]).
struct Knobs {
    size: FabSize,
    color: FabColor,
    extended: bool,
    label: String,
}

impl Default for Knobs {
    /// The reference's own `_FabsPlaygroundState` field initializers.
    fn default() -> Self {
        Self {
            size: FabSize::Medium,
            color: FabColor::Primary,
            extended: true,
            label: "Compose".to_string(),
        }
    }
}

/// Size label — the reference's `M3EFabSize.name`.
fn size_label(size: FabSize) -> &'static str {
    match size {
        FabSize::Small => "small",
        FabSize::Medium => "medium",
        FabSize::Large => "large",
    }
}

/// Color label — the reference's `M3EFabColor.name`.
fn color_label(color: FabColor) -> &'static str {
    match color {
        FabColor::Primary => "primary",
        FabColor::Secondary => "secondary",
        FabColor::Tertiary => "tertiary",
        FabColor::Surface => "surface",
    }
}

/// The "FAB" preview.
fn fab_preview(state: &Knobs) -> AnyView<Knobs> {
    any(fab(any(icon(icons::ADD)), |_: &mut Knobs| {})
        .size(state.size)
        .color(state.color)
        .label("Add"))
}

/// The "Extended FAB" preview.
fn extended_preview(state: &Knobs) -> AnyView<Knobs> {
    any(extended_fab(state.label.clone(), |_: &mut Knobs| {})
        .icon(any(icon(icons::EDIT)))
        .color(state.color)
        .extended(state.extended))
}

/// The two paste-ready snippets for the current knob state — the reference's
/// `_snippets`, in Frust rather than Dart.
fn snippets(state: &Knobs) -> Vec<PlaySnippet> {
    let fab_code = format!(
        "fab(any(icon(icons::ADD)), on_press)\n    .size(FabSize::{size:?})\n    .color(FabColor::{color:?})\n    .label(\"Add\");",
        size = state.size,
        color = state.color,
    );
    let extended_code = format!(
        "extended_fab(\"{label}\", on_press)\n    .icon(any(icon(icons::EDIT)))\n    .color(FabColor::{color:?})\n    .extended({extended});",
        label = state.label,
        color = state.color,
        extended = state.extended,
    );
    vec![
        play_snippet("FAB", fab_code),
        play_snippet("Extended FAB", extended_code),
    ]
}

/// "FAB" controls: size, color.
fn fab_panel(state: &Knobs) -> AnyView<Knobs> {
    control_panel(
        "FAB",
        vec![
            play_enum_segmented(
                "Size",
                state.size,
                &SIZES,
                size_label,
                |state: &mut Knobs, next: FabSize| state.size = next,
            ),
            play_enum_segmented(
                "Color",
                state.color,
                &COLORS,
                color_label,
                |state: &mut Knobs, next: FabColor| state.color = next,
            ),
        ],
    )
}

/// "Extended FAB" controls: extended toggle, label.
fn extended_panel(state: &Knobs) -> AnyView<Knobs> {
    control_panel(
        "Extended FAB",
        vec![
            play_switch("Extended", state.extended, |state: &mut Knobs, next| {
                state.extended = next;
            }),
            play_text_field("Label", state.label.clone(), |state: &mut Knobs, next| {
                state.label = next;
            }),
        ],
    )
}

/// The page body: preview cards, snippets, then controls.
fn body(state: &Knobs) -> AnyView<Knobs> {
    playground_body(
        vec![
            play_preview_card("FAB", fab_preview(state)),
            play_preview_card("Extended FAB", extended_preview(state)),
        ],
        snippets(state),
        vec![fab_panel(state), extended_panel(state)],
    )
}

/// This page's knob component — see the [module docs](self).
struct FabsPlayground;

impl Component for FabsPlayground {
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
    any(component(FabsPlayground))
}

#[cfg(test)]
mod tests {
    use super::{COLORS, Knobs, SIZES, body};

    #[test]
    fn the_page_builds_across_every_size() {
        let mut knobs = Knobs::default();
        for size in SIZES {
            knobs.size = size;
            let _view = body(&knobs);
        }
    }

    #[test]
    fn the_page_builds_across_every_color() {
        let mut knobs = Knobs::default();
        for color in COLORS {
            knobs.color = color;
            let _view = body(&knobs);
        }
    }

    #[test]
    fn the_page_builds_collapsed_and_with_an_empty_label() {
        let knobs = Knobs {
            extended: false,
            label: String::new(),
            ..Knobs::default()
        };
        let _view = body(&knobs);
    }
}
