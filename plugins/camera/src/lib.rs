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
//! an earlier device test proved.
//! The string is deliberately **target-gated** (see
//! [`CameraSession::preview_view_type`]'s doc): Android returns a
//! fully-qualified class name, iOS a bare runtime name — never one shared
//! literal.
//!
//! # Backends
//!
//! The crate landed as the **crate + frozen contract** first,
//! and the real backends filled in behind that unchanged
//! API: [`android`] (CameraX via `FrustCameraHost.kt`), [`apple`]
//! (AVFoundation), and the image stream on both. On every
//! other target — desktop preview, wasm, anything without a camera backend —
//! [`unsupported`] fails every call soft with
//! [`CameraError::PlatformNotInitialized`], never a panic (the
//! `frust-shared-preferences` old-scaffold-degrade shape).
//!
//! One capability is deliberately **not** symmetric:
//! [`ImageFormat::Bgra`] is an Apple-only stream format (see its own doc).
//!
//! # Barcode decoding
//!
//! [`barcode`] is a platform-independent QR/barcode decoder over
//! [`ImageFrame`]s or a caller-supplied luma buffer — no camera involved,
//! and no `cfg` of its own, so it builds and runs identically on every
//! target this crate targets, including a plain desktop `cargo test`.
//! [`CameraSession::start_barcode_stream`] composes it directly with the
//! live image stream, sharing the session's single stream with
//! [`CameraSession::start_image_stream`] under explicit single-occupancy
//! rules — see [`CameraError::StreamBusy`].
//!
//! # Blocking API — pair with `spawn_blocking`, never the UI thread
//!
//! **Three** calls in the table below can block the calling thread — but
//! only the first two refuse to run on the platform's UI thread at all;
//! [`CameraSession::close`] carries no such guard and genuinely blocks it
//! (see the note below the table). Like `frust-secure-storage`'s gated
//! calls, callers pair each with `frust_reactive::spawn_blocking`:
//!
//! | Call | Blocks until | Deadline (Android / Apple) |
//! |---|---|---|
//! | [`Camera::request_permission`] | the permission machinery resolves (a system dialog on Android; `AVCaptureDevice`'s `requestAccessForMediaType:completionHandler:` on iOS) | 120 s / 120 s |
//! | [`CameraSession::take_picture`] | the photo has been written to disk, or the capture failed | 15 s / 10 s |
//! | [`CameraSession::close`] (dropping a [`CameraSession`] without calling it first runs the identical hop) | the capture device is released — Apple parks on the session's serial queue, which may still be working through [`Camera::open`]'s in-flight `startRunning()` or a just-issued stream-start attach; Android's JNI call only waits for the teardown to be *scheduled*, not finished | doesn't block (Android) / none — no timeout, by design (Apple) |
//!
//! **Never call [`Camera::request_permission`] or [`CameraSession::take_picture`]
//! on the UI thread.** On Android every answer above is relayed through the
//! main `Looper` (CameraX's completion callbacks, the Activity lifecycle the
//! permission relay rides on), so a UI-thread caller would park on the very
//! queue carrying its own wake-up — a self-deadlock that could only end at
//! the deadline above, far past Android's ~5 s ANR threshold. On Apple the
//! shape differs — `requestAccess`'s completion runs on an arbitrary queue —
//! but the consent alert still needs a free main thread to be presented, so
//! a UI-thread caller waits out the deadline for a dialog its own wait is
//! suppressing.
//!
//! Both therefore **fail fast** instead of parking: a call made on the
//! platform's UI thread (Android: `Looper.myLooper() ==
//! Looper.getMainLooper()`; Apple: the main run loop) returns
//! [`CameraError::UiThread`] immediately, before any platform work starts.
//! The guard covers every path that can block. Calls that answer without
//! waiting are exempt and remain callable from anywhere: on Apple,
//! [`Camera::request_permission`] when the authorization status is already
//! decided (granted/denied/restricted) returns straight away and is never
//! refused — only the prompt-and-wait path is guarded.
//!
//! [`CameraSession::close`] carries **no** [`CameraError::UiThread`] guard:
//! it never refuses the UI thread, it genuinely parks it — by design, since
//! the whole point of the wait is that a caller reopening right after (a
//! lens switch) can depend on the device having actually been released.
//! Call it — and let a [`CameraSession`] drop, since dropping one on Apple
//! runs the identical session-queue hop — off the UI thread whenever a
//! stream start may still be mid-flight on the session queue;
//! `frust_reactive::spawn_blocking` is still the right tool even though
//! nothing here reports [`CameraError::UiThread`].
//!
//! [`CameraSession::set_torch`] and [`CameraSession::torch_available`] are
//! deliberately **not** rows in the table above: neither waits on a platform
//! answer, and neither ever hops onto a queue synchronously. Android hands
//! CameraX a fire-and-forget `enableTorch` without waiting on its
//! `ListenableFuture`; Apple fires the request onto the session's own serial
//! queue with an **asynchronous** dispatch and answers availability from a
//! cached atomic refreshed at three explicit points (open's configuration
//! transaction, after `startRunning` returns, after each `set_torch` body —
//! it is NOT autonomously kept current; re-check on later rebuilds rather
//! than latching the first answer), rather than the synchronous queue hop
//! [`CameraSession::close`] still uses. Both stay callable from any thread —
//! including the UI thread — and neither needs `spawn_blocking`.
//!
//! The stream **start/stop** calls
//! ([`CameraSession::start_image_stream`],
//! [`CameraSession::start_barcode_stream`], and their `stop_*` counterparts)
//! are non-rows too, sharing `set_torch`'s treatment for the same reason:
//! Android answers a start synchronously (its CameraX bind is itself
//! fire-and-forget), and Apple hands the attach/detach to the session queue
//! with an **asynchronous** dispatch, so neither can park a UI-thread caller
//! behind an in-flight `startRunning()`. `Ok(())` from a start therefore
//! means the stream was **accepted**; a failure only Apple's queue can
//! discover (the session rejecting the video output) surfaces through
//! [`CameraSession::take_stream_error`] instead, releasing that start's
//! stream claim as it lands so a retry is never refused with
//! [`CameraError::StreamBusy`] — except when the session closes while the
//! start is still waiting its turn on the queue, which is treated as a
//! cancellation and reports nothing at all (see
//! [`SessionBackend::start_image_stream`]'s doc). A stop retires the
//! running callback on the calling thread and returns before the platform
//! detach runs, so no frame reaches the app's callback after `stop_*`
//! returns.

