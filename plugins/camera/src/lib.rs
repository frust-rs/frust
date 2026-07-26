//! `frust-camera`: a platform-independent camera API — permission, preview,
//! still capture, and a YUV/BGRA image stream — CameraX on Android
//! (`dev.frust.camera.FrustCameraHost`, driven over this plugin's own JNI
//! surface), AVFoundation on iOS (driven from Rust directly via
//! `objc2-av-foundation`, no Swift camera glue).
//!
//! # Charter: a platform plugin
//!
//! Like [`frust-shared-preferences`](../frust_shared_preferences/index.html)
//! and
//! [`frust-secure-storage`](../frust_secure_storage/index.html), this is a
//! **platform plugin** (see `docs/ARCHITECTURE.md`'s Module Structure): it
//! depends on `frust-plugin` plus FFI crates only, and carries **no other
//! `frust-*` framework dependency**. An app adds this crate to its own
//! `Cargo.toml` alongside `frust`; the facade does not depend on or
//! re-export it.
//!
//! # Preview is a platform view, not a widget
//!
//! Unlike a typical plugin, this crate never paints its own preview.
//! [`CameraSession::preview_view_type`] returns the `viewType` string an app
//! feeds straight into `frust::platform_view` (the `frust-widgets`
//! `platform_view` module) — the native-sibling compositing slot
//! `workflow/plans/features/frust-camera/PLAN.md`'s Phase 0 device-proved.
//! The string is deliberately **target-gated** (platform-views W5 finding,
//! restated on [`CameraSession::preview_view_type`]'s doc): Android returns a
//! fully-qualified class name, iOS a bare runtime name — never one shared
//! literal.
//!
//! # Backends
//!
//! The crate landed as the **crate + frozen contract** (Plan Phase 1
//! preamble, task 02) and the real backends filled in behind that unchanged
//! API: [`android`] (task 06, CameraX via `FrustCameraHost.kt`), [`apple`]
//! (task 07, AVFoundation), and the image stream on both (task 09). On every
//! other target — desktop preview, wasm, anything without a camera backend —
//! [`unsupported`] fails every call soft with
//! [`CameraError::PlatformNotInitialized`], never a panic (the
//! `frust-shared-preferences` old-scaffold-degrade shape).
//!
//! One capability is deliberately **not** symmetric:
//! [`ImageFormat::Bgra`] is an Apple-only stream format (see its own doc).
//!
//! # Blocking API — pair with `spawn_blocking`, never the UI thread
//!
//! **Two** calls block until the platform answers. Like
//! `frust-secure-storage`'s gated calls, callers pair each with
//! `frust_reactive::spawn_blocking`:
//!
//! | Call | Blocks until | Deadline (Android / Apple) |
//! |---|---|---|
//! | [`Camera::request_permission`] | the permission machinery resolves (a system dialog on Android; `AVCaptureDevice`'s `requestAccessForMediaType:completionHandler:` on iOS) | 120 s / 120 s |
//! | [`CameraSession::take_picture`] | the photo has been written to disk, or the capture failed | 15 s / 10 s |
//!
//! **Never call either on the UI thread.** On Android every answer above is
//! relayed through the main `Looper` (CameraX's completion callbacks, the
//! Activity lifecycle the permission relay rides on), so a UI-thread caller
//! would park on the very queue carrying its own wake-up — a self-deadlock
//! that could only end at the deadline above, far past Android's ~5 s ANR
//! threshold. On Apple the shape differs — `requestAccess`'s completion runs
//! on an arbitrary queue — but the consent alert still needs a free main
//! thread to be presented, so a UI-thread caller waits out the deadline for a
//! dialog its own wait is suppressing.
//!
//! Both backends therefore **fail fast** instead of parking: a call made on
//! the platform's UI thread (Android: `Looper.myLooper() ==
//! Looper.getMainLooper()`; Apple: the main run loop) returns
//! [`CameraError::UiThread`] immediately, before any platform work starts.
//! The guard covers every path that can block. Calls that answer without
//! waiting are exempt and remain callable from anywhere: on Apple,
//! [`Camera::request_permission`] when the authorization status is already
//! decided (granted/denied/restricted) returns straight away and is never
//! refused — only the prompt-and-wait path is guarded.

