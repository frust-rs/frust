//! The overlay toast/snackbar service (PLAN.md's "Toasts/snackbars + undo").
//!
//! [`ToastController`] is a cheap, cloneable, `Send + Sync` handle — the same
//! shape as `NavigatorController`: callers only *record* a request (via
//! [`show`](ToastController::show) /
//! [`show_with_action`](ToastController::show_with_action)), and the live queue
//! lives behind an [`RwSignal`] the [`ToastOverlay`] tracks and re-renders. The
//! controller is created once in [`crate::HuddleState`] and mounted into the
//! shell's reserved overlay slot (`Stack` top layer) as
//! [`toast_overlay`]; every screen reaches it through the app state, so a toast
//! floats above whatever screen is on top.
//!
//! # Auto-dismiss
//!
//! Each queued toast schedules its own removal on the background reactive
//! runtime ([`forgekit::spawn`] + `clean_signals::time::sleep`), writing the
//! removal back through the queue signal, which wakes the shell. The dismiss
//! delay is configurable ([`ToastController::with_dismiss_after`]) so the
//! headless suite can drive a short-lived toast deterministically.
//!
//! # Phase C seam
//!
//! The overlay renders each toast as a `filled_card` with its text and an
//! optional action button (the undo affordance Phase C's swipe actions call
//! [`show_with_action`](ToastController::show_with_action) for). A slide-in +
//! spring entrance driven by an `AnimationController` is the intended visual
//! polish (see the task's item 5); this skeleton ships the load-bearing
//! controller/queue/overlay seam with a static card and a timer-driven
//! dismiss, and Phase C layers the entrance animation on top without changing
//! the [`ToastController`] contract.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use forgekit::{
    Align, Alignment, AnyView, Column, EdgeInsets, Get, GetUntracked, Padding, RwSignal, SizedBox,
    Update, any, filled_card, text,
};

/// The default auto-dismiss delay (Material snackbar-ish ~3s).
pub const DEFAULT_DISMISS_AFTER: Duration = Duration::from_secs(3);

/// A toast's optional action button (the "undo" affordance).
#[derive(Clone)]
pub struct ToastAction {
    /// The button label (e.g. `"Undo"`).
    pub label: String,
    /// The callback fired when the action is tapped. Captured `Send + Sync`
    /// state only (typically `Copy` signals / cloneable controller handles),
    /// mirroring the app's signal-capture pattern for state-less page-builder
    /// closures — a toast action does not receive `&mut State`.
    pub callback: Arc<dyn Fn() + Send + Sync>,
}

/// One live toast in the queue.
#[derive(Clone)]
pub struct ToastEntry {
    /// Stable id (used to dismiss the exact toast, even as others come and go).
    pub id: u64,
    /// The toast message.
    pub text: String,
    /// The optional action button.
    pub action: Option<ToastAction>,
}

struct ToastInner {
    entries: RwSignal<Vec<ToastEntry>>,
    next_id: AtomicU64,
    dismiss_after: Duration,
}

/// A cloneable handle to the app's toast queue. See the [module docs](self).
#[derive(Clone)]
pub struct ToastController {
    inner: Arc<ToastInner>,
}

impl Default for ToastController {
    fn default() -> Self {
        Self::new()
    }
}

impl ToastController {
    /// A controller with the [`DEFAULT_DISMISS_AFTER`] delay.
    pub fn new() -> Self {
        Self::with_dismiss_after(DEFAULT_DISMISS_AFTER)
    }

    /// A controller with an explicit auto-dismiss delay — the headless suite
    /// uses a short delay so a `show` → auto-dismiss round trip completes fast.
    pub fn with_dismiss_after(dismiss_after: Duration) -> Self {
        Self {
            inner: Arc::new(ToastInner {
                entries: RwSignal::new(Vec::new()),
                next_id: AtomicU64::new(1),
                dismiss_after,
            }),
        }
    }

    /// Queue a plain informational toast. Returns the new toast's id.
    pub fn show(&self, text: impl Into<String>) -> u64 {
        self.push(ToastEntry {
            id: 0,
            text: text.into(),
            action: None,
        })
    }

