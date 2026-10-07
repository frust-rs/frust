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

/// HomePage's state type, named once so the widget fns below follow a `--target state` edit.
type HomeState = <HomePage as Component>::State;

impl Component for HomePage {
    type State = u32; // STATE-TYPE

    fn init(&self) -> HomeState {
        0 // STATE-INIT
    }

    fn build(&self, state: &mut HomeState) -> impl View<HomeState> {
        let count = *state; // STATE-READ
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

fn count_label(count: u32) -> impl View<HomeState> + use<> {
    text(format!("count: {count}")).size(48.0)
}

fn increment_button() -> impl View<HomeState> + use<> {
    button("Increment", |count: &mut HomeState| *count += 1) // STATE-INC
}