// Platform backends (Plan Phase 1 preamble lands the crate + frozen contract;
// Phases 1-3 / tasks 06/07/09 fill the real implementations behind the same
// `SessionBackend` trait). Every real target routes through `Camera::open`'s
// selection point below.
#[cfg(target_os = "android")]
mod android;
#[cfg(target_vendor = "apple")]
mod apple;
#[cfg(not(any(target_os = "android", target_vendor = "apple")))]
mod unsupported;

use std::path::Path;
use std::sync::Arc;

/// Which physical camera to open.
///
/// Maps to CameraX's `CameraSelector.LENS_FACING_*` on Android (`Back` = 1,
/// `Front` = 0 — `androidx.camera.core.CameraSelector`, a published
/// constant, not community-approximate) and `AVCaptureDevice.Position` on
/// iOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Lens {
    /// The rear-facing camera — the default for still capture.
    Back,
    /// The front-facing (selfie) camera.
    Front,
}

/// The capture/preview resolution to request.
///
/// v1 is intentionally minimal — [`CameraSession::preview_aspect_ratio`]
/// tells the app how to size its preview slot regardless of which variant
/// was requested, so [`Resolution`] only needs to steer the platform's own
/// selection, never dictate exact output dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Resolution {
    /// The platform's own default selection (CameraX's un-configured
    /// `Preview`/`ImageCapture` resolution strategy; AVFoundation's
    /// `AVCaptureSessionPresetHigh`).
    Auto,
    /// A specific target size, best-effort (CameraX `ResolutionSelector`;
    /// the nearest AVFoundation `AVCaptureSessionPreset` on iOS). A platform
    /// may deliver a different actual size — always read it back from the
    /// delivered frame/photo, never assume this round-trips exactly.
    Explicit {
        /// Target width in pixels.
        width: u32,
        /// Target height in pixels.
        height: u32,
    },
}

/// The result of [`Camera::request_permission`].
///
/// Mirrors the Android JNI contract's `requestPermission` return codes
/// (`0`/`1`/`2`/`3` — see the [`android`] module doc's contract table);
/// iOS's `AVAuthorizationStatus` collapses onto the same four variants
/// (`authorized`/`denied`+`restricted`/n/a/`notDetermined`, the last two
/// folded per iOS having no "no cached Activity yet" concept).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PermissionStatus {
    /// The app may use the camera.
    Granted,
    /// The user (or a device policy) denied access.
    Denied,
    /// No cached Activity exists yet to show the system permission dialog
    /// (Android-specific — see the module doc's *Preview is a platform
    /// view* / PLAN.md's permission-plumbing risk: request again once a
    /// `platform_view` preview slot exists).
    NeedsUi,
    /// The system dialog is showing; the resolved status arrives
    /// asynchronously (Android: `nativeOnPermissionResult`).
    Pending,
}

/// A delivered image-stream frame's pixel format — v1 is exactly
/// the two Flutter `camera`-parity formats: Android `ImageFormat.YUV_420_888`
/// / iOS `kCVPixelFormatType_420YpCbCr8BiPlanarFullRange` ("420f") for
/// [`Yuv420`](Self::Yuv420), and a packed 32-bit BGRA buffer for
/// [`Bgra`](Self::Bgra).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImageFormat {
    /// YUV 4:2:0, planar (Android: 3 planes) or bi-planar (iOS: 2) — the
    /// format **both** platforms deliver, and the one a cross-platform
    /// callback should ask for.
    Yuv420,
    /// Packed 32-bit BGRA — 1 [`ImagePlane`]. **Apple-only.**
    ///
    /// [`CameraSession::start_image_stream`] reports
    /// [`CameraError::Platform`] for this format on Android: CameraX's only
    /// packed-32-bit `ImageAnalysis` output is `RGBA_8888` (byte order
    /// `R,G,B,A`), so no zero-copy BGRA buffer exists to hand over. Flutter's
    /// `camera` plugin draws the same platform line.
    Bgra,
}

