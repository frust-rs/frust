//! The macOS alert arm: an `NSAlert` built, presented as a window sheet and
//! resolved on the main thread, over `super::apple_host`'s shared helpers
//! (the main-queue hop, host discovery and the live-presentation guard).
//!
//! # Mapping
//!
//! | `AlertSpec` | AppKit |
//! |---|---|
//! | `title` / `message` | `NSAlert.messageText` / `.informativeText` — always set, even when empty: unlike UIKit's optional title/message, `NSAlert`'s setters take a plain `&NSString`, never `nil`, so an empty string still reserves its line |
//! | up to three `actions`, in spec order | `addButtonWithTitle:`, one call per action, in the same order |
//! | `role: Destructive` | `NSButton.hasDestructiveAction = true` (a caution styling hint; no crash/no-op on a system too old to honor it) |
//! | `style` / `anchor` | ignored — macOS has no action-sheet idiom, so `ActionSheet` presents the same window sheet as `Alert`, and `anchor` has nothing to point at |
//!
//! `cancelable` has no macOS analog either: unlike an iPad popover's outside
//! tap, a window sheet has no click-away dismissal, so this arm never
//! produces `AlertOutcome::Cancelled` — only `Action`, `Dismissed` (this
//! module's own [`super::dismiss`] or a call before anything is presented)
//! and `HostLost`.
//!
//! # Escape and Return
//!
//! AppKit assigns two key equivalents on its own: the **first added**
//! button gets Return, and any button **literally titled** "Cancel" gets
//! Escape. Since [`ActionRole::Cancel`] carries whatever label the caller
//! gave it, this arm does not rely on that title match — it sets the
//! Cancel-role button's `keyEquivalent` to Escape explicitly, whatever its
//! position or label. Buttons are always added in spec order, so the
//! automatic Return still lands on the first action unless that action is
//! itself the Cancel-role one, in which case the explicit Escape assignment
//! wins (matching AppKit's own behavior for a button really titled
//! "Cancel": it takes Escape even when added first). At most one action may
//! carry the Cancel role ([`AlertSpec::validate`]), so this is never
//! ambiguous.
//!
//! Pressing Return resolves `AlertOutcome::Action(id)` for the first action,
//! and pressing Escape resolves `AlertOutcome::Action(id)` for the
//! Cancel-role action. Since macOS sheets have no click-away dismissal
//! (unlike iPad popovers), this arm never produces a bare `Cancelled` —
//! the only way to get `Dismissed` is via [`super::dismiss`] or a call
//! before presentation started.
//!
//! # Resolution
//!
//! Every path resolves the presentation by first taking its live entry back
//! **by generation** (`apple_host::take_live_as`) — the first path to get
//! there wins, and every later one (a stale sheet completion, a duplicate
//! notification) finds nothing:
//!
//! - **A button** (mouse click or its key equivalent) — the one completion
//!   handler `beginSheetModalForWindow:completionHandler:` installs, mapping
//!   the returned `NSModalResponse` (`NSAlertFirstButtonReturn` + the
//!   button's index) back to that action's id → `AlertOutcome::Action`. A
//!   response outside that range (nothing this arm's own dismiss path
//!   produces today, since that path resolves before the handler ever
//!   fires — see below) falls back to `Dismissed` rather than being treated
//!   as an error.
//! - **[`super::dismiss`]** — takes the live entry down immediately
//!   (`AlertOutcome::Dismissed` sent at once, not deferred to the sheet
//!   completion, which has nothing left to resolve by the time it
//!   eventually fires) and closes the sheet with `NSWindow endSheet:`.
//! - **The presenting window closing** — observed via
//!   `NSWindowWillCloseNotification` on the ONE presenter observer object
//!   (mirroring `crates/frust-shell-macos/src/appkit_glue.rs`'s
//!   `ActivationObserver` `define_class!` shape) → `AlertOutcome::HostLost`.
//!
//! # One observer object per presentation
//!
//! [`FrustNativeAlertObserver`] is a `define_class!` object carrying its
//! presentation's generation. It observes only the presenting window's
//! close notification, is held strongly by the live entry for the
//! presentation's lifetime, and holds nothing itself, so there is no retain
//! cycle. When the entry is resolved, the alert and the observer are
//! **autoreleased** rather than released, because the resolving call is
//! usually running *inside* one of them (the sheet's own completion
//! handler, the observer's own notification callback) and must not free its
//! own caller.
//!
//! # A displaced presentation
//!
//! A caller that drops its [`super::Presentation`] frees the Busy slot but
//! not the sheet on screen, so a newer request can find the older alert
//! still live. `start` takes it down **first**, through the same
//! immediate-resolve path [`super::dismiss`] uses (its `Dismissed` outcome
//! goes to the dropped receiver and is discarded) — end the old sheet, then
//! discover the new presentation's anchor, then build and present it. This
//! order matters: while a sheet is attached, AppKit makes its panel the key
//! (and often the main) window, so discovering the anchor before ending the
//! old sheet risks anchoring the new alert on a panel about to close. See
//! `apple_host`'s module doc's *Host discovery* for the anchor side of this
//! (it never resolves to a sheet either, as a second line of defense).
//!
//! # Not observed
//!
//! `NSWindowWillCloseNotification` fires only when the presenting window
//! actually closes. A window merely ordered out (`orderOut:`, hidden but not
//! closed) leaves the sheet attached to a now-invisible window with no
//! callback this arm can see; the presentation then stays pending until
//! [`super::dismiss`] resolves it `Dismissed` or its caller drops the
//! future.
//!
//! # Command-Q termination
//!
//! When the user presses ⌘Q to quit the application while a sheet is
//! presented, the sheet's completion handler never fires — AppKit terminates
//! the process before the handler runs. As a result, `HostLost` is *not*
//! required for a clean quit: a caller that drops its [`super::Presentation`]
//! without waiting for an outcome, or lets a future go unresolved, will
//! still exit cleanly even when an alert is live on screen. To ensure
//! responsive quit behavior, do not require `HostLost` or `Dismissed` for
//! shutdown logic.
//!
//! # `unsafe`
//!
//! Four blocks, each with its own `SAFETY` comment: `NSObject`'s `init` on
//! the observer, reading the `NSWindowWillCloseNotification` extern static,
//! and the selector-based `addObserver:selector:name:object:`/
//! `removeObserver:` pair.

