//! The Apple (iOS + macOS) presentation host: where a presentation goes, how
//! a request reaches the main thread, and what keeps a live presentation's
//! objects alive until its one outcome — the helpers both Apple arms build
//! on (iOS's alert arm is `super::apple_alert`; macOS's is
//! `super::appkit_alert`).
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
//!   visible window — but never a sheet: a window-modal sheet needs a window
//!   on screen to attach to, and while a sheet is presented AppKit makes its
//!   panel the key window (often the main window too), so a key or main
//!   window that `isSheet()` resolves through its `sheetParent()` instead —
//!   only when that parent is itself still visible — and the "first visible
//!   window" fallback considers a non-sheet window outright. The pure
//!   decision behind this is [`resolve_sheet`], unit-tested in this module.
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
    non_sheet_anchor(app.keyWindow())
        .or_else(|| non_sheet_anchor(app.mainWindow()))
        .or_else(|| {
            app.windows()
                .iter()
                .find(|window| window.isVisible() && !window.isSheet())
        })
}

/// Resolve `window` to a usable, non-sheet anchor — itself when it is not a
/// sheet, else its sheet parent when that parent is still visible, else
/// nothing (this tier has no candidate; the caller tries the next one). The
/// pure decision is [`resolve_sheet`].
#[cfg(target_os = "macos")]
fn non_sheet_anchor(window: Option<Retained<NSWindow>>) -> Option<Retained<NSWindow>> {
    let window = window?;
    let is_sheet = window.isSheet();
    let parent = is_sheet.then(|| window.sheetParent()).flatten();
    match resolve_sheet(is_sheet, parent.as_ref().map(|p| p.isVisible())) {
        SheetResolution::Itself => Some(window),
        SheetResolution::Parent => parent,
        SheetResolution::None => None,
    }
}

/// The never-a-sheet resolution rule itself (module doc's *Host discovery*),
/// pulled out as a pure function of two facts so it runs under test without
/// an `NSWindow`: a non-sheet is used as-is; a sheet is used only through a
/// parent that is itself visible; anything else has no usable candidate.
#[cfg(target_os = "macos")]
fn resolve_sheet(is_sheet: bool, parent_is_visible: Option<bool>) -> SheetResolution {
    if !is_sheet {
        SheetResolution::Itself
    } else if parent_is_visible == Some(true) {
        SheetResolution::Parent
    } else {
        SheetResolution::None
    }
}

/// [`resolve_sheet`]'s answer.
#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SheetResolution {
    /// Not a sheet — use the window itself.
    Itself,
    /// A sheet whose parent is visible — use the parent instead.
    Parent,
    /// A sheet with no usable parent (none, or not visible) — this tier has
    /// no candidate.
    None,
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

/// What runs once a displaced presentation's dismissal has finished — the
/// successor's own presentation step. See [`LivePresentation::dismiss`].
pub(crate) type AfterDismiss = Box<dyn FnOnce(MainThreadMarker)>;

/// Everything one live presentation keeps alive until its outcome — see the
/// module doc's *The live-presentation guard*. Implemented by each
/// presentation arm over its own state (controller, blocks, delegate,
/// sender).
pub(crate) trait LivePresentation: Any {
    /// Take the platform UI down and resolve the presentation's dismissed
    /// outcome. Runs on the main thread, with the entry already out of
    /// [`LIVE`].
    ///
    /// `then`, when given, is a displacing presentation's own presentation
    /// step. An implementation must never park itself back under its old
    /// generation while `then` is `Some` — the entry is already gone and a
    /// newer one is about to take its place — and must run `then` exactly
    /// once: after its platform UI has finished dismissing, immediately when
    /// nothing was ever on screen, or — when the platform is already
    /// mid-dismissal for some other reason and a successor is waiting
    /// ([`DisplacedAction::ResolveAndContinue`]) — at once as well, without
    /// waiting for that other dismissal's completion, so the successor may
    /// be refused [`NoHost`](super::PresentError::NoHost) mid-transition
    /// (`apple_alert.rs`'s and `apple_sheet.rs`'s `take_down`). `then` is
    /// `None` for a genuinely programmatic dismissal ([`dismiss_live`]),
    /// where an implementation may still re-park itself if the platform is
    /// already dismissing it for some other reason — a race this call lost.
    /// See [`displaced_action`] for the shared decision an implementation's
    /// `take_down` typically makes once its platform UI is confirmed on
    /// screen.
    fn dismiss(self: Box<Self>, mtm: MainThreadMarker, then: Option<AfterDismiss>);
}