// Platform backends (the crate + frozen contract landed first; the real
// implementations filled in behind the same
// `SessionBackend` trait). Every real target routes through `Camera::open`'s
// selection point below.
#[cfg(target_os = "android")]
mod android;
#[cfg(target_vendor = "apple")]
mod apple;
#[cfg(not(any(target_os = "android", target_vendor = "apple")))]
mod unsupported;

// Platform-independent QR/barcode decoding over an `ImageFrame`/luma buffer
// — no `cfg` of its own (see the module doc), unlike the backends above.
pub mod barcode;

pub use barcode::{BarcodeStreamOptions, DetectionPolicy};

use std::path::Path;
use std::sync::{Arc, Mutex};

use barcode::Barcode;

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
    /// view*: request again once a
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

/// How a backend reports a stream start it **accepted** but could not
/// complete — the deferred half of [`SessionBackend::start_image_stream`]'s
/// error contract, for a backend whose attach work runs on a platform queue
/// the start call deliberately does not wait on.
///
/// `FnOnce`: one start can fail at most once, and a start that succeeds
/// simply drops the sink. Fired from whatever thread the backend discovered
/// the failure on — [`CameraSession`]'s own sink only touches a lock and an
/// atomic, never app code.
pub(crate) type StreamErrorSink = Box<dyn FnOnce(CameraError) + Send + 'static>;

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

    /// This session's single image stream is already claimed by the
    /// *other* stream kind. [`CameraSession::start_image_stream`] refuses
    /// while a [`CameraSession::start_barcode_stream`] composition is
    /// active, and vice versa; a second
    /// [`CameraSession::start_barcode_stream`] call while one is already
    /// running is refused too (no silent re-bind, unlike
    /// [`CameraSession::start_image_stream`]'s own restart-in-place
    /// behavior — see that method's doc), so a running detection policy's
    /// state never goes ambiguous. Distinct from [`Self::InUse`], which is
    /// device-level (another session, or another app, holding the camera
    /// itself) rather than a claim this crate arbitrates in-process.
    ///
    /// Call the matching `stop_image_stream`/`stop_barcode_stream` first,
    /// then retry.
    #[error(
        "the camera session's image stream is already claimed by the other stream kind — call \
         the matching stop_image_stream/stop_barcode_stream first"
    )]
    StreamBusy,

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
    /// is wrong, only the calling thread. The table's third row,
    /// [`CameraSession::close`], deliberately has no guard of its own — see
    /// its own doc for why it blocks the UI thread instead of refusing it.
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
    ///
    /// Reports a failure through **exactly one** of two paths, never both
    /// and never neither: a synchronous `Err` for everything it can decide
    /// on the calling thread, or `on_error` for a failure it could only
    /// discover after accepting the start (the Apple backend's queue-async
    /// attach). A backend that decides everything synchronously (Android)
    /// drops the sink unused.
    ///
    /// **Session-closed cancellation is the one documented exception.** If
    /// [`CameraSession::close`] lands before a queue-async attach ever runs,
    /// the backend may drop `on_error` unfired instead of reporting
    /// anything: the caller already knows the session is gone from
    /// `close()`'s own contract, so there is nothing left to report — only a
    /// start that never got the chance to happen, not a failure of one that
    /// did. The Apple backend takes exactly this path.
    fn start_image_stream(
        &self,
        format: ImageFormat,
        on_frame: Box<ImageFrameCallback>,
        on_error: StreamErrorSink,
    ) -> Result<(), CameraError>;
    /// See [`CameraSession::stop_image_stream`] — infallible, and never
    /// waits on a platform queue.
    fn stop_image_stream(&self);
    /// See [`CameraSession::set_torch`].
    ///
    /// The allowance mirrors [`capture`]'s: on a target with no camera
    /// backend [`CameraSession`]'s torch methods answer from [`unsupported`]
    /// instead of dispatching here (they are cfg-split like [`Camera::open`]),
    /// so this declaration is legitimately uncalled there — and nothing
    /// implements this trait on that target anyway. The trait keeps one shape
    /// on every target rather than growing a `cfg` of its own.
    #[cfg_attr(
        not(any(target_os = "android", target_vendor = "apple")),
        allow(dead_code)
    )]
    fn set_torch(&self, on: bool) -> Result<(), CameraError>;
    /// See [`CameraSession::torch_available`] (and [`Self::set_torch`] for
    /// the allowance).
    #[cfg_attr(
        not(any(target_os = "android", target_vendor = "apple")),
        allow(dead_code)
    )]
    fn torch_available(&self) -> bool;
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
    /// [`Self::request_permission`] alongside it (permission is requested
    /// once a `platform_view` preview
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

        Ok(CameraSession {
            backend,
            stream_claim: Arc::default(),
            stream_error: Arc::default(),
        })
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
    /// Single-occupancy claim over this session's one image stream — see
    /// [`CameraError::StreamBusy`]'s doc for the semantics this enforces.
    /// Shared (`Arc`) because a deferred stream-start failure releases its
    /// own claim from a backend queue, long after the start call returned —
    /// see [`Self::stream_error_sink`].
    stream_claim: Arc<occupancy::StreamOccupancy>,
    /// Where a deferred stream-start failure lands until
    /// [`Self::take_stream_error`] takes it. Shared for the same reason.
    stream_error: Arc<StreamErrorSlot>,
}

