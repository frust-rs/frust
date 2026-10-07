//! A stateless child card: a second module and a second `component(..)` child, proving a patch reaches
//! more than one component.

use frust::{Component, View, column, text};

/// A card under the counter showing its own sentinel line.
pub struct CounterCard;

impl Component for CounterCard {
    type State = ();

    fn init(&self) {}

    fn build(&self, _state: &mut ()) -> impl View<()> {
        let sentinel = text("card-sentinel: v0"); // SENTINEL-CARD
        column().child(text("Counter card")).child(sentinel)
    }
}
