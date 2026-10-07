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
//! # Divergence: `Wrap` becomes a plain `Column` (G10)
//!
//! This workspace has no flow/wrap layout primitive and no horizontal-scroll
//! primitive either, so the "All types" preview cannot lay its four chips out
//! in a single row without risking the G10 overflow class (a `Row` of
//! all-inflexible fixed-width children reports a clamped size but paints its
//! children at their full natural extents —
//! `crate::pages::playground::find::progress`'s module docs). A chip's
//! natural width is label/knob-dependent (`show_leading`, and `Input`'s
//! always-on delete icon), so no compile-time chunk count is derivable the
//! way `crate::pages::playground::view::shapes`'s fixed-tile catalog can
//! derive one (its `CATALOG_COLUMNS`) — measured at a phone-ish card's
//! ~296px inner width, the four chips' combined natural width already
//! reaches ~329px with every chip unselected/non-elevated (the row's
//! cheapest case). [`spaced_column`]
//! (one chip per row, vertically) is the only reflow that is safe regardless
//! of knob state, so the "All types" preview uses that instead of the
//! `spaced_row` idiom `crate::pages::theme_config_page` uses for its
//! fixed-diameter seed swatches, rather than the reference's
//! `Wrap(spacing: 8, runSpacing: 8)`.

use frust::{
    AnyView, Column, Component, Get, RwSignal, Set, SizedBox, View, any, component, stack,
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

    fn build(&self, state: &mut ChipsPlaygroundState) -> impl View<ChipsPlaygroundState> {
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

        any(stack().child(body).child(panel))
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

/// All four types, one per row, `Filter` selected only when the app-level
/// `selected` knob is set — the reference's `for (type in M3EChipType.values)`
/// preview, laid out via [`spaced_column`] (see this file's module docs'
/// `Wrap` divergence, G10).
fn all_types_row(
    selected: bool,
    elevated: bool,
    show_leading: bool,
) -> impl View<ChipsPlaygroundState> {
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
    spaced_column(chips, 8.0)
}

/// Lay `items` out vertically with `gap`px between each pair, one per row —
/// the reflow-safe alternative to a `spaced_row` of variable-width children
/// (see this file's module docs' `Wrap` divergence, G10). Left-aligned
/// (`CrossAxisAlignment::Start`, `Column`'s default): a chip's own natural
/// width, never a fixed one, so there is nothing to stretch or center
/// against.
fn spaced_column<State: 'static>(items: Vec<AnyView<State>>, gap: f64) -> impl View<State> {
    let mut children: Vec<AnyView<State>> = Vec::with_capacity(items.len() * 2);
    for item in items {
        if !children.is_empty() {
            children.push(any(SizedBox::<State>(None, Some(gap))));
        }
        children.push(item);
    }
    Column(children)
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

    /// G10 regression: [`super::all_types_row`] must never paint past the
    /// "All types" card's real inner width at a phone-ish device width —
    /// the same paint-extent idiom
    /// `crate::pages::playground::find::progress`'s own G10 test uses.
    ///
    /// Reverting the fix (back to a `spaced_row` of the four chips) fails
    /// this test: even in the row's cheapest case (every chip
    /// unselected/non-elevated, `Input`'s delete icon always on), the four
    /// chips' combined natural width (~329px) paints straight through a
    /// phone-ish card's ~296px inner width.
    #[test]
    fn all_types_row_never_paints_past_the_cards_inner_width() {
        use std::any::Any;

        use frust::authoring::Point;
        use frust::authoring::text::TextContext;
        use frust::authoring::{
            BoxConstraints, BuildCtx, LayoutCtx, PaintCtx, PaintScene, Size, View, Widget,
        };
        use frust::kurbo::{BezPath, Shape as _};
        use frust::peniko::{Brush, Color};

        const DEVICE_WIDTH: f64 = 360.0;
        let card_inner_width = DEVICE_WIDTH - 4.0 * frust_material::MaterialSpacing::LG;

        #[derive(Default)]
        struct MaxXScene {
            max_x: f64,
        }
        impl PaintScene for MaxXScene {
            fn fill_rect(&mut self, _origin: Point, _size: Size, _color: Color) {}
            fn draw_text(&mut self, _origin: Point, _text: &str) {}
            fn fill_rounded_rect(
                &mut self,
                origin: Point,
                size: Size,
                _radius: f64,
                _color: Color,
            ) {
                self.max_x = self.max_x.max(origin.x + size.width);
            }
            fn stroke_path(&mut self, origin: Point, path: &BezPath, _width: f64, _brush: &Brush) {
                self.max_x = self.max_x.max(origin.x + path.bounding_box().x1);
            }
        }

        let view = super::all_types_row(false, false, false);
        let mut counter = 0u64;
        let mut widget = view.build(&mut BuildCtx::new(&mut counter));
        let bc = BoxConstraints::new(Size::ZERO, Size::new(card_inner_width, 400.0));
        let mut text_ctx = TextContext::new();
        let mut layout_ctx = LayoutCtx::with_resources(Some(&mut text_ctx as &mut dyn Any), None);
        let size = widget.layout(&mut layout_ctx, &bc);
        assert!(
            size.width <= card_inner_width + 1e-6,
            "the chip row's own reported size {size:?} must not exceed the \
             card's inner width {card_inner_width}"
        );

        let mut ctx = PaintCtx::new(Point::ZERO, size);
        let mut scene = MaxXScene::default();
        widget.paint(&mut ctx, &mut scene);
        assert!(
            scene.max_x <= card_inner_width + 1e-6,
            "G10 regressed: painted x {} exceeds the card's inner width {} \
             (reported size {size:?})",
            scene.max_x,
            card_inner_width
        );
    }
}