/// One image plane of a delivered [`ImageFrame`]: a **borrowed**,
/// zero-copy view onto the platform's own buffer, valid only for the
/// duration of the [`CameraSession::start_image_stream`] callback — see that
/// method's close-deadline contract.
#[derive(Debug)]
#[non_exhaustive]
pub struct ImagePlane<'a> {
    /// The plane's raw bytes (`GetDirectBufferAddress` on Android,
    /// `CVPixelBufferGetBaseAddressOfPlane` on iOS — both zero-copy).
    pub data: &'a [u8],
    /// Bytes between the start of consecutive rows.
    pub row_stride: usize,
    /// Bytes between consecutive pixels within a row (Android's
    /// `Plane.pixelStride`; always `1` for a tightly-packed iOS plane).
    pub pixel_stride: usize,
}

/// One delivered camera frame.
///
/// # Close-deadline contract
///
/// [`CameraSession::start_image_stream`]'s callback receives this by
/// reference: every [`ImagePlane::data`] slice borrows the platform's own
/// in-flight buffer, held open only until the callback **returns**. The
/// callback must copy or fully consume the data before returning — Android's
/// `ImageProxy.close()` runs only after the JNI call back into Kotlin
/// completes, and with `STRATEGY_KEEP_ONLY_LATEST` a deferred `close()`
/// **stalls every subsequent frame** (only one may be in flight); on Apple
/// the `CVPixelBuffer` read lock is released the moment the callback returns
/// and AVFoundation recycles the buffer, while a slow callback blocks its own
/// serial delivery queue and the frames behind it are discarded
/// (`alwaysDiscardsLateVideoFrames`). Never store an
/// [`ImageFrame`]/[`ImagePlane`] past the callback's return.
#[derive(Debug)]
#[non_exhaustive]
pub struct ImageFrame<'a> {
    /// The frame's pixel format — always the format requested via
    /// [`CameraSession::start_image_stream`].
    pub format: ImageFormat,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Clockwise rotation, in degrees, needed to present the frame upright
    /// (device-orientation-dependent).
    pub rotation_degrees: i32,
    /// The frame's planes, in platform-native order (1 for
    /// [`ImageFormat::Bgra`], 2-3 for [`ImageFormat::Yuv420`]).
    pub planes: &'a [ImagePlane<'a>],
}

/// A [`CameraSession::start_image_stream`] frame callback. Runs on a
/// plugin-owned thread (Android's `ImageAnalysis` executor; iOS's serial
/// sample-buffer-delegate queue) — **never** the UI thread, and never do a
/// signal write from inside it (`docs/CODE_STANDARDS.md`'s heavy-work
/// routing rule); hand frames off via `frust_reactive::use_task` or a
/// signal write scheduled back onto the UI thread.
pub type ImageFrameCallback = dyn Fn(&ImageFrame<'_>) + Send + 'static;

/// Errors from a [`Camera`]/[`CameraSession`] operation.
///
/// `thiserror`-derived per `docs/CODE_STANDARDS.md`: callers match on the
/// variant (e.g. distinguishing [`Self::PermissionDenied`] from
/// [`Self::InUse`] to decide whether to prompt the user or just retry)
/// rather than only displaying it.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum CameraError {
    /// The camera permission was denied (a synchronous convenience some
    /// operations report directly, distinct from
    /// [`PermissionStatus::Denied`]'s async probe result).
    #[error("camera permission was denied")]
    PermissionDenied,

    /// This platform has no camera backend at all ([`unsupported`]'s
    /// fail-soft contract — desktop preview, wasm, anything not Android or
    /// Apple), or, on Android, the host shell never installed the
    /// `(JavaVM, Context)` platform handles this crate's JNI calls need (an
    /// old scaffold predating `nativeInitPlatform`). Never a panic.
    #[error("camera platform not initialized")]
    PlatformNotInitialized,

    /// The camera is already open elsewhere (another [`CameraSession`], or
    /// another app holding exclusive access).
    #[error("the camera is already in use")]
    InUse,

    /// An operation was attempted on a [`CameraSession`] after
    /// [`CameraSession::close`].
    #[error("the camera session is closed")]
    SessionClosed,

    /// A **blocking** call (the module doc's *Blocking API* table:
    /// [`Camera::request_permission`], [`CameraSession::take_picture`]) was
    /// made on the platform's UI thread, where parking on a platform answer
    /// deadlocks the thread that has to deliver it. The call is refused
    /// immediately, before any platform work starts — re-issue it from
    /// `frust_reactive::spawn_blocking`.
    ///
    /// A fail-fast guard, not a capability report: nothing about the camera
    /// is wrong, only the calling thread.
    #[error(
        "camera call refused: this is a blocking call and was made on the UI thread — re-issue it \
         from `spawn_blocking`"
    )]
    UiThread,

    /// An underlying file I/O operation failed (e.g.
    /// [`CameraSession::take_picture`] writing to an unwritable path).
    #[error("camera I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A backend-specific failure that isn't one of the above — including a
    /// capability the requesting platform does not have (the module doc's
    /// *Backends*, e.g. [`ImageFormat::Bgra`] on Android).
    #[error("camera platform error: {0}")]
    Platform(String),
}