impl CameraSession {
    /// The `viewType` string to pass to `frust::platform_view` for this
    /// session's live preview slot.
    ///
    /// **Target-gated — never one shared literal** (see
    /// `docs/CODE_STANDARDS.md`'s Naming Conventions LAW): Android
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
    /// **Never blocks, and is callable from any thread — including the UI
    /// thread** (module doc's *Blocking API*): the platform-side attach is
    /// handed to the backend's own queue rather than waited on, so `Ok(())`
    /// means the stream was **accepted**, not that frames are flowing yet.
    /// A failure discovered after that acceptance surfaces through
    /// [`Self::take_stream_error`] instead of this `Result` — and releases
    /// this call's stream claim as it lands, so a retry is not refused with
    /// [`CameraError::StreamBusy`].
    ///
    /// Never blocks on a frame; calling it again restarts the stream at the
    /// newly requested format — this raw-stream re-bind stays legal even
    /// after this method started sharing the session's stream claim with
    /// [`Self::start_barcode_stream`] (unchanged behavior).
    ///
    /// # Errors
    /// [`CameraError::StreamBusy`] while [`Self::start_barcode_stream`]
    /// holds the session's stream claim — call [`Self::stop_barcode_stream`]
    /// first. [`CameraError::SessionClosed`] after [`Self::close`];
    /// [`CameraError::Platform`] for [`ImageFormat::Bgra`] on Android (see
    /// that variant's doc), or if the platform refuses the stream
    /// synchronously.
    pub fn start_image_stream(
        &self,
        format: ImageFormat,
        on_frame: impl Fn(&ImageFrame<'_>) + Send + 'static,
    ) -> Result<(), CameraError> {
        let token = self.stream_claim.claim_raw()?;
        let result = self.backend.start_image_stream(
            format,
            Box::new(on_frame),
            self.stream_error_sink(token),
        );
        if result.is_err() {
            // Android can refuse before anything is detached (its own
            // `ensure_open` check runs first — see
            // `android::AndroidSession::start_image_stream`'s doc), so a
            // failed re-bind does not always mean the *previous* raw stream
            // stopped running. Restore the pre-call claim state instead of
            // unconditionally releasing: a generation-guarded
            // compare-and-restore (`occupancy::StreamOccupancy::restore_on_error`)
            // that leaves an earlier still-running stream claimed — so
            // `stop_image_stream` can still reach it — while never
            // clobbering a claim some other call has legitimately taken
            // since.
            self.stream_claim.restore_on_error(token);
        }
        result
    }

    /// Stop a stream started with [`Self::start_image_stream`], without
    /// touching the preview. A no-op if no raw stream is running —
    /// including while [`Self::start_barcode_stream`] holds the claim
    /// instead, which this never touches (see [`CameraError::StreamBusy`]'s
    /// doc).
    ///
    /// **Never blocks, callable from any thread** (module doc's *Blocking
    /// API*): the running callback is retired on the calling thread, so no
    /// frame reaches it after this returns, while the platform-side detach
    /// runs on the backend's own queue afterwards.
    pub fn stop_image_stream(&self) {
        if self.stream_claim.release_raw() {
            self.backend.stop_image_stream();
        }
    }

    /// Decodes barcodes on the live camera feed. Claims the session's
    /// single image stream (see [`CameraError::StreamBusy`]); runs the
    /// decoder inside the stream callback on the plugin-owned thread
    /// (lossy-latest backpressure self-regulates decode cost — the same
    /// contract [`Self::start_image_stream`] documents). `on_detect` fires
    /// only when `opts.detection` emits — **never** with an empty slice —
    /// on that same thread: the [`ImageFrame`] callback rules apply, no
    /// signal writes; hand off via `frust_reactive::use_task` or a
    /// rescheduled write.
    ///
    /// Always requests [`ImageFormat::Yuv420`] internally — the format
    /// [`barcode::decode_frame`] takes its zero-copy path for;
    /// [`ImageFormat::Bgra`] has no barcode-decode path of its own.
    ///
    /// Non-blocking and accepted-not-confirmed exactly like
    /// [`Self::start_image_stream`], including the
    /// [`Self::take_stream_error`] path a deferred failure surfaces
    /// through — this is the composition's own contract too, not just the
    /// raw stream's.
    ///
    /// # Errors
    /// [`CameraError::StreamBusy`] while [`Self::start_image_stream`]'s raw
    /// stream, or another [`Self::start_barcode_stream`] call, already
    /// holds the claim — no silent re-bind (unlike
    /// [`Self::start_image_stream`]), so a running [`DetectionPolicy`]'s
    /// state never goes ambiguous; call [`Self::stop_barcode_stream`] first.
    /// Otherwise the same errors [`Self::start_image_stream`] can report.
    pub fn start_barcode_stream(
        &self,
        opts: BarcodeStreamOptions,
        on_detect: impl Fn(&[Barcode]) + Send + 'static,
    ) -> Result<(), CameraError> {
        let token = self.stream_claim.claim_barcode()?;
        let callback = barcode::stream::build_callback(opts, on_detect);
        let result = self.backend.start_image_stream(
            ImageFormat::Yuv420,
            Box::new(callback),
            self.stream_error_sink(token),
        );
        if result.is_err() {
            // Same generation-guarded restore as `start_image_stream` — see
            // that method's doc. `claim_barcode` never re-binds, so this
            // token's prior state is always "none": a failure always clears
            // back to unclaimed (unless someone else has claimed since).
            self.stream_claim.restore_on_error(token);
        }
        result
    }

    /// Stops a running barcode stream (no-op if none — including while
    /// [`Self::start_image_stream`] holds the claim instead, which this
    /// never touches). Releases the stream claim. Non-blocking, from any
    /// thread, exactly like [`Self::stop_image_stream`].
    pub fn stop_barcode_stream(&self) {
        if self.stream_claim.release_barcode() {
            self.backend.stop_image_stream();
        }
    }

    /// Take the most recent stream-start failure a backend reported
    /// **after** its start call had already returned `Ok(())`, if any.
    ///
    /// The deferred half of
    /// [`Self::start_image_stream`]/[`Self::start_barcode_stream`]'s error
    /// contract (module doc's *Blocking API*): those calls hand the
    /// platform-side attach to a backend queue rather than parking the
    /// caller on it, which leaves a rejected attach no synchronous `Result`
    /// to travel back through. It lands here instead — and releases the
    /// failed start's stream claim on the way, so a retry sees a free
    /// session rather than [`CameraError::StreamBusy`].
    ///
    /// Reading takes the value (it is reported once, and
    /// [`CameraError`] is not [`Clone`]), so poll it from a rebuild and keep
    /// what it returns in the caller's own state — the same "re-check rather
    /// than latch" shape [`Self::torch_available`] asks for. `None` is the
    /// normal answer, including on Android, whose backend reports every
    /// start failure synchronously.
    ///
    /// A late `None` is not a health check: a stream that was accepted and
    /// attached but has yet to deliver a frame reports nothing here — and
    /// neither does a start cancelled by a session close that landed before
    /// its queue-async attach ever ran ([`SessionBackend::start_image_stream`]'s
    /// documented exception).
    pub fn take_stream_error(&self) -> Option<CameraError> {
        self.stream_error.take()
    }

    /// The [`StreamErrorSink`] for a start holding `token`. Delegates to
    /// [`record_deferred_stream_error`], pulled out to a free function so it
    /// can be unit-tested directly — no host test target can construct a
    /// full [`CameraSession`] (module doc's *Backends*).
    fn stream_error_sink(&self, token: occupancy::ClaimToken) -> StreamErrorSink {
        let claim = Arc::clone(&self.stream_claim);
        let slot = Arc::clone(&self.stream_error);
        Box::new(move |error| record_deferred_stream_error(&claim, &slot, token, error))
    }

    /// Turns the torch (the flash unit held on, in continuous mode) on or
    /// off.
    ///
    /// **Non-blocking, callable from any thread — including the UI thread —
    /// unconditionally** (module doc's *Blocking API*): Android hands CameraX
    /// a fire-and-forget `enableTorch` without waiting on its
    /// `ListenableFuture`, and Apple fires the request onto the session's
    /// serial queue **asynchronously**, returning before the queue body ever
    /// runs rather than waiting even briefly for its turn.
    ///
    /// Torch is **session-level** state: it survives
    /// [`Self::start_image_stream`]/[`Self::start_barcode_stream`] and their
    /// stops (a stream never touches it) and dies with [`Self::close`], which
    /// releases the camera device and with it the torch.
    ///
    /// # Errors
    /// [`CameraError::SessionClosed`] after [`Self::close`] — the only error
    /// both platforms report synchronously.
    ///
    /// **Android** additionally reports [`CameraError::Platform`]
    /// synchronously where the active lens has no controllable torch (see
    /// [`Self::torch_available`]), where CameraX has not finished binding the
    /// camera yet (before the first `bindToLifecycle` completes — retry,
    /// this is not a permanent refusal), or where the camera control
    /// otherwise refuses the request.
    ///
    /// **Apple never returns [`CameraError::Platform`] from this call.**
    /// `Ok(())` means the request was *accepted* onto the session queue, not
    /// that AVFoundation applied it — a refusal (no controllable torch, the
    /// device mid cool-off, a configuration-lock conflict) surfaces only as
    /// [`Self::torch_available`] not flipping (or flipping back) to `true`,
    /// never through this method's `Result`. This is the crate's
    /// already-documented "accepted, not confirmed" torch framing
    /// (`README.md` §5), now true of the whole call on Apple rather than
    /// only the Android arm it originally described.
    ///
    /// [`CameraError::PlatformNotInitialized`] on a target with no camera
    /// backend at all.
    #[cfg(any(target_os = "android", target_vendor = "apple"))]
    pub fn set_torch(&self, on: bool) -> Result<(), CameraError> {
        self.backend.set_torch(on)
    }

    /// No camera backend exists on this target at all (module doc's
    /// *Backends*), so the torch fails soft exactly like every other call —
    /// see [`Camera::open`]'s own unsupported arm, which is why no
    /// [`CameraSession`] can even exist here to call this.
    #[cfg(not(any(target_os = "android", target_vendor = "apple")))]
    pub fn set_torch(&self, on: bool) -> Result<(), CameraError> {
        let _ = on;
        unsupported::set_torch()
    }

    /// Whether the active lens has a controllable torch.
    ///
    /// `false` on a device with no flash unit, on most front lenses, on every
    /// target with no camera backend, and after [`Self::close`]. Infallible by
    /// API shape: a platform query that fails is reported as `false` (and
    /// logged) rather than as an error, since a caller can only use this to
    /// decide whether to offer a torch control.
    ///
    /// Non-blocking and callable from any thread, like [`Self::set_torch`].
    /// On Apple this is a lock-free read of a cached value refreshed at
    /// session open, once capture starts, and after every [`Self::set_torch`]
    /// call — see the Apple backend's own doc for the refresh points — so it
    /// never hops onto the session queue either.
    #[cfg(any(target_os = "android", target_vendor = "apple"))]
    pub fn torch_available(&self) -> bool {
        self.backend.torch_available()
    }

    /// See [`Self::set_torch`]'s unsupported arm — always `false`.
    #[cfg(not(any(target_os = "android", target_vendor = "apple")))]
    pub fn torch_available(&self) -> bool {
        unsupported::torch_available()
    }

    /// Close the session and release the camera, stopping any running image
    /// or barcode stream with it. A no-op if already closed; every
    /// subsequent fallible [`CameraSession`] method call reports
    /// [`CameraError::SessionClosed`].
    ///
    /// **Blocks the calling thread on Apple, with no [`CameraError::UiThread`]
    /// guard** (module doc's *Blocking API* table): it hops onto the
    /// session's serial queue synchronously and waits, by design — a caller
    /// that reopens right after (a lens switch) depends on the release
    /// having actually happened, so unlike [`Camera::request_permission`]/
    /// [`Self::take_picture`] this call never refuses the UI thread, it
    /// parks it. Call it (and let a [`CameraSession`] drop, which runs the
    /// same hop on Apple) off the UI thread — `frust_reactive::spawn_blocking`
    /// — whenever a stream start may still be mid-flight on the session
    /// queue. Android's close does not block: its JNI call only schedules
    /// the teardown.
    pub fn close(&self) {
        self.stream_claim.clear();
        self.backend.close();
    }
}

// --- Stream occupancy --------------------------------------------------------

/// Single-occupancy claim over a [`CameraSession`]'s one image stream — see
/// [`CameraError::StreamBusy`]'s doc for the full semantics table this
/// enforces.
///
/// Pure state machine, deliberately backend-independent — like
/// [`capture`] below, it lives in the platform-independent crate root
/// rather than beside a `#[cfg]`-gated backend, but for a stronger reason
/// than that module's "only compiles for one target": [`CameraSession`]
/// cannot even be *constructed* on this desktop `cargo test` host at all
/// ([`unsupported::open`] fails before ever building one, per that module's
/// doc), so the tests below check the claim directly instead of going
/// through a real session.
///
/// # Generation-guarded restore, not unconditional release, on a failed start
///
/// `claim_raw`/`claim_barcode` return a [`ClaimToken`] on success rather
/// than a bare `Ok(())`. When the backend call the claim was gating then
/// fails, `start_image_stream`/`start_barcode_stream` hand the token to
/// [`StreamOccupancy::restore_on_error`] instead of unconditionally
/// releasing — a plain "always clear it" is wrong: Android's
/// `ensure_open` check ([`crate::android::AndroidSession::start_image_stream`])
/// can refuse before anything is detached, so a failed re-bind does not
/// always mean the *previous* raw stream stopped running, and clearing the
/// claim out from under it would leave that stream unclaimed and therefore
/// unstoppable short of [`crate::CameraSession::close`]. The token's
/// generation (bumped on every successful claim/release/clear) guards the
/// other direction too: if some other call has claimed or released since,
/// the restore is a no-op rather than clobbering state this call never
/// actually disturbed.
///
/// [`StreamOccupancy::restore_on_deferred_error`] is the same idea for the
/// *other* failure path — a queue-async backend's [`crate::StreamErrorSink`]
/// firing after its start already returned `Ok(())` — but with different
/// [`Prior::SameKind`] semantics: see that method's own doc for why a
/// same-kind restart's claim must be released there even though the
/// synchronous path above must not. Both methods report back whether the
/// token they were handed was still current, which
/// [`crate::CameraSession::stream_error_sink`] uses to decide whether a
/// deferred failure is still worth recording at all — a stale generation's
/// failure must not land in [`crate::StreamErrorSlot`] and overwrite (or be
/// misread as belonging to) a newer stream's outcome.
mod occupancy {
    use std::sync::Mutex;