/// [`LivePresentation::dismiss`]'s three-way decision once a presentation is
/// confirmed on screen — see [`displaced_action`], the pure function behind
/// it.
///
/// Only the iOS arms' `take_down` (`apple_alert`, `apple_sheet`) call this
/// outside its own tests today — the macOS arm's sheet ends synchronously,
/// so it never needs the decision (`appkit_alert`'s module doc's *A
/// displaced presentation*). Kept here, rather than duplicated per iOS arm,
/// so the pure decision runs under `cargo test` on any host.
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisplacedAction {
    /// The platform is already mid-dismissal for some other reason and
    /// nothing is waiting on the outcome: re-park under the same
    /// generation and let that other dismissal resolve it — a programmatic
    /// dismiss that lost this race.
    Repark,
    /// Either the platform is already mid-dismissal with a successor
    /// waiting (a second dismissal call would be ignored, and its own
    /// completion never run), or there is nothing left to wait for at all:
    /// resolve the outcome now and hand off to the continuation at once.
    ResolveAndContinue,
    /// On screen and not already leaving: ask the platform to dismiss it,
    /// resolve from that dismissal's own completion, and hand off to the
    /// continuation from there — never racing the animation.
    CloseThenContinue,
}

/// The decision behind [`DisplacedAction`], pulled out as a pure function of
/// two facts so it runs under test without a `UIViewController`/`NSAlert`:
/// whether the platform is already mid-dismissal for some other reason
/// (`is_being_dismissed`, asked before this call touches anything), and
/// whether a successor presentation is waiting on the outcome
/// (`has_continuation`, i.e. `then.is_some()`). A presentation confirmed
/// **not** on screen at all answers `ResolveAndContinue` directly, without
/// calling this — see each arm's `take_down`.
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub(crate) fn displaced_action(
    is_being_dismissed: bool,
    has_continuation: bool,
) -> DisplacedAction {
    match (is_being_dismissed, has_continuation) {
        (true, false) => DisplacedAction::Repark,
        (true, true) => DisplacedAction::ResolveAndContinue,
        (false, _) => DisplacedAction::CloseThenContinue,
    }
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

/// Take the live entry out unconditionally, regardless of generation —
/// unlike [`take_live`]/[`take_live_as`], which only ever take *their own*
/// presentation. For an arm that must take a displaced presentation down
/// **before** it can be mistaken for something else: macOS's `start` calls
/// this ahead of [`presenting_anchor`], since an attached sheet becomes the
/// key (and often main) window while it is up (module doc's *Host
/// discovery*). Only the macOS arm needs this; the iOS arm re-discovers its
/// anchor from the displaced alert's own dismissal completion instead (see
/// `apple_alert`'s module doc's *A displaced presentation*).
#[cfg(target_os = "macos")]
pub(crate) fn take_live_any(_mtm: MainThreadMarker) -> Option<Box<dyn LivePresentation>> {
    LIVE.with(|live| live.borrow_mut().take().map(|entry| entry.presentation))
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
            presentation.dismiss(mtm, None);
        }
    });
}

/// [`resolve_sheet`]'s macOS-only decision, isolated from `NSWindow` — see
/// the module doc's *Host discovery*.
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{SheetResolution, resolve_sheet};

    #[test]
    fn a_non_sheet_resolves_to_itself_whatever_the_parent_fact_says() {
        assert_eq!(resolve_sheet(false, None), SheetResolution::Itself);
        assert_eq!(resolve_sheet(false, Some(false)), SheetResolution::Itself);
        assert_eq!(resolve_sheet(false, Some(true)), SheetResolution::Itself);
    }

    #[test]
    fn a_sheet_resolves_to_its_visible_parent() {
        assert_eq!(resolve_sheet(true, Some(true)), SheetResolution::Parent);
    }

    #[test]
    fn a_sheet_with_no_visible_parent_has_no_candidate() {
        assert_eq!(resolve_sheet(true, None), SheetResolution::None);
        assert_eq!(resolve_sheet(true, Some(false)), SheetResolution::None);
    }
}

/// [`displaced_action`]'s decision, tested independent of any platform
/// object — see [`LivePresentation::dismiss`]'s doc for what each answer
/// means to a `take_down`.
#[cfg(test)]
mod displaced_action_tests {
    use super::{DisplacedAction, displaced_action};

    #[test]
    fn already_dismissing_with_nothing_waiting_reparks() {
        assert_eq!(displaced_action(true, false), DisplacedAction::Repark);
    }

    #[test]
    fn already_dismissing_with_a_continuation_resolves_and_continues_at_once() {
        assert_eq!(
            displaced_action(true, true),
            DisplacedAction::ResolveAndContinue
        );
    }

    #[test]
    fn not_yet_dismissing_always_closes_then_continues() {
        assert_eq!(
            displaced_action(false, false),
            DisplacedAction::CloseThenContinue
        );
        assert_eq!(
            displaced_action(false, true),
            DisplacedAction::CloseThenContinue
        );
    }
}