/// One backend implementation — [`android`]'s `AndroidSession` on Android,
/// [`apple`]'s `AppleSession` on Apple targets. [`unsupported`] never
/// constructs one: every [`Camera`] call fails before a session would exist.
///
/// Crate-private and deliberately mirrors [`CameraSession`]'s public shape
/// one-to-one — [`CameraSession`] is a thin dispatch wrapper over this.
pub(crate) trait SessionBackend: Send + Sync {
    /// See [`CameraSession::preview_view_type`].
    fn preview_view_type(&self) -> &'static str;
    /// See [`CameraSession::params_json`].
    fn params_json(&self) -> String;
    /// See [`CameraSession::preview_aspect_ratio`].
    fn preview_aspect_ratio(&self) -> f32;
    /// See [`CameraSession::take_picture`].
    fn take_picture(&self, path: &Path) -> Result<(), CameraError>;
    /// See [`CameraSession::start_image_stream`].
    fn start_image_stream(
        &self,
        format: ImageFormat,
        on_frame: Box<ImageFrameCallback>,
    ) -> Result<(), CameraError>;
    /// See [`CameraSession::stop_image_stream`].
    fn stop_image_stream(&self);
    /// See [`CameraSession::close`].
    fn close(&self);
}

/// The camera entry point — no instance, just [`Self::request_permission`]
/// and [`Self::open`].
pub struct Camera;

impl Camera {
    /// Request camera permission, blocking until the platform resolves it
    /// (or reports [`PermissionStatus::NeedsUi`]/
    /// [`PermissionStatus::Pending`] — see that type's docs).
    ///
    /// Pair with `frust_reactive::spawn_blocking`; never call on the UI
    /// thread — a UI-thread call is refused with [`CameraError::UiThread`]
    /// rather than parking (module doc's *Blocking API*).
    ///
    /// # Errors
    /// [`CameraError::UiThread`] when called on the platform's UI thread;
    /// [`CameraError::PlatformNotInitialized`] on a platform with no camera
    /// backend, or an old Android scaffold predating `nativeInitPlatform`;
    /// [`CameraError::Platform`] on a platform failure (a JNI error, a
    /// missing AVFoundation constant).
    pub fn request_permission() -> Result<PermissionStatus, CameraError> {
        #[cfg(target_os = "android")]
        {
            android::request_permission()
        }
        #[cfg(target_vendor = "apple")]
        {
            apple::request_permission()
        }
        #[cfg(not(any(target_os = "android", target_vendor = "apple")))]
        {
            unsupported::request_permission()
        }
    }

