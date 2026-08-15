//! The AppKit boundary: this crate's sanctioned-unsafe zone, and the only
//! module that talks to the Objective-C runtime directly.
//!
//! Everything else in the crate is safe Rust over `muda`'s and `winit`'s own
//! safe bindings; the one thing neither offers is a **reopen** signal — the
//! Dock-icon click that re-activates an app whose windows are all closed.
//! winit 0.30 surfaces no such event, and AppKit only reports it to
//! `NSApplication`'s delegate (`applicationShouldHandleReopen:`), which winit
//! owns and this crate must not displace. So the glue here observes the
//! *notification* instead — `NSApplicationDidBecomeActiveNotification`, posted
//! on the same activation — through an observer object of this crate's own,
//! registered on the default `NSNotificationCenter` beside winit's delegate
//! rather than in place of it.
//!
//! # The `unsafe` in this module
//!
//! Four sites, each with a `SAFETY:` note at its call site, mirroring the
//! audit discipline of `frust-shell-android`'s `jni_glue` and
//! `frust-shell-ios`'s `ffi_glue`:
//!
//! 1. `define_class!`'s `#[unsafe(super(NSObject))]` / `#[unsafe(method(…))]`
//!    — declaring the observer class and the selector AppKit will call.
//! 2. `msg_send![super(this), init]` — `NSObject`'s designated initializer,
//!    run once on a freshly allocated instance.
//! 3. `NSNotificationCenter::addObserver_selector_name_object` — untyped in
//!    `objc2` (the observer, selector and object are all unchecked), plus the
//!    read of the `NSApplicationDidBecomeActiveNotification` `extern` static
//!    that names the notification.
//! 4. `NSNotificationCenter::removeObserver` in `Drop` — the other half of
//!    (3), and the reason [`AppActivationObserver`] is a retained handle
//!    rather than a fire-and-forget registration: the notification center
//!    keeps an **unretained** reference, so the observer must outlive the
//!    registration and the registration must end before the observer does.
//!
//! # No unwind across the AppKit boundary
//!
//! The observer method is called by AppKit, so a panic escaping it would
//! unwind into an Objective-C frame — undefined behavior, not a bug (see
//! `docs/CODE_STANDARDS.md`). The body is wrapped in `catch_unwind`, the
//! `frust_shell_common::guard` contract applied locally (this crate does not
//! depend on `frust-shell-common`).
//!
//! # What this module deliberately does *not* do
//!
//! The application's activation policy is winit's: it sets
//! `NSApplicationActivationPolicy` itself while launching (overridable through
//! `EventLoopBuilderExtMacOS::with_activation_policy`), so a second, competing
//! `setActivationPolicy:` here would only be able to disagree with it.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::NSApplicationDidBecomeActiveNotification;
use objc2_foundation::{NSNotification, NSNotificationCenter};

use crate::lifecycle::Lifecycle;

define_class!(
    // SAFETY:
    // - `NSObject` imposes no subclassing requirements.
    // - `ActivationObserver` does not implement `Drop` (the handle that owns
    //   it does).
    #[unsafe(super(NSObject))]
    // AppKit posts the notification on the main thread, and the `Lifecycle`
    // this drives is the main thread's own.
    #[thread_kind = MainThreadOnly]
    #[ivars = Arc<Lifecycle>]
    struct ActivationObserver;

    impl ActivationObserver {
        /// `NSApplicationDidBecomeActiveNotification`'s callback: the app was
        /// re-activated, which is as close as AppKit's notification vocabulary
        /// gets to "reopen" (see the module docs).
        ///
        /// The selector is namespaced because it is added to a class of this
        /// crate's own — nothing else may claim the same name on it.
        #[unsafe(method(frustApplicationDidBecomeActive:))]
        fn did_become_active(&self, _notification: &NSNotification) {
            let lifecycle = Arc::clone(self.ivars());
            // A panic here would unwind into AppKit's dispatch (see the
            // module docs); swallow and log it instead.
            let result = catch_unwind(AssertUnwindSafe(move || lifecycle.on_app_activated()));
            if result.is_err() {
                log::error!(
                    "frust-shell-macos: panic while handling an app re-activation; the window \
                     may stay hidden"
                );
            }
        }
    }

    unsafe impl NSObjectProtocol for ActivationObserver {}
);

impl ActivationObserver {
    fn new(mtm: MainThreadMarker, lifecycle: Arc<Lifecycle>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(lifecycle);
        // SAFETY: `init` is `NSObject`'s designated initializer, sent once to
        // a freshly allocated instance whose ivars are already set.
        unsafe { msg_send![super(this), init] }
    }
}

/// A live registration for the app-activation ("reopen") notification, driving
/// [`Lifecycle::on_app_activated`] until it is dropped.
///
/// Dropping it unregisters — see the module docs for why that is not optional.
pub(crate) struct AppActivationObserver {
    observer: Retained<ActivationObserver>,
}

impl AppActivationObserver {
    /// Register on the default notification center, or `None` off the main
    /// thread (where AppKit may not be touched at all).
    ///
    /// The caller is `DesktopExtensions::on_window_created`, which the shared
    /// core runs on the winit event loop's thread — the main thread on macOS —
    /// so `None` means something is structurally wrong rather than that reopen
    /// is unavailable, and is logged as such.
    pub(crate) fn install(lifecycle: Arc<Lifecycle>) -> Option<Self> {
        let Some(mtm) = MainThreadMarker::new() else {
            log::error!(
                "frust-shell-macos: not on the main thread; Dock-click reopen will not work"
            );
            return None;
        };

        let observer = ActivationObserver::new(mtm, lifecycle);
        let center = NSNotificationCenter::defaultCenter();
        // SAFETY:
        // - `observer` is an instance of a class that implements the selector
        //   below, and it stays alive (retained by the returned handle) until
        //   `Drop` unregisters it — the center holds it unretained.
        // - `frustApplicationDidBecomeActive:` is declared on that class with
        //   the single-`NSNotification` signature AppKit invokes.
        // - `NSApplicationDidBecomeActiveNotification` is AppKit's own
        //   notification-name static, read only after AppKit is loaded (this
        //   runs from a live event loop).
        // - Passing `None` as the object observes the notification from any
        //   sender, which for an application-level notification is the only
        //   sender there is.
        unsafe {
            center.addObserver_selector_name_object(
                &observer,
                sel!(frustApplicationDidBecomeActive:),
                Some(NSApplicationDidBecomeActiveNotification),
                None,
            );
        }

        Some(Self { observer })
    }
}

impl Drop for AppActivationObserver {
    fn drop(&mut self) {
        let center = NSNotificationCenter::defaultCenter();
        // SAFETY: `observer` is the object registered in `install` and is
        // still alive here (its `Retained` is dropped after this body), so the
        // center is left holding no dangling reference.
        unsafe { center.removeObserver(&self.observer) };
    }
}

impl std::fmt::Debug for AppActivationObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppActivationObserver").finish()
    }
}
