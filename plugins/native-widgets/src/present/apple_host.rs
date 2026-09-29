//! The Apple (iOS + macOS) presentation host: where a presentation goes, how
//! a request reaches the main thread, and what keeps a live presentation's
//! objects alive until its one outcome — the helpers both Apple arms build
//! on (iOS's alert arm is `super::apple_alert`), plus macOS's placeholder
//! `Host` until its own alert arm lands.
//!
//! # Host discovery
//!
//! [`presenting_anchor`] answers what a presentation is presented *from*:
//!
//! - **iOS** — the window of a foreground-active window scene (its key
//!   window, else its first window; any scene's window as a fallback), then
//!   that window's `rootViewController`, then the `presentedViewController`
//!   chain walked to the topmost controller not already being dismissed —
//!   UIKit refuses a presentation from a controller that is itself presenting
//!   something. The scene walk is `frust-auth-session`'s `anchor_window`.
//! - **macOS** — the key window, else the main window, else the first
//!   visible window: a window-modal sheet needs a window on screen.
//!
//! `None` means [`PresentError::NoHost`](super::PresentError::NoHost): the
//! app has no UI to present over yet (a request during launch, before the
//! first scene/window exists).
//!
//! # Main-thread dispatch
//!
//! A request arrives on whatever thread polled or called it, and every
//! UIKit/AppKit presentation call is main-thread-only. [`on_main`] bounces
//! the work onto `dispatch_get_main_queue()` and returns at once — the
//! never-block-the-caller shape of `frust-auth-session`'s Apple backend —
//! so a host's `Ok(())` means "handed to the main queue", and a failure
//! discovered there (no host) resolves the presentation's sender instead.
//! The work always runs on a later main-queue turn, even when the request
//! came from the main thread, so an arm never re-enters its caller.
//!
//! # The live-presentation guard
//!
//! A presented alert or sheet is retained by UIKit/AppKit only as long as it
//! is on screen, and its handlers are blocks this crate built — so the arm
//! parks everything the presentation needs (its controller, its blocks, its
//! delegate, its sender) in [`LIVE`], one entry tagged with the
//! presentation's generation, until the terminal outcome. Every later touch
//! — a handler firing, [`AlertHost::dismiss`](super::AlertHost::dismiss),
//! the host going away — takes the entry back **by generation**: a stale
//! callback for a finished presentation finds nothing and does nothing, and
//! can never resolve the one live now. `LIVE` is a `thread_local!` because
//! `Retained`/`RcBlock` are `!Send` and everything touching it already runs
//! on the main thread (every accessor takes a [`MainThreadMarker`] to say
//! so).
//!
//! # `unsafe`
//!
//! One block: [`on_main`]'s `MainThreadMarker::new_unchecked()`, proving that
//! a `dispatch_get_main_queue()` callback runs on the real main thread.

#[cfg(not(any(target_os = "ios", target_os = "macos")))]
compile_error!("the presentation Apple host targets iOS and macOS only");

use std::any::Any;
use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};

use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2::rc::Retained;

#[cfg(target_os = "macos")]
use objc2_app_kit::{NSApplication, NSWindow};
#[cfg(target_os = "ios")]
use objc2_ui_kit::{
    UIApplication, UISceneActivationState, UIViewController, UIWindow, UIWindowScene,
};

#[cfg(target_os = "macos")]
use super::{AlertHost, AlertOutcome, AlertSpec, PresentError, Sender};

/// What a presentation is presented from: the topmost view controller on
/// iOS, a window (for a window-modal sheet) on macOS.
#[cfg(target_os = "ios")]
pub(crate) type PresentingAnchor = UIViewController;
/// What a presentation is presented from: the topmost view controller on
/// iOS, a window (for a window-modal sheet) on macOS.
#[cfg(target_os = "macos")]
pub(crate) type PresentingAnchor = NSWindow;

/// Run `f` on the main thread, on a later main-queue turn — see the module
/// doc's *Main-thread dispatch*.
///
/// `f` runs under `catch_unwind`: it runs inside libdispatch's frames, and
/// no unwind may cross back into them. A panic is logged; any sender `f`
/// held is dropped with it, which resolves that presentation as a
/// [`PresentError::Platform`](super::PresentError::Platform) error rather
/// than leaving it pending.
pub(crate) fn on_main(f: impl FnOnce(MainThreadMarker) + Send + 'static) {
    DispatchQueue::main().exec_async(move || {
        // SAFETY: this closure is submitted to `dispatch_get_main_queue()`,
        // which always executes its blocks on the process's main thread, so
        // constructing a `MainThreadMarker` here without re-checking is
        // sound (`frust-auth-session`'s Apple backend makes the same claim).
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        if catch_unwind(AssertUnwindSafe(|| f(mtm))).is_err() {
            log::error!(
                "frust-native-widgets: a presentation step panicked on the main queue; its \
                 presentation resolves as a platform error"
            );
        }
    });
}

/// The controller to present from — see the module doc's *Host discovery*.
#[cfg(target_os = "ios")]
pub(crate) fn presenting_anchor(mtm: MainThreadMarker) -> Option<Retained<PresentingAnchor>> {
    let mut top = anchor_window(mtm)?.rootViewController()?;
    while let Some(next) = top.presentedViewController() {
        if next.isBeingDismissed() {
            break;
        }
        top = next;
    }
    Some(top)
}