    /// Open a [`CameraSession`] on the given [`Lens`] at the requested
    /// [`Resolution`] (best-effort — see that type's docs).
    ///
    /// Session configuration is asynchronous on every real backend (CameraX,
    /// AVFoundation): a returned [`CameraSession`] does not yet guarantee a
    /// live camera — read [`CameraSession::preview_aspect_ratio`] (`0.0`
    /// until the platform reports its first frame geometry) and drive
    /// [`Self::request_permission`] alongside it, per PLAN.md's permission-
    /// plumbing note (permission is requested once a `platform_view` preview
    /// slot exists, not necessarily before `open`).
    ///
    /// # Errors
    /// [`CameraError::PlatformNotInitialized`] on a platform with no camera
    /// backend (module doc's *Backends*); [`CameraError::PermissionDenied`],
    /// [`CameraError::InUse`], or [`CameraError::Platform`] when the platform
    /// refuses to open the device.
    // Split by target at the function level (rather than an early-return
    // arm inside one body) so neither cfg configuration leaves the other's
    // dead branch for clippy's `needless_return`/`unreachable_code` to trip
    // over — each version below is a complete, self-contained definition.
    #[cfg(any(target_os = "android", target_vendor = "apple"))]
    pub fn open(lens: Lens, resolution: Resolution) -> Result<CameraSession, CameraError> {
        #[cfg(target_os = "android")]
        let backend: Arc<dyn SessionBackend> =
            Arc::new(android::AndroidSession::open(lens, resolution)?);
        #[cfg(target_vendor = "apple")]
        let backend: Arc<dyn SessionBackend> =
            Arc::new(apple::AppleSession::open(lens, resolution)?);

        Ok(CameraSession { backend })
    }

    /// No camera backend exists on this target at all (module doc's
    /// *Backends*) — fail soft before ever constructing a session,
    /// matching [`unsupported::request_permission`]'s arm above.
    #[cfg(not(any(target_os = "android", target_vendor = "apple")))]
    pub fn open(lens: Lens, resolution: Resolution) -> Result<CameraSession, CameraError> {
        let _ = (lens, resolution);
        unsupported::open()
    }
}

/// An open camera — preview slot wiring, still capture, and an image stream.
/// Cheap to hold; every operation dispatches to the platform backend
/// [`Camera::open`] selected.
pub struct CameraSession {
    backend: Arc<dyn SessionBackend>,
}

