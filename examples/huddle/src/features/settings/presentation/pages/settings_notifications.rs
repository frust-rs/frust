//! Notification settings (`/you/settings/notifications`) — a controller-backed
//! frequency radio group plus per-type toggle switches (a Huddle showcase
//! screen).
//!
//! Hosts a [`NotificationsController`] via
//! [`use_controller`](clean_signals_frust::use_controller) — the same
//! component-scoped seam the appearance screen uses. The frequency
//! [`radio`](frust::radio) group and the [`Switch`](frust_material::Switch)es are
//! controlled components: each reports its requested value into a controller
//! signal, and the next rebuild reflects the stored value back (no theme is
//! applied — these are pure preference data).

use std::sync::Arc;

use frust::{
    AnyView, Axis, Column, CrossAxisAlignment, EdgeInsets, FlexView, Get, Padding, Row, Set,
    SizedBox, any, component, flexible, inflexible, radio, scroll_view, text,
};
use frust_material::{Switch, app_bar};

use crate::HuddleState;
use crate::failure::HuddleFailure;
use crate::features::settings::{NotifFrequency, NotificationsController};
use clean_signals_frust::use_controller;

/// The notification-settings route entry point.
pub fn notifications_screen() -> AnyView<HuddleState> {
    any(component(NotificationsScreen))
}

/// The notification-settings `Component` (stateless config).
struct NotificationsScreen;

/// Retained state: the hosted controller (its signals are the only mutable
/// state).
struct NotificationsState {
    controller: Arc<NotificationsController>,
}

impl frust::Component for NotificationsScreen {
    type State = NotificationsState;

    fn init(&self) -> NotificationsState {
        let controller =
            use_controller::<NotificationsController, HuddleFailure>(NotificationsController::new);
        NotificationsState { controller }
    }

    fn build(&self, state: &mut NotificationsState) -> AnyView<NotificationsState> {
        // Tracked reads: a later `set` on any of these wakes the frame.
        let freq = state.controller.frequency.get();
        let sound = state.controller.sound.get();
        let vibrate = state.controller.vibrate.get();
        let previews = state.controller.previews.get();

        let mut children: Vec<AnyView<NotificationsState>> = Vec::new();
        children.push(any(text("Notify me about").size(16.0)));

        // The frequency radio group (the new `radio` widget).
        for choice in NotifFrequency::ALL {
            children.push(any(radio(choice == freq, choice.label()).on_select(
                move |st: &mut NotificationsState| {
                    st.controller.frequency.set(choice);
                },
            )));
        }

        children.push(any(SizedBox(None, Some(20.0))));
        children.push(any(text("Alert style").size(16.0)));
        children.push(toggle_row("Play a sound", sound, |st, on| {
            st.controller.sound.set(on)
        }));
        children.push(toggle_row("Vibrate", vibrate, |st, on| {
            st.controller.vibrate.set(on)
        }));
        children.push(toggle_row("Show message preview", previews, |st, on| {
            st.controller.previews.set(on)
        }));

        any(FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(any(app_bar::<NotificationsState>("Notifications"))),
                flexible(
                    1,
                    any(scroll_view(Padding(
                        EdgeInsets::all(16.0),
                        Column(children),
                    ))),
                ),
            ],
        )
        .cross_axis(CrossAxisAlignment::Stretch))
    }
}

/// A labelled toggle row: the label, a spacer, and a `Switch` reporting into
/// `on_toggle`.
fn toggle_row<F>(label: &str, on: bool, on_toggle: F) -> AnyView<NotificationsState>
where
    F: Fn(&mut NotificationsState, bool) + 'static,
{
    any(Row(vec![
        any(text(label.to_string()).size(14.0)),
        any(SizedBox(Some(12.0), None)),
        any(Switch(on, on_toggle)),
    ]))
}