    use crate::CameraError;

    /// Which kind of stream, if any, currently holds the claim.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum Claim {
        #[default]
        None,
        Raw,
        Barcode,
    }

    /// What a [`StreamOccupancy::claim_raw`]/[`StreamOccupancy::claim_barcode`]
    /// call observed *immediately before* the successful transition it made —
    /// the two shapes [`StreamOccupancy::restore_on_error`] can undo.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Prior {
        /// No claim existed before this call — a fresh claim.
        None,
        /// This call's own kind already held the claim (`claim_raw`'s legal
        /// Raw→Raw re-bind only; `claim_barcode` never re-binds, so every
        /// token it produces carries [`Self::None`] here instead).
        SameKind,
    }

    /// The locked state: the current claim, plus a generation bumped on
    /// every successful mutation (a claim, a release, or a restore that
    /// actually clears one) — see [`ClaimToken`]/[`StreamOccupancy::restore_on_error`].
    #[derive(Debug, Default)]
    struct Locked {
        claim: Claim,
        generation: u64,
    }

    /// [`crate::CameraSession`]'s second field, alongside `backend`.
    #[derive(Debug, Default)]
    pub(crate) struct StreamOccupancy(Mutex<Locked>);

    /// A successful [`StreamOccupancy::claim_raw`]/[`StreamOccupancy::claim_barcode`]
    /// call's receipt: the generation that transition produced, plus the
    /// [`Prior`] state it observed — consumed by
    /// [`StreamOccupancy::restore_on_error`]/
    /// [`StreamOccupancy::restore_on_deferred_error`] if the backend call
    /// the claim was gating then fails. Deliberately opaque: a caller holds
    /// this only to hand it back, on either path — dropped (committed) on
    /// success, or passed to one of the two restores on failure.
    ///
    /// `Copy` because a start has **two** failure paths to restore from: the
    /// synchronous `Err` a backend returns ([`StreamOccupancy::restore_on_error`]),
    /// and the deferred [`crate::StreamErrorSink`] a queue-async backend
    /// fires later ([`StreamOccupancy::restore_on_deferred_error`]). Only
    /// one of them ever runs per start
    /// ([`crate::SessionBackend::start_image_stream`]'s contract), so there
    /// is never a race between the two — each token is restored at most
    /// once.
    #[derive(Debug, Clone, Copy)]
    pub(crate) struct ClaimToken {
        prior: Prior,
        generation: u64,
    }

    impl StreamOccupancy {
        fn lock(&self) -> std::sync::MutexGuard<'_, Locked> {
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }

        /// [`crate::CameraSession::start_image_stream`]'s occupancy check:
        /// refused only while a barcode stream holds the claim; re-binds
        /// freely otherwise — the existing raw-stream restart behavior,
        /// unchanged by this claim. On success, bumps the generation and
        /// returns a [`ClaimToken`] recording what this call did, for
        /// [`Self::restore_on_error`] if the backend call it gates fails.
        pub(crate) fn claim_raw(&self) -> Result<ClaimToken, CameraError> {
            let mut state = self.lock();
            if state.claim == Claim::Barcode {
                return Err(CameraError::StreamBusy);
            }
            let prior = if state.claim == Claim::Raw {
                Prior::SameKind
            } else {
                Prior::None
            };
            state.generation += 1;
            state.claim = Claim::Raw;
            Ok(ClaimToken {
                prior,
                generation: state.generation,
            })
        }

        /// [`crate::CameraSession::start_barcode_stream`]'s occupancy
        /// check: refused while *either* a raw stream or another barcode
        /// stream already holds the claim — no silent re-bind, unlike
        /// [`Self::claim_raw`] (keeps a running `DetectionFilter`'s state
        /// unambiguous), so every token this produces has [`Prior::None`].
        pub(crate) fn claim_barcode(&self) -> Result<ClaimToken, CameraError> {
            let mut state = self.lock();
            if state.claim != Claim::None {
                return Err(CameraError::StreamBusy);
            }
            state.generation += 1;
            state.claim = Claim::Barcode;
            Ok(ClaimToken {
                prior: Prior::None,
                generation: state.generation,
            })
        }

        /// Undo a [`ClaimToken`]'s transition after the **synchronous**
        /// backend call it gated failed — the generation-guarded
        /// compare-and-restore `start_image_stream`/`start_barcode_stream`
        /// call instead of an unconditional release.
        ///
        /// Returns whether `token` was still current: `false` means some
        /// other call has claimed or released since — that state wins, and
        /// this is a no-op, never clobbering a claim this call never
        /// actually disturbed. Otherwise: a fresh claim ([`Prior::None`])
        /// clears back to [`Claim::None`]; a re-bind over the same kind
        /// ([`Prior::SameKind`]) leaves the claim exactly as it was — the
        /// earlier stream it belonged to may still be running (Android's
        /// `ensure_open` can refuse *before* anything is detached — see the
        /// module doc), and only its own `stop_*` call may release it.
        ///
        /// See [`Self::restore_on_deferred_error`] for the other failure
        /// path's counterpart, which resolves the same [`Prior::SameKind`]
        /// case differently.
        pub(crate) fn restore_on_error(&self, token: ClaimToken) -> bool {
            let mut state = self.lock();
            if state.generation != token.generation {
                return false;
            }
            if token.prior == Prior::None {
                state.claim = Claim::None;
                state.generation += 1;
            }
            true
        }

        /// Undo a [`ClaimToken`]'s transition after a **deferred** failure —
        /// the [`crate::StreamErrorSink`] a queue-async backend fires after
        /// its start call already returned `Ok(())`. Returns whether `token`
        /// was still current, exactly like [`Self::restore_on_error`]; the
        /// caller ([`crate::CameraSession::stream_error_sink`]) uses that to
        /// decide whether the failure is still worth recording at all.
        ///
        /// Unlike [`Self::restore_on_error`], a same-kind restart's claim
        /// ([`Prior::SameKind`]) is released here too, not left standing.
        /// The two paths need different answers for the identical `Prior`
        /// because they fail at different points relative to the platform
        /// detach: [`Self::restore_on_error`]'s "leave it as-is" case exists
        /// for Android's `ensure_open` shape, which can refuse a re-bind
        /// *before* the previous stream is detached, so that earlier stream
        /// may genuinely still be running. A deferred failure never has that
        /// shape — the only backend that ever fires this sink is Apple's
        /// queue-async attach, and `attach_stream` unconditionally detaches
        /// whatever was running *before* it ever tries to add the
        /// replacement output (`apple::detach_stream`), so by the time a
        /// deferred failure lands nothing is attached, same-kind restart or
        /// not. Leaving the claim standing here would strand it: a same-kind
        /// retry stays legal regardless, but a kind switch
        /// (`start_barcode_stream` after a failed raw re-bind) would wrongly
        /// see [`crate::CameraError::StreamBusy`] over a stream that no
        /// longer exists.
        pub(crate) fn restore_on_deferred_error(&self, token: ClaimToken) -> bool {
            let mut state = self.lock();
            if state.generation != token.generation {
                return false;
            }
            state.claim = Claim::None;
            state.generation += 1;
            true
        }

        /// [`crate::CameraSession::stop_image_stream`]: clears the claim
        /// only if a raw stream currently holds it, and reports whether it
        /// did — the caller uses this to decide whether to actually reach
        /// the backend, since stopping the wrong kind must be a pure no-op
        /// (never touching a running barcode stream). Bumps the generation
        /// whenever it actually clears the claim, so a
        /// [`Self::restore_on_error`] call for a token predating this
        /// release can never resurrect it.
        pub(crate) fn release_raw(&self) -> bool {
            let mut state = self.lock();
            if state.claim == Claim::Raw {
                state.claim = Claim::None;
                state.generation += 1;
                true
            } else {
                false
            }
        }

        /// Mirrors [`Self::release_raw`] for
        /// [`crate::CameraSession::stop_barcode_stream`].
        pub(crate) fn release_barcode(&self) -> bool {
            let mut state = self.lock();
            if state.claim == Claim::Barcode {
                state.claim = Claim::None;
                state.generation += 1;
                true
            } else {
                false
            }
        }

        /// [`crate::CameraSession::close`]: clears the claim
        /// unconditionally, regardless of which kind (if any) held it, and
        /// always bumps the generation — even from an already-`None` claim
        /// — so a [`Self::restore_on_error`] call racing a `close` can never
        /// resurrect a claim after it.
        pub(crate) fn clear(&self) {
            let mut state = self.lock();
            state.claim = Claim::None;
            state.generation += 1;
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn raw_then_raw_is_a_legal_rebind() {
            let occ = StreamOccupancy::default();
            assert!(occ.claim_raw().is_ok());
            assert!(occ.claim_raw().is_ok(), "raw re-bind stays legal");
        }

        #[test]
        fn raw_while_barcode_active_is_busy() {
            let occ = StreamOccupancy::default();
            occ.claim_barcode().unwrap();
            assert!(matches!(occ.claim_raw(), Err(CameraError::StreamBusy)));
        }

        #[test]
        fn barcode_while_raw_active_is_busy() {
            let occ = StreamOccupancy::default();
            occ.claim_raw().unwrap();
            assert!(matches!(occ.claim_barcode(), Err(CameraError::StreamBusy)));
        }

        #[test]
        fn barcode_while_barcode_active_is_busy_no_silent_rebind() {
            let occ = StreamOccupancy::default();
            occ.claim_barcode().unwrap();
            assert!(matches!(occ.claim_barcode(), Err(CameraError::StreamBusy)));
        }

        #[test]
        fn stop_clears_only_its_own_kind() {
            let occ = StreamOccupancy::default();

            occ.claim_raw().unwrap();
            assert!(
                !occ.release_barcode(),
                "stopping barcode while raw is active is a no-op"
            );
            assert!(occ.release_raw(), "stopping raw actually releases it");
            assert!(!occ.release_raw(), "a second stop is a no-op");

            occ.claim_barcode().unwrap();
            assert!(
                !occ.release_raw(),
                "stopping raw while barcode is active is a no-op"
            );
            assert!(occ.release_barcode());
        }

        #[test]
        fn close_clears_any_claim() {
            let occ = StreamOccupancy::default();
            occ.claim_barcode().unwrap();
            occ.clear();
            assert!(occ.claim_raw().is_ok(), "claim released after clear");
        }

        #[test]
        fn a_released_claim_can_be_reclaimed_by_either_kind() {
            let occ = StreamOccupancy::default();
            occ.claim_raw().unwrap();
            occ.release_raw();
            assert!(occ.claim_barcode().is_ok());
        }

        /// The confirmed bug, as a test: an already-running raw stream's
        /// re-bind fails (e.g. Android's `ensure_open` refusing before
        /// anything is detached — see `crate::android::AndroidSession::start_image_stream`'s
        /// doc). Restoring must leave the *earlier* raw claim intact, not
        /// clear it — otherwise the still-running stream becomes unclaimed,
        /// and `stop_image_stream`'s gated stop never reaches the backend.
        #[test]
        fn restore_after_a_failed_rebind_leaves_the_earlier_raw_claim_intact() {
            let occ = StreamOccupancy::default();
            occ.claim_raw().unwrap(); // the already-running raw stream
            let token = occ.claim_raw().unwrap(); // the re-bind attempt
            occ.restore_on_error(token); // ...whose backend call failed

            assert!(
                matches!(occ.claim_barcode(), Err(CameraError::StreamBusy)),
                "the claim is still Raw"
            );
            assert!(
                occ.release_raw(),
                "a subsequent stop_image_stream must still reach the backend"
            );
        }

        /// The deferred path's own bug, as a test: unlike the synchronous
        /// path above, a **deferred** same-kind restart failure must
        /// release the claim, not leave it standing — the queue-async
        /// backend that ever fires this path (Apple) always detaches the
        /// earlier stream before it even attempts the replacement, so by
        /// the time this failure lands nothing is attached, same-kind
        /// restart or not (see `restore_on_deferred_error`'s doc). Leaving
        /// the claim as `Raw` here would strand a kind switch behind
        /// `StreamBusy` over a stream that no longer exists.
        #[test]
        fn deferred_restore_releases_a_same_kind_restart_claim_too() {
            let occ = StreamOccupancy::default();
            occ.claim_raw().unwrap(); // the already-running raw stream
            let token = occ.claim_raw().unwrap(); // the re-bind attempt

            assert!(
                occ.restore_on_deferred_error(token),
                "the token was still current"
            );

            assert!(
                occ.claim_barcode().is_ok(),
                "the claim is released, not left standing as Raw"
            );
        }

        /// The deferred restore's own generation guard: a stale token must
        /// not clobber a claim taken since, exactly like the synchronous
        /// path's `restore_is_a_no_op_once_someone_else_has_claimed_since`.
        #[test]
        fn deferred_restore_is_a_no_op_once_someone_else_has_claimed_since() {
            let occ = StreamOccupancy::default();
            let stale = occ.claim_raw().unwrap(); // A
            occ.claim_raw().unwrap(); // B's concurrent re-bind

            assert!(!occ.restore_on_deferred_error(stale), "A's token is stale");
            assert!(
                matches!(occ.claim_barcode(), Err(CameraError::StreamBusy)),
                "B's claim is untouched"
            );
        }

        #[test]
        fn restore_after_a_failed_fresh_claim_clears_it() {
            let occ = StreamOccupancy::default();
            let token = occ.claim_raw().unwrap(); // None -> Raw
            occ.restore_on_error(token); // ...whose backend call failed

            assert!(occ.claim_raw().is_ok(), "claim restored to None");
        }

        /// A's token goes stale the moment B legitimately claims (a legal
        /// Raw→Raw re-bind, generation bumps) — A's belated restore must be
        /// a no-op, never clobbering B's still-active claim.
        #[test]
        fn restore_is_a_no_op_once_someone_else_has_claimed_since() {
            let occ = StreamOccupancy::default();
            let stale = occ.claim_raw().unwrap(); // A
            occ.claim_raw().unwrap(); // B's concurrent re-bind

            occ.restore_on_error(stale); // A's belated restore

            assert!(
                matches!(occ.claim_barcode(), Err(CameraError::StreamBusy)),
                "B's claim is untouched"
            );
            assert!(occ.release_raw(), "B's claim can still be stopped normally");
        }

        /// A start has two restore paths (a synchronous `Err` and the
        /// deferred [`crate::StreamErrorSink`]), so one token can be
        /// restored twice — the second must not clear a claim taken since.
        #[test]
        fn restoring_one_token_twice_never_clears_a_later_claim() {
            let occ = StreamOccupancy::default();
            let token = occ.claim_barcode().unwrap();
            occ.restore_on_error(token);

            occ.claim_raw().unwrap(); // a later, legitimate start
            occ.restore_on_error(token); // the same token's second path

            assert!(
                matches!(occ.claim_barcode(), Err(CameraError::StreamBusy)),
                "the later claim survives"
            );
        }

        /// A token from before a `release_raw` cannot resurrect the claim it
        /// once produced.
        #[test]
        fn restore_cannot_resurrect_a_claim_a_release_already_cleared() {
            let occ = StreamOccupancy::default();
            let token = occ.claim_raw().unwrap();
            assert!(occ.release_raw(), "release bumps the generation");

            occ.restore_on_error(token);

            assert!(
                occ.claim_barcode().is_ok(),
                "still None — nothing resurrected"
            );
        }

        /// Same as above, for `clear` (`CameraSession::close`'s path).
        #[test]
        fn restore_cannot_resurrect_a_claim_a_clear_already_cleared() {
            let occ = StreamOccupancy::default();
            let token = occ.claim_raw().unwrap();
            occ.clear(); // bumps the generation, like `release_raw` above

            occ.restore_on_error(token);

            assert!(
                occ.claim_barcode().is_ok(),
                "still None — nothing resurrected"
            );
        }
    }
}