impl CameraSession {
    /// The `viewType` string to pass to `frust::platform_view` for this
    /// session's live preview slot.
    ///
    /// **Target-gated — never one shared literal** (platform-views W5
    /// finding, `docs/CODE_STANDARDS.md`'s Naming Conventions LAW): Android
    /// returns the fully-qualified `"dev.frust.camera.CameraPreviewFactory"`
    /// (the embedding module's `FrustViewHost` resolves it via the app
    /// classloader), iOS the bare `"CameraPreviewFactory"` (resolved via
    /// `NSClassFromString`). This value is real today, independent of the
    /// rest of this crate's phased-delivery stub bodies (module doc).
    pub fn preview_view_type(&self) -> &'static str {
        self.backend.preview_view_type()
    }

    /// The current preview-slot parameters, as JSON — the payload an app
    /// threads into `platform_view(...).params(...)` so a params update
    /// (e.g. a session id change) reaches the native factory without a
    /// slot teardown/recreate.
    pub fn params_json(&self) -> String {
        self.backend.params_json()
    }

    /// The live preview's width/height aspect ratio, or `0.0` before the
    /// platform has reported its first frame geometry (Android: before the
    /// first `TransformationInfo`; matches the JNI contract's
    /// `previewAspectRatio` return-code table). An app sizes its preview
    /// slot off this value rather than a fixed aspect.
    pub fn preview_aspect_ratio(&self) -> f32 {
        self.backend.preview_aspect_ratio()
    }

    /// Capture a still photo to `path`, **blocking** until the platform
    /// reports the capture finished (Android: `nativeOnPictureTaken`; Apple:
    /// the photo-capture delegate) or the per-backend deadline elapses — 15 s
    /// Android / 10 s Apple, the module doc's *Blocking API* table.
    ///
    /// Pair with `frust_reactive::spawn_blocking`; never call on the UI
    /// thread — a UI-thread call is refused with [`CameraError::UiThread`]
    /// rather than parking (module doc's *Blocking API*).
    ///
    /// Each attempt is correlated with its own completion (Android: a
    /// Rust-minted request id echoed back through `nativeOnPictureTaken`;
    /// Apple: a per-capture channel bound to one delegate), so a late answer
    /// to a timed-out capture can never resolve a later one. `Ok(())`
    /// therefore means **this** call's photo was written to `path`.
    ///
    /// # Errors
    /// [`CameraError::UiThread`] when called on the platform's UI thread;
    /// [`CameraError::SessionClosed`] after [`Self::close`];
    /// [`CameraError::Io`] if `path` can't be written;
    /// [`CameraError::Platform`] if the platform refuses the capture, reports
    /// a failed one, or never completes it within the deadline.
    pub fn take_picture(&self, path: &Path) -> Result<(), CameraError> {
        self.backend.take_picture(path)
    }

    /// Start delivering [`ImageFrame`]s of the requested [`ImageFormat`] to
    /// `on_frame` at camera rate, lossy-latest (a slow consumer drops
    /// frames rather than queuing). See [`ImageFrame`]'s close-deadline
    /// contract and [`ImageFrameCallback`]'s threading contract.
    ///
    /// Never blocks on a frame; calling it again restarts the stream at the
    /// newly requested format.
    ///
    /// # Errors
    /// [`CameraError::SessionClosed`] after [`Self::close`];
    /// [`CameraError::Platform`] for [`ImageFormat::Bgra`] on Android (see
    /// that variant's doc), or if the platform refuses the stream.
    pub fn start_image_stream(
        &self,
        format: ImageFormat,
        on_frame: impl Fn(&ImageFrame<'_>) + Send + 'static,
    ) -> Result<(), CameraError> {
        self.backend.start_image_stream(format, Box::new(on_frame))
    }

    /// Stop a stream started with [`Self::start_image_stream`], without
    /// touching the preview. A no-op if no stream is running.
    pub fn stop_image_stream(&self) {
        self.backend.stop_image_stream();
    }

    /// Close the session and release the camera, stopping any running image
    /// stream with it. A no-op if already closed; every subsequent fallible
    /// [`CameraSession`] method call reports
    /// [`CameraError::SessionClosed`].
    pub fn close(&self) {
        self.backend.close();
    }
}

// --- Still-capture completion correlation ------------------------------------

