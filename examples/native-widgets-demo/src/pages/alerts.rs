//! **Alerts** — native alert presentations through
//! `show_native_alert_into`, each answered by exactly one [`AlertOutcome`]
//! written into a signal this page reads back:
//!
//! - a three-role alert (`Default`/`Destructive`/`Cancel`), with a
//!   cancelable toggle;
//! - an action sheet anchored to its own trigger button — the anchor is the
//!   button's painted window rect, recorded by
//!   [`anchor_probe`](super::common::anchor_probe) (see its doc for how the
//!   rect is computed);
//! - "show while showing": two requests from one tap, the second refused
//!   `PresentError::Busy` at once (one presentation per process, never
//!   queued);
//! - a programmatic dismiss 2 s after presenting (`present::dismiss`),
//!   resolving `Dismissed`.
//!
//! Platform behavior: iOS/iPadOS present `UIAlertController` (the action
//! sheet rises from the bottom on iPhone, and on iPad is a popover pointing
//! at the anchor); macOS an `NSAlert` window sheet (ActionSheet style and the
//! anchor ignored — the alert fallback, as designed; `Cancelled` never
//! occurs); Android an `AlertDialog` (style/anchor ignored); a Linux/Windows
//! desktop preview reports `Unsupported`. No trigger here is a native slot:
//! zero native slots at rest.

use std::time::Duration;

use frust::{AnyView, FlexChild, Get, GetUntracked, RwSignal, Set, button, checkbox, inflexible};
use frust_native_widgets::{
    ActionRole, AlertOutcome, AlertSpec, AlertStyle, PresentError, PresentationHandle, present,
    show_native_alert,
};

use crate::{Tab, push_toast};

use super::common::{
    S, anchor_probe, anchor_rect, block, caption, gap, label, local_sig, page_column, page_header,
    readout,
};

/// This page's index in [`SECTION_LABELS`](crate::SECTION_LABELS).
const SECTION: usize = 3;

/// The [`anchor_probe`] key the action-sheet trigger records under.
const ACTION_SHEET_ANCHOR: &str = "alerts.action_sheet";

/// How long the auto-dismiss demo leaves its alert up.
const AUTO_DISMISS_AFTER: Duration = Duration::from_secs(2);

local_sig!(outcome_sig, Option<AlertOutcome>, None);
local_sig!(status_sig, String, "No request yet.".to_string());
local_sig!(cancelable_sig, bool, true);

/// The three-role alert.
fn three_role_spec(cancelable: bool) -> AlertSpec {
    let mut spec = AlertSpec::new(
        "Discard changes?",
        "Three roles: Save (default), Discard (destructive), Keep editing (cancel).",
    )
    .with_action("save", "Save", ActionRole::Default)
    .with_action("discard", "Discard", ActionRole::Destructive)
    .with_action("keep", "Keep editing", ActionRole::Cancel);
    spec.cancelable = cancelable;
    spec
}

/// A readable line for one presentation error.
fn describe_error(err: &PresentError) -> String {
    match err {
        PresentError::Busy => "refused: Busy (another presentation is live)".to_string(),
        PresentError::Unsupported => "refused: Unsupported on this platform".to_string(),
        PresentError::NoHost => "refused: no host to present over".to_string(),
        other => format!("refused: {other}"),
    }
}

/// Count a presentation outcome that landed while another tab is showing, so
/// the shell's Dialogs badge reads it. Event/task context only.
pub(super) fn note_outcome(state_tab: RwSignal<Tab>, unseen: RwSignal<u32>) {
    if state_tab.get_untracked() != Tab::Dialogs {
        unseen.set(unseen.get_untracked().saturating_add(1));
    }
}

/// Present `spec`, clearing the previous outcome first. The status line says
/// "presented" at once for an accepted request; a refusal (`Busy`,
/// `Unsupported`...) resolves on the next task-queue pump and replaces it.
/// The outcome is awaited on the UI task queue, written into this page's
/// signal and counted for the Dialogs badge when another tab is showing.
/// Returns the handle when shown.
fn present_alert(state: &S, what: &str, spec: AlertSpec) -> Option<PresentationHandle> {
    outcome_sig().set(None);
    let presentation = show_native_alert(spec);
    let handle = presentation.handle();
    if handle.is_some() {
        status_sig().set(format!("{what}: presented"));
    }
    let (tab, unseen) = (state.tab, state.unseen_outcomes);
    let what = what.to_string();
    frust::spawn_local(async move {
        match presentation.await {
            Ok(outcome) => {
                outcome_sig().set(Some(outcome));
                note_outcome(tab, unseen);
            }
            Err(err) => status_sig().set(format!("{what}: {}", describe_error(&err))),
        }
    });
    handle
}

/// A readable line for the last outcome.
fn describe_outcome(outcome: Option<&AlertOutcome>) -> String {
    match outcome {
        None => "Outcome: (none yet)".to_string(),
        Some(AlertOutcome::Action(id)) => format!("Outcome: Action(\"{id}\")"),
        Some(AlertOutcome::Cancelled) => "Outcome: Cancelled".to_string(),
        Some(AlertOutcome::Dismissed) => "Outcome: Dismissed (programmatic)".to_string(),
        Some(AlertOutcome::HostLost) => "Outcome: HostLost".to_string(),
    }
}