// --- Deferred stream-start failures ------------------------------------------

/// Where a stream-start failure a backend could only discover **after**
/// accepting the start waits until [`CameraSession::take_stream_error`]
/// takes it.
///
/// One slot per session, holding the most recent failure: an unread earlier
/// one is replaced rather than queued, since only the latest is still
/// actionable. A stale generation's failure never reaches this slot at all
/// — see [`record_deferred_stream_error`], the sink logic that guards
/// entry here.
#[derive(Debug, Default)]
struct StreamErrorSlot(Mutex<Option<CameraError>>);

impl StreamErrorSlot {
    /// Poison-tolerant like [`occupancy::StreamOccupancy`]'s own lock: the
    /// slot holds one `Option` no panic could half-break, and a poisoned
    /// lock here would turn every later camera call into a panic of its own.
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<CameraError>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn record(&self, error: CameraError) {
        *self.lock() = Some(error);
    }

    fn take(&self) -> Option<CameraError> {
        self.lock().take()
    }
}

/// The core logic behind [`CameraSession::stream_error_sink`], pulled into a
/// free function so it can be unit-tested without a full [`CameraSession`] —
/// no host test target can construct one (module doc's *Backends*).
///
/// Restores `token`'s claim via
/// [`occupancy::StreamOccupancy::restore_on_deferred_error`] first, and
/// records the failure into `slot` only if that restore reports `token` was
/// still current. Recording is deliberately gated on the *same* generation
/// check the restore already makes — not run unconditionally before it —
/// because a sink firing after a later start has already claimed this
/// session's stream is reporting on a stale generation: recording it
/// anyway would overwrite (or be misread as belonging to) the newer
/// stream's own outcome. A stale failure is logged at `debug` and dropped
/// instead of recorded; a live one is logged at `warn` as before.
fn record_deferred_stream_error(
    claim: &occupancy::StreamOccupancy,
    slot: &StreamErrorSlot,
    token: occupancy::ClaimToken,
    error: CameraError,
) {
    if claim.restore_on_deferred_error(token) {
        log::warn!("frust-camera: an accepted stream start then failed: {error}");
        slot.record(error);
    } else {
        log::debug!(
            "frust-camera: dropping a deferred stream-start failure from a superseded stream \
             generation: {error}"
        );
    }
}