/// Pure, platform-independent correlation of a still-capture **request** with
/// **its own** completion.
///
/// Only the Android backend (`android.rs`) routes through this: its
/// completions arrive as a fire-and-forget `nativeOnPictureTaken` callback
/// that has to be matched back to the caller waiting for it. The Apple
/// backend needs nothing here — it correlates structurally, giving each
/// capture its own `sync_channel` bound to one delegate, so a stale
/// completion has nowhere to land.
///
/// It lives here, in the platform-independent crate root, rather than inside
/// `android.rs` for one reason: `android.rs` compiles only for `--target
/// *-linux-android`, so nothing defined there can be exercised by `cargo
/// test` on any host. The correctness argument this type carries — *a late
/// completion for a timed-out or superseded request must never resolve the
/// one now in flight* — is the whole point, so it is checked by the tests
/// below rather than only asserted in a doc comment.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod capture {
    /// One session's in-flight still capture: the request id the blocked
    /// caller is waiting for, plus the completion that has landed **for that
    /// request**.
    #[derive(Debug, Default)]
    pub(crate) struct CaptureSlot {
        /// The last id [`Self::arm`] minted. Ids are per-session, start at
        /// `1`, and are never reused, so `0` is a safe "no request" sentinel
        /// where one has to cross an FFI boundary.
        last_request_id: i64,
        /// The request the current caller is blocked on, if any.
        awaited: Option<i64>,
        /// The completion delivered for [`Self::awaited`] — never for any
        /// other request (see [`Self::deliver`]).
        outcome: Option<bool>,
    }

    impl CaptureSlot {
        /// Mint the next request id and arm the wait on it, discarding any
        /// previous request's state: that caller has either already read its
        /// outcome or given up at its deadline, so its completion is stale by
        /// definition.
        pub(crate) fn arm(&mut self) -> i64 {
            // `wrapping_add` rather than `+`: an overflow panic near an FFI
            // boundary is worse than a wrap that needs 2^63 captures in one
            // session to reach.
            self.last_request_id = self.last_request_id.wrapping_add(1);
            self.awaited = Some(self.last_request_id);
            self.outcome = None;
            self.last_request_id
        }

        /// Record `request_id`'s completion.
        ///
        /// Returns `false` — recording **nothing** — when the completion is
        /// not the awaited request's. That is the whole guard: a late answer
        /// to a timed-out or superseded capture must never resolve the one
        /// now in flight, or the caller would believe a photo was written
        /// when none was. The caller logs the drop.
        pub(crate) fn deliver(&mut self, request_id: i64, ok: bool) -> bool {
            if self.awaited != Some(request_id) {
                return false;
            }
            self.outcome = Some(ok);
            true
        }

        /// The completion for `request_id`: `Some(ok)` once it has landed,
        /// `None` while it is still outstanding — and `None` forever for a
        /// request some later [`Self::arm`] superseded.
        pub(crate) fn outcome(&self, request_id: i64) -> Option<bool> {
            if self.awaited == Some(request_id) {
                self.outcome
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::capture::CaptureSlot;

    #[test]
    fn a_completion_resolves_its_own_request() {
        let mut slot = CaptureSlot::default();
        let request = slot.arm();

        assert_eq!(slot.outcome(request), None, "outstanding before delivery");
        assert!(
            slot.deliver(request, true),
            "the awaited request is accepted"
        );
        assert_eq!(slot.outcome(request), Some(true));
    }

    #[test]
    fn a_failed_completion_is_reported_as_such() {
        let mut slot = CaptureSlot::default();
        let request = slot.arm();

        assert!(slot.deliver(request, false));
        assert_eq!(slot.outcome(request), Some(false));
    }

    /// The review's root cause C, as a test: capture #1 times out, capture #2
    /// arms, then #1's completion finally arrives. It must NOT resolve #2.
    #[test]
    fn a_late_completion_never_resolves_the_request_that_superseded_it() {
        let mut slot = CaptureSlot::default();
        let timed_out = slot.arm();
        let in_flight = slot.arm();
        assert_ne!(timed_out, in_flight, "each request gets its own id");

        assert!(
            !slot.deliver(timed_out, true),
            "a completion for the superseded request is dropped"
        );
        assert_eq!(
            slot.outcome(in_flight),
            None,
            "the in-flight capture is still outstanding"
        );
        assert_eq!(
            slot.outcome(timed_out),
            None,
            "and the superseded one can never settle either"
        );

        // The real completion still resolves the in-flight request normally.
        assert!(slot.deliver(in_flight, true));
        assert_eq!(slot.outcome(in_flight), Some(true));
        assert_eq!(
            slot.outcome(timed_out),
            None,
            "the superseded request never reads the newer outcome"
        );
    }

    #[test]
    fn a_completion_for_an_unarmed_slot_is_dropped() {
        let mut slot = CaptureSlot::default();

        // Nothing armed: a host-side completion (a session torn down and
        // reopened, a version-skewed host) has no caller to resolve.
        assert!(!slot.deliver(1, true));
        assert_eq!(slot.outcome(1), None);
    }

    #[test]
    fn request_ids_are_unique_and_start_at_one() {
        let mut slot = CaptureSlot::default();

        assert_eq!(slot.arm(), 1, "0 stays free as an FFI-side sentinel");
        assert_eq!(slot.arm(), 2);
        assert_eq!(slot.arm(), 3);
    }
}