/// One tap, two alerts: the first presents, the second is refused `Busy`. The
/// refusal resolves on the next task-queue pump and completes the status line.
fn request_two(state: &mut S) {
    let first = present_alert(
        state,
        "First",
        AlertSpec::new(
            "First alert",
            "A second request is refused while this is up.",
        )
        .with_action("ok", "OK", ActionRole::Default),
    );
    let first = if first.is_some() {
        "first: presented"
    } else {
        "first: refused"
    };
    let second = show_native_alert(
        AlertSpec::new("Second alert", "Never shown while the first is live.").with_action(
            "ok",
            "OK",
            ActionRole::Default,
        ),
    );
    frust::spawn_local(async move {
        let second = match second.await {
            Ok(_) => "second: presented (unexpected while the first is live)".to_string(),
            Err(err) => format!("second: {}", describe_error(&err)),
        };
        status_sig().set(format!("{first}; {second}"));
    });
}

/// The deep-link slug of this section: the anchor its tab scrolls to.
pub const ANCHOR: &str = "Alerts";

/// The section body, without a page header: the rows the Dialogs tab stacks
/// under its own section header. See the [module docs](self).
pub fn section(state: &S) -> Vec<FlexChild<S>> {
    // Copy handle, cheap to move into the auto-dismiss timer's async block
    // below — see its `Err(join_err)` arm.
    let toasts = state.toasts;
    // Tracked reads: the outcome arrives from the platform's main-thread
    // callback, outside any frust event pass.
    let outcome = outcome_sig().get();
    let status = status_sig().get();
    let cancelable = cancelable_sig().get();

    let three_role = block(vec![
        inflexible(label("Three-role alert")),
        gap(4.0),
        inflexible(caption(
            "Cancelable matters only where the platform has a non-action dismissal: Android's \
             back key / outside tap resolve Cancelled; an iPad action-sheet popover's outside \
             tap; nothing on macOS or a UIKit alert.",
        )),
        gap(6.0),
        inflexible(checkbox(cancelable, "cancelable", |_: &mut S, on: bool| {
            cancelable_sig().set(on)
        })),
        gap(6.0),
        inflexible(button("Show three-role alert", move |state: &mut S| {
            present_alert(state, "Three-role alert", three_role_spec(cancelable));
        })),
    ]);

    let action_sheet = block(vec![
        inflexible(label("Action sheet, anchored to this button")),
        gap(4.0),
        inflexible(caption(
            "The anchor is this button's painted window rect (an AnchorRect in logical points). \
             iPhone ignores it; iPad requires it (without one the request is refused \
             InvalidSpec); macOS and Android show an ordinary alert.",
        )),
        gap(6.0),
        inflexible(anchor_probe(
            ACTION_SHEET_ANCHOR,
            button("Show action sheet", move |state: &mut S| {
                let mut spec = three_role_spec(cancelable);
                spec.title = "Share draft".to_string();
                spec.message = "Choose an action.".to_string();
                spec.style = AlertStyle::ActionSheet;
                spec.anchor = anchor_rect(ACTION_SHEET_ANCHOR);
                present_alert(state, "Action sheet", spec);
            }),
        )),
        gap(4.0),
        inflexible(caption(match anchor_rect(ACTION_SHEET_ANCHOR) {
            Some(a) => format!(
                "Last recorded anchor: x {:.0}, y {:.0}, {:.0} x {:.0}",
                a.x, a.y, a.width, a.height
            ),
            None => "Last recorded anchor: (not painted yet)".to_string(),
        })),
    ]);

    let busy = block(vec![
        inflexible(label("Show while showing (Busy)")),
        gap(4.0),
        inflexible(caption(
            "One tap requests two alerts. The first presents; the second is refused Busy at \
             once \u{2014} one presentation per process, never queued. Answer the first to \
             free the slot.",
        )),
        gap(6.0),
        inflexible(button("Request two alerts", request_two)),
    ]);

    let auto_dismiss = block(vec![
        inflexible(label("Programmatic dismiss after 2 s")),
        gap(4.0),
        inflexible(caption(
            "Presents, then calls present::dismiss with the returned handle 2 s later: the \
             outcome must read Dismissed. Answer it sooner and the later dismiss is a no-op \
             (a stale handle is ignored).",
        )),
        gap(6.0),
        inflexible(button("Show, auto-dismiss in 2 s", move |state: &mut S| {
            let Some(handle) = present_alert(
                state,
                "Auto-dismiss",
                AlertSpec::new("Going away", "This alert dismisses itself in 2 seconds.")
                    .with_action("ok", "OK", ActionRole::Default),
            ) else {
                return;
            };
            frust::spawn_local(async move {
                // Match the join `Result` and surface a failure rather than
                // silently dropping it (`examples/playground`'s
                // `camera.rs` convention).
                match frust::spawn_blocking(|| std::thread::sleep(AUTO_DISMISS_AFTER)).await {
                    Ok(()) => present::dismiss(&handle),
                    Err(join_err) => {
                        log::warn!(
                            "native-widgets-demo alerts: auto-dismiss timer panicked: {join_err}"
                        );
                        push_toast(toasts, "Alerts: auto-dismiss timer failed".to_string());
                    }
                }
            });
        })),
    ]);

    let readouts = block(vec![
        inflexible(label("Readout")),
        gap(4.0),
        inflexible(readout(format!("Last request: {status}"))),
        inflexible(readout(describe_outcome(outcome.as_ref()))),
    ]);

    vec![readouts, three_role, action_sheet, busy, auto_dismiss]
}

/// The whole section as a standalone page: its [`page_header`] then
/// [`section`]. Kept for [`super::current`] and the headless gate tests; the
/// Dialogs tab stacks [`section`] under its own section header instead.
pub fn page(state: &S) -> AnyView<S> {
    let mut children = vec![page_header(SECTION)];
    children.extend(section(state));
    page_column(children)
}
