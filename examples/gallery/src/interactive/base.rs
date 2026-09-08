//! Stateful constructors for `Base`-design cases — see the
//! [module docs](super) for what this table is and why it sits beside the
//! registry rather than inside it.

use frust_core::{AnyView, Component, any, component};
use frust_widgets::{Axis, CrossAxisAlignment, FlexView, SizedBox, checkbox, inflexible};

use super::{Entry, framed};

/// This catalog's slice of the side table [`super::entries`] concatenates.
pub const INTERACTIVE: &[Entry] = &[("checkbox", checkbox_case)];

/// Retained state for the interactive `checkbox` case: one bool per box,
/// seeded to the same values the recorded case hardcodes so the live page
/// opens on the frame the poster shows.
struct CheckboxState {
    notifications: bool,
    marketing: bool,
}

/// The `checkbox` case with somewhere for its callbacks to write.
///
/// `checkbox` is controlled — it fires `on_toggle(state, !checked)` and never
/// flips its own `checked` — so the toggle only becomes visible if the value
/// handed back down on the next rebuild has changed. A `Component`'s `State`
/// is exactly that: plain retained data the handler mutates through
/// `EventCtx::state_mut`, and `build` re-runs on every rebuild pass, so the
/// mutation reaches the pixels on the next frame with no signal involved.
struct CheckboxCase;

impl Component for CheckboxCase {
    type State = CheckboxState;

    fn init(&self) -> Self::State {
        CheckboxState {
            notifications: true,
            marketing: false,
        }
    }

    fn build(&self, state: &mut Self::State) -> AnyView<Self::State> {
        framed(
            FlexView::new(
                Axis::Horizontal,
                vec![
                    inflexible(checkbox(
                        state.notifications,
                        "Notifications",
                        |state: &mut CheckboxState, checked| state.notifications = checked,
                    )),
                    inflexible(SizedBox(Some(28.0), None)),
                    inflexible(checkbox(
                        state.marketing,
                        "Marketing emails",
                        |state: &mut CheckboxState, checked| state.marketing = checked,
                    )),
                ],
            )
            .cross_axis(CrossAxisAlignment::Center),
        )
    }
}

/// The [`super::Build`] the table registers: a `()`-stated `AnyView` around
/// the stateful component, legal because `ComponentView<C>` implements
/// `View<Outer>` for every `Outer`.
fn checkbox_case() -> AnyView<()> {
    any(component(CheckboxCase))
}
