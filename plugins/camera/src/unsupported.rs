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