#[cfg(not(target_os = "macos"))]
compile_error!("the AppKit alert arm targets macOS only");

use std::panic::{AssertUnwindSafe, catch_unwind};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSModalResponse, NSWindow, NSWindowWillCloseNotification,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSString};

use super::apple_host::{
    LivePresentation, dismiss_live, install_live, on_main, presenting_anchor, take_live_any,
    take_live_as,
};
use super::{ActionRole, AlertHost, AlertOutcome, AlertSpec, PresentError, Sender};

/// The sheet's completion handler — AppKit's `void (^)(NSModalResponse)`.
type CompletionHandler = RcBlock<dyn Fn(NSModalResponse)>;

/// The macOS host: `super::AlertHost` over `NSAlert` presented as a window
/// sheet.
pub(crate) struct Host;

impl AlertHost for Host {
    fn show_alert(
        spec: AlertSpec,
        tx: Sender<AlertOutcome>,
        generation: u64,
    ) -> Result<(), PresentError> {
        on_main(move |mtm| start(mtm, spec, tx, generation));
        Ok(())
    }

    fn dismiss(generation: u64) {
        dismiss_live(generation);
    }
}

/// The main-queue half of a request: take down any displaced live entry
/// first (module doc's *A displaced presentation*), discover the presenting
/// window, build the alert, park it as the live entry, then present it.
fn start(mtm: MainThreadMarker, spec: AlertSpec, tx: Sender<AlertOutcome>, generation: u64) {
    if let Some(displaced) = take_live_any(mtm) {
        displaced.dismiss(mtm);
    }
    let Some(window) = presenting_anchor(mtm) else {
        tx.send(Err(PresentError::NoHost));
        return;
    };
    let live = LiveAlert::build(mtm, &spec, tx, generation, window);
    if let Some(unexpected) = install_live(mtm, generation, Box::new(live)) {
        // Nothing else can have claimed the slot between the take above and
        // here on this main-queue turn; dismiss defensively rather than
        // leaking a second live entry if that invariant ever changes.
        unexpected.dismiss(mtm);
    }
    present_live(mtm, generation);
}

