//! Titlebar theming: keeping the window's non-client area on the same
//! brightness as the app's own resolved theme.
//!
//! winit 0.30 themes the Windows titlebar natively (`Window::set_theme`, which
//! drives the DWM immersive-dark-mode attribute for us), so this module holds
//! no FFI at all — only the decision of *when* to call it.
//!
//! # Why there is still a latch
//!
//! [`TitlebarTheme`] used to compare the newly wanted brightness against the
//! last one it had *applied*, and dropped a repeat as a no-op. That was wrong:
//! it assumed re-applying the same brightness twice could only ever be
//! redundant, but on Windows it can be the only way to correct a titlebar the
//! OS itself changed out from under the app (see "The residual gap" below).
//! Round-1 removes that comparison — [`TitlebarTheme::take_pending`] now
//! re-issues `Window::set_theme` for *every* wanted brightness a caller hands
//! it, unconditionally.
//!
//! That is safe from spam only because the caller side is already
//! change-gated: [`take_pending`](TitlebarTheme::take_pending) is driven off
//! [`DesktopExtensions::on_theme_brightness_changed`](frust_shell_desktop::extensions::DesktopExtensions::on_theme_brightness_changed),
//! and `frust-shell-desktop`'s `ShellHandler::apply_theme` calls that hook
//! only when its own `brightness_change_to_notify(self.brightness_notified,
//! self.theme.brightness)` reports an actual transition — a plain `!=`
//! comparison against the last *notified* value, gating every call site that
//! feeds `apply_theme` (the initial seed, `WindowEvent::ThemeChanged`, and the
//! per-frame app-override poll). So this module is never asked to re-theme a
//! window at an unchanged brightness merely because a frame happened; it is
//! only ever asked when the shared core itself believes the brightness moved.
//! `take_pending` trusts that and re-applies every time, which is what closes
//! the app-brightness-change repro this round targets.
//!
//! What the latch that remains is *for*: the window is not guaranteed to
//! exist yet when the first brightness resolves. `wanted` alone carries that
//! — [`take_pending`](TitlebarTheme::take_pending) is simply never called
//! while `WindowsExtensions` holds no window (see `sync_titlebar_theme` in
//! `lib.rs`), so a resolution that arrives first coalesces to whatever is
//! still `wanted` when the window turns up, instead of being lost. Nothing
//! more elaborate than `Option::take` is needed for that.
//!
//! # The residual gap this round does not close
//!
//! winit 0.30.13's `Window::set_theme` never writes
//! `window_state.preferred_theme` (a winit bug, not a Windows one). Its own
//! `WM_SETTINGCHANGE` handler branches on that field being `None` to mean "no
//! app override, follow the system" — which, because of the bug, is true even
//! right after this crate has called `set_theme` — so any `WM_SETTINGCHANGE`
//! (theme-related or not) can cause winit to silently re-apply the *system*
//! theme to the titlebar, overwriting what this crate forced, without firing
//! `WindowEvent::ThemeChanged` in every case.
//!
//! Re-applying on every core-signaled brightness change (above) closes the
//! path where the app's own resolved brightness later moves to a different
//! value and this crate gets to re-assert it. It does **not** close the path
//! where the brightness the app wants has not changed at all — most notably
//! while an app-forced override is active, where `apply_theme`'s
//! "override-wins" rule deliberately holds `self.theme.brightness` steady
//! across a platform `ThemeChanged`, so the edge-gate never re-fires the hook
//! and this module is never told anything happened. Nothing in this module
//! can detect that drift on its own — there is no OS-side signal reaching it
//! to react to. Closing that requires shell-owned system-theme detection
//! (e.g. polling the registry key winit itself would consult), which stays
//! deferred — the 2026-08-19 Windows runtime gate's manual probe did not
//! reproduce the revert; see `docs/LIMITATIONS.md`'s
//! `desktop-windows-titlebar-theme-revert` entry.

use frust_theme::Brightness;
use winit::window::{Theme, Window};

/// The titlebar's wanted brightness, not yet handed to `Window::set_theme`
/// (see the module docs).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TitlebarTheme {
    wanted: Option<Brightness>,
}

impl TitlebarTheme {
    /// A titlebar carrying whatever theme Windows gave it.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Record the shell's newly resolved brightness, overwriting whatever was
    /// wanted before (only the latest value survives to the next
    /// [`take_pending`](Self::take_pending), including across a window that
    /// does not exist yet — see the module docs).
    pub(crate) fn wants(&mut self, brightness: Brightness) {
        self.wanted = Some(brightness);
    }

