//! Windows desktop shell: the native Win32 half of `frust-shell-desktop`'s
//! [`DesktopExtensions`](frust_shell_desktop::extensions::DesktopExtensions)
//! seam.
//!
//! [`WindowsExtensions`] is this platform's implementation of the six-hook
//! trait: a native menu bar built from `DesktopConfig::menu_spec` (via
//! `muda`'s `HMENU` bindings) with `TranslateAcceleratorW`-driven
//! accelerators (`EventLoopBuilderExtWindows::with_msg_hook`), an
//! AppUserModelID for taskbar identity/grouping, and any other Win32
//! integration the cross-platform core in `frust-shell-desktop` does not —
//! and must not — know about. Every hook still resolves to
//! [`DesktopExtensions`](frust_shell_desktop::extensions::DesktopExtensions)'s
//! no-op default, so today [`WindowsExtensions`] behaves identically to
//! [`NoExtensions`](frust_shell_desktop::extensions::NoExtensions) — a
//! compilable skeleton a follow-on task fills in.
//!
//! # Inert off Windows
//!
//! This crate is a workspace member on every host: its Win32 bindings
//! (`muda`, `windows-sys`) are declared only in a `cfg(target_os =
//! "windows")` dependency table (see `Cargo.toml`), so a non-Windows host —
//! this repo's Linux dev/CI machine — builds none of them;
//! [`WindowsExtensions`] itself has no Windows-only field or import, so it
//! compiles identically everywhere. This mirrors `frust-shell-android`'s
//! inert-off-target shape (see its own crate docs).

use frust_shell_desktop::extensions::DesktopExtensions;

/// The Windows [`DesktopExtensions`] implementation.
///
/// Carries no state yet — every hook is still the trait's no-op default (see
/// the module docs). Native menu-bar installation, the `with_msg_hook`
/// accelerator wiring, and AppUserModelID identity land in a follow-on task,
/// which is expected to add fields here (e.g. a retained `muda::Menu`)
/// rather than replace this type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowsExtensions;

impl WindowsExtensions {
    /// A fresh, hook-free extension set — see the struct docs for what still
    /// needs to land.
    pub fn new() -> Self {
        Self
    }
}

impl DesktopExtensions for WindowsExtensions {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_matches_default() {
        assert_eq!(WindowsExtensions::new(), WindowsExtensions);
    }

    #[test]
    fn implements_desktop_extensions_with_every_hook_at_its_default() {
        // A compile-time assertion as much as a runtime one: this only
        // builds if `WindowsExtensions` actually implements the trait.
        fn assert_impl<E: DesktopExtensions>() {}
        assert_impl::<WindowsExtensions>();
    }
}
