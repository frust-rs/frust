//! Chips: the reference's `ChipsPlayground`.
//!
//! Two previews (the currently-picked type, and all four side by side), one
//! snippet, and one control panel: `Type`, `Label`, `Leading icon`,
//! `Elevated`, `Selected`.
//!
//! # One `M3EChip` + `M3EChipType`, four Frust constructors
//!
//! The reference drives every preview off a single `M3EChip(type: ...)`
//! widget; `frust_material` splits chips into four dedicated constructors
//! ([`assist_chip`], [`filter_chip`], [`input_chip`], [`suggestion_chip`] —
//! see `chips.rs`'s own module docs, *Four types, one color table*). This
//! page mirrors that split with a local [`ChipKind`] enum and a
//! [`build_chip`] dispatcher, rather than inventing a unified type this
//! catalog has no seam for.
//!
//! # Type picker: `play_enum_menu_field`/`play_enum_menu_panel`
//!
//! The reference's `PlayEnumMenu` is this kit's two-piece
//! [`crate::widgets::playground::play_enum_menu_field`]/
//! [`play_enum_menu_panel`] pair — the field mounts in the control panel's
//! row list, the panel mounts at this page's own outer [`frust::Stack`],
//! both anchored through [`ChipsPlaygroundState::menu_anchor`], exactly the
//! pattern the kit's own module docs prescribe.
//!
//! # Descoped: `Enabled`
//!
//! The reference's `Enabled` switch gates `onPressed`/`onDeleted` (and dims
//! `M3EChip`'s own disabled colors). None of the four Frust chip
//! constructors carry a disabled state or an optional press handler —
//! `on_press` is a required argument on every one of them — so there is no
//! prop for the control to drive; it is omitted here rather than faking a
//! functional-only no-op.
//!
//! # Divergence: `leading` is a text glyph, not a vector icon
//!
//! `AssistChipView::leading`/`SuggestionChipView::leading`/
//! `InputChipView::leading` each take `impl Into<String>` — a second nested
//! text run, not an [`frust::IconSource`] (`chips.rs`'s own module docs).
//! [`LEADING_EDIT_GLYPH`]/[`LEADING_TAG_GLYPH`] stand in for the reference's
//! `M3EIcons.edit`/`M3EIcons.tag`. `FilterChipView` carries no `.leading()`
//! builder at all — the "Selected type" preview's leading toggle has no
//! effect while `Filter` is picked, which the snippet reflects.
//!
//! # Divergence: `Wrap` becomes a plain `Row`
//!
//! This workspace has no flow/wrap layout primitive; the "All types"
//! preview lays its four chips out in a single non-wrapping `Row` (the same
//! `spaced_row` idiom `crate::pages::theme_config_page` uses for its seed
//! swatches) rather than the reference's `Wrap(spacing: 8, runSpacing: 8)`.

use frust::{
    AnyView, Component, CrossAxisAlignment, Get, Row, RwSignal, Set, SizedBox, Stack, any,
    component,
};
use frust_material::{OverlayAnchor, assist_chip, filter_chip, input_chip, suggestion_chip};

use crate::AppState;
use crate::catalog::DemoEntry;
use crate::widgets::playground::{
    control_panel, play_enum_menu_field, play_enum_menu_panel, play_preview_card, play_snippet,
    play_switch, play_text_field, playground_body,
};

/// Text-glyph stand-in for the reference's `M3EIcons.edit` leading icon —
/// see this file's module docs' `leading` divergence.
const LEADING_EDIT_GLYPH: &str = "\u{270E}";
/// Text-glyph stand-in for the reference's `M3EIcons.tag` leading icon, used
/// by the "All types" preview.
const LEADING_TAG_GLYPH: &str = "#";

/// The reference's `M3EChipType`, narrowed to this catalog's four Frust
/// constructors.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ChipKind {
    Assist,
    Filter,
    Input,
    Suggestion,
}

impl ChipKind {
    const ALL: [ChipKind; 4] = [
        ChipKind::Assist,
        ChipKind::Filter,
        ChipKind::Input,
        ChipKind::Suggestion,
    ];

    /// The reference's `type.name` — also this file's `frust_material`
    /// constructor-name stem.
    fn label(self) -> &'static str {
        match self {
            ChipKind::Assist => "assist",
            ChipKind::Filter => "filter",
            ChipKind::Input => "input",
            ChipKind::Suggestion => "suggestion",
        }
    }
}

/// See the page contract in [`crate::pages::playground`]. No page-level
/// heading of its own — see [`super::checkbox::page`]'s doc for why `entry`
/// goes unused.
pub fn page(_entry: DemoEntry) -> AnyView<AppState> {
    any(component(ChipsPlayground))
}

