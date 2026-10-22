//! The Lab tab's root page: the tab header, the at-rest slot table, then the
//! Stress, Sandwich, macOS and TabBar sections, each under its own section
//! header, in one scroll on the tab's shared
//! [`ScrollController`](frust::ScrollController). Anchors work as on the
//! Controls tab (see [`controls_tab`](super::controls_tab)).
//!
//! The Lab is the gate harness: it holds what is not a design pattern to copy
//! but a measurement or a reference — the slot accounting, the mount/unmount
//! cycler and 50-slot toggle, frust paint around native views, the AppKit
//! class map, and the bare embedded `UITabBar` to compare with the app's
//! real tab bar.

use frust::{AnyView, FlexChild, inflexible};

use super::common::{
    AT_REST_SLOTS, S, anchor_probe, block, caption, gap, label, page_column, page_header, readout,
    tab_body,
};
use super::controls_tab::{scroll_on_request, titled};
use super::{SECTION_SUMMARIES, macos, sandwich, stress, tab_bar};
use crate::{SANDWICH_LABEL, SECTION_LABELS, Tab};

/// The [`anchor_probe`](super::common::anchor_probe) key of this tab's header.
const TOP: &str = "lab.top";

/// Every anchor this tab scrolls to.
const ANCHORS: &[&str] = &[
    stress::ANCHOR,
    sandwich::ANCHOR,
    macos::ANCHOR,
    tab_bar::ANCHOR,
];

/// The at-rest table: every tab's documented native-slot count for this
/// build, the chrome's, and the rule for hidden tabs.
fn at_rest_table() -> FlexChild<S> {
    let mut rows = vec![
        inflexible(label(
            "Documented native slots at rest, per tab (this build)",
        )),
        gap(4.0),
        inflexible(caption(
            "Counted with the whole body visible: a slot is published only while painted, so \
             a body clipped by a short window publishes fewer.",
        )),
        gap(6.0),
    ];
    for tab in Tab::ALL {
        rows.push(inflexible(readout(format!(
            "{}: {}",
            tab.title(),
            AT_REST_SLOTS[tab.index()]
        ))));
    }
    rows.extend([
        inflexible(readout("Chrome (bars and toolbar): 0")),
        gap(4.0),
        inflexible(caption(
            "The chrome is never a slot, on any platform: the native bars are host surfaces, \
             and the fallback bars are frust-drawn.",
        )),
        gap(6.0),
        inflexible(caption(
            "Hidden tabs: every tab stays laid out but only the showing one paints, so from \
             the first frame after a switch the published set is the new tab's alone. The host \
             then disposes the unpublished slots at its own pace (the live count may lag a \
             switch) and re-creates them when the tab returns.",
        )),
    ]);
    block(rows)
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &S) -> AnyView<S> {
    scroll_on_request(state, Tab::Lab, TOP, ANCHORS);
    tab_body(
        state,
        Tab::Lab,
        vec![
            inflexible(anchor_probe(TOP, page_column(vec![page_header(Tab::Lab)]))),
            at_rest_table(),
            titled(
                stress::ANCHOR,
                SECTION_LABELS[7],
                SECTION_SUMMARIES[7],
                stress::section(state),
            ),
            titled(
                sandwich::ANCHOR,
                SANDWICH_LABEL,
                sandwich::SANDWICH_SUMMARY,
                sandwich::section(state),
            ),
            titled(
                macos::ANCHOR,
                SECTION_LABELS[1],
                SECTION_SUMMARIES[1],
                macos::section(state),
            ),
            titled(
                tab_bar::ANCHOR,
                SECTION_LABELS[4],
                SECTION_SUMMARIES[4],
                tab_bar::section(state),
            ),
        ],
    )
}
