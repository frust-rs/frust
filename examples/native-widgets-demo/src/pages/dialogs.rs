//! The Dialogs tab's root page: the tab header, then the Alerts, Sheet and
//! Menus sections, each under its own section header, in one scroll on the
//! tab's shared scroll controller (anchors work as on the Controls tab; see
//! [`controls_tab`](super::controls_tab)).
//!
//! The Menus section is explanation plus a readout: the "+" and "..." menus
//! in the bar are native pull-downs (`UIMenu` on iOS, `PopupMenu` on
//! Android), the Comment toolbar on a Home detail page is native too (an iOS
//! toolbar, an Android FAB). The readout is frust state fed by the
//! scaffold's `events_into` signal; the menu is chrome. Zero native slots.

use frust::{AnyView, FlexChild, Get, inflexible};
use frust_native_widgets::ChromeEvent;

use super::common::{
    S, anchor_probe, block, caption, gap, label, page_column, page_header, readout, tab_body,
};
use super::controls_tab::{scroll_on_request, titled};
use super::{SECTION_SUMMARIES, alerts, sheet};
use crate::{SECTION_LABELS, Tab};

/// The deep-link slug of the Menus section.
pub const MENUS_ANCHOR: &str = "Menus";

/// The [`anchor_probe`](super::common::anchor_probe) key of this tab's header.
const TOP: &str = "dialogs.top";

/// Every anchor this tab scrolls to.
const ANCHORS: &[&str] = &[alerts::ANCHOR, sheet::ANCHOR, MENUS_ANCHOR];

/// A readable line for the last chrome event, `None` before any.
fn describe_event(event: Option<&ChromeEvent>) -> String {
    match event {
        None => "Last chrome event: (none yet)".to_string(),
        Some(event) => format!("Last chrome event: {event:?}"),
    }
}

/// The last toolbar/action tap, if the last event was one.
fn describe_action(event: Option<&ChromeEvent>) -> String {
    match event {
        Some(ChromeEvent::ActionTapped(id)) => format!("Last ActionTapped id: {id:?}"),
        _ => "Last ActionTapped id: (the last event was not an action tap)".to_string(),
    }
}

/// The Menus section body: what the bar's menus and the detail toolbar are,
/// and the readout of the last chrome event.
fn menus_section(state: &S) -> Vec<FlexChild<S>> {
    let event = state.chrome_event.get();
    vec![
        block(vec![
            inflexible(label("Pull-down menus")),
            gap(4.0),
            inflexible(caption(
                "The '+' and '...' menus in the bar are native pull-downs (UIMenu on iOS, \
                 PopupMenu on Android), not frust widgets. Choosing an item reports a \
                 ChromeEvent the app routes.",
            )),
            gap(6.0),
            inflexible(readout(describe_event(event.as_ref()))),
        ]),
        block(vec![
            inflexible(label("Toolbar")),
            gap(4.0),
            inflexible(caption(
                "The 'Comment' toolbar on a Home detail page is native too: an iOS toolbar, \
                 an Android FAB. Its tap arrives as ActionTapped with the action's id.",
            )),
            gap(6.0),
            inflexible(readout(describe_action(event.as_ref()))),
            gap(2.0),
            inflexible(caption(
                "The readout is frust; the menu and the toolbar are chrome.",
            )),
        ]),
    ]
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &S) -> AnyView<S> {
    scroll_on_request(state, Tab::Dialogs, TOP, ANCHORS);
    tab_body(
        state,
        Tab::Dialogs,
        vec![
            inflexible(anchor_probe(
                TOP,
                page_column(vec![page_header(Tab::Dialogs)]),
            )),
            titled(
                alerts::ANCHOR,
                SECTION_LABELS[3],
                SECTION_SUMMARIES[3],
                alerts::section(state),
            ),
            titled(
                sheet::ANCHOR,
                SECTION_LABELS[5],
                SECTION_SUMMARIES[5],
                sheet::section(state),
            ),
            titled(
                MENUS_ANCHOR,
                MENUS_ANCHOR,
                "The bar's pull-down menus and the detail toolbar, with the last event read back.",
                menus_section(state),
            ),
        ],
    )
}
