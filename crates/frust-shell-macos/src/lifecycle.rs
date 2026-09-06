//! macOS quit/reopen semantics: what a window-close request means, and what an
//! app re-activation ("reopen") does about a window this shell hid earlier.
//!
//! The macOS convention is that closing the last window does **not** quit the
//! app — the app stays running with no window until ⌘Q, and clicking its Dock
//! icon brings the window back. `DesktopConfig::quit_on_last_window_closed`
//! selects between that and the cross-platform exit-on-close behavior the
//! shared core has always had.
//!
//! # Decision, then effect
//!
//! Both transitions are split in two: a pure [`decide_close`]/[`decide_reopen`]
//! that answers *what should happen* from plain booleans, and a [`Lifecycle`]
//! method that applies the answer to a window. That split is what makes the
//! semantics testable on a build host with no window system — a
//! `winit::Window` cannot be constructed without a live event loop, so the
//! window is held behind the [`WindowVisibility`] trait and a test drives the
//! whole state machine against a fake.
//!
//! # Shared with the AppKit glue
//!
//! [`Lifecycle`] is held behind an `Arc` because two callers drive it: the
//! extension's `on_close_requested` hook, and the app-activation observer in
//! [`crate::appkit_glue`] (winit 0.30 surfaces no reopen event of its own).
//! Both run on the main thread; the interior `Mutex` exists to make the shared
//! handle sound, not to arbitrate a real cross-thread race, and it is taken
//! poison-tolerantly since the observer's caller is AppKit itself.

use std::sync::{Arc, Mutex, PoisonError};

use winit::window::Window;

/// The window operations this module performs, behind a trait so the lifecycle
/// state machine is testable without a live event loop (see the module docs).
pub(crate) trait WindowVisibility: Send + Sync {
    /// Show or hide the window (macOS: order it in / order it out).
    fn set_visible(&self, visible: bool);
    /// Ask for a redraw, so a re-shown window paints its current frame rather
    /// than whatever the compositor last held for it.
    fn request_redraw(&self);
}

impl WindowVisibility for Window {
    fn set_visible(&self, visible: bool) {
        Window::set_visible(self, visible);
    }

    fn request_redraw(&self) {
        Window::request_redraw(self);
    }
}

/// What a `WindowEvent::CloseRequested` should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseDecision {
    /// Exit the event loop — the shared core's own shutdown path (`run_app`
    /// returns, the frame executor drops, the render thread joins and persists
    /// its pipeline cache).
    Quit,
    /// Hide the window and keep the loop running (the macOS convention).
    Hide,
}

/// What an app re-activation should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReopenDecision {
    /// Re-show the window this shell hid on an earlier close request.
    ShowWindow,
    /// Nothing to do — the app was activated with its window already up (the
    /// common case, including the activation at launch).
    Ignore,
}

/// Decide a close request from configuration and whether a window is actually
/// retained.
///
/// `has_window` is not a formality: refusing a close request the shell cannot
/// act on would leave an app with a visible window nothing can close, so a
/// missing window falls back to the historical exit rather than to the macOS
/// convention.
pub(crate) fn decide_close(quit_on_last_window_closed: bool, has_window: bool) -> CloseDecision {
    if quit_on_last_window_closed || !has_window {
        CloseDecision::Quit
    } else {
        CloseDecision::Hide
    }
}

/// Decide an app re-activation: re-show only a window *this shell* hid.
///
/// An activation with nothing hidden is the overwhelmingly common case (⌘-tab,
/// a click on the window, the activation at launch) and must not touch the
/// window — forcing `set_visible(true)` there would fight a window the app or
/// the user minimized on purpose.
pub(crate) fn decide_reopen(hidden: bool, has_window: bool) -> ReopenDecision {
    if hidden && has_window {
        ReopenDecision::ShowWindow
    } else {
        ReopenDecision::Ignore
    }
}

/// The quit/reopen state machine: the configured quit policy plus the window
/// to act on and whether this shell currently has it hidden.
#[derive(Debug)]
pub(crate) struct Lifecycle {
    quit_on_last_window_closed: bool,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    window: Option<Arc<dyn WindowVisibility>>,
    /// Set only by this shell's own hide — never by a user-initiated
    /// minimize/hide, which this shell cannot observe and must not undo.
    hidden: bool,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inner")
            .field("window", &self.window.is_some())
            .field("hidden", &self.hidden)
            .finish()
    }
}

