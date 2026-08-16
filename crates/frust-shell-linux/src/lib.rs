//! Linux desktop shell: the native half of `frust-shell-desktop`'s
//! [`DesktopExtensions`](frust_shell_desktop::extensions::DesktopExtensions)
//! seam.
//!
//! [`LinuxExtensions`] is this platform's implementation of the six-hook
//! trait. Unlike `frust-shell-macos`/`-windows`, Linux has no native menu
//! bar to build — the menu is widget-drawn — so this crate's native surface
//! is deliberately thin: a Wayland `app_id`/X11 `WM_CLASS`
//! (`WindowAttributes::with_name`) and a window icon, both reachable through
//! `winit`'s own cross-platform API rather than a GTK/X11 binding of its
//! own (see `Cargo.toml`'s module doc). Only
//! [`on_window_attributes`](frust_shell_desktop::extensions::DesktopExtensions::on_window_attributes)
//! is implemented — every other hook keeps the trait's no-op default, so
//! [`LinuxExtensions`] behaves identically to
//! [`NoExtensions`](frust_shell_desktop::extensions::NoExtensions) outside
//! that one hook.
//!
//! Like every workspace member, this crate also compiles — inert — off its own
//! target: the `winit` Wayland/X11 extension traits are reached only from
//! `with_app_id`'s gated arm, so a macOS or Windows host still builds it (see
//! that function's docs).

use frust_shell_desktop::config::{DesktopConfig, IconData};
use frust_shell_desktop::extensions::DesktopExtensions;
use winit::window::{Icon, WindowAttributes};

/// The Linux [`DesktopExtensions`] implementation.
///
/// Carries exactly the two [`DesktopConfig`] fields its one hook needs —
/// [`app_id`](DesktopConfig::app_id) and
/// [`window_icon`](DesktopConfig::window_icon) — cloned out at construction
/// time by [`LinuxExtensions::new`] rather than holding the whole config, so
/// the facade can build this once per run without keeping `DesktopConfig`
/// alive alongside it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinuxExtensions {
    app_id: Option<String>,
    window_icon: Option<IconData>,
}

impl LinuxExtensions {
    /// Build the extension set from the parts of `config` this shell acts
    /// on. `app_id` becomes the Wayland `app_id`/X11 `WM_CLASS`;
    /// `window_icon` becomes the X11 window icon (Wayland has no window-icon
    /// concept and ignores it by design — see
    /// [`on_window_attributes`](DesktopExtensions::on_window_attributes)).
    /// Neither field is consumed if `config` leaves it unset: the window is
    /// built exactly as it would be with [`NoExtensions`](frust_shell_desktop::extensions::NoExtensions).
    pub fn new(config: &DesktopConfig) -> Self {
        Self {
            app_id: config.app_id.clone(),
            window_icon: config.window_icon.clone(),
        }
    }
}

impl DesktopExtensions for LinuxExtensions {
    /// Attaches the Wayland `app_id`/X11 `WM_CLASS` and the window icon, in
    /// that order.
    ///
    /// The `app_id` half goes through `with_app_id`, which carries the
    /// platform gating; the icon half is plain cross-platform
    /// `WindowAttributes` API and runs everywhere. Unset `app_id` makes no
    /// call at all, leaving winit's own default in place.
    ///
    /// The icon is built with `winit::window::Icon::from_rgba`, which
    /// reaches X11's window icon; Wayland has no window-icon protocol and
    /// silently ignores `WindowAttributes::window_icon` (icons come from the
    /// `.desktop` entry's `Icon=` key there instead — Phase B's job, not
    /// this hook's). `IconData`'s own invariant
    /// (`rgba.len() == width * height * 4`) means `from_rgba` failing here
    /// would be a `winit` behavior change, not a data problem this crate
    /// caused — logged and skipped rather than panicking.
    fn on_window_attributes(&mut self, attributes: WindowAttributes) -> WindowAttributes {
        let mut attributes = attributes;

        if let Some(app_id) = &self.app_id {
            attributes = with_app_id(attributes, app_id);
        }

        if let Some(icon_data) = &self.window_icon {
            match Icon::from_rgba(
                icon_data.rgba().to_vec(),
                icon_data.width(),
                icon_data.height(),
            ) {
                Ok(icon) => attributes = attributes.with_window_icon(Some(icon)),
                Err(err) => {
                    log::warn!("frust-shell-linux: failed to build window icon: {err}");
                }
            }
        }

        attributes
    }
}

