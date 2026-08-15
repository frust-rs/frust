//! Titlebar theming: keeping the window's non-client area on the same
//! brightness as the app's own resolved theme.
//!
//! winit 0.30 themes the Windows titlebar natively (`Window::set_theme`, which
//! drives the DWM immersive-dark-mode attribute for us), so this module holds
//! no FFI at all — only the decision of *when* to call it.
//!
//! # Why there is a latch at all
//!
//! `DesktopExtensions::on_theme_brightness_changed` is already change-gated by
//! the shared core, so a naive implementation could just call `set_theme` per
//! hook fire. Two things make that wrong:
//!
//! - The window is not guaranteed to exist yet when the first brightness is
//!   resolved. [`TitlebarTheme`] separates *wanting* a brightness from having
//!   *applied* one, so a resolution that arrives without a window is applied at
//!   window creation instead of being lost.
//! - Nothing else may re-apply per frame: a repeated `set_theme` at unchanged
//!   brightness is a non-client-area repaint, i.e. a visible titlebar flicker.
//!
//! Both fall out of one rule — apply only the difference — which is what
//! [`TitlebarTheme::take_pending`] answers, and what this module's tests pin.

use frust_theme::Brightness;
use winit::window::{Theme, Window};

/// The titlebar's wanted-vs-applied brightness (see the module docs).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TitlebarTheme {
    wanted: Option<Brightness>,
    applied: Option<Brightness>,
}

impl TitlebarTheme {
    /// A titlebar carrying whatever theme Windows gave it.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Record the shell's newly resolved brightness.
    pub(crate) fn wants(&mut self, brightness: Brightness) {
        self.wanted = Some(brightness);
    }

    /// The theme to hand `Window::set_theme`, or `None` when the titlebar
    /// already carries the wanted brightness.
    ///
    /// Marks it applied, so the *caller must actually apply what it returns* —
    /// a caller that drops the value silently desynchronizes the titlebar until
    /// the next brightness change.
    pub(crate) fn take_pending(&mut self) -> Option<Theme> {
        let wanted = self.wanted?;
        if self.applied == Some(wanted) {
            return None;
        }
        self.applied = Some(wanted);
        Some(winit_theme(wanted))
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
    fn the_same_brightness_is_never_reapplied() {
        // The flicker guard: a hook fire (or a window-creation re-sync) at an
        // unchanged brightness must not re-theme the non-client area.
        let mut titlebar = TitlebarTheme::new();
        titlebar.wants(Brightness::Light);
        assert_eq!(titlebar.take_pending(), Some(Theme::Light));
        assert_eq!(titlebar.take_pending(), None);
        titlebar.wants(Brightness::Light);
        assert_eq!(titlebar.take_pending(), None);
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
        // The caller drops `take_pending`'s value only by not calling it —
        // which is exactly what it does while `self.window` is still `None`,
        // so the resolution survives to window creation.
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
