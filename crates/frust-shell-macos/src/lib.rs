//! macOS desktop shell: the native AppKit half of `frust-shell-desktop`'s
//! [`DesktopExtensions`] seam.
//!
//! [`MacosExtensions`] implements the platform behavior the cross-platform
//! core in `frust-shell-desktop` does not — and must not — know about:
//!
//! - a **native menu bar** built from `DesktopConfig`: the standard,
//!   app-named application menu (About/Hide/Hide Others/Show All/Quit) plus
//!   the app's own `MenuSpec`, with an activation reaching app code through
//!   `frust_reactive::push_menu_event` on the frame its own wake produces —
//!   one activation per frame, each carried by a frame of its own (the `menu`
//!   module);
//! - **quit and reopen semantics**: `quit_on_last_window_closed = false` hides
//!   the window on a close request instead of exiting, and a Dock-click
//!   re-activation brings it back (the `lifecycle` module);
//! - the AppKit calls neither `winit` nor `muda` expose, confined to the
//!   `appkit_glue` module — this crate's sanctioned-unsafe zone, in the shape
//!   of `frust-shell-android`'s `jni_glue` and `frust-shell-ios`'s `ffi_glue`.
//!
//! # Shutdown
//!
//! Two quit routes exist, and they are not the same path:
//!
//! - **A window close that quits** (`quit_on_last_window_closed = true`, the
//!   default) returns `CloseAction::Exit`, so the shared core exits its event
//!   loop, `run_app` returns, and the frame executor drops — the render thread
//!   joins after a final present and its best-effort pipeline-cache persist.
//!   That is the desktop core's own clean-shutdown path, unchanged by this
//!   crate.
//! - **The standard Quit item and ⌘Q** are AppKit's own `terminate:`, because
//!   they must work while this shell's window is hidden and no frame is being
//!   produced — a Quit routed through the per-frame pump would do nothing in
//!   exactly the state the macOS convention creates. `winit`'s application
//!   delegate still turns AppKit's termination into a `LoopExiting` dispatch,
//!   but the executor's `Drop` does not run on that route (the pipeline-cache
//!   write it guards is a documented no-op on macOS anyway — Metal exposes no
//!   pipeline cache; see `frust-shell-desktop`'s `cache` module).
//!
//! Neither route calls `std::process::exit`.
//!
//! - **native platform views**: a plugin's `NSView` hosted above the wgpu
//!   surface and placed from the desktop core's per-frame command batch (the
//!   `platform_view` module).
//!
//! # Inert off macOS
//!
//! This crate is a workspace member on every host: its AppKit bindings
//! (`muda`, `objc2`, `objc2-app-kit`, `objc2-foundation`) are declared only in
//! a `cfg(target_os = "macos")` dependency table (see `Cargo.toml`), and every
//! module that names them is `cfg`-gated to match. Off macOS the type still
//! exists and every hook still compiles — the menu is never built, the AppKit
//! observer is never installed (so nothing ever reports a reopen), no platform
//! view is ever parented, and the close policy — plain `winit` — is the only
//! part that still runs. Nothing installs this extension off macOS anyway; the
//! point is that the workspace builds. This mirrors `frust-shell-android`'s
//! inert-off-target shape (see its own crate docs).
//!
//! The platform-view host is the one module that is only *half* gated: its
//! AppKit operations are macOS-only, but the slot bookkeeping and retain
//! accounting they drive are ordinary Rust compiled and unit-tested
//! everywhere, against a fake factory. That is deliberate — see that module's
//! own docs.

use std::sync::Arc;

use frust_shell_common::platform_view::ViewCommand;
use frust_shell_desktop::config::{DEFAULT_APP_NAME, DesktopConfig, MenuSpec};
use frust_shell_desktop::extensions::{CloseAction, DesktopExtensions};
use winit::window::Window;

