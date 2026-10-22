//! **Sheet** — the native page sheet through `show_native_sheet_into`: a
//! request answered by exactly one [`SheetOutcome`], its content a
//! constrained native schema (title, message, image, up to three action rows —
//! never frust widgets), themed from the active frust [`Theme`].
//!
//! Variants: medium + large with a grabber (the default), a non-dismissible
//! sheet (no swipe-down — only a row or a programmatic dismiss ends it), a
//! custom 0.4 detent (iOS 16+; iOS 15 falls back to the nearest system
//! detent, logged), and a programmatic run that expands to large after 1 s
//! and dismisses after 3 s through the returned `SheetHandle`. User detent
//! drags stream through `on_detent` into the detent readout; they are never
//! outcomes.
//!
//! iOS/iPadOS present `UISheetPresentationController`; Android presents a
//! Material `BottomSheetDialog`: Medium is half height (ratio 0.5), Large is
//! expanded, `Custom(f)` is half-ratio `f`. Only ONE half ratio is draggable
//! at a time (the others are reachable through `select_detent`);
//! `largest_undimmed` and `scrolling_expands` are ignored on Android; detent
//! changes still stream to `on_detent`. macOS and every Linux/Windows desktop
//! preview refuse with `PresentError::Unsupported`, which the status line
//! reports - the designed behavior there, not a failure. An iPad in regular
//! width presents a centered form sheet that ignores detents (see the plugin
//! README). Zero native slots at rest.

use std::time::Duration;

use frust::{AnyView, FlexChild, Get, Set, Theme, button, inflexible};
use frust_native_widgets::{
    ActionRole, Detent, PresentError, SheetContent, SheetHandle, SheetOutcome, SheetSpec,
    show_native_sheet,
};

use crate::push_toast;

use super::alerts::note_outcome;
use super::common::{
    S, block, caption, demo_image_bytes, gap, label, local_sig, page_column, page_header, readout,
    theme,
};

/// This page's index in [`SECTION_LABELS`](crate::SECTION_LABELS).
const SECTION: usize = 5;

/// The native class backing a sheet on this target.
const SHEET_CLASS: &str = if cfg!(target_os = "android") {
    "BottomSheetDialog"
} else if cfg!(target_os = "ios") {
    "UISheetPresentationController"
} else {
    "nothing (Unsupported)"
};

/// The custom detent's fraction of the available height.
const CUSTOM_DETENT: f64 = 0.4;

/// When the programmatic run expands to [`Detent::Large`].
const EXPAND_AFTER: Duration = Duration::from_secs(1);
/// When the programmatic run dismisses, counted from the expand.
const DISMISS_AFTER_EXPAND: Duration = Duration::from_secs(2);

local_sig!(outcome_sig, Option<SheetOutcome>, None);
local_sig!(status_sig, String, "No request yet.".to_string());
local_sig!(detent_sig, String, "(none reported)".to_string());

/// The shared content: title, message, the demo image and three action rows.
fn content() -> SheetContent {
    SheetContent::new()
        .with_title("Share draft")
        .with_message("Anyone with the link can read it. Drag the grabber to change detents.")
        .with_image(demo_image_bytes())
        .with_action("copy", "Copy link", ActionRole::Default)
        .with_action("stop", "Stop sharing", ActionRole::Destructive)
        .with_action("close", "Close", ActionRole::Cancel)
}

/// A sheet spec over [`content`], themed from `theme`, streaming user detent
/// changes into the detent readout.
fn base_spec(theme: &Theme) -> SheetSpec {
    SheetSpec::new(content())
        .with_theme(theme)
        .on_detent(|detent| detent_sig().set(format!("{detent:?}")))
}

/// Present `spec`, clearing the last outcome. An accepted request reads
/// "presented" at once; a refusal resolves on the next task-queue pump and
/// replaces it. The outcome is awaited on the UI task queue, written into this
/// page's signal and counted for the Dialogs badge when another tab is
/// showing. Returns the handle when shown.
fn present_sheet(state: &S, what: &str, spec: SheetSpec) -> Option<SheetHandle> {
    outcome_sig().set(None);
    detent_sig().set("(none reported)".to_string());
    let presentation = show_native_sheet(spec);
    let handle = presentation.sheet_handle();
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
            Err(PresentError::Unsupported) => status_sig().set(format!(
                "{what}: Unsupported \u{2014} native sheets are iOS/iPadOS/Android only (designed)"
            )),
            Err(err) => status_sig().set(format!("{what}: refused: {err}")),
        }
    });
    handle
}

/// A readable line for the last outcome.
fn describe_outcome(outcome: Option<&SheetOutcome>) -> String {
    match outcome {
        None => "Outcome: (none yet)".to_string(),
        Some(SheetOutcome::Action(id)) => format!("Outcome: Action(\"{id}\")"),
        Some(SheetOutcome::Dismissed(reason)) => format!("Outcome: Dismissed({reason:?})"),
        Some(SheetOutcome::HostLost) => "Outcome: HostLost".to_string(),
    }
}

