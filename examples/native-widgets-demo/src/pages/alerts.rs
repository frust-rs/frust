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

use frust::{AnyView, Get, Set, any, button, checkbox, inflexible};
use frust_native_widgets::{
    ActionRole, AlertOutcome, AlertSpec, AlertStyle, PresentError, present, show_native_alert_into,
};

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

/// Present `spec`, clearing the previous outcome first and recording the
/// synchronous result in the status line. Returns the handle when shown.
fn present_alert(what: &str, spec: AlertSpec) -> Option<present::PresentationHandle> {
    outcome_sig().set(None);
    match show_native_alert_into(spec, outcome_sig()) {
        Ok(handle) => {
            status_sig().set(format!("{what}: presented"));
            Some(handle)
        }
        Err(err) => {
            status_sig().set(format!("{what}: {}", describe_error(&err)));
            None
        }
    }
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

/// See the page-fn contract in [`crate::pages`] and the [module docs](self).
pub fn page(_state: &S) -> AnyView<S> {
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
        inflexible(any(checkbox(
            cancelable,
            "cancelable",
            |_: &mut S, on: bool| cancelable_sig().set(on),
        ))),
        gap(6.0),
        inflexible(any(button("Show three-role alert", move |_: &mut S| {
            present_alert("Three-role alert", three_role_spec(cancelable));
        }))),
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
            button("Show action sheet", move |_: &mut S| {
                let mut spec = three_role_spec(cancelable);
                spec.title = "Share draft".to_string();
                spec.message = "Choose an action.".to_string();
                spec.style = AlertStyle::ActionSheet;
                spec.anchor = anchor_rect(ACTION_SHEET_ANCHOR);
                present_alert("Action sheet", spec);
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
        inflexible(any(button("Request two alerts", |_: &mut S| {
            let first = present_alert(
                "First",
                AlertSpec::new(
                    "First alert",
                    "A second request is refused while this is up.",
                )
                .with_action("ok", "OK", ActionRole::Default),
            );
            let second =
                show_native_alert_into(
                    AlertSpec::new("Second alert", "Never shown while the first is live.")
                        .with_action("ok", "OK", ActionRole::Default),
                    outcome_sig(),
                );
            let second = match second {
                Ok(_) => "second: presented (unexpected while the first is live)".to_string(),
                Err(err) => format!("second: {}", describe_error(&err)),
            };
            let first = if first.is_some() {
                "first: presented"
            } else {
                "first: refused"
            };
            status_sig().set(format!("{first}; {second}"));
        }))),
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
        inflexible(any(button("Show, auto-dismiss in 2 s", |_: &mut S| {
            let Some(handle) = present_alert(
                "Auto-dismiss",
                AlertSpec::new("Going away", "This alert dismisses itself in 2 seconds.")
                    .with_action("ok", "OK", ActionRole::Default),
            ) else {
                return;
            };
            frust::spawn_local(async move {
                let _ = frust::spawn_blocking(|| std::thread::sleep(AUTO_DISMISS_AFTER)).await;
                present::dismiss(&handle);
            });
        }))),
    ]);

    let readouts = block(vec![
        inflexible(label("Readout")),
        gap(4.0),
        inflexible(readout(format!("Last request: {status}"))),
        inflexible(readout(describe_outcome(outcome.as_ref()))),
    ]);

    page_column(vec![
        page_header(SECTION),
        readouts,
        three_role,
        action_sheet,
        busy,
        auto_dismiss,
    ])
}