/// Stateless configuration; all state lives in [`ChipsPlaygroundState`].
struct ChipsPlayground;

/// This page's own knobs — nested per the page contract, never on
/// [`AppState`]. [`menu_anchor`](Self::menu_anchor) is the one non-signal
/// field: it is shared, not read/written through `Get`/`Set`, per
/// [`OverlayAnchor`]'s own contract.
struct ChipsPlaygroundState {
    kind: RwSignal<ChipKind>,
    selected: RwSignal<bool>,
    elevated: RwSignal<bool>,
    show_leading: RwSignal<bool>,
    label: RwSignal<String>,
    menu_open: RwSignal<bool>,
    /// Shared between [`play_enum_menu_field`] and [`play_enum_menu_panel`] —
    /// see this file's module docs' Type picker section.
    menu_anchor: OverlayAnchor,
}

impl Component for ChipsPlayground {
    type State = ChipsPlaygroundState;

    fn init(&self) -> ChipsPlaygroundState {
        ChipsPlaygroundState {
            kind: RwSignal::new(ChipKind::Assist),
            selected: RwSignal::new(false),
            elevated: RwSignal::new(false),
            show_leading: RwSignal::new(true),
            label: RwSignal::new("Chip".to_string()),
            menu_open: RwSignal::new(false),
            menu_anchor: OverlayAnchor::new(),
        }
    }

    fn build(&self, state: &mut ChipsPlaygroundState) -> AnyView<ChipsPlaygroundState> {
        let kind = state.kind.get();
        let selected = state.selected.get();
        let elevated = state.elevated.get();
        let show_leading = state.show_leading.get();
        let label = state.label.get();
        let menu_open = state.menu_open.get();

        let leading = show_leading.then_some(LEADING_EDIT_GLYPH);
        let selected_preview = play_preview_card(
            "Selected type",
            build_chip(
                kind,
                &label,
                selected,
                elevated,
                leading,
                kind == ChipKind::Input,
            ),
        );
        let all_types_preview =
            play_preview_card("All types", all_types_row(selected, elevated, show_leading));

        let snippet = play_snippet(
            "Selected type",
            chip_snippet(kind, &label, selected, elevated, show_leading),
        );

        let type_field = play_enum_menu_field::<ChipsPlaygroundState, ChipKind>(
            "Type",
            kind,
            &ChipKind::ALL,
            ChipKind::label,
            &state.menu_anchor,
            menu_open,
            |s: &mut ChipsPlaygroundState, open: bool| s.menu_open.set(open),
        );

        let body = playground_body(
            vec![selected_preview, all_types_preview],
            vec![snippet],
            vec![control_panel(
                "Appearance",
                vec![
                    type_field,
                    play_text_field(
                        "Label",
                        label.clone(),
                        |s: &mut ChipsPlaygroundState, v: String| s.label.set(v),
                    ),
                    play_switch(
                        "Leading icon",
                        show_leading,
                        |s: &mut ChipsPlaygroundState, v: bool| s.show_leading.set(v),
                    ),
                    play_switch(
                        "Elevated",
                        elevated,
                        |s: &mut ChipsPlaygroundState, v: bool| s.elevated.set(v),
                    ),
                    play_switch(
                        "Selected",
                        selected,
                        |s: &mut ChipsPlaygroundState, v: bool| s.selected.set(v),
                    ),
                ],
            )],
        );

        let panel = play_enum_menu_panel::<ChipsPlaygroundState, ChipKind>(
            kind,
            &ChipKind::ALL,
            ChipKind::label,
            &state.menu_anchor,
            menu_open,
            |s: &mut ChipsPlaygroundState, open: bool| s.menu_open.set(open),
            |s: &mut ChipsPlaygroundState, next: ChipKind| s.kind.set(next),
        );

        any(Stack(vec![body, panel]))
    }
}

