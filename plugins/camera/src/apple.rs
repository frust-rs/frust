//! The Apple (iOS; macOS desktop preview shares this arm like every other
//! plugin) backend — AVFoundation, driven **from Rust directly** via
//! `objc2-av-foundation` (no Swift camera glue; the Swift package task 08
//! ships is the preview `CameraPreviewFactory` view only).
//!
//! # Phased delivery
//!
//! **Task 07** wires the real `AVCaptureSession`/`AVCaptureDevice`/
//! `AVCapturePhotoOutput` calls this module's `Cargo.toml` dependencies
//! (`plugins/camera/Cargo.toml`'s target-gated Apple section, pinned in full
//! now per task 02 — "Cargo.toml complete up front" — so task 07 edits no
//! manifest) already support. This module (task 02) fixes the API shape
//! only: every operation beyond [`AppleSession::preview_view_type`] reports
//! [`CameraError::Platform`]`("not yet implemented")`.
//!
//! Unlike [`crate::android`], there is no cross-language JNI contract here —
//! Rust talks to AVFoundation directly, so there is nothing to freeze beyond
//! this crate's own public API (`crate` module doc) and the C export a later
//! phase adds (`frust_camera_session_handle()`, PLAN.md Phase 2 step 2 — not
//! this task).

use std::path::Path;

use crate::{
    CameraError, ImageFormat, ImageFrameCallback, Lens, PermissionStatus, Resolution,
    SessionBackend,
};

/// The `viewType` this crate's iOS preview slot resolves to — the bare
/// `@objc(...)` runtime name `FrustViewHost` looks up via
/// `NSClassFromString` (`docs/CODE_STANDARDS.md`'s platform-view factory
/// `viewType` LAW — iOS carries no package, unlike Android's fully-qualified
/// form). Shipped by `plugins/camera/platform/ios/`'s
/// `CameraPreviewFactory.swift` (task 08).
const PREVIEW_VIEW_TYPE: &str = "CameraPreviewFactory";

/// [`crate::Camera::request_permission`]'s Apple arm.
///
/// Task 07 fills the real `AVCaptureDevice.requestAccessForMediaType:
/// completionHandler:` bridge (module doc); until then every call reports
/// [`CameraError::Platform`].
pub(crate) fn request_permission() -> Result<PermissionStatus, CameraError> {
    Err(not_yet_implemented("request_permission"))
}

/// The Apple [`SessionBackend`] — an `AVCaptureSession` (task 07 fills the
/// actual handle; this phase holds none).
pub(crate) struct AppleSession;

impl AppleSession {
    /// [`crate::Camera::open`]'s Apple arm.
    ///
    /// Session configuration is asynchronous on the real backend
    /// (`beginConfiguration`/`commitConfiguration`), so this succeeds even
    /// during this crate's stub phase — the returned session is inert until
    /// task 07 lands the real `AVCaptureSession` wiring; every
    /// capture/stream operation reports [`CameraError::Platform`] until
    /// then.
    pub(crate) fn open(_lens: Lens, _resolution: Resolution) -> Result<Self, CameraError> {
        Ok(Self)
    }
}

impl SessionBackend for AppleSession {
    fn preview_view_type(&self) -> &'static str {
        PREVIEW_VIEW_TYPE
    }

    fn params_json(&self) -> String {
        "{}".to_string()
    }

    fn preview_aspect_ratio(&self) -> f32 {
        // `0.0` until the platform reports its first frame geometry —
        // matches the Android contract's documented initial value
        // (`crate::android`'s module doc), kept symmetric across backends.
        0.0
    }

    fn take_picture(&self, _path: &Path) -> Result<(), CameraError> {
        Err(not_yet_implemented("take_picture"))
    }

    fn start_image_stream(
        &self,
        _format: ImageFormat,
        _on_frame: Box<ImageFrameCallback>,
    ) -> Result<(), CameraError> {
        Err(not_yet_implemented("start_image_stream"))
    }

    fn stop_image_stream(&self) {
        log::debug!("frust-camera: stop_image_stream — stub, task 09 fills");
    }

    fn close(&self) {
        log::debug!("frust-camera: close — stub, task 07 fills");
    }
}

/// Every stub operation's error, named per call site so a log/error message
/// says exactly what hasn't landed yet (module doc's *Phased delivery*).
fn not_yet_implemented(op: &str) -> CameraError {
    CameraError::Platform(format!(
        "apple camera backend: {op} not yet implemented (task 07)"
    ))
}