/// Present the live entry for `generation`. A no-op when the entry is gone
/// (resolved — e.g. `HostLost` from a window that closed — while a
/// displaced alert was being taken down).
fn present_live(mtm: MainThreadMarker, generation: u64) {
    let Some(live) = take_live_as::<LiveAlert>(mtm, generation) else {
        return;
    };
    live.alert
        .beginSheetModalForWindow_completionHandler(&live.window, Some(&live.completion));
    // Re-parked under the same generation, which nothing else can have
    // claimed during this main-queue turn.
    if let Some(unexpected) = install_live(mtm, generation, live) {
        unexpected.dismiss(mtm);
    }
}

/// Run an AppKit callback's body without letting a panic unwind into
/// AppKit's frames (`docs/PLUGINS_CODE_STANDARDS.md`'s no-unwind-near-FFI
/// rule). The sender a panicking body held is dropped with it, which
/// resolves its presentation as a platform error rather than leaving it
/// pending.
fn guarded(which: &str, body: impl FnOnce()) {
    if catch_unwind(AssertUnwindSafe(body)).is_err() {
        log::error!("frust-native-widgets: the alert's {which} callback panicked");
    }
}

/// Resolve presentation `generation` with `outcome` if it is still the live
/// entry — the one path every AppKit callback this arm registers takes.
fn resolve(generation: u64, outcome: Result<AlertOutcome, PresentError>) {
    // Every callback this arm registers runs on the main thread; a `None`
    // here would be an AppKit contract break, answered by doing nothing.
    let Some(mtm) = MainThreadMarker::new() else {
        log::error!("frust-native-widgets: an alert callback arrived off the main thread");
        return;
    };
    if let Some(live) = take_live_as::<LiveAlert>(mtm, generation) {
        live.finish(outcome);
    }
}

/// Map a sheet completion's `NSModalResponse` back to the action it
/// represents — `NSAlertFirstButtonReturn` + the button's index, in the
/// order the buttons were added (module doc's *Mapping*). Anything outside
/// that range falls back to `Dismissed` (module doc's *Resolution*).
fn map_response(response: NSModalResponse, action_ids: &[String]) -> AlertOutcome {
    let offset = response - NSAlertFirstButtonReturn;
    usize::try_from(offset)
        .ok()
        .and_then(|index| action_ids.get(index))
        .map_or(AlertOutcome::Dismissed, |id| {
            AlertOutcome::Action(id.clone())
        })
}

/// Everything one presented alert keeps alive until its outcome.
struct LiveAlert {
    tx: Sender<AlertOutcome>,
    alert: Retained<NSAlert>,
    /// The window the sheet is attached to — what `dismiss` ends the sheet
    /// on, and what `observer` watches for `HostLost`.
    window: Retained<NSWindow>,
    observer: Retained<FrustNativeAlertObserver>,
    /// Retained by AppKit's own internal copy too, once
    /// `beginSheetModalForWindow:completionHandler:` runs; held here so the
    /// entry owns everything the presentation built.
    completion: CompletionHandler,
}

impl LiveAlert {
    fn build(
        mtm: MainThreadMarker,
        spec: &AlertSpec,
        tx: Sender<AlertOutcome>,
        generation: u64,
        window: Retained<NSWindow>,
    ) -> Self {
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(&spec.title));
        alert.setInformativeText(&NSString::from_str(&spec.message));

        let mut action_ids = Vec::with_capacity(spec.actions.len());
        for action in &spec.actions {
            let button = alert.addButtonWithTitle(&NSString::from_str(&action.label));
            match action.role {
                ActionRole::Default => {}
                // Module doc's *Escape and Return*: set explicitly rather
                // than relying on AppKit's own title-based match, since a
                // Cancel-role action may carry any label.
                ActionRole::Cancel => button.setKeyEquivalent(&NSString::from_str("\u{1b}")),
                ActionRole::Destructive => button.setHasDestructiveAction(true),
            }
            action_ids.push(action.id.clone());
        }

