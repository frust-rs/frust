//! The Controls tab's root page: the tab header (live-slot readout, platform),
//! then the Controls, New and Composite sections, each under its own section
//! header, in one scroll on the tab's shared
//! [`ScrollController`](frust::ScrollController).
//!
//! # Anchors
//!
//! Every section header is wrapped in an [`anchor_probe`] keyed by the
//! section's `ANCHOR` (its deep-link slug). A deep link to a section lands
//! here as a [`pending_anchor`](crate::NativeWidgetsDemoState::pending_anchor)
//! request; [`scroll_on_request`] applies each new request (by its
//! `sequence`) once the probes have painted: the target offset is the
//! header's painted window y minus the tab header's, so the section's header
//! ends where the body starts. The same helper serves the Dialogs tab.

use std::cell::Cell;
use std::time::Duration;

use frust::{AnyView, FlexChild, Get, inflexible, text};

use super::common::{
    S, accent, anchor_probe, anchor_rect, caption, page_column, page_header, tab_body,
};
use super::{SECTION_SUMMARIES, composite, controls, new_controls};
use crate::{SECTION_LABELS, Tab};

/// The [`anchor_probe`] key of this tab's header: the body's origin.
const TOP: &str = "controls_tab.top";

/// How many times [`scroll_on_request`] looks for painted probes before it
/// gives up on a request, and the pause between looks.
const PROBE_ATTEMPTS: u32 = 10;
const PROBE_PAUSE: Duration = Duration::from_millis(100);

thread_local! {
    /// The last [`AnchorRequest::sequence`](crate::AnchorRequest) each tab
    /// scheduled a scroll for. Plain storage: `build` must not write signals.
    static SCHEDULED: Cell<[u64; Tab::COUNT]> = const { Cell::new([0; Tab::COUNT]) };
}

/// One titled section: its header (the anchor probe, the label, its summary)
/// above `children`, in the page's padded column.
pub(super) fn titled(
    anchor: &'static str,
    title: &str,
    summary: &str,
    children: Vec<FlexChild<S>>,
) -> FlexChild<S> {
    let header = vec![
        inflexible(anchor_probe(
            anchor,
            text(title.to_string()).size(18.0).color(accent()),
        )),
        inflexible(caption(summary.to_string())),
    ];
    inflexible(page_column(header.into_iter().chain(children).collect()))
}

/// Scroll `tab`'s body to the section a new
/// [`pending_anchor`](crate::NativeWidgetsDemoState::pending_anchor) request
/// names, when it names one of `anchors`. Build-time only: it reads the
/// signal, then schedules the scroll on the UI task queue, retrying until the
/// anchor and `top` probes have both painted.
pub(super) fn scroll_on_request(
    state: &S,
    tab: Tab,
    top: &'static str,
    anchors: &'static [&'static str],
) {
    let Some(request) = state.pending_anchor.get() else {
        return;
    };
    if !anchors.contains(&request.anchor) {
        return;
    }
    let mut scheduled = SCHEDULED.with(Cell::get);
    if scheduled[tab.index()] >= request.sequence {
        return;
    }
    scheduled[tab.index()] = request.sequence;
    SCHEDULED.with(|cell| cell.set(scheduled));

    let controller = state.scroll(tab).clone();
    frust::spawn_local(async move {
        for _ in 0..PROBE_ATTEMPTS {
            if let (Some(section), Some(origin)) = (anchor_rect(request.anchor), anchor_rect(top)) {
                controller.jump_to((section.y - origin.y).max(0.0));
                return;
            }
            // Match the join `Result` and surface a failure rather than
            // dropping it.
            if let Err(join_err) = frust::spawn_blocking(|| std::thread::sleep(PROBE_PAUSE)).await {
                log::warn!("native-widgets-demo: anchor scroll timer panicked: {join_err}");
                return;
            }
        }
        log::warn!(
            "native-widgets-demo: anchor {:?} never painted; not scrolling",
            request.anchor
        );
    });
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &S) -> AnyView<S> {
    scroll_on_request(
        state,
        Tab::Controls,
        TOP,
        &[controls::ANCHOR, new_controls::ANCHOR, composite::ANCHOR],
    );
    tab_body(
        state,
        Tab::Controls,
        vec![
            inflexible(anchor_probe(
                TOP,
                page_column(vec![page_header(Tab::Controls)]),
            )),
            titled(
                controls::ANCHOR,
                SECTION_LABELS[0],
                SECTION_SUMMARIES[0],
                controls::section(state),
            ),
            titled(
                new_controls::ANCHOR,
                SECTION_LABELS[2],
                SECTION_SUMMARIES[2],
                new_controls::section(state),
            ),
            titled(
                composite::ANCHOR,
                SECTION_LABELS[6],
                SECTION_SUMMARIES[6],
                composite::section(state),
            ),
        ],
    )
}