#[cfg(target_os = "macos")]
mod appkit_glue;
// Both modules carry the halves only the AppKit path drives — the reopen
// transition (whose caller is `appkit_glue`) and the menu construction and
// activation queue (whose caller is `muda`'s handler). Off macOS those callers
// do not compile, so their targets read as dead code on this repo's Linux
// build host; the allow is scoped to that host, leaving real dead code on the
// macOS build (the one that ships) still reported.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod lifecycle;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod menu;
// Half-gated, unlike the two above: the AppKit operations inside are
// macOS-only, but the OS-neutral host they drive compiles everywhere so its
// ownership accounting can be unit-tested on any build host. Off macOS that
// host has no caller, hence the same scoped allow.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod platform_view;

use lifecycle::{CloseDecision, Lifecycle};

/// The macOS [`DesktopExtensions`] implementation — construct it from the same
/// [`DesktopConfig`] handed to
/// [`run_desktop_with`](frust_shell_desktop::run_desktop_with) and install it
/// there (the `frust` facade does this on macOS targets).
///
/// The configuration is read once, at construction: this type owns the
/// resolved application-menu name and a copy of the app's `MenuSpec`, and
/// builds the native menu when the window arrives.
pub struct MacosExtensions {
    /// The name the application menu is titled with (see
    /// [`resolve_app_name`]).
    app_name: String,
    /// The app's own menu declaration, appended after the application menu.
    menu_spec: Option<MenuSpec>,
    /// Close/reopen policy plus the retained window, shared with the AppKit
    /// activation observer (see [`lifecycle`]).
    lifecycle: Arc<Lifecycle>,
    /// The installed menu bar. Retained because dropping it tears the native
    /// menu down.
    #[cfg(target_os = "macos")]
    menu: Option<muda::Menu>,
    /// The live reopen registration; dropping it unregisters.
    #[cfg(target_os = "macos")]
    activation: Option<appkit_glue::AppActivationObserver>,
    /// The hosted native views: one `NSView` per live platform-view slot,
    /// parented above the window's content view (see [`platform_view`]).
    #[cfg(target_os = "macos")]
    view_host: platform_view::AppKitViewHost,
}

impl MacosExtensions {
    /// Build the macOS extension set for `config`.
    pub fn new(config: &DesktopConfig) -> Self {
        #[cfg(target_os = "macos")]
        let bundle_name = appkit_glue::bundle_display_name();
        #[cfg(not(target_os = "macos"))]
        let bundle_name: Option<String> = None;
        Self {
            app_name: resolve_app_name(
                config.app_name.as_deref(),
                bundle_name.as_deref(),
                current_exe_stem().as_deref(),
            ),
            menu_spec: config.menu_spec.clone(),
            lifecycle: Arc::new(Lifecycle::new(config.quit_on_last_window_closed)),
            #[cfg(target_os = "macos")]
            menu: None,
            #[cfg(target_os = "macos")]
            activation: None,
            #[cfg(target_os = "macos")]
            view_host: platform_view::AppKitViewHost::default(),
        }
    }
}

impl DesktopExtensions for MacosExtensions {
    fn on_window_created(&mut self, window: &Arc<Window>) {
        // The one hook that receives the window: retain it for the hooks that
        // need it later (hide-on-close, the reopen re-show) and as the redraw
        // target a menu activation wakes.
        self.lifecycle.set_window(window.clone());
        let bridge = menu::bridge();
        bridge.set_known_ids(menu::collect_app_item_ids(self.menu_spec.as_ref()));
        bridge.set_waker(window.clone());

        #[cfg(target_os = "macos")]
        {
            // Ordering matters: route activations into the queue before the
            // menu that produces them exists.
            menu::install_event_handler();
            self.menu = menu::install_menu(&self.app_name, self.menu_spec.as_ref());
            self.activation =
                appkit_glue::AppActivationObserver::install(Arc::clone(&self.lifecycle));
        }
    }