    /// The theme to hand `Window::set_theme`, or `None` when nothing is
    /// wanted right now.
    ///
    /// Takes the value, so the *caller must actually apply what it returns* —
    /// a caller that drops the value silently desynchronizes the titlebar
    /// until the next [`wants`](Self::wants) call. Unlike before round 1, this
    /// no longer compares against what was last applied: every wanted
    /// brightness a caller hands in via `wants` is returned once, even if it
    /// repeats the previous one (see the module docs for why that is safe and
    /// necessary).
    pub(crate) fn take_pending(&mut self) -> Option<Theme> {
        self.wanted.take().map(winit_theme)
    }
}

/// The winit theme for a resolved app brightness.
fn winit_theme(brightness: Brightness) -> Theme {
    match brightness {
        Brightness::Light => Theme::Light,
        Brightness::Dark => Theme::Dark,
    }
}

/// Theme the window's titlebar.
///
/// Always an explicit theme, never `None` ("follow the system"): the shell's
/// resolved brightness already *is* the system preference wherever the app has
/// not forced one, and where it has, the forced theme is the one the titlebar
/// must match (`DesktopExtensions::on_theme_brightness_changed`'s motivating
/// case).
#[cfg(target_os = "windows")]
pub(crate) fn apply(window: &Window, theme: Theme) {
    log::debug!("frust-shell-windows: theming the titlebar {theme:?}");
    window.set_theme(Some(theme));
}

// Inert off Windows: `set_theme` reaches no titlebar this crate owns.
#[cfg(not(target_os = "windows"))]
pub(crate) fn apply(window: &Window, theme: Theme) {
    let _ = (window, theme);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_titlebar_has_nothing_to_apply() {
        assert_eq!(TitlebarTheme::new().take_pending(), None);
    }

    #[test]
    fn the_first_resolved_brightness_is_applied() {
        let mut titlebar = TitlebarTheme::new();
        titlebar.wants(Brightness::Dark);
        assert_eq!(titlebar.take_pending(), Some(Theme::Dark));
    }

    #[test]
    fn take_pending_returns_nothing_twice_in_a_row_with_no_new_want() {
        // `take_pending` is a true take: it hands out a wanted brightness
        // exactly once, and only a fresh `wants` call arms another.
        let mut titlebar = TitlebarTheme::new();
        titlebar.wants(Brightness::Light);
        assert_eq!(titlebar.take_pending(), Some(Theme::Light));
        assert_eq!(titlebar.take_pending(), None);
    }

    #[test]
    fn a_repeated_brightness_is_re_applied_round_1() {
        // Round 1: dropped the `applied == wanted` short-circuit. A second
        // `wants` call with the SAME brightness must still produce a
        // `Some` from `take_pending` — this is what lets the shell re-issue
        // `Window::set_theme` after Windows has silently reverted the
        // titlebar out from under a still-active app override (see the
        // module docs' "residual gap" section). The core's own
        // `brightness_change_to_notify` edge-gate is what keeps this from
        // becoming per-frame spam — it only re-notifies on an actual
        // brightness transition, so a repeat reaching this module already
        // means the core believes something changed.
        let mut titlebar = TitlebarTheme::new();
        titlebar.wants(Brightness::Light);
        assert_eq!(titlebar.take_pending(), Some(Theme::Light));
        titlebar.wants(Brightness::Light);
        assert_eq!(titlebar.take_pending(), Some(Theme::Light));
    }

    #[test]
    fn a_changed_brightness_is_applied_again() {
        let mut titlebar = TitlebarTheme::new();
        titlebar.wants(Brightness::Light);
        assert_eq!(titlebar.take_pending(), Some(Theme::Light));
        titlebar.wants(Brightness::Dark);
        assert_eq!(titlebar.take_pending(), Some(Theme::Dark));
        assert_eq!(titlebar.take_pending(), None);
        titlebar.wants(Brightness::Light);
        assert_eq!(titlebar.take_pending(), Some(Theme::Light));
    }

    #[test]
    fn a_brightness_resolved_before_there_is_a_window_stays_pending() {
        // `WindowsExtensions::sync_titlebar_theme` (lib.rs) only calls
        // `take_pending` once it holds a window, so a resolution that arrives
        // first (or several, coalescing to the latest) survives untaken until
        // window creation calls it for the first time.
        let mut titlebar = TitlebarTheme::new();
        titlebar.wants(Brightness::Dark);
        // (no window yet: `take_pending` is not called)
        titlebar.wants(Brightness::Light);
        assert_eq!(titlebar.take_pending(), Some(Theme::Light));
    }

    #[test]
    fn brightness_maps_to_the_matching_winit_theme() {
        assert_eq!(winit_theme(Brightness::Light), Theme::Light);
        assert_eq!(winit_theme(Brightness::Dark), Theme::Dark);
    }
}
