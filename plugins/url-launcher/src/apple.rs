//! The iOS backend — `UIApplication.openURL(_:options:completionHandler:)`
//! via `objc2-ui-kit`.
//!
//! `#[cfg(target_os = "ios")]` only — **not** `target_vendor = "apple"`:
//! `UIApplication` is UIKit, which doesn't exist on macOS (see
//! [`crate::desktop`]'s module doc for the macOS opener instead). Matches
//! `frust-haptics`'s `apple.rs` rationale for the same `target_os`/
//! `target_vendor` split.
//!
//! # Main-thread dispatch
//!
//! `UIApplication` is UIKit's `MainThreadOnly` — both the `objc2-ui-kit`
//! binding (`#[thread_kind = MainThreadOnly]`, requiring a
//! [`MainThreadMarker`] to construct/call it) and Apple's own documentation
//! agree `UIApplication.shared`/`openURL:options:completionHandler:` must be
//! used from the main thread. Unlike [`crate::android`]'s `startActivity`
//! (which carries no such requirement and is called inline), this backend
//! cannot simply call through from whatever thread `open_external` runs
//! on — so [`open_external`] dispatches the entire lookup-and-open sequence
//! onto `dispatch_get_main_queue()` **asynchronously** (`DispatchQueue::exec_async`),
//! returning `Ok(())` immediately without waiting for it to run. This is
//! deliberate, not merely convenient: a *synchronous* main-thread bounce
//! (`exec_sync`, or `dispatch2::run_on_main`'s blocking fallback for a
//! non-main caller) would violate this crate's fire-and-forget,
//! never-block-the-caller contract if `open_external` is ever called from a
//! background thread — `exec_async` never blocks regardless of the calling
//! thread.
//!
//! `exec_async`'s closure runs *after* `open_external` has already
//! returned, so a caller cannot observe `openURL:options:completionHandler:`
//! failing (no handler for the URL, the app being suspended, …) — there is
//! nothing to report back on this platform's path (see the crate doc's
//! *Fire-and-forget* section).
//!
//! # `unsafe`
//!
//! One call: `objc2-ui-kit` 0.3.2 marks
//! `openURL:options:completionHandler:` `unsafe` (its binding's own doc:
//! "`options` generic should be of the correct type") — [`open_on_main`]
//! satisfies that by constructing the options dictionary with the exact
//! generic parameters the binding declares
//! (`NSDictionary<UIApplicationOpenExternalURLOptionsKey, AnyObject>`). The
//! plain, deprecated `openURL:` (`UIApplication::openURL`, safe but
//! deprecated since iOS 10) is not used instead — see the crate's
//! originating task for why the modern, completion-handler-carrying
//! selector is preferred despite requiring the one `unsafe` call.
//!
//! # No `canOpenURL:`
//!
//! `canOpenURL:` requires the calling app to declare every custom URL scheme
//! it wants to probe in its `Info.plist`'s `LSApplicationQueriesSchemes`
//! (iOS 9+), and this crate only ever hands it `http`/`https` schemes (see
//! `crate::url`'s validator) — Safari (or an equivalent default handler) is
//! guaranteed present for those on any real device, so the query would add
//! an `Info.plist` dependency for no discriminating value. This backend
//! calls `openURL:options:completionHandler:` unconditionally instead.

use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSDictionary, NSString, NSURL};
use objc2_ui_kit::{UIApplication, UIApplicationOpenExternalURLOptionsKey};

use crate::UrlLauncherError;

/// Dispatch the lookup-and-open sequence to the main queue and return
/// immediately (the module doc's *Main-thread dispatch*). `open_external`
/// always returns `Ok(())`: there is nothing left to observe once the async
/// dispatch is scheduled (see the crate doc's *Fire-and-forget* section).
pub(crate) fn open_external(url: &str) -> Result<(), UrlLauncherError> {
    let url = url.to_owned();
    DispatchQueue::main().exec_async(move || {
        // SAFETY: this closure is submitted to `dispatch_get_main_queue()`,
        // which always executes its blocks on the actual OS main thread
        // (module doc's *Main-thread dispatch*) — so constructing a
        // `MainThreadMarker` here without re-checking is sound.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        open_on_main(mtm, &url);
    });
    Ok(())
}

/// Runs on the main queue (see [`open_external`]). A `url` that fails
/// `NSURL::URLWithString` (malformed beyond what `crate::url::validate`
/// already checked) is silently dropped — there is no caller left to report
/// back to at this point.
fn open_on_main(mtm: MainThreadMarker, url: &str) {
    let Some(ns_url) = NSURL::URLWithString(&NSString::from_str(url)) else {
        return;
    };
    let app = UIApplication::sharedApplication(mtm);
    let options: Retained<NSDictionary<UIApplicationOpenExternalURLOptionsKey, AnyObject>> =
        NSDictionary::new();
    // SAFETY: the options dictionary is the empty, correctly-typed
    // `NSDictionary<UIApplicationOpenExternalURLOptionsKey, AnyObject>` the
    // binding's own doc names as its only safety obligation (module doc's
    // *`unsafe`* section); `ns_url` is a live `NSURL` retained for the
    // duration of this call; `completion` is `None`.
    unsafe {
        app.openURL_options_completionHandler(&ns_url, &options, None);
    }
}
