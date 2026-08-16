//! Windows desktop shell: the native Win32 half of `frust-shell-desktop`'s
//! [`DesktopExtensions`] seam.
//!
//! [`WindowsExtensions`] implements the trait's hooks against one
//! [`DesktopConfig`] — nothing here is hardcoded, and an app that configures
//! none of it gets the plain winit window the shared core builds on its own:
//!
//! | Config | Windows integration |
//! |--------|---------------------|
//! | `app_id` | The process AppUserModelID (taskbar grouping, notification attribution), set before the first window exists |
//! | `window_icon` | Both `ICON_SMALL` (titlebar/alt-tab) and `ICON_BIG` (taskbar), attached at window-attribute time |
//! | `menu_spec` | A native `HMENU` menu bar on the window's own handle, with `TranslateAcceleratorW` accelerators and activations queued by muda's event handler, each carrying its own redraw into `frust_reactive::push_menu_event` |
//! | (resolved theme) | The titlebar's light/dark appearance, via winit's native `Window::set_theme` |
//!
//! # Module shape
//!
//! - `menu` — the platform-free `MenuSpec` → menu plan mapping and the
//!   activation queue behind it, plus the muda menu built from the plan and the
//!   one-activation-per-frame pump that empties the queue.
//! - `theme` — the titlebar's wanted-brightness latch (round 1: re-applies on
//!   every core-signaled brightness change, not just the first — see that
//!   module's docs for why, and for the residual gap it does not close).
//! - `win32_glue` — this crate's **sanctioned-unsafe zone**: every `unsafe`
//!   block, and every `windows-sys`/winit-Windows-extension call, lives there
//!   and nowhere else (`docs/SHELLS_ARCHITECTURE.md`'s convention, the
//!   `frust-shell-android` `jni_glue` / `frust-shell-ios` `ffi_glue`
//!   precedent).
//!
//! # Inert off Windows
//!
//! This crate is a workspace member on every host: its Win32 bindings (`muda`,
//! `windows-sys`) are declared only in a `cfg(target_os = "windows")`
//! dependency table (see `Cargo.toml`), and each native entry point in
//! `win32_glue`/`menu`/`theme` pairs its real body with a do-nothing stand-in
//! for every other target. So [`WindowsExtensions`] has the same fields and the
//! same hook bodies everywhere and compiles on this repo's Linux dev/CI
//! machine, where it simply does nothing — the same inert-off-target shape
//! `frust-shell-android` has (see its own crate docs). The mapping and latching
//! logic that *can* be host-tested deliberately is: it lives in the
//! platform-free halves of `menu` and `theme`.

use std::sync::Arc;

use frust_shell_desktop::config::{DesktopConfig, IconData};
use frust_shell_desktop::extensions::{DesktopEventLoopBuilder, DesktopExtensions};
use frust_theme::Brightness;
use winit::window::{Window, WindowAttributes};

use crate::menu::{InstalledMenu, MenuPlan};
use crate::theme::TitlebarTheme;
use crate::win32_glue::AcceleratorTable;

// The platform-free halves of these modules (the menu plan, the titlebar
// latch, the accelerator slot) are driven only by the Windows arms beside
// them, so on any other host nothing outside their test modules calls them.
// The lint stays armed for the Windows target, where every one of them must
// stay live — the `frust-shell-android` `ffi_support`/`sync_tail` shape.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod menu;
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod theme;
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod win32_glue;

