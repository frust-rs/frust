//! The inert backend for every target that is neither Android nor Apple
//! (desktop Linux/Windows preview, wasm, anything else this crate has no
//! camera backend for).
//!
//! Mirrors `frust-shared-preferences`' old-scaffold-degrade shape: every
//! call fails soft with [`CameraError::PlatformNotInitialized`], never a
//! panic, and — unlike [`crate::android`]/[`crate::apple`] — never even
//! constructs a [`CameraSession`], since there is no backend for one to
//! wrap.

use crate::{CameraError, CameraSession, PermissionStatus};

/// [`crate::Camera::request_permission`]'s unsupported-platform arm.
pub(crate) fn request_permission() -> Result<PermissionStatus, CameraError> {
    Err(CameraError::PlatformNotInitialized)
}

/// [`crate::Camera::open`]'s unsupported-platform arm.
pub(crate) fn open() -> Result<CameraSession, CameraError> {
    Err(CameraError::PlatformNotInitialized)
}

/// [`CameraSession::set_torch`]'s unsupported-platform arm — the same
/// fail-soft error every other call reports here, never a panic.
///
/// Reachable only in principle: [`open`] above refuses before a
/// [`CameraSession`] exists to call it on, so this arm exists to keep the
/// public API's contract answerable on *every* target rather than leaving one
/// method's behavior undefined where no backend exists.
pub(crate) fn set_torch() -> Result<(), CameraError> {
    Err(CameraError::PlatformNotInitialized)
}

/// [`CameraSession::torch_available`]'s unsupported-platform arm.
///
/// `false`, not an error: the method is infallible by API shape (a caller
/// only uses it to decide whether to offer a torch control), and no target
/// without a camera backend has a torch to control.
pub(crate) fn torch_available() -> bool {
    false
}

#[cfg(test)]
mod tests {
    //! Compiled and run only on a target with **no** camera backend — i.e.
    //! neither the Android nor the Apple compile gate, and not a macOS
    //! `cargo test --workspace` host either (macOS is `target_vendor =
    //! "apple"`, so it builds `crate::apple`). A plain Linux/Windows host's
    //! `cargo test` runs them; `cargo check --all-targets --target
    //! wasm32-unknown-unknown -p frust-camera` compiles them anywhere.

    use super::*;

    #[test]
    fn every_entry_point_fails_soft_rather_than_panicking() {
        assert!(matches!(
            request_permission(),
            Err(CameraError::PlatformNotInitialized)
        ));
        assert!(matches!(
            open().map(|_| ()),
            Err(CameraError::PlatformNotInitialized)
        ));
        assert!(matches!(
            set_torch(),
            Err(CameraError::PlatformNotInitialized)
        ));
    }

    #[test]
    fn torch_is_never_available_without_a_backend() {
        assert!(!torch_available());
    }
}