    /// Queue a toast with an action button (the undo affordance). `callback`
    /// runs on tap; the toast is dismissed immediately afterward. Returns the
    /// new toast's id.
    pub fn show_with_action(
        &self,
        text: impl Into<String>,
        label: impl Into<String>,
        callback: impl Fn() + Send + Sync + 'static,
    ) -> u64 {
        self.push(ToastEntry {
            id: 0,
            text: text.into(),
            action: Some(ToastAction {
                label: label.into(),
                callback: Arc::new(callback),
            }),
        })
    }

    /// Remove a toast by id (a no-op if it already auto-dismissed).
    pub fn dismiss(&self, id: u64) {
        self.inner.entries.update(|v| v.retain(|e| e.id != id));
    }

    /// The live queue, as a tracked read (drives the overlay's rebuild).
    pub fn snapshot(&self) -> Vec<ToastEntry> {
        self.inner.entries.get()
    }

    /// The live queue, read without tracking — for tests/inspection.
    pub fn snapshot_untracked(&self) -> Vec<ToastEntry> {
        self.inner.entries.get_untracked()
    }

    fn push(&self, mut entry: ToastEntry) -> u64 {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        entry.id = id;
        self.inner.entries.update(|v| v.push(entry));

        // Schedule auto-dismiss on the background runtime; the signal write
        // wakes the shell (and the headless pump loop) when it fires.
        let entries = self.inner.entries;
        let after = self.inner.dismiss_after;
        forgekit::spawn(async move {
            clean_signals::time::sleep(after).await;
            entries.update(|v| v.retain(|e| e.id != id));
        });

        id
    }
}

/// The overlay `Component` mounted in the shell's `Stack` top layer. Reads the
/// [`ToastController`] queue and paints each toast bottom-anchored above every
/// screen.
pub fn toast_overlay(controller: ToastController) -> AnyView<crate::HuddleState> {
    any(forgekit::component(ToastOverlay { controller }))
}

/// The toast overlay component (see [`toast_overlay`]).
pub struct ToastOverlay {
    controller: ToastController,
}

/// Retained overlay state: just the controller handle (all live data is the
/// controller's signal).
pub struct ToastOverlayState {
    controller: ToastController,
}

impl forgekit::Component for ToastOverlay {
    type State = ToastOverlayState;

    fn init(&self) -> ToastOverlayState {
        ToastOverlayState {
            controller: self.controller.clone(),
        }
    }

    fn build(&self, state: &mut ToastOverlayState) -> AnyView<ToastOverlayState> {
        let entries = state.controller.snapshot(); // tracked

        // Bottom-anchored stack of toast cards. An empty queue renders an empty
        // (zero-child) column that paints nothing, so the overlay is inert when
        // there is nothing to show.
        let mut rows: Vec<AnyView<ToastOverlayState>> = Vec::new();
        for entry in entries {
            rows.push(toast_card(entry));
            rows.push(any(SizedBox(None, Some(8.0))));
        }

        // `Align` at the bottom-center places the toast column above the
        // keyboard/nav area; the overlay layer itself fills the window.
        any(Align(Alignment::new(0.0, 1.0), Column(rows)))
    }
}

/// Render one toast as a `filled_card` with its text and optional action button.
fn toast_card(entry: ToastEntry) -> AnyView<ToastOverlayState> {
    let id = entry.id;
    let mut row: Vec<AnyView<ToastOverlayState>> = vec![any(text(entry.text).size(14.0))];

    if let Some(action) = entry.action {
        row.push(any(SizedBox(Some(12.0), None)));
        let callback = action.callback.clone();
        row.push(any(forgekit::Button(
            action.label,
            move |st: &mut ToastOverlayState| {
                (callback)();
                st.controller.dismiss(id);
            },
        )));
    }

    any(Padding(
        EdgeInsets::all(8.0),
        filled_card(Padding(
            EdgeInsets::symmetric(16.0, 12.0),
            forgekit::Row(row),
        )),
    ))
}
