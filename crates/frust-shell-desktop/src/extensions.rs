//! The per-OS extension seam: the hooks a native desktop shell crate
//! (`frust-shell-macos`/`-windows`/`-linux`) plugs into this shared winit core.
//!
//! This crate owns the cross-platform half of a desktop app — the event loop,
//! the frame pipeline, input/IME/theme translation, accessibility — and knows
//! nothing about menu bars, Dock reopen semantics, taskbar identity, or
//! `WM_CLASS`. [`DesktopExtensions`] is where that native half attaches: a
//! trait of six hooks, each with a no-op default, invoked at the six points in
//! the loop where a platform integration has something to say. [`NoExtensions`]
//! is the whole-set no-op, and is what [`run_desktop`](crate::run_desktop) —
//! the zero-config dev preview — installs, so the preview path pays nothing for
//! a seam it doesn't use.
//!
//! # Static dispatch, not `dyn`
//!
//! [`run_desktop_with`](crate::run_desktop_with) is generic over
//! `E: DesktopExtensions` rather than taking a `Box<dyn DesktopExtensions>`:
//! [`DesktopExtensions::on_event_loop_builder`] hands out a
//! [`DesktopEventLoopBuilder`], which a platform crate extends through winit's
//! own `EventLoopBuilderExt*` traits — object safety would be a live constraint
//! there, and there is exactly one extension per binary anyway (chosen by
//! `cfg(target_os)` at the facade), so a vtable would buy nothing.
//!
//! # The window reaches an extension exactly once
//!
//! Only [`DesktopExtensions::on_window_created`] receives the window, as an
//! `&Arc<Window>` an extension is expected to **clone and retain** if it needs
//! one later. Every other hook is window-free. That is deliberate: the hooks
//! that need a window (hide-on-close, titlebar theming) need it at a moment
//! this core cannot always guarantee one exists, and threading an
//! `Option<&Window>` through five signatures would make every implementation
//! handle a case its own retained handle already answers. It also keeps five of
//! the six hooks unit-testable without a live event loop — a `winit::Window`
//! cannot be constructed without one.

use std::sync::Arc;

use frust_theme::Brightness;
use winit::event_loop::EventLoopBuilder;
use winit::window::{Window, WindowAttributes};

use crate::app_handler::ShellUserEvent;

/// The winit event-loop builder this shell builds its loop from, named so a
/// per-OS crate can take it in [`DesktopExtensions::on_event_loop_builder`]
/// without spelling the shell's own (doc-hidden) user-event type.
///
/// Windows' menu accelerators are the motivating consumer:
/// `EventLoopBuilderExtWindows::with_msg_hook` is implemented on exactly this
/// type, and must be installed before the loop is built.
pub type DesktopEventLoopBuilder = EventLoopBuilder<ShellUserEvent>;

/// What the shell should do about a window-close request (see
/// [`DesktopExtensions::on_close_requested`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    /// Exit the event loop — this shell's historical unconditional behavior and
    /// the default. Returning from `run_app` is what drops the frame executor,
    /// which is what persists the pipeline cache, so this is also the only
    /// action that runs the real shutdown path.
    Exit,
    /// Leave the loop running: the extension has already handled the request
    /// (macOS hiding the window instead of quitting, per
    /// `DesktopConfig::quit_on_last_window_closed`). The extension then owns
    /// the eventual real exit — nothing else in this core will call
    /// `event_loop.exit()` on its behalf, so a shell that returns this and
    /// never quits leaves the app running with no window, which is precisely
    /// the macOS convention it exists for.
    KeepRunning,
}