    fn pump(&mut self) {
        // The per-OS half of the signal-poll seam: the activation taken here is
        // visible to *this* frame's rebuild, since the core calls `pump` at the
        // top of the redraw pass. `push_menu_event` is UI-thread-only, which is
        // why the queue exists rather than a push from the platform callback
        // itself — and exactly one activation crosses per frame, because the
        // signal behind `push_menu_event` holds one event and this frame reads
        // it once. `take_next` asks for a further redraw while more remain (see
        // the `menu` module docs).
        if let Some(id) = menu::bridge().take_next() {
            frust_reactive::push_menu_event(id);
        }
    }

    fn on_platform_view_commands(
        &mut self,
        window: &Arc<Window>,
        scale: f64,
        commands: &[ViewCommand],
    ) {
        // `scale` goes unread on purpose, and that is the interesting part of
        // this hook on macOS: AppKit places a subview in the very logical
        // points the command already carries, so converting to physical px —
        // what the mobile shells do at their own FFI boundary — would place
        // every hosted view at `scale` times its correct size and offset on a
        // Retina display. The other arguments are consumed here so the
        // off-macOS build, where the body below does not exist, reads them
        // too.
        let _ = (scale, window, commands);
        #[cfg(target_os = "macos")]
        self.view_host.apply(window, commands);
    }

    fn on_platform_views_suspended(&mut self) {
        // Not a hide: the view hierarchy these were parented into is going
        // away, so every hosted view goes back to its factory. The next batch
        // after the surface returns replays a `Create` for every live slot.
        #[cfg(target_os = "macos")]
        self.view_host.suspend();
    }

    fn on_close_requested(&mut self) -> CloseAction {
        match self.lifecycle.on_close_requested() {
            CloseDecision::Quit => CloseAction::Exit,
            CloseDecision::Hide => CloseAction::KeepRunning,
        }
    }
}

impl std::fmt::Debug for MacosExtensions {
    /// Hand-written: `muda::Menu` implements no `Debug`, and the menu's
    /// interesting state is whether one is installed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("MacosExtensions");
        debug
            .field("app_name", &self.app_name)
            .field("menu_spec", &self.menu_spec)
            .field("lifecycle", &self.lifecycle);
        #[cfg(target_os = "macos")]
        debug
            .field("menu_installed", &self.menu.is_some())
            .field("reopen_observer", &self.activation.is_some())
            .field("hosted_platform_views", &self.view_host.hosted_count());
        debug.finish()
    }
}

/// The name the application menu (title and its About/Hide/Quit item labels)
/// carries: the configured `DesktopConfig::app_name`, else the main bundle's
/// display name, else the running executable's file stem, else
/// [`DEFAULT_APP_NAME`].
///
/// The middle fallbacks mirror what macOS itself would show for the same
/// process: a `.app`-bundled binary is named by its
/// `CFBundleDisplayName`/`CFBundleName`
/// ([`appkit_glue::bundle_display_name`]; macbook-gate-r2 finding F-4 — the
/// stem gave a bundled app "Quit gate_app" where AppKit's own bold menu
/// title already said "Gate App"), an unbundled one by its process name
/// (`NSProcessInfo`'s, which is this same stem) — never the window title's
/// `Frust` placeholder. `DesktopConfig::app_name`'s docs keep the unset case
/// distinguishable precisely so a shell can make this choice.
fn resolve_app_name(
    app_name: Option<&str>,
    bundle_name: Option<&str>,
    exe_stem: Option<&str>,
) -> String {
    app_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .or_else(|| bundle_name.map(str::trim).filter(|name| !name.is_empty()))
        .or_else(|| exe_stem.map(str::trim).filter(|stem| !stem.is_empty()))
        .unwrap_or(DEFAULT_APP_NAME)
        .to_string()
}

