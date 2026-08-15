//! macOS desktop shell: the native AppKit half of `frust-shell-desktop`'s
//! [`DesktopExtensions`](frust_shell_desktop::extensions::DesktopExtensions)
//! seam.
//!
//! [`MacosExtensions`] is this platform's implementation of the six-hook
//! trait: a native menu bar built from `DesktopConfig::menu_spec` (via
//! `muda`'s `NSMenu` bindings), Dock reopen semantics, hide-on-close instead
//! of quit-on-close, and any other AppKit integration the cross-platform
//! core in `frust-shell-desktop` does not — and must not — know about. Every
//! hook still resolves to
//! [`DesktopExtensions`](frust_shell_desktop::extensions::DesktopExtensions)'s
//! no-op default, so today [`MacosExtensions`] behaves identically to
//! [`NoExtensions`](frust_shell_desktop::extensions::NoExtensions) — a
//! compilable skeleton a follow-on task fills in.
//!
//! # Inert off macOS
//!
//! This crate is a workspace member on every host: its AppKit bindings
//! (`muda`, `objc2`, `objc2-app-kit`, `objc2-foundation`) are declared only
//! in a `cfg(target_os = "macos")` dependency table (see `Cargo.toml`), so a
//! non-macOS host — this repo's Linux dev/CI machine — builds none of them;
//! [`MacosExtensions`] itself has no macOS-only field or import, so it
//! compiles identically everywhere. This mirrors `frust-shell-android`'s
//! inert-off-target shape (see its own crate docs).

use frust_shell_desktop::extensions::DesktopExtensions;

/// The macOS [`DesktopExtensions`] implementation.
///
/// Carries no state yet — every hook is still the trait's no-op default (see
/// the module docs). Native menu-bar installation, Dock reopen, and
/// hide-on-close behavior land in a follow-on task, which is expected to add
/// fields here (e.g. a retained `muda::Menu`) rather than replace this type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MacosExtensions;

impl MacosExtensions {
    /// A fresh, hook-free extension set — see the struct docs for what still
    /// needs to land.
    pub fn new() -> Self {
        Self
    }
}

impl DesktopExtensions for MacosExtensions {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_matches_default() {
        assert_eq!(MacosExtensions::new(), MacosExtensions);
    }

    #[test]
    fn implements_desktop_extensions_with_every_hook_at_its_default() {
        // A compile-time assertion as much as a runtime one: this only
        // builds if `MacosExtensions` actually implements the trait.
        fn assert_impl<E: DesktopExtensions>() {}
        assert_impl::<MacosExtensions>();
    }
}
