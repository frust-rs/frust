//! Widgets · Cards screen — REAL (ported from `examples/catalog` in task 03).
//!
//! The catalog's Cards tab: elevated/filled/outlined [cards](elevated_card)
//! plus a 1000-row [`list_view`] (`ListView::builder`) proving the windowed
//! virtualization pattern (`docs/ARCHITECTURE.md`'s `ListView` precedent). A
//! row press updates the "Selected row" banner above the list.
//!
//! # Shape
//!
//! Like [the Controls screen](crate::screens::widgets_controls), `routes.rs`
//! calls this no-arg and re-invokes the builder every rebuild, so the
//! `selected_row` demo signal lives in a nested [`Component`]'s [retained
//! state](CardsState), created once in [`Component::init`].

use forgekit::{
    AnyView, Axis, Column, Component, CrossAxisAlignment, FlexView, Get, ONE_LINE_HEIGHT, Row,
    RwSignal, Set, SizedBox, any, component, elevated_card, filled_card, flexible, inflexible,
    list_item, list_view, outlined_card, text,
};

use crate::ShellState;

/// The Cards exhibit route entry point: a nested [`Component`] so the
/// `selected_row` signal survives the router's per-rebuild page rebuild (see
/// the [module docs](self)).
pub fn cards_screen() -> AnyView<ShellState> {
    any(component(CardsExhibit::default()))
}

/// The Cards exhibit's retained state: which list row was last pressed.
#[derive(Clone, Copy)]
struct CardsState {
    selected_row: RwSignal<Option<usize>>,
}

/// The Cards exhibit component: stateless configuration; the pressed-row signal
/// lives in its [`CardsState`].
#[derive(Default)]
struct CardsExhibit;

impl Component for CardsExhibit {
    type State = CardsState;

    fn init(&self) -> CardsState {
        CardsState {
            selected_row: RwSignal::new(None),
        }
    }

    fn build(&self, state: &mut CardsState) -> AnyView<CardsState> {
        let selected_row = state.selected_row;

        let header = Column(vec![
            any(text("Cards + ListView").size(24.0)),
            any(Row(vec![
                any(elevated_card(text("Elevated").size(16.0))),
                any(SizedBox(Some(8.0), None)),
                any(filled_card(text("Filled").size(16.0))),
                any(SizedBox(Some(8.0), None)),
                any(outlined_card(text("Outlined").size(16.0))),
            ])),
            any(text(format!(
                "Selected row: {}",
                selected_row
                    .get()
                    .map(|i| i.to_string())
                    .unwrap_or_else(|| "(none)".to_string())
            ))
            .size(14.0)),
            any(text("1000-row virtualized list:").size(14.0)),
        ]);

        let list = list_view::<CardsState>(1000, ONE_LINE_HEIGHT, move |i| {
            any(list_item::<CardsState>(format!("Row {i}"))
                .supporting(format!("Item index {i}"))
                .on_press(move |_s: &mut CardsState| selected_row.set(Some(i))))
        });

        any(
            FlexView::new(Axis::Vertical, vec![inflexible(header), flexible(1, list)])
                .cross_axis(CrossAxisAlignment::Stretch),
        )
    }
}