/// The running executable's file stem, or `None` when it cannot be resolved
/// (an unreadable `/proc`, a deleted binary) — a diagnostic-only lookup, never
/// a failure.
fn current_exe_stem() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.file_stem()?.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use frust_shell_desktop::config::{MenuItemSpec, MenuRole};

    use super::*;

    #[test]
    fn implements_desktop_extensions() {
        // A compile-time assertion as much as a runtime one: this only builds
        // if `MacosExtensions` actually implements the trait.
        fn assert_impl<E: DesktopExtensions>() {}
        assert_impl::<MacosExtensions>();
    }

    #[test]
    fn a_configured_app_name_titles_the_application_menu() {
        assert_eq!(
            resolve_app_name(Some("Huddle"), Some("Bundle Huddle"), Some("huddle-dev")),
            "Huddle"
        );
    }

    /// The bundled case (macbook-gate-r2 F-4): with no configured name, the
    /// bundle's display name beats the executable stem, so the default menu
    /// items say "Quit Gate App" rather than "Quit gate_app".
    #[test]
    fn an_unnamed_bundled_app_takes_the_bundle_display_name() {
        assert_eq!(
            resolve_app_name(None, Some("Gate App"), Some("gate_app")),
            "Gate App"
        );
    }

    #[test]
    fn an_unnamed_app_falls_back_to_the_executable_stem_then_to_the_default() {
        assert_eq!(
            resolve_app_name(None, None, Some("huddle-dev")),
            "huddle-dev"
        );
        assert_eq!(resolve_app_name(None, None, None), DEFAULT_APP_NAME);
    }

    #[test]
    fn a_blank_name_is_treated_as_unset() {
        // A menu titled with whitespace is indistinguishable from a broken
        // menu bar on screen.
        assert_eq!(
            resolve_app_name(Some("   "), None, Some("huddle-dev")),
            "huddle-dev"
        );
        assert_eq!(
            resolve_app_name(None, Some(" "), Some("huddle-dev")),
            "huddle-dev"
        );
        assert_eq!(resolve_app_name(Some(""), None, None), DEFAULT_APP_NAME);
        assert_eq!(resolve_app_name(None, None, Some("  ")), DEFAULT_APP_NAME);
    }

    #[test]
    fn the_config_is_read_once_at_construction() {
        let config =
            DesktopConfig::new()
                .with_app_name("Huddle")
                .with_menu_spec(MenuSpec::new().with_item(MenuItemSpec::submenu(
                    "File",
                    MenuSpec::new().with_item(MenuItemSpec::item("file.open", "Open…")),
                )));
        let extensions = MacosExtensions::new(&config);

        assert_eq!(extensions.app_name, "Huddle");
        assert_eq!(extensions.menu_spec, config.menu_spec);
        assert!(!extensions.lifecycle.is_hidden());
    }

    #[test]
    fn the_default_config_keeps_the_historical_exit_on_close() {
        // No window is retained in a host test, which is the other reason this
        // exits — either way the zero-config behavior is preserved.
        let mut extensions = MacosExtensions::new(&DesktopConfig::new());
        assert_eq!(extensions.on_close_requested(), CloseAction::Exit);
    }

    #[test]
    fn a_pump_with_nothing_queued_pushes_nothing() {
        // `push_menu_event` panics off the UI thread and needs a live reactive
        // runtime, so reaching it from a host test would be loud; the point
        // here is that an idle frame never does.
        let mut extensions = MacosExtensions::new(&DesktopConfig::new());
        extensions.pump();
        extensions.pump();
    }

    #[test]
    fn a_role_only_menu_declares_no_app_ids() {
        let config =
            DesktopConfig::new().with_menu_spec(MenuSpec::new().with_item(MenuItemSpec::submenu(
                "App",
                MenuSpec::new().with_item(MenuItemSpec::role(MenuRole::Quit)),
            )));
        let extensions = MacosExtensions::new(&config);
        assert!(menu::collect_app_item_ids(extensions.menu_spec.as_ref()).is_empty());
    }

    #[test]
    fn debug_reports_the_configured_state() {
        let extensions = MacosExtensions::new(&DesktopConfig::new().with_app_name("Huddle"));
        let rendered = format!("{extensions:?}");
        assert!(rendered.contains("Huddle"), "{rendered}");
    }
}