/// The Windows [`DesktopExtensions`] implementation.
///
/// Built from the app's [`DesktopConfig`] (see [`WindowsExtensions::new`]) and
/// handed to `frust_shell_desktop::run_desktop_with` by the facade. It retains
/// what the later hooks need: the window (for titlebar theming), the installed
/// menu (whose `HMENU`/`HACCEL` live exactly as long as it does), and the
/// accelerator table the builder-stage message hook reads.
///
/// Derives nothing: it owns a `muda::Menu` on Windows, which is neither `Debug`
/// nor `PartialEq`, and there is exactly one of these per process anyway (the
/// facade builds it from the app's config and hands it straight to
/// `run_desktop_with`).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub struct WindowsExtensions {
    /// Taskbar identity, set process-wide at the builder-stage hook.
    app_id: Option<String>,
    /// Window/taskbar icon, attached at the window-attributes hook.
    window_icon: Option<IconData>,
    /// The planned menu bar, resolved once at construction (the native menu is
    /// built from it when there is a window to attach it to).
    menu_plan: MenuPlan,
    /// Shared with the message hook; filled once the menu is installed.
    accelerators: AcceleratorTable,
    titlebar_theme: TitlebarTheme,
    /// Retained from `on_window_created` — the one hook that receives it.
    window: Option<Arc<Window>>,
    /// Retained so the native menu outlives its installation; dropping it
    /// un-menus the window and invalidates [`Self::accelerators`].
    menu: Option<InstalledMenu>,
}

impl WindowsExtensions {
    /// Build the Windows integration for `config`.
    ///
    /// Borrows rather than consumes: the same [`DesktopConfig`] is also handed
    /// to `run_desktop_with`, which reads the fields this shell does not (the
    /// window title). Only the three identity fields this platform integrates
    /// — `app_id`, `window_icon`, `menu_spec` — are retained.
    pub fn new(config: &DesktopConfig) -> Self {
        Self {
            app_id: config.app_id.clone(),
            window_icon: config.window_icon.clone(),
            menu_plan: MenuPlan::from_spec(config.menu_spec.as_ref()),
            accelerators: AcceleratorTable::new(),
            titlebar_theme: TitlebarTheme::new(),
            window: None,
            menu: None,
        }
    }

    /// Apply any pending titlebar brightness, if there is a window to apply it
    /// to.
    ///
    /// Called from both hooks that can leave one pending — the brightness hook
    /// itself, and window creation (the brightness can resolve first, and a
    /// theme applied to a window that does not exist yet would be lost).
    fn sync_titlebar_theme(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if let Some(theme) = self.titlebar_theme.take_pending() {
            theme::apply(&window, theme);
        }
    }
}

impl DesktopExtensions for WindowsExtensions {
    /// Claims the process's shell identity and installs the accelerator
    /// message hook.
    ///
    /// Both belong to this hook specifically: an AppUserModelID must be set
    /// before the first window is created for the taskbar to honor it, and
    /// winit accepts a message hook only on the builder.
    fn on_event_loop_builder(&mut self, builder: &mut DesktopEventLoopBuilder) {
        if let Some(app_id) = &self.app_id {
            win32_glue::set_app_user_model_id(app_id);
        }
        // Installed even with no menu configured: the hook is a no-op until an
        // accelerator table is published, and there is no second chance to
        // install it once the loop is built.
        win32_glue::install_accelerator_hook(builder, self.accelerators.clone());
    }

    /// Attaches the configured window/taskbar icon — winit takes both only at
    /// creation time.
    fn on_window_attributes(&mut self, attributes: WindowAttributes) -> WindowAttributes {
        match &self.window_icon {
            Some(icon) => win32_glue::with_icons(attributes, icon),
            None => attributes,
        }
    }

    /// Retains the window, routes menu activations into the shell's own queue,
    /// installs the native menu bar on the window's `HWND`, and applies any
    /// brightness that resolved before the window existed.
    fn on_window_created(&mut self, window: &Arc<Window>) {
        self.window = Some(Arc::clone(window));
        // Ordering matters: the queue must know which ids it accepts, and which
        // window a menu activation wakes, before the menu that produces them
        // exists. The retained `Arc<Window>` is that wake target, and it is
        // load-bearing — neither route into the menu produces a winit event
        // (a click arrives as a `WM_COMMAND`, which winit has no event for; an
        // accelerator is consumed by this crate's message hook before winit
        // sees the keystroke at all), so without an explicit `request_redraw`
        // the dirty-driven loop would never produce the frame that delivers the
        // activation (see the `menu` module).
        let bridge = menu::bridge();
        bridge.set_known_ids(self.menu_plan.activation_ids().clone());
        // `window.clone()`, not `Arc::clone(window)`: the unsizing coercion to
        // `Arc<dyn MenuWaker>` needs the method call's inferred target type.
        bridge.set_waker(window.clone());
        menu::install_event_handler();
        self.menu = menu::install(&self.menu_plan, window, &self.accelerators);
        self.sync_titlebar_theme();
    }