/// Build one chip of `kind` — the one dispatch point every preview routes
/// through. `leading` is ignored for [`ChipKind::Filter`]: `FilterChipView`
/// carries no `.leading()` builder (see this file's module docs).
fn build_chip(
    kind: ChipKind,
    label: &str,
    selected: bool,
    elevated: bool,
    leading: Option<&str>,
    deletable: bool,
) -> AnyView<ChipsPlaygroundState> {
    match kind {
        ChipKind::Assist => {
            let mut view = assist_chip(label.to_string(), |_: &mut ChipsPlaygroundState| {})
                .elevated(elevated);
            if let Some(glyph) = leading {
                view = view.leading(glyph);
            }
            any(view)
        }
        ChipKind::Filter => any(filter_chip(
            label.to_string(),
            selected,
            |s: &mut ChipsPlaygroundState, next: bool| s.selected.set(next),
        )
        .elevated(elevated)),
        ChipKind::Suggestion => {
            let mut view = suggestion_chip(label.to_string(), |_: &mut ChipsPlaygroundState| {})
                .elevated(elevated);
            if let Some(glyph) = leading {
                view = view.leading(glyph);
            }
            any(view)
        }
        ChipKind::Input => {
            let mut view =
                input_chip(label.to_string(), |_: &mut ChipsPlaygroundState| {}).elevated(elevated);
            if let Some(glyph) = leading {
                view = view.leading(glyph);
            }
            if deletable {
                view = view.on_deleted(|_: &mut ChipsPlaygroundState| {});
            }
            any(view)
        }
    }
}

/// All four types side by side, `Filter` selected only when the app-level
/// `selected` knob is set — the reference's `for (type in M3EChipType.values)`
/// preview, laid out via [`spaced_row`] (see this file's module docs' `Wrap`
/// divergence).
fn all_types_row(
    selected: bool,
    elevated: bool,
    show_leading: bool,
) -> AnyView<ChipsPlaygroundState> {
    let leading = show_leading.then_some(LEADING_TAG_GLYPH);
    let chips: Vec<AnyView<ChipsPlaygroundState>> = ChipKind::ALL
        .into_iter()
        .map(|kind| {
            let is_selected = kind == ChipKind::Filter && selected;
            build_chip(
                kind,
                kind.label(),
                is_selected,
                elevated,
                leading,
                kind == ChipKind::Input,
            )
        })
        .collect();
    spaced_row(chips, 8.0)
}

/// Lay `items` out horizontally with `gap`px between each pair — mirrors
/// `crate::pages::theme_config_page`'s identical helper.
fn spaced_row<State: 'static>(items: Vec<AnyView<State>>, gap: f64) -> AnyView<State> {
    let mut children: Vec<AnyView<State>> = Vec::with_capacity(items.len() * 2);
    for item in items {
        if !children.is_empty() {
            children.push(any(SizedBox::<State>(Some(gap), None)));
        }
        children.push(item);
    }
    any(Row(children).cross_axis(CrossAxisAlignment::Center))
}

/// The paste-ready Frust equivalent of the "Selected type" preview.
fn chip_snippet(
    kind: ChipKind,
    label: &str,
    selected: bool,
    elevated: bool,
    show_leading: bool,
) -> String {
    let leading_line = if show_leading && kind != ChipKind::Filter {
        format!("\n    .leading({LEADING_EDIT_GLYPH:?})")
    } else {
        String::new()
    };
    let deleted_line = if kind == ChipKind::Input {
        "\n    .on_deleted(|state| {})".to_string()
    } else {
        String::new()
    };

    if kind == ChipKind::Filter {
        format!(
            "frust_material::filter_chip(\n    {label:?},\n    {selected},\n    |state, next| state.selected = next,\n)\n.elevated({elevated}){leading_line}{deleted_line};"
        )
    } else {
        let ctor = match kind {
            ChipKind::Assist => "assist_chip",
            ChipKind::Suggestion => "suggestion_chip",
            ChipKind::Input => "input_chip",
            ChipKind::Filter => unreachable!("handled above"),
        };
        format!(
            "frust_material::{ctor}(\n    {label:?},\n    |_state, _| {{}},\n)\n.elevated({elevated}){leading_line}{deleted_line};"
        )
    }
}

#[cfg(test)]
mod tests {
    use frust::Component;

    use super::{ChipKind, ChipsPlayground, ChipsPlaygroundState};

    fn state() -> ChipsPlaygroundState {
        ChipsPlayground.init()
    }

    /// Every reachable `(kind, selected, elevated, show_leading)` combination
    /// still builds a page, including the type picker's field+panel pair
    /// sharing one [`frust_material::OverlayAnchor`].
    #[test]
    fn the_page_builds_across_every_knob_state() {
        use frust::Set;

        let mut s = state();
        for kind in ChipKind::ALL {
            for selected in [false, true] {
                for elevated in [false, true] {
                    for show_leading in [false, true] {
                        for menu_open in [false, true] {
                            s.kind.set(kind);
                            s.selected.set(selected);
                            s.elevated.set(elevated);
                            s.show_leading.set(show_leading);
                            s.menu_open.set(menu_open);
                            let _view = ChipsPlayground.build(&mut s);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_page_fn_builds_from_its_catalog_entry() {
        let entry = crate::catalog::find_by_id("chips").expect("catalog entry exists");
        let _view = super::page(entry);
    }
}
