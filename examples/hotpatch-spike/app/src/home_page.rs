//! The home screen: a sentinel line, a counter and a child card, built from small widget fns.
//!
//! `measure.sh` edits the lines carrying a `// SENTINEL-*` or `// STATE-*` marker in place; keep each
//! marker on the line it annotates.

use frust::{Component, CrossAxisAlignment, View, button, column, component, text};

use crate::counter_card::CounterCard;

/// The counter screen; the count is its local state.
pub struct HomePage {
    pub title: String,
}

/// HomePage's local state. A named struct on purpose: `measure.sh --target state-field` adds a field
/// to it, which keeps the type's identity (and the seam's symbol) while changing its layout
/// (RESULTS.md, row D2).
pub struct HomeState {
    pub count: u32, // STATE-FIELDS
}

/// The state type the widget fns are written against, so a `--target state-type` edit (which swaps
/// `State` for a tuple) only has to rewrite the marked lines.
type PageState = <HomePage as Component>::State;

impl Component for HomePage {
    type State = HomeState; // STATE-TYPE

    fn init(&self) -> PageState {
        HomeState { count: 0 } // STATE-INIT
    }

    fn build(&self, state: &mut PageState) -> impl View<PageState> {
        let count = state.count; // STATE-READ
        let sentinel = text("hotpatch-sentinel: v0"); // SENTINEL-HOME
        column()
            .child(text(self.title.clone()).size(24.0))
            .child(sentinel)
            .child(count_label(count))
            .child(increment_button())
            .child(component(CounterCard))
            .cross_axis(CrossAxisAlignment::Center)
    }
}

fn count_label(count: u32) -> impl View<PageState> + use<> {
    text(format!("count: {count}")).size(48.0)
}

fn increment_button() -> impl View<PageState> + use<> {
    button("Increment", |state: &mut PageState| state.count += 1) // STATE-INC
}
