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
//! own (see `Cargo.toml`'s module doc). Every hook still resolves to
//! [`DesktopExtensions`](frust_shell_desktop::extensions::DesktopExtensions)'s
//! no-op default, so today [`LinuxExtensions`] behaves identically to
//! [`NoExtensions`](frust_shell_desktop::extensions::NoExtensions) — a
//! compilable skeleton a follow-on task fills in.

use frust_shell_desktop::extensions::DesktopExtensions;

/// The Linux [`DesktopExtensions`] implementation.
///
/// Carries no state yet — every hook is still the trait's no-op default (see
/// the module docs). The `app_id`/`WM_CLASS` and window-icon attachment land
/// in a follow-on task, which is expected to add fields here rather than
/// replace this type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinuxExtensions;

impl LinuxExtensions {
    /// A fresh, hook-free extension set — see the struct docs for what still
    /// needs to land.
    pub fn new() -> Self {
        Self
    }
}

impl DesktopExtensions for LinuxExtensions {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_matches_default() {
        assert_eq!(LinuxExtensions::new(), LinuxExtensions);
    }

    #[test]
    fn implements_desktop_extensions_with_every_hook_at_its_default() {
        // A compile-time assertion as much as a runtime one: this only
        // builds if `LinuxExtensions` actually implements the trait.
        fn assert_impl<E: DesktopExtensions>() {}
        assert_impl::<LinuxExtensions>();
    }
}