/// The hooks a per-OS desktop shell implements to add native behavior to this
/// shared winit core.
///
/// Every method has a no-op default, so an implementation writes only the hooks
/// it uses; [`NoExtensions`] implements none at all. The six hooks, in the
/// order a running app meets them:
///
/// 1. [`on_event_loop_builder`](Self::on_event_loop_builder) — before the loop
///    is built.
/// 2. [`on_window_attributes`](Self::on_window_attributes) — before the window
///    is created.
/// 3. [`on_window_created`](Self::on_window_created) — after it is created,
///    before it is shown.
/// 4. [`pump`](Self::pump) — once per frame, at the top.
/// 5. [`on_theme_brightness_changed`](Self::on_theme_brightness_changed) —
///    whenever the resolved theme brightness changes.
/// 6. [`on_close_requested`](Self::on_close_requested) — on a close request.
pub trait DesktopExtensions {
    /// Called with the winit event-loop builder, before `build()`.
    ///
    /// The one hook that runs before anything else exists — no runtime, no
    /// window, no frame. Its purpose is the platform-specific builder
    /// extensions winit only accepts here, notably
    /// `EventLoopBuilderExtWindows::with_msg_hook` (the message hook a Windows
    /// shell needs so `TranslateAcceleratorW` can turn a menu accelerator into
    /// a menu event before winit consumes the key).
    fn on_event_loop_builder(&mut self, builder: &mut DesktopEventLoopBuilder) {
        let _ = builder;
    }

    /// Called with the window attributes the core assembled from
    /// [`DesktopConfig`](crate::DesktopConfig), returning the attributes to
    /// actually create the window with.
    ///
    /// Runs **after** the core's own defaults (title, initial size, and the
    /// `visible(false)` the accessibility adapter's creation contract requires),
    /// so an extension can override any of them — a Linux shell attaching
    /// `with_name` (Wayland `app_id`/X11 `WM_CLASS`) or a window icon does it
    /// here, since winit accepts neither after creation.
    ///
    /// Leaving `visible` alone matters: the adapter must be constructed before
    /// the window is ever shown, and this core makes it visible itself once
    /// that is done.
    fn on_window_attributes(&mut self, attributes: WindowAttributes) -> WindowAttributes {
        attributes
    }

    /// Called once, immediately after the window is created and its
    /// accessibility adapter constructed, but **before** the window is shown.
    ///
    /// This is where a native menu is attached to the live window handle (muda's
    /// `init_for_nsapp`/`init_for_hwnd`) and where a platform identity call that
    /// needs the window (a Windows AppUserModelID, a taskbar icon) belongs.
    ///
    /// It is also the **only** hook that receives the window: clone the `Arc` if
    /// a later hook needs it (see the module docs).
    fn on_window_created(&mut self, window: &Arc<Window>) {
        let _ = window;
    }

    /// Called once per frame, at the top of the redraw pass — before the theme
    /// and font polls, and before the rebuild.
    ///
    /// This is the per-OS half of the signal-poll seam idiom: a shell hands over
    /// at most *one* queued activation per frame through
    /// `frust_reactive::push_menu_event` (the seam is a single-slot signal, so a
    /// batch pushed in one pass would coalesce to its last entry) and requests
    /// another frame while more remain queued. The position guarantees the
    /// activation delivered on this frame is visible to *this* frame's rebuild
    /// rather than waiting for the next one.
    fn pump(&mut self) {}

    /// Called whenever the shell's resolved theme brightness actually changes —
    /// its first resolution in `resumed`, a platform appearance change
    /// (`WindowEvent::ThemeChanged`), and an app-forced override arriving or
    /// clearing through the per-frame `set_app_theme`/`clear_app_theme` poll.
    ///
    /// "Actually changes" is enforced by the core, which tracks the last value
    /// it reported: a re-push at unchanged brightness (an override swapping one
    /// dark theme for another) fires nothing, and the hook never fires
    /// per-frame.
    ///
    /// The motivating consumer is the Windows titlebar: winit themes it from
    /// the *system* preference on its own, so only an app-forced theme needs the
    /// shell to call `Window::set_theme` — which is why the hook must fire on
    /// the override path and not merely on the platform event.
    fn on_theme_brightness_changed(&mut self, brightness: Brightness) {
        let _ = brightness;
    }

