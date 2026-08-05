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
//!
//! [`CameraSession::set_torch`] and [`CameraSession::torch_available`] are
//! deliberately **not** rows in the table above: neither waits on a platform
//! answer (Android hands CameraX a fire-and-forget `enableTorch`; Apple takes
//! one short synchronous hop onto the session's own serial queue, the same
//! shape [`CameraSession::close`] uses), so both stay callable from any
//! thread — including the UI thread — and neither needs `spawn_blocking`.

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
use std::sync::Arc;

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
            stream_claim: occupancy::StreamOccupancy::default(),
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
    stream_claim: occupancy::StreamOccupancy,
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
    /// that variant's doc), or if the platform refuses the stream.
    pub fn start_image_stream(
        &self,
        format: ImageFormat,
        on_frame: impl Fn(&ImageFrame<'_>) + Send + 'static,
    ) -> Result<(), CameraError> {
        self.stream_claim.claim_raw()?;
        let result = self.backend.start_image_stream(format, Box::new(on_frame));
        if result.is_err() {
            // The attempt never actually started (or clobbered a prior
            // raw stream while trying to rebind — both backends detach any
            // existing stream before attempting the new one, so nothing is
            // running now either way): release the claim rather than
            // leaving a phantom "raw stream active" state behind.
            self.stream_claim.release_raw();
        }
        result
    }

    /// Stop a stream started with [`Self::start_image_stream`], without
    /// touching the preview. A no-op if no raw stream is running —
    /// including while [`Self::start_barcode_stream`] holds the claim
    /// instead, which this never touches (see [`CameraError::StreamBusy`]'s
    /// doc).
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
        self.stream_claim.claim_barcode()?;
        let callback = barcode::stream::build_callback(opts, on_detect);
        let result = self
            .backend
            .start_image_stream(ImageFormat::Yuv420, Box::new(callback));
        if result.is_err() {
            self.stream_claim.release_barcode();
        }
        result
    }

    /// Stops a running barcode stream (no-op if none — including while
    /// [`Self::start_image_stream`] holds the claim instead, which this
    /// never touches). Releases the stream claim.
    pub fn stop_barcode_stream(&self) {
        if self.stream_claim.release_barcode() {
            self.backend.stop_image_stream();
        }
    }

    /// Turns the torch (the flash unit held on, in continuous mode) on or
    /// off.
    ///
    /// **Non-blocking, callable from any thread — including the UI thread**
    /// (module doc's *Blocking API*): Android hands CameraX a fire-and-forget
    /// `enableTorch` without waiting on its `ListenableFuture`, and Apple
    /// takes one short synchronous hop onto the session's serial queue.
    ///
    /// Torch is **session-level** state: it survives
    /// [`Self::start_image_stream`]/[`Self::start_barcode_stream`] and their
    /// stops (a stream never touches it) and dies with [`Self::close`], which
    /// releases the camera device and with it the torch.
    ///
    /// # Errors
    /// [`CameraError::SessionClosed`] after [`Self::close`];
    /// [`CameraError::Platform`] where the active lens has no controllable
    /// torch (see [`Self::torch_available`]), where the platform has not
    /// finished binding the camera yet (Android: before CameraX's first
    /// `bindToLifecycle` completes — retry, this is not a permanent
    /// refusal), or where the platform refuses the request;
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

    /// [`crate::CameraSession`]'s second field, alongside `backend`.
    #[derive(Debug, Default)]
    pub(crate) struct StreamOccupancy(Mutex<Claim>);

    impl StreamOccupancy {
        fn lock(&self) -> std::sync::MutexGuard<'_, Claim> {
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }

        /// [`crate::CameraSession::start_image_stream`]'s occupancy check:
        /// refused only while a barcode stream holds the claim; re-binds
        /// freely otherwise — the existing raw-stream restart behavior,
        /// unchanged by this claim.
        pub(crate) fn claim_raw(&self) -> Result<(), CameraError> {
            let mut claim = self.lock();
            if *claim == Claim::Barcode {
                return Err(CameraError::StreamBusy);
            }
            *claim = Claim::Raw;
            Ok(())
        }

        /// [`crate::CameraSession::start_barcode_stream`]'s occupancy
        /// check: refused while *either* a raw stream or another barcode
        /// stream already holds the claim — no silent re-bind, unlike
        /// [`Self::claim_raw`] (keeps a running `DetectionFilter`'s state
        /// unambiguous).
        pub(crate) fn claim_barcode(&self) -> Result<(), CameraError> {
            let mut claim = self.lock();
            if *claim != Claim::None {
                return Err(CameraError::StreamBusy);
            }
            *claim = Claim::Barcode;
            Ok(())
        }

        /// [`crate::CameraSession::stop_image_stream`]: clears the claim
        /// only if a raw stream currently holds it, and reports whether it
        /// did — the caller uses this to decide whether to actually reach
        /// the backend, since stopping the wrong kind must be a pure no-op
        /// (never touching a running barcode stream).
        pub(crate) fn release_raw(&self) -> bool {
            let mut claim = self.lock();
            if *claim == Claim::Raw {
                *claim = Claim::None;
                true
            } else {
                false
            }
        }

        /// Mirrors [`Self::release_raw`] for
        /// [`crate::CameraSession::stop_barcode_stream`].
        pub(crate) fn release_barcode(&self) -> bool {
            let mut claim = self.lock();
            if *claim == Claim::Barcode {
                *claim = Claim::None;
                true
            } else {
                false
            }
        }

        /// [`crate::CameraSession::close`]: clears the claim
        /// unconditionally, regardless of which kind (if any) held it.
        pub(crate) fn clear(&self) {
            *self.lock() = Claim::None;
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
