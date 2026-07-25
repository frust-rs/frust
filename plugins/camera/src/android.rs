//! The Android backend — CameraX, driven from a `dev.frust.camera.FrustCameraHost`
//! Kotlin helper (`plugins/camera/platform/android/`) over this crate's own
//! JNI surface. **Task 06** wires the real Rust→Kotlin calls and the
//! `nativeOn*` export bodies below; this module (task 02) fixes the API
//! shape and the JNI symbol surface — every operation beyond
//! [`AndroidSession::preview_view_type`] reports
//! [`CameraError::Platform`]`("not yet implemented")`.
//!
//! # The frozen contract
//!
//! **Tasks 05 (`FrustCameraHost.kt`) and 06 (this module's real backend)
//! build to this table — changing it means updating both task files
//! first.** `dev.frust.camera` is a subpackage of the embedding module's
//! `dev.frust` (`dev.frust` is `frust-embedding`'s exclusive package —
//! `docs/CODE_STANDARDS.md`'s Plugin Conventions); `FrustCameraHost`'s
//! package is baked into every JNI symbol name below, so it may never move
//! once shipped.
//!
//! ## Rust → Kotlin (static methods on `FrustCameraHost`, resolved via
//! `context.getClassLoader().loadClass(...)` — the `FrustBiometric`
//! mechanism, `plugins/secure-storage/src/android.rs`)
//!
//! | Method | Signature (Java) | Notes |
//! |---|---|---|
//! | `requestPermission` | `() -> int` | `0` [`PermissionStatus::Granted`] / `1` [`PermissionStatus::Denied`] / `2` [`PermissionStatus::NeedsUi`] (no Activity cached yet) / `3` [`PermissionStatus::Pending`] (dialog shown; result arrives via [`Java_dev_frust_camera_FrustCameraHost_nativeOnPermissionResult`]) |
//! | `openCamera` | `(int lensFacing) -> int` | ≥0 session id; <0 error code. Async config; state via [`Java_dev_frust_camera_FrustCameraHost_nativeOnCameraState`] |
//! | `closeCamera` | `(int session) -> void` | |
//! | `takePicture` | `(int session, String path) -> int` | `0` started; completion via [`Java_dev_frust_camera_FrustCameraHost_nativeOnPictureTaken`] |
//! | `previewAspectRatio` | `(int session) -> float` | `0.0` until the first `TransformationInfo` |
//! | `startImageStream` | `(int session, int format) -> int` | wired in task 09 |
//! | `stopImageStream` | `(int session) -> void` | task 09 |
//!
//! ## Kotlin → Rust (this crate's own `#[unsafe(no_mangle)]` JNI exports —
//! package baked into the symbol names, so `dev.frust.camera.FrustCameraHost`
//! may never move once shipped)
//!
//! - [`Java_dev_frust_camera_FrustCameraHost_nativeOnPermissionResult`]`(env, class, granted: jboolean)`
//! - [`Java_dev_frust_camera_FrustCameraHost_nativeOnCameraState`]`(env, class, session: jint, state: jint)` — `0` Configuring / `1` Running / `2` Closed / `3` Error
//! - [`Java_dev_frust_camera_FrustCameraHost_nativeOnPictureTaken`]`(env, class, session: jint, ok: jboolean, path: JString)`
//! - (task 09 adds `nativeOnImageFrame`)
//!
//! Every export below is stubbed log-only (task 02) so the symbol surface is
//! fixed; task 06 fills the real handling. Each upgrades its
//! [`jni::EnvUnowned`] via [`jni::EnvUnowned::with_env`], which wraps the
//! body in `catch_unwind` — the crate's own no-unwind-across-FFI guarantee
//! (`docs/CODE_STANDARDS.md`'s Language Idioms), matching the pattern
//! `frust-shell-android`'s own JNI exports use.

use std::path::Path;

use jni::EnvUnowned;
use jni::errors::LogErrorAndDefault;
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jint};

use crate::{
    CameraError, ImageFormat, ImageFrameCallback, Lens, PermissionStatus, Resolution,
    SessionBackend,
};

/// The `viewType` this crate's Android preview slot resolves to — the
/// fully-qualified class name the embedding module's `FrustViewHost` looks
/// up via the app classloader (`docs/CODE_STANDARDS.md`'s platform-view
/// factory `viewType` LAW). Shipped by `plugins/camera/platform/android/`'s
/// `CameraPreviewFactory.kt` (task 05).
const PREVIEW_VIEW_TYPE: &str = "dev.frust.camera.CameraPreviewFactory";