impl Lifecycle {
    /// A lifecycle with no window yet — `set_window` supplies it once
    /// `DesktopExtensions::on_window_created` fires.
    pub(crate) fn new(quit_on_last_window_closed: bool) -> Self {
        Self {
            quit_on_last_window_closed,
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Retain the window every later transition acts on.
    pub(crate) fn set_window(&self, window: Arc<dyn WindowVisibility>) {
        self.lock().window = Some(window);
    }

    /// Whether this shell currently has the window hidden.
    ///
    /// Test-only: the transitions below own the flag and no shipping path
    /// reads it back (both of them decide from the `Inner` they already hold),
    /// so on a macOS build — where the module carries no `allow(dead_code)` —
    /// an ungated accessor would be exactly the dead code that scoping is
    /// meant to keep reportable.
    #[cfg(test)]
    pub(crate) fn is_hidden(&self) -> bool {
        self.lock().hidden
    }

    /// Apply a close request: hide the window and keep running, or report that
    /// the shell should exit.
    pub(crate) fn on_close_requested(&self) -> CloseDecision {
        let mut inner = self.lock();
        let decision = decide_close(self.quit_on_last_window_closed, inner.window.is_some());
        if decision == CloseDecision::Hide
            && let Some(window) = inner.window.clone()
        {
            window.set_visible(false);
            inner.hidden = true;
        }
        decision
    }

    /// Apply an app re-activation: re-show a window this shell hid.
    pub(crate) fn on_app_activated(&self) -> ReopenDecision {
        let mut inner = self.lock();
        let decision = decide_reopen(inner.hidden, inner.window.is_some());
        if decision == ReopenDecision::ShowWindow
            && let Some(window) = inner.window.clone()
        {
            window.set_visible(true);
            // A window ordered back in has no pending redraw of its own: the
            // shared core is dirty-driven (`ControlFlow::Wait`), so without
            // this the re-shown window can sit on a stale frame until some
            // unrelated event wakes the loop.
            window.request_redraw();
            inner.hidden = false;
        }
        decision
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A poisoned lock means a panic already unwound through here; the
        // state behind it is two plain fields, so recovering it is strictly
        // better than panicking again — this is reached from an AppKit
        // callback, where an unwind would be undefined behavior.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Records the visibility calls the lifecycle makes, standing in for the
    /// `winit::Window` a host test cannot construct.
    #[derive(Debug, Default)]
    struct FakeWindow {
        calls: Mutex<Vec<String>>,
    }

    impl FakeWindow {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().expect("test mutex").clone()
        }
    }

    impl WindowVisibility for FakeWindow {
        fn set_visible(&self, visible: bool) {
            self.calls
                .lock()
                .expect("test mutex")
                .push(format!("set_visible:{visible}"));
        }

        fn request_redraw(&self) {
            self.calls
                .lock()
                .expect("test mutex")
                .push("request_redraw".to_string());
        }
    }

    fn with_window(quit_on_last_window_closed: bool) -> (Lifecycle, Arc<FakeWindow>) {
        let lifecycle = Lifecycle::new(quit_on_last_window_closed);
        let window = Arc::new(FakeWindow::default());
        lifecycle.set_window(window.clone());
        (lifecycle, window)
    }

    // --- the pure decisions ---

    #[test]
    fn quit_on_last_window_closed_always_exits() {
        assert_eq!(decide_close(true, true), CloseDecision::Quit);
        assert_eq!(decide_close(true, false), CloseDecision::Quit);
    }

    #[test]
    fn the_macos_convention_hides_only_when_a_window_is_retained() {
        assert_eq!(decide_close(false, true), CloseDecision::Hide);
        // Nothing to hide: refusing the close would strand the app.
        assert_eq!(decide_close(false, false), CloseDecision::Quit);
    }

    #[test]
    fn reopen_shows_only_a_window_this_shell_hid() {
        assert_eq!(decide_reopen(true, true), ReopenDecision::ShowWindow);
        assert_eq!(decide_reopen(false, true), ReopenDecision::Ignore);
        assert_eq!(decide_reopen(true, false), ReopenDecision::Ignore);
        assert_eq!(decide_reopen(false, false), ReopenDecision::Ignore);
    }

    // --- the state machine ---

    #[test]
    fn a_close_request_under_the_default_policy_exits_and_touches_nothing() {
        let (lifecycle, window) = with_window(true);
        assert_eq!(lifecycle.on_close_requested(), CloseDecision::Quit);
        assert!(window.calls().is_empty());
        assert!(!lifecycle.is_hidden());
    }

    #[test]
    fn a_close_request_under_the_macos_policy_hides_the_window() {
        let (lifecycle, window) = with_window(false);
        assert_eq!(lifecycle.on_close_requested(), CloseDecision::Hide);
        assert_eq!(window.calls(), vec!["set_visible:false"]);
        assert!(lifecycle.is_hidden());
    }

    #[test]
    fn a_reopen_after_a_hide_re_shows_and_repaints_the_window() {
        let (lifecycle, window) = with_window(false);
        lifecycle.on_close_requested();
        assert_eq!(lifecycle.on_app_activated(), ReopenDecision::ShowWindow);
        assert_eq!(
            window.calls(),
            vec!["set_visible:false", "set_visible:true", "request_redraw"]
        );
        assert!(!lifecycle.is_hidden());
    }

    #[test]
    fn an_activation_with_nothing_hidden_leaves_the_window_alone() {
        // Every ⌘-tab back into a normal app takes this path — a window the
        // user minimized must not be forced back up.
        let (lifecycle, window) = with_window(false);
        assert_eq!(lifecycle.on_app_activated(), ReopenDecision::Ignore);
        assert!(window.calls().is_empty());
    }

    #[test]
    fn a_second_activation_after_a_reopen_is_a_no_op() {
        let (lifecycle, window) = with_window(false);
        lifecycle.on_close_requested();
        lifecycle.on_app_activated();
        assert_eq!(lifecycle.on_app_activated(), ReopenDecision::Ignore);
        assert_eq!(
            window.calls(),
            vec!["set_visible:false", "set_visible:true", "request_redraw"]
        );
    }

    #[test]
    fn hide_and_reopen_cycle_repeatedly() {
        let (lifecycle, window) = with_window(false);
        for _ in 0..3 {
            assert_eq!(lifecycle.on_close_requested(), CloseDecision::Hide);
            assert_eq!(lifecycle.on_app_activated(), ReopenDecision::ShowWindow);
        }
        assert_eq!(window.calls().len(), 9);
        assert!(!lifecycle.is_hidden());
    }

    #[test]
    fn a_lifecycle_with_no_window_exits_and_ignores_activation() {
        let lifecycle = Lifecycle::new(false);
        assert_eq!(lifecycle.on_close_requested(), CloseDecision::Quit);
        assert_eq!(lifecycle.on_app_activated(), ReopenDecision::Ignore);
        assert!(!lifecycle.is_hidden());
    }
}