/// One trigger block: a heading, a caption and a button.
fn trigger(
    title: &str,
    note: &str,
    button_label: &str,
    handler: impl Fn(&mut S) + 'static,
) -> frust::FlexChild<S> {
    block(vec![
        inflexible(label(title)),
        gap(4.0),
        inflexible(caption(note)),
        gap(6.0),
        inflexible(button(button_label, handler)),
    ])
}

/// The deep-link slug of this section: the anchor its tab scrolls to.
pub const ANCHOR: &str = "Sheet";

/// The section body, without a page header: the rows the Dialogs tab stacks
/// under its own section header. See the [module docs](self).
pub fn section(state: &S) -> Vec<FlexChild<S>> {
    // Copy handle, cheap to move into the programmatic run's timers below —
    // see their `Err(join_err)` arms.
    let toasts = state.toasts;
    let outcome = outcome_sig().get();
    let status = status_sig().get();
    let detent = detent_sig().get();
    // Captured at build: an event handler runs outside the build pass, where
    // no theme context is provided.
    let active = theme();

    let readouts = block(vec![
        inflexible(label("Readout")),
        gap(4.0),
        inflexible(readout(format!("Last request: {status}"))),
        inflexible(readout(describe_outcome(outcome.as_ref()))),
        inflexible(readout(format!("Last user detent: {detent}"))),
    ]);

    let arms = block(vec![
        inflexible(label("Per-platform arm")),
        gap(4.0),
        inflexible(caption(format!(
            "iOS/iPadOS UISheetPresentationController \u{b7} Android BottomSheetDialog \u{b7} \
             macOS and desktop previews Unsupported \u{2014} here: {}.",
            SHEET_CLASS
        ))),
        gap(2.0),
        inflexible(caption(
            "Android: Medium = half height (ratio 0.5), Large = expanded, Custom(f) = half \
             ratio f. Only one half ratio is draggable at a time (others via select_detent); \
             largest_undimmed and scrolling_expands are ignored; detent changes still stream \
             to the readout below.",
        )),
    ]);

    let t1 = active.clone();
    let standard = trigger(
        "Medium + large, grabber",
        "The default spec: resting at medium, draggable to large, swipe down to dismiss.",
        "Show sheet",
        move |state: &mut S| {
            present_sheet(
                state,
                "Medium + large",
                base_spec(&t1).with_detents([Detent::Medium, Detent::Large]),
            );
        },
    );

    let t2 = active.clone();
    let non_dismissible = trigger(
        "Non-dismissible",
        "No swipe-down: only an action row ends it.",
        "Show non-dismissible sheet",
        move |state: &mut S| {
            present_sheet(
                state,
                "Non-dismissible",
                base_spec(&t2).with_dismissible(false),
            );
        },
    );

    let t3 = active.clone();
    let custom = trigger(
        "Custom 0.4 detent",
        "Opens at 40% height with large available. iOS 16+; iOS 15 uses the nearest of \
         medium/large.",
        "Show 0.4 sheet",
        move |state: &mut S| {
            present_sheet(
                state,
                "Custom 0.4",
                base_spec(&t3)
                    .with_detents([Detent::Custom(CUSTOM_DETENT), Detent::Large])
                    .with_selected(Detent::Custom(CUSTOM_DETENT)),
            );
        },
    );

    let t4 = active;
    let programmatic = trigger(
        "Programmatic control",
        "Presents at medium, expands to large after 1 s (select_detent), then dismisses after \
         2 s more: the outcome must read Dismissed(Programmatic).",
        "Show, expand, dismiss",
        move |state: &mut S| {
            let Some(handle) = present_sheet(state, "Programmatic", base_spec(&t4)) else {
                return;
            };
            frust::spawn_local(async move {
                // Match each join `Result` and surface a failure rather than
                // silently dropping it (`examples/playground`'s `camera.rs`
                // convention); an expand-timer failure skips the dismiss
                // timer too, since the sheet's detent state is now unknown.
                match frust::spawn_blocking(|| std::thread::sleep(EXPAND_AFTER)).await {
                    Ok(()) => handle.select_detent(Detent::Large),
                    Err(join_err) => {
                        log::warn!(
                            "native-widgets-demo sheet: programmatic expand timer panicked: \
                             {join_err}"
                        );
                        push_toast(
                            toasts,
                            "Sheet: programmatic expand timer failed".to_string(),
                        );
                        return;
                    }
                }
                match frust::spawn_blocking(|| std::thread::sleep(DISMISS_AFTER_EXPAND)).await {
                    Ok(()) => handle.dismiss(),
                    Err(join_err) => {
                        log::warn!(
                            "native-widgets-demo sheet: programmatic dismiss timer panicked: \
                             {join_err}"
                        );
                        push_toast(
                            toasts,
                            "Sheet: programmatic dismiss timer failed".to_string(),
                        );
                    }
                }
            });
        },
    );

    vec![
        readouts,
        arms,
        standard,
        non_dismissible,
        custom,
        programmatic,
    ]
}

/// The whole section as a standalone page: its [`page_header`] then
/// [`section`]. Kept for [`super::current`] and the headless gate tests; the
/// Dialogs tab stacks [`section`] under its own section header instead.
pub fn page(state: &S) -> AnyView<S> {
    let mut children = vec![page_header(SECTION)];
    children.extend(section(state));
    page_column(children)
}