/// [`crate::Camera::request_permission`]'s Android arm.
///
/// Task 06 fills the real `FrustCameraHost.requestPermission()` call (the
/// contract table above); until then every call reports
/// [`CameraError::Platform`].
pub(crate) fn request_permission() -> Result<PermissionStatus, CameraError> {
    Err(not_yet_implemented("request_permission"))
}

/// The Android [`SessionBackend`] — one CameraX session owned by
/// `FrustCameraHost.kt`, referenced here by its integer session id.
pub(crate) struct AndroidSession {
    /// The id `FrustCameraHost.openCamera` will return (task 06). This
    /// phase never calls it, so every session holds the sentinel `-1`.
    session_id: i32,
}

impl AndroidSession {
    /// [`crate::Camera::open`]'s Android arm.
    ///
    /// Session opening on the real backend is asynchronous (state arrives
    /// via [`Java_dev_frust_camera_FrustCameraHost_nativeOnCameraState`]),
    /// so this succeeds even during this crate's stub phase — the returned
    /// session is inert until task 06 lands the real
    /// `FrustCameraHost.openCamera` call; every capture/stream operation
    /// reports [`CameraError::Platform`] until then.
    pub(crate) fn open(_lens: Lens, _resolution: Resolution) -> Result<Self, CameraError> {
        Ok(Self { session_id: -1 })
    }
}

impl SessionBackend for AndroidSession {
    fn preview_view_type(&self) -> &'static str {
        PREVIEW_VIEW_TYPE
    }

    fn params_json(&self) -> String {
        format!(r#"{{"sessionId":{}}}"#, self.session_id)
    }

    fn preview_aspect_ratio(&self) -> f32 {
        // `0.0` until the platform reports its first `TransformationInfo` —
        // the contract table's own documented initial value, not a stub
        // placeholder.
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
        log::debug!(
            "frust-camera: close(session_id={}) — stub, task 06 fills",
            self.session_id
        );
    }
}

/// Every stub operation's error, named per call site so a log/error message
/// says exactly what hasn't landed yet (module doc's *Phased delivery*).
fn not_yet_implemented(op: &str) -> CameraError {
    CameraError::Platform(format!(
        "android camera backend: {op} not yet implemented (task 06)"
    ))
}

// --- Kotlin -> Rust JNI exports (contract table above) ---------------------

/// `Java_dev_frust_camera_FrustCameraHost_nativeOnPermissionResult` — fires
/// once the system permission dialog resolves
/// ([`PermissionStatus::Pending`]'s eventual answer). Log-only stub; task 06
/// wires this into whatever pending-request state tracks the caller's
/// original [`crate::Camera::request_permission`] call.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_camera_FrustCameraHost_nativeOnPermissionResult<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    granted: jboolean,
) {
    env.with_env(|_env| {
        log::debug!(
            "frust-camera: nativeOnPermissionResult(granted={granted}) — stub, task 06 fills"
        );
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `Java_dev_frust_camera_FrustCameraHost_nativeOnCameraState` — `state` is
/// `0` Configuring / `1` Running / `2` Closed / `3` Error (contract table
/// above). Log-only stub; task 06 wires this into the owning
/// [`AndroidSession`]'s tracked state.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_camera_FrustCameraHost_nativeOnCameraState<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    session: jint,
    state: jint,
) {
    env.with_env(|_env| {
        log::debug!(
            "frust-camera: nativeOnCameraState(session={session}, state={state}) — stub, task 06 \
             fills"
        );
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `Java_dev_frust_camera_FrustCameraHost_nativeOnPictureTaken` — completion
/// callback for [`AndroidSession::take_picture`] (contract table above).
/// Log-only stub; task 06 wires this into the caller's original
/// [`crate::CameraSession::take_picture`] result.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_camera_FrustCameraHost_nativeOnPictureTaken<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    session: jint,
    ok: jboolean,
    path: JString<'local>,
) {
    env.with_env(|_env| {
        log::debug!(
            "frust-camera: nativeOnPictureTaken(session={session}, ok={ok}, path={path}) — stub, \
             task 06 fills"
        );
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}