        let completion: CompletionHandler = RcBlock::new(move |response: NSModalResponse| {
            guarded("sheet completion", || {
                resolve(generation, Ok(map_response(response, &action_ids)));
            });
        });

        let observer = FrustNativeAlertObserver::new(mtm, generation);
        observer.observe_close(&window);

        Self {
            tx,
            alert,
            window,
            observer,
            completion,
        }
    }

    /// The terminal step: stop observing, deliver `outcome`, and let the
    /// AppKit objects go at the end of the current run-loop turn (module
    /// doc's *One observer object per presentation*).
    fn finish(self, outcome: Result<AlertOutcome, PresentError>) {
        let Self {
            tx,
            alert,
            observer,
            ..
        } = self;
        observer.stop_observing();
        tx.send(outcome);
        let _ = Retained::autorelease_ptr(alert);
        let _ = Retained::autorelease_ptr(observer);
    }
}

impl LivePresentation for LiveAlert {
    /// Resolve `Dismissed` at once (module doc's *Resolution*), then close
    /// the sheet — its eventual completion callback finds nothing left to
    /// resolve.
    fn dismiss(self: Box<Self>, _mtm: MainThreadMarker) {
        let window = Retained::clone(&self.window);
        let sheet = self.alert.window();
        self.finish(Ok(AlertOutcome::Dismissed));
        window.endSheet(&sheet);
    }
}

/// The per-presentation observer's state: whose presentation it answers
/// for.
struct ObserverIvars {
    generation: u64,
}

define_class!(
    // SAFETY:
    // - `NSObject` has no subclassing requirements.
    // - The ivar is a plain `u64` with no `Drop` impl, so the macro's
    //   generated `dealloc` has nothing extra to uphold.
    #[unsafe(super(NSObject))]
    // AppKit posts the notification on the main thread, and the arm builds
    // it there (`Self::alloc(mtm)`).
    #[thread_kind = MainThreadOnly]
    #[ivars = ObserverIvars]
    struct FrustNativeAlertObserver;

    unsafe impl NSObjectProtocol for FrustNativeAlertObserver {}

    impl FrustNativeAlertObserver {
        /// `NSWindowWillCloseNotification` for the presenting window →
        /// `HostLost`.
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            let generation = self.ivars().generation;
            guarded("window close", || {
                resolve(generation, Ok(AlertOutcome::HostLost));
            });
        }
    }
);

impl FrustNativeAlertObserver {
    fn new(mtm: MainThreadMarker, generation: u64) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ObserverIvars { generation });
        // SAFETY: `NSObject`'s designated initializer, called on a freshly
        // allocated instance whose ivars are already set (the idiom
        // `apple_alert::FrustNativeAlertDelegate::new` uses).
        unsafe { msg_send![super(this), init] }
    }

    /// Observe `window` closing (→ `HostLost`).
    fn observe_close(&self, window: &NSWindow) {
        let center = NSNotificationCenter::defaultCenter();
        // SAFETY: `NSWindowWillCloseNotification` is an AppKit constant
        // `NSString`, initialized before any Rust code runs.
        let name = unsafe { NSWindowWillCloseNotification };
        // SAFETY: `self` is a live object implementing `windowWillClose:`
        // (defined above, taking the one `NSNotification` argument the
        // selector-based API passes), `name` is a notification name and
        // `window` the object posting it. `stop_observing` removes this
        // observer before the live entry lets it go.
        unsafe {
            center.addObserver_selector_name_object(
                self,
                sel!(windowWillClose:),
                Some(name),
                Some(window),
            );
        }
    }

    /// Undo [`Self::observe_close`] — a no-op when it never ran.
    fn stop_observing(&self) {
        // SAFETY: removing an observer, registered or not, has no
        // precondition beyond `self` being a live object.
        unsafe { NSNotificationCenter::defaultCenter().removeObserver(self) };
    }
}
