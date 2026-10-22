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
//! header's content-space y minus the tab header's, so the section's header
//! ends where the body starts. Every measurement is taken against the scroll
//! viewport's own window rect in the same paint epoch (see
//! [`fresh_anchor_rect`]), plus the controller's offset, so it is right from
//! any current offset; a section the scroll view culled is approached by its
//! remembered position, or in viewport-sized steps, until it paints. The same
//! helper serves the Dialogs tab.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::Duration;

use frust::{AnyView, FlexChild, Get, ScrollController, inflexible, text};

use super::common::{
    S, accent, anchor_probe, caption, fresh_anchor_rect, page_column, page_header, tab_body,
    viewport_key,
};
use super::{SECTION_SUMMARIES, composite, controls, new_controls};
use crate::{SECTION_LABELS, Tab};

/// The [`anchor_probe`] key of this tab's header: the body's origin.
pub const TOP: &str = "controls_tab.top";

/// Every anchor this tab scrolls to, in body order.
pub const ANCHORS: &[&str] = &[controls::ANCHOR, new_controls::ANCHOR, composite::ANCHOR];

/// How many times [`scroll_on_request`] steps toward a request before it
/// gives up on it, and the pause between steps (one or more frames).
const PROBE_ATTEMPTS: u32 = 40;
const PROBE_PAUSE: Duration = Duration::from_millis(100);

/// How close (logical px) the offset must be to the target to count as landed.
const LANDED_TOLERANCE: f64 = 1.0;

/// How much of a viewport one blind step toward a never-painted section covers.
const STEP_FRACTION: f64 = 0.8;

thread_local! {
    /// The last [`AnchorRequest::sequence`](crate::AnchorRequest) each tab
    /// scheduled a scroll for. Plain storage: `build` must not write signals.
    static SCHEDULED: Cell<[u64; Tab::COUNT]> = const { Cell::new([0; Tab::COUNT]) };

    /// The last measured content-space y of each probe, by key: a coarse
    /// estimate for a section the scroll view has culled. Plain storage.
    static CONTENT_Y: RefCell<HashMap<&'static str, f64>> = RefCell::new(HashMap::new());
}

/// How [`anchor_scroll_step`] left a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorStep {
    /// The target header already sits where the body starts.
    Landed,
    /// A jump was issued (or nothing could be measured yet): step again after
    /// the next frame.
    Pending,
}

/// One measuring step of an anchor scroll, to run after each frame.
///
/// Reads the probes painted in the latest epoch against `tab`'s scroll
/// viewport, so a probe the scroll view culled is simply absent rather than
/// stale. With the target painted it jumps to its content-space y less the
/// tab header's (clamped to the scrollable range), or reports
/// [`AnchorStep::Landed`] when already there. With it culled it jumps to its
/// remembered position, or one viewport-sized step toward it (by body order of
/// the probes that did paint), and the next step corrects.
pub fn anchor_scroll_step(
    controller: &ScrollController,
    tab: Tab,
    top: &'static str,
    anchor: &'static str,
    anchors: &'static [&'static str],
) -> AnchorStep {
    let Some(viewport) = fresh_anchor_rect(viewport_key(tab)) else {
        return AnchorStep::Pending;
    };
    let offset = controller.offset();
    let content_y = |key: &str| fresh_anchor_rect(key).map(|rect| rect.y - viewport.y + offset);
    CONTENT_Y.with(|map| {
        let mut map = map.borrow_mut();
        for &key in anchors.iter().chain(std::iter::once(&top)) {
            if let Some(y) = content_y(key) {
                map.insert(key, y);
            }
        }
    });
    let remembered = |key: &str| CONTENT_Y.with(|map| map.borrow().get(key).copied());
    let body_start = remembered(top).unwrap_or(0.0);
    let clamp = |target: f64| target.clamp(0.0, controller.max_offset());

    if let Some(y) = content_y(anchor) {
        let target = clamp(y - body_start);
        if (target - offset).abs() <= LANDED_TOLERANCE {
            return AnchorStep::Landed;
        }
        controller.jump_to(target);
        return AnchorStep::Pending;
    }
    let step = controller.viewport_extent() * STEP_FRACTION;
    let position = |key: &str| anchors.iter().position(|a| *a == key);
    let wanted = position(anchor);
    let target = match remembered(anchor) {
        Some(y) => clamp(y - body_start),
        None => {
            let painted_before = anchors
                .iter()
                .any(|a| position(a) < wanted && content_y(a).is_some());
            let painted_after = anchors
                .iter()
                .any(|a| position(a) > wanted && content_y(a).is_some());
            if painted_after && !painted_before {
                clamp(offset - step)
            } else {
                clamp(offset + step)
            }
        }
    };
    if (target - offset).abs() > LANDED_TOLERANCE {
        controller.jump_to(target);
    }
    AnchorStep::Pending
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
/// signal, then schedules the scroll on the UI task queue, which runs an
/// [`anchor_scroll_step`] every pause until the section has landed.
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
            // Match the join `Result` and surface a failure rather than
            // dropping it.
            if let Err(join_err) = frust::spawn_blocking(|| std::thread::sleep(PROBE_PAUSE)).await {
                log::warn!("native-widgets-demo: anchor scroll timer panicked: {join_err}");
                return;
            }
            if anchor_scroll_step(&controller, tab, top, request.anchor, anchors)
                == AnchorStep::Landed
            {
                return;
            }
        }
        log::warn!(
            "native-widgets-demo: anchor {:?} did not land; giving up",
            request.anchor
        );
    });
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &S) -> AnyView<S> {
    scroll_on_request(state, Tab::Controls, TOP, ANCHORS);
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