    /// Hands one queued menu activation to `frust_reactive::push_menu_event`,
    /// so it is visible to *this* frame's rebuild (the signal-poll seam idiom —
    /// see the hook's own docs).
    ///
    /// One per frame, not the whole queue: the reactive menu source is a
    /// single-slot signal this frame reads once, so a batch would coalesce to
    /// its last event. The queue's remainder rides further frames the bridge
    /// requests for itself (see the `menu` module).
    fn pump(&mut self) {
        menu::pump_menu_events();
    }

    /// Themes the titlebar to match the app's resolved brightness.
    fn on_theme_brightness_changed(&mut self, brightness: Brightness) {
        self.titlebar_theme.wants(brightness);
        self.sync_titlebar_theme();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use frust_shell_desktop::config::{MenuItemSpec, MenuRole, MenuSpec};

    #[test]
    fn implements_desktop_extensions() {
        // A compile-time assertion as much as a runtime one: this only builds
        // if `WindowsExtensions` actually implements the trait.
        fn assert_impl<E: DesktopExtensions>() {}
        assert_impl::<WindowsExtensions>();
    }

    #[test]
    fn a_default_config_configures_no_integration_at_all() {
        let extensions = WindowsExtensions::new(&DesktopConfig::new());
        assert_eq!(extensions.app_id, None);
        assert_eq!(extensions.window_icon, None);
        // Nothing to install: the zero-config preview window keeps no menu bar.
        assert!(extensions.menu_plan.is_empty());
    }

    #[test]
    fn every_integration_is_taken_from_the_config() {
        let icon = IconData::from_rgba(vec![0; 4], 1, 1).expect("1x1 RGBA is valid");
        let config = DesktopConfig::new()
            .with_app_name("Fake")
            .with_app_id("dev.frust.fake")
            .with_window_icon(icon.clone())
            .with_menu_spec(
                MenuSpec::new().with_item(MenuItemSpec::submenu(
                    "File",
                    MenuSpec::new()
                        .with_item(MenuItemSpec::item("file.open", "Open…"))
                        .with_item(MenuItemSpec::role(MenuRole::Quit)),
                )),
            );
        let extensions = WindowsExtensions::new(&config);

        assert_eq!(extensions.app_id.as_deref(), Some("dev.frust.fake"));
        assert_eq!(extensions.window_icon, Some(icon));
        assert!(!extensions.menu_plan.is_empty());
        assert!(extensions.menu_plan.activation_ids().contains("file.open"));
    }

    #[test]
    fn the_attributes_hook_leaves_an_unconfigured_window_untouched() {
        let mut extensions = WindowsExtensions::new(&DesktopConfig::new());
        let attributes = WindowAttributes::default().with_title("Frust");
        let result = extensions.on_window_attributes(attributes.clone());
        assert_eq!(result.title, attributes.title);
        assert!(result.window_icon.is_none());
    }

    #[test]
    fn a_brightness_change_before_the_window_exists_is_not_lost() {
        // `on_window_created` is the only hook that receives the window, so a
        // brightness resolved before it must stay pending rather than being
        // marked applied against a window that is not there.
        let mut extensions = WindowsExtensions::new(&DesktopConfig::new());
        extensions.on_theme_brightness_changed(Brightness::Dark);
        assert_eq!(
            extensions.titlebar_theme.take_pending(),
            Some(winit::window::Theme::Dark)
        );
    }

    #[test]
    fn pumping_without_a_menu_is_harmless() {
        // The frame loop calls `pump` unconditionally, including on the path
        // where no menu was configured or installation was refused.
        let mut extensions = WindowsExtensions::new(&DesktopConfig::new());
        extensions.pump();
        extensions.pump();
    }
}