#[cfg(test)]
mod stream_error_tests {
    use super::occupancy::StreamOccupancy;
    use super::{CameraError, StreamErrorSlot, record_deferred_stream_error};

    #[test]
    fn an_empty_slot_reports_nothing() {
        assert!(StreamErrorSlot::default().take().is_none());
    }

    #[test]
    fn a_recorded_failure_is_reported_exactly_once() {
        let slot = StreamErrorSlot::default();
        slot.record(CameraError::Platform("rejected".to_string()));

        assert!(matches!(slot.take(), Some(CameraError::Platform(m)) if m == "rejected"));
        assert!(slot.take().is_none(), "taking clears the slot");
    }

    #[test]
    fn the_most_recent_failure_wins() {
        let slot = StreamErrorSlot::default();
        slot.record(CameraError::Platform("first".to_string()));
        slot.record(CameraError::SessionClosed);

        assert!(matches!(slot.take(), Some(CameraError::SessionClosed)));
    }

    /// A deferred failure from a still-current generation is recorded
    /// normally — the ordinary, non-stale path.
    #[test]
    fn a_current_generations_deferred_failure_is_recorded() {
        let claim = StreamOccupancy::default();
        let slot = StreamErrorSlot::default();
        let token = claim.claim_raw().unwrap();

        record_deferred_stream_error(
            &claim,
            &slot,
            token,
            CameraError::Platform("rejected".into()),
        );

        assert!(matches!(slot.take(), Some(CameraError::Platform(m)) if m == "rejected"));
    }

    /// The confirmed bug, pinned as a test: a sink from generation N firing
    /// after generation N+1 has already started must record nothing — a
    /// stale deferred failure must never overwrite (or be mistaken for) a
    /// newer stream's own outcome in [`StreamErrorSlot`].
    #[test]
    fn a_stale_generations_deferred_failure_records_nothing() {
        let claim = StreamOccupancy::default();
        let slot = StreamErrorSlot::default();
        let stale_token = claim.claim_raw().unwrap(); // generation N

        claim.claim_raw().unwrap(); // generation N+1 starts before N's failure lands

        record_deferred_stream_error(
            &claim,
            &slot,
            stale_token,
            CameraError::Platform("stale".into()),
        );

        assert!(
            slot.take().is_none(),
            "a stale generation's failure must not be recorded"
        );
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