    /// Called on `WindowEvent::CloseRequested`, deciding whether the loop exits.
    ///
    /// The default is [`CloseAction::Exit`] — this shell's historical
    /// unconditional behavior, and therefore what the dev preview still does.
    /// A macOS shell returns [`CloseAction::KeepRunning`] after hiding its
    /// retained window when `DesktopConfig::quit_on_last_window_closed` is
    /// `false`, and lets the platform Quit item exit later; the pipeline-cache
    /// persistence path runs on that real exit either way, since it hangs off
    /// the frame executor's drop when `run_app` returns rather than off this
    /// event.
    fn on_close_requested(&mut self) -> CloseAction {
        CloseAction::Exit
    }
}

/// The no-op extension set: every hook keeps this core's own behavior.
///
/// Installed by [`run_desktop`](crate::run_desktop), so the zero-config dev
/// preview behaves exactly as it did before the seam existed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoExtensions;

impl DesktopExtensions for NoExtensions {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A spy over every hook that can be driven without a live event loop
    /// (i.e. all but `on_window_created`, which needs a real `winit::Window` —
    /// see the module docs). Records the call order so a caller can assert both
    /// *that* a hook ran and *when*.
    #[derive(Debug, Default)]
    struct SpyExtensions {
        calls: Vec<String>,
        close_action: Option<CloseAction>,
    }

    impl DesktopExtensions for SpyExtensions {
        fn on_window_attributes(&mut self, attributes: WindowAttributes) -> WindowAttributes {
            self.calls.push("on_window_attributes".to_string());
            attributes.with_title("spied")
        }

        fn pump(&mut self) {
            self.calls.push("pump".to_string());
        }

        fn on_theme_brightness_changed(&mut self, brightness: Brightness) {
            self.calls.push(format!("brightness:{brightness:?}"));
        }

        fn on_close_requested(&mut self) -> CloseAction {
            self.calls.push("on_close_requested".to_string());
            self.close_action.unwrap_or(CloseAction::Exit)
        }
    }

    #[test]
    fn no_extensions_leaves_window_attributes_untouched() {
        let attributes = WindowAttributes::default().with_title("Frust");
        let result = NoExtensions.on_window_attributes(attributes.clone());
        assert_eq!(result.title, attributes.title);
        assert_eq!(result.visible, attributes.visible);
    }

    #[test]
    fn no_extensions_keeps_the_historical_close_behavior() {
        // The zero-config preview must still exit on a close request.
        assert_eq!(NoExtensions.on_close_requested(), CloseAction::Exit);
    }

    #[test]
    fn no_extensions_hooks_are_all_no_ops() {
        // Nothing to observe — this pins that each hook exists with a default
        // body an implementation may leave unwritten (a compile-time assertion
        // as much as a runtime one).
        let mut ext = NoExtensions;
        ext.pump();
        ext.on_theme_brightness_changed(Brightness::Dark);
    }

    #[test]
    fn an_extension_can_override_the_attributes_the_core_assembled() {
        let mut spy = SpyExtensions::default();
        let result = spy.on_window_attributes(WindowAttributes::default().with_title("Frust"));
        assert_eq!(result.title, "spied");
        assert_eq!(spy.calls, vec!["on_window_attributes"]);
    }

    #[test]
    fn an_extension_can_refuse_a_close_request() {
        let mut spy = SpyExtensions {
            close_action: Some(CloseAction::KeepRunning),
            ..SpyExtensions::default()
        };
        assert_eq!(spy.on_close_requested(), CloseAction::KeepRunning);
        assert_eq!(spy.calls, vec!["on_close_requested"]);
    }

    #[test]
    fn hook_calls_are_observable_in_order() {
        let mut spy = SpyExtensions::default();
        spy.pump();
        spy.on_theme_brightness_changed(Brightness::Dark);
        spy.pump();
        assert_eq!(spy.calls, vec!["pump", "brightness:Dark", "pump"]);
    }
}