/// Attaches `app_id` as both the Wayland `app_id` and the X11 `WM_CLASS`.
///
/// Both `winit` extension traits are called: each backend reads only the
/// attributes its own platform understands, so calling the X11 setter on a
/// Wayland session (and vice versa) is inert rather than wrong.
/// `general`/`instance` are both set to the same `app_id` — this shell carries
/// no separate instance-name concept, and a duplicate `general`/`instance`
/// pair is a well-formed `WM_CLASS`.
///
/// # Where this arm compiles
///
/// `winit::platform::wayland`/`::x11` exist only where `winit`'s own build
/// script defines `wayland_platform`/`x11_platform`: its `free_unix` alias
/// (`unix`, minus Apple, Android and emscripten) with `redox` excluded, and its
/// default `wayland`/`x11` features on (winit 0.30.13 `build.rs`). The `cfg`
/// below mirrors that predicate rather than naming Linux alone, so this crate
/// never reaches for a module `winit` did not compile — the inert-off-target
/// rule every workspace member follows (`frust-shell-android`'s precedent):
/// `cargo check/clippy --workspace` on a macOS or Windows host must build this
/// crate too. Off those targets the `not`-arm below returns the attributes
/// untouched — there is no window-manager identity concept there to attach one
/// to.
#[cfg(all(
    unix,
    not(target_os = "macos"),
    not(target_os = "ios"),
    not(target_os = "android"),
    not(target_os = "emscripten"),
    not(target_os = "redox")
))]
fn with_app_id(attributes: WindowAttributes, app_id: &str) -> WindowAttributes {
    use winit::platform::wayland::WindowAttributesExtWayland;
    use winit::platform::x11::WindowAttributesExtX11;

    let attributes =
        WindowAttributesExtWayland::with_name(attributes, app_id.to_string(), app_id.to_string());
    WindowAttributesExtX11::with_name(attributes, app_id.to_string(), app_id.to_string())
}

/// The off-target arm: no Wayland/X11 identity to set, so the attributes pass
/// through unchanged (see the gated arm above for why this crate compiles at
/// all off free-unix hosts).
#[cfg(not(all(
    unix,
    not(target_os = "macos"),
    not(target_os = "ios"),
    not(target_os = "android"),
    not(target_os = "emscripten"),
    not(target_os = "redox")
)))]
fn with_app_id(attributes: WindowAttributes, _app_id: &str) -> WindowAttributes {
    attributes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_icon() -> IconData {
        // 1x1 opaque red pixel — smallest valid IconData.
        IconData::from_rgba(vec![255, 0, 0, 255], 1, 1).expect("1x1 RGBA is valid")
    }

    #[test]
    fn new_carries_nothing_from_an_unconfigured_config() {
        let ext = LinuxExtensions::new(&DesktopConfig::default());
        assert_eq!(ext, LinuxExtensions::default());
    }

    #[test]
    fn new_carries_the_app_id_and_icon_out_of_the_config() {
        let icon = sample_icon();
        let config = DesktopConfig::new()
            .with_app_id("dev.frust.huddle")
            .with_window_icon(icon.clone());
        let ext = LinuxExtensions::new(&config);
        assert_eq!(ext.app_id.as_deref(), Some("dev.frust.huddle"));
        assert_eq!(ext.window_icon, Some(icon));
    }

    #[test]
    fn unset_app_id_and_icon_leave_the_attributes_untouched() {
        let mut ext = LinuxExtensions::default();
        let attributes = WindowAttributes::default().with_title("Frust");
        let result = ext.on_window_attributes(attributes.clone());
        // No app_id/icon means neither `with_name` call nor
        // `with_window_icon` runs — the only observable slot from outside
        // this crate, `window_icon`, stays exactly as the core set it
        // (`winit::window::Icon` has no `PartialEq`, so `is_none` is the
        // comparison available).
        assert!(result.window_icon.is_none());
        assert_eq!(result.title, attributes.title);
    }

    // Only the hosts where `with_app_id`'s gated arm compiles can observe a
    // `with_name` call at all (see that function's docs).
    #[cfg(all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android"),
        not(target_os = "emscripten"),
        not(target_os = "redox")
    ))]
    #[test]
    fn a_configured_app_id_lands_in_the_debug_output_of_both_backends() {
        let mut ext = LinuxExtensions {
            app_id: Some("dev.frust.huddle".to_string()),
            window_icon: None,
        };
        let result = ext.on_window_attributes(WindowAttributes::default());
        // `platform_specific` is private to `winit`, so `Debug` is the only
        // outside-the-crate way to observe that `with_name` actually landed
        // — both `WindowAttributesExtWayland` and `WindowAttributesExtX11`
        // write the same underlying `ApplicationName` on Linux, so one
        // assertion covers both calls.
        let debug = format!("{result:?}");
        assert!(
            debug.contains("dev.frust.huddle"),
            "expected the app_id in the window-attributes debug output, got: {debug}"
        );
    }

    // The counterpart of the test above on every other host (the macOS and
    // Windows dev hosts run this same workspace gate): the hook still runs and
    // still hands its attributes back, it simply attaches no window-manager
    // identity.
    #[cfg(not(all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android"),
        not(target_os = "emscripten"),
        not(target_os = "redox")
    )))]
    #[test]
    fn a_configured_app_id_passes_the_attributes_through_off_free_unix() {
        let mut ext = LinuxExtensions {
            app_id: Some("dev.frust.huddle".to_string()),
            window_icon: None,
        };
        let attributes = WindowAttributes::default().with_title("Frust");
        let result = ext.on_window_attributes(attributes.clone());
        assert_eq!(result.title, attributes.title);
    }

    #[test]
    fn a_configured_icon_lands_in_the_window_icon_attribute() {
        let icon = sample_icon();
        let mut ext = LinuxExtensions {
            app_id: None,
            window_icon: Some(icon),
        };
        let result = ext.on_window_attributes(WindowAttributes::default());
        assert!(result.window_icon.is_some());
    }

    #[test]
    fn implements_desktop_extensions() {
        // A compile-time assertion as much as a runtime one: this only
        // builds if `LinuxExtensions` actually implements the trait.
        fn assert_impl<E: DesktopExtensions>() {}
        assert_impl::<LinuxExtensions>();
    }
}