/// The window to present from — see the module doc's *Host discovery*.
#[cfg(target_os = "macos")]
pub(crate) fn presenting_anchor(mtm: MainThreadMarker) -> Option<Retained<PresentingAnchor>> {
    let app = NSApplication::sharedApplication(mtm);
    app.keyWindow()
        .or_else(|| app.mainWindow())
        .or_else(|| app.windows().iter().find(|window| window.isVisible()))
}

/// The window of a foreground-active window scene (key window first),
/// falling back to any window scene's window. `None` — no window scene, or
/// none with a window — means the app has no UI to present over yet.
#[cfg(target_os = "ios")]
fn anchor_window(mtm: MainThreadMarker) -> Option<Retained<UIWindow>> {
    let scenes = UIApplication::sharedApplication(mtm).connectedScenes();

    let mut fallback = None;
    for scene in scenes.iter() {
        let Some(window_scene) = scene.downcast_ref::<UIWindowScene>() else {
            continue;
        };
        let Some(window) = window_scene
            .keyWindow()
            .or_else(|| window_scene.windows().firstObject())
        else {
            continue;
        };

        if scene.activationState() == UISceneActivationState::ForegroundActive {
            return Some(window);
        }
        fallback = fallback.or(Some(window));
    }

    fallback
}

/// Everything one live presentation keeps alive until its outcome — see the
/// module doc's *The live-presentation guard*. Implemented by each
/// presentation arm over its own state (controller, blocks, delegate,
/// sender).
pub(crate) trait LivePresentation: Any {
    /// Take the platform UI down programmatically and resolve the
    /// presentation's dismissed outcome. Runs on the main thread, with the
    /// entry already out of [`LIVE`].
    fn dismiss(self: Box<Self>, mtm: MainThreadMarker);
}

/// One live presentation, tagged with its generation.
struct LiveEntry {
    generation: u64,
    presentation: Box<dyn LivePresentation>,
}

thread_local! {
    /// The one live presentation, if any. Main thread only.
    static LIVE: RefCell<Option<LiveEntry>> = const { RefCell::new(None) };
}

/// Park `presentation` as the live entry for `generation`, answering the
/// entry it displaced, if any — one whose caller dropped the future (which
/// frees the Busy slot, not the platform UI). The arm decides what to do
/// with a displaced entry (typically [`LivePresentation::dismiss`] it, whose
/// outcome then has no receiver and is discarded). Returned rather than
/// dropped here so no arm code runs inside the `LIVE` borrow.
// Consumed by the iOS alert arm; macOS has no arm on it yet.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) fn install_live(
    _mtm: MainThreadMarker,
    generation: u64,
    presentation: Box<dyn LivePresentation>,
) -> Option<Box<dyn LivePresentation>> {
    LIVE.with(|live| {
        live.borrow_mut()
            .replace(LiveEntry {
                generation,
                presentation,
            })
            .map(|displaced| displaced.presentation)
    })
}

/// Take the live entry out, but only if it is still `generation`'s.
pub(crate) fn take_live(
    _mtm: MainThreadMarker,
    generation: u64,
) -> Option<Box<dyn LivePresentation>> {
    LIVE.with(|live| {
        let mut slot = live.borrow_mut();
        match slot.as_ref() {
            Some(entry) if entry.generation == generation => {
                slot.take().map(|entry| entry.presentation)
            }
            _ => None,
        }
    })
}

/// [`take_live`] for an arm that needs its own concrete state back (to
/// resolve an action through its sender): taken only if the entry is
/// `generation`'s **and** of type `P`, so a mismatch leaves it in place.
// Consumed by the iOS alert arm; macOS has no arm on it yet.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) fn take_live_as<P: LivePresentation>(
    _mtm: MainThreadMarker,
    generation: u64,
) -> Option<Box<P>> {
    LIVE.with(|live| {
        let mut slot = live.borrow_mut();
        let matches = slot.as_ref().is_some_and(|entry| {
            entry.generation == generation && (&*entry.presentation as &dyn Any).is::<P>()
        });
        if !matches {
            return None;
        }
        let presentation: Box<dyn Any> = slot.take()?.presentation;
        presentation.downcast::<P>().ok()
    })
}

/// Programmatically dismiss presentation `generation` on the main thread: a
/// no-op when it is no longer the live entry (already answered, or never
/// parked).
pub(crate) fn dismiss_live(generation: u64) {
    on_main(move |mtm| {
        if let Some(presentation) = take_live(mtm, generation) {
            presentation.dismiss(mtm);
        }
    });
}

/// The macOS placeholder host (iOS selects `super::apple_alert`'s). Until
/// macOS's alert arm is built, a request still runs host discovery on the
/// main thread — resolving [`PresentError::NoHost`] when there is nothing to
/// present over — and answers [`PresentError::Unsupported`] otherwise.
#[cfg(target_os = "macos")]
pub(crate) struct Host;

#[cfg(target_os = "macos")]
impl AlertHost for Host {
    fn show_alert(
        _spec: AlertSpec,
        tx: Sender<AlertOutcome>,
        _generation: u64,
    ) -> Result<(), PresentError> {
        on_main(move |mtm| {
            let outcome = match presenting_anchor(mtm) {
                None => Err(PresentError::NoHost),
                Some(_) => {
                    log::warn!("frust-native-widgets: no native alert is built for macOS");
                    Err(PresentError::Unsupported)
                }
            };
            tx.send(outcome);
        });
        Ok(())
    }

    fn dismiss(generation: u64) {
        dismiss_live(generation);
    }
}
