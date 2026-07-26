//! The Apple (iOS; macOS desktop preview shares this arm like every other
//! plugin) backend — AVFoundation, driven **from Rust directly** via
//! `objc2-av-foundation` (no Swift camera glue; the Swift package task 08
//! ships is the preview `CameraPreviewFactory` view only).
//!
//! # What this module owns
//!
//! - **Session** ([`AppleSession::open`]): device discovery
//!   (`AVCaptureDevice.defaultDeviceWithDeviceType:mediaType:position:`),
//!   `AVCaptureDeviceInput`, `AVCapturePhotoOutput`, and the
//!   `beginConfiguration`/`commitConfiguration` pair — all of it on this
//!   plugin's own **serial dispatch queue** (see *Threading* below).
//! - **Permission** ([`request_permission`]): the
//!   `authorizationStatusForMediaType:` fast path, plus the
//!   `requestAccessForMediaType:completionHandler:` completion handler
//!   bridged onto a channel so the caller's blocking API resolves.
//! - **Still capture** ([`SessionInner::take_picture`]): a
//!   [`PhotoCaptureDelegate`] (`define_class!`, thread kind
//!   [`AnyThread`](objc2::AnyThread) — AVFoundation fires photo callbacks on
//!   an arbitrary background queue) that writes the JPEG bytes to the
//!   caller's path and unblocks the waiting call.
//! - **The preview seam** ([`frust_camera_session_handle`]): the C symbol the
//!   Swift `CameraPreviewFactory` (task 08) calls to attach an
//!   `AVCaptureVideoPreviewLayer` to a live session.
//! - **The image stream** ([`SessionInner::start_image_stream`], task 09): an
//!   `AVCaptureVideoDataOutput` plus a [`SampleBufferDelegate`]
//!   (`define_class!`, [`AnyThread`](objc2::AnyThread) — same shape as the
//!   photo delegate) delivering `CVPixelBuffer` planes zero-copy on its own
//!   serial queue.
//!
//! # Threading
//!
//! Every session mutation (`addInput`/`addOutput`, the configuration
//! transaction, `startRunning`/`stopRunning`, and the capture request) runs on
//! one plugin-owned **serial** `dispatch2` queue — never on the caller's
//! thread, and never on the `CADisplayLink`/UI thread mid-frame (PLAN.md's
//! iOS *Threading* risk). Object *allocation* (`AVCaptureSession::new()` and
//! friends) happens on the calling thread, which mutates no live session.
//!
//! A running image stream gets a **second**, dedicated serial queue
//! ([`FRAMES_QUEUE_LABEL`]) for its sample-buffer delegate, never the session
//! queue: a frame callback runs app code for as long as it likes, and sharing
//! the session queue would both stall session operations behind it and
//! deadlock a callback that stops its own stream.
//!
//! [`Camera::request_permission`](crate::Camera::request_permission) and
//! [`CameraSession::take_picture`](crate::CameraSession::take_picture) both
//! **block** on this backend (a system dialog; a capture round trip), so
//! callers pair them with `frust_reactive::spawn_blocking` exactly as the
//! crate doc says — never the UI thread. Both have a bounded wait
//! ([`PERMISSION_TIMEOUT`]/[`CAPTURE_TIMEOUT`]) so a lost platform callback
//! degrades to a typed result instead of a permanently parked thread.
//!
//! [`SessionInner::take_picture`] additionally **fails fast** when it is
//! called on the main run loop ([`reject_on_main_thread`], applied to
//! `take_picture` and to `request_permission`'s prompt path — the fast
//! already-decided statuses never block, so they stay callable anywhere) —
//! the Apple half
//! of the crate-wide main-thread rule whose Android half
//! (`Looper.myLooper() == Looper.getMainLooper()`) lives in the `android`
//! backend module. A UI-thread caller gets an immediate, diagnosable
//! [`CameraError::UiThread`] instead of a ten-second frozen frame — the
//! same typed error the Android half reports.
//!
//! # `unsafe`
//!
//! Confined to this module and each `# Safety`-noted, the
//! `frust-secure-storage`/`frust-shared-preferences` apple-backend precedent
//! (`docs/CODE_STANDARDS.md`'s sanctioned zones): `objc2` marks every
//! AVFoundation message send `unsafe`, edition-2024 marks reading an `extern`
//! constant static `unsafe`, and two thread-safety assertions
//! ([`QueueBound`], the C export's raw pointer) carry the reasoning the
//! compiler cannot check.
//!
//! Nothing here unwinds across an FFI boundary: the `define_class!` delegate
//! methods and the `extern "C"` export each wrap their body in
//! [`std::panic::catch_unwind`] (`docs/CODE_STANDARDS.md`'s Plugin
//! Conventions — this crate cannot use `frust-shell-common`'s `guard`, which
//! lives above the plugin charter line).

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, define_class, msg_send, sel};
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceInput,
    AVCaptureDevicePosition, AVCaptureDeviceType, AVCaptureDeviceTypeBuiltInWideAngleCamera,
    AVCaptureOutput, AVCapturePhoto, AVCapturePhotoCaptureDelegate, AVCapturePhotoOutput,
    AVCapturePhotoSettings, AVCaptureSession, AVCaptureSessionPreset,
    AVCaptureSessionPreset640x480, AVCaptureSessionPreset1280x720, AVCaptureSessionPreset1920x1080,
    AVCaptureSessionPreset3840x2160, AVCaptureSessionPresetPhoto, AVCaptureVideoDataOutput,
    AVCaptureVideoDataOutputSampleBufferDelegate, AVError, AVMediaType, AVMediaTypeVideo,
};
use objc2_core_media::{CMSampleBuffer, CMVideoFormatDescriptionGetDimensions};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBaseAddressOfPlane,
    CVPixelBufferGetBytesPerRow, CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeight,
    CVPixelBufferGetHeightOfPlane, CVPixelBufferGetPlaneCount, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_32BGRA,
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
};
use objc2_foundation::{NSDictionary, NSError, NSNumber, NSObjectProtocol, NSString};

use crate::{
    CameraError, ImageFormat, ImageFrame, ImageFrameCallback, ImagePlane, Lens, PermissionStatus,
    Resolution, SessionBackend,
};

/// The `viewType` this crate's iOS preview slot resolves to — the bare
/// `@objc(...)` runtime name `FrustViewHost` looks up via
/// `NSClassFromString` (`docs/CODE_STANDARDS.md`'s platform-view factory
/// `viewType` LAW — iOS carries no package, unlike Android's fully-qualified
/// form). Shipped by `plugins/camera/platform/ios/`'s
/// `CameraPreviewFactory.swift` (task 08).
const PREVIEW_VIEW_TYPE: &str = "CameraPreviewFactory";

/// The label of this plugin's serial session queue (module doc's
/// *Threading*). Reverse-DNS per libdispatch convention; one queue per
/// session, so the label repeats across sessions by design (labels are
/// diagnostic, not identifying).
const SESSION_QUEUE_LABEL: &str = "dev.frust.camera.session";

/// The label of a running image stream's sample-buffer queue (module doc's
/// *Threading*) — one per stream, deliberately separate from
/// [`SESSION_QUEUE_LABEL`]'s session queue.
const FRAMES_QUEUE_LABEL: &str = "dev.frust.camera.frames";

/// The clockwise rotation [`crate::ImageFrame::rotation_degrees`] reports for
/// every streamed frame.
///
/// Buffers are delivered **un-rotated** — the video connection is left alone
/// rather than paying `videoRotationAngle`'s per-frame rotation cost on a
/// stream a consumer usually feeds to its own pipeline. That matches the
/// Android backend, where `ImageAnalysis` hands over sensor-oriented buffers
/// plus `ImageInfo.rotationDegrees`.
///
/// The value is a constant `90` for the same reason
/// [`PORTRAIT_ROTATION_ANGLE`] is: v1 pins portrait (this plugin has no
/// `UIWindowScene` to read an interface orientation from), so an upright
/// presentation of a sensor-native buffer is one quarter turn clockwise.
/// Rotation tracking is the same follow-up, not a separate gap.
const STREAM_ROTATION_DEGREES: i32 = 90;

/// How many plane slots a delivered [`crate::ImageFrame`] can carry — 3, the
/// crate-wide maximum; iOS uses 2 (`420f`) or 1 (`BGRA`).
const MAX_PLANES: usize = 3;

/// How long [`request_permission`] waits for
/// `requestAccessForMediaType:completionHandler:` before giving up on the
/// blocking bridge and reporting [`PermissionStatus::Pending`].
///
/// Generous on purpose: the completion handler only fires once the user
/// dismisses the system dialog, and a user may take a while. On expiry the
/// dialog is still live and the answer still lands in the OS's own
/// bookkeeping, so a later call reads it back via the
/// `authorizationStatusForMediaType:` fast path — which is exactly what
/// [`PermissionStatus::Pending`] tells the caller to do.
const PERMISSION_TIMEOUT: Duration = Duration::from_secs(120);

/// How long [`SessionInner::take_picture`] waits for the photo delegate
/// before reporting a [`CameraError::Platform`] timeout.
///
/// A still capture is a sub-second operation even with flash and
/// `AVCapturePhotoQualityPrioritizationQuality`; 10s is a "the callback was
/// lost" bound, not a performance budget.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

/// The `AVCaptureConnection.videoRotationAngle` applied to the capture
/// connection at open, in degrees counter-clockwise from the horizon —
/// `90` is AVFoundation's portrait ("home button down") orientation.
///
/// v1 pins portrait rather than tracking device orientation: the plugin has
/// no `UIWindowScene` handle to read an interface orientation from, and
/// `AVCaptureDevice.RotationCoordinator` (iOS 17+) needs a preview layer
/// that lives on the Swift side (task 08). Rotation tracking is a follow-up,
/// not a v1 gap this constant hides — see the module doc.
///
/// `f64` rather than `CGFloat` to avoid a direct `objc2-core-foundation`
/// dependency; the two are the same type on every 64-bit Apple target (the
/// only ones this crate builds for).
const PORTRAIT_ROTATION_ANGLE: f64 = 90.0;

/// The next session id [`AppleSession::open`] hands out — the integer the
/// preview slot's `params_json` carries and [`frust_camera_session_handle`]
/// resolves back to a live `AVCaptureSession`.
///
/// Starts at `1` so `0` is never a valid id (matching the Android backend's
/// `-1` sentinel: neither backend's "no session" value is ever a real one).
static NEXT_SESSION_ID: AtomicI32 = AtomicI32::new(1);

/// Every live session, keyed by the id above.
///
/// A process-global registry is what lets the C export
/// ([`frust_camera_session_handle`]) resolve a plain integer — the only thing
/// a platform-view `params_json` payload can carry across the FFI boundary —
/// back to the session object. Entries are inserted by [`AppleSession::open`]
/// and removed by [`SessionInner::close`].
static SESSIONS: LazyLock<Mutex<HashMap<i32, Arc<SessionInner>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The session registry, poison-tolerant.
///
/// A panic while the registry lock is held would poison it and turn every
/// later camera call — including the `extern "C"` export — into a panic of
/// its own, which is exactly what must not cross an FFI boundary
/// (`docs/CODE_STANDARDS.md`). The map holds no invariant a panic could half
/// break (it is a plain id → session table), so recovering the guard is
/// sound.
fn sessions() -> MutexGuard<'static, HashMap<i32, Arc<SessionInner>>> {
    SESSIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A value handed to (or shared with) this plugin's serial session queue.
///
/// AVFoundation objects are `!Send`/`!Sync` in `objc2`'s type system, but
/// `dispatch2`'s `exec_sync`/`exec_async` require `Send` closures, so
/// crossing the queue boundary needs one confined assertion.
///
/// # Safety
///
/// Every `QueueBound` in this module wraps AVFoundation objects whose *use*
/// is confined to one serial queue (module doc's *Threading*): the session,
/// its input/output, and a per-capture settings+delegate pair are only ever
/// messaged from inside a `DispatchQueue::exec_sync` body on the owning
/// session's queue, and a serial queue runs at most one such body at a time.
/// The wrapper is never cloned, never handed to a second queue, and the
/// objects it holds are otherwise only released (a thread-safe operation on
/// any ObjC object) when the wrapper drops.
struct QueueBound<T> {
    value: T,
}

// SAFETY: see the type's `# Safety` doc — access is serialized by the owning
// session's serial dispatch queue.
unsafe impl<T> Send for QueueBound<T> {}
// SAFETY: as above; `&QueueBound<T>` only ever reaches a closure that runs on
// that same serial queue.
unsafe impl<T> Sync for QueueBound<T> {}

impl<T> QueueBound<T> {
    fn new(value: T) -> Self {
        Self { value }
    }

    /// The wrapped value. Callers must already be on (or be dispatching onto)
    /// the owning serial queue — see the type's `# Safety` doc.
    fn get(&self) -> &T {
        &self.value
    }

    /// Take the wrapped value, consuming the wrapper. Used to hand freshly
    /// allocated objects *into* a queue body that then owns them (the image
    /// stream's output/delegate/queue triple).
    fn into_inner(self) -> T {
        self.value
    }
}

/// The AVFoundation object graph one open session owns.
struct AvObjects {
    session: Retained<AVCaptureSession>,
    /// The device input, held for the session's lifetime: dropping it would
    /// release the device the session is still wired to.
    input: Retained<AVCaptureDeviceInput>,
    photo_output: Retained<AVCapturePhotoOutput>,
    /// The running image stream, or `None`.
    ///
    /// A [`RefCell`] rather than a lock: every read and write happens inside a
    /// body on the owning session's **serial** queue ([`QueueBound`]'s
    /// `# Safety` doc), so there is never a second borrower to contend with —
    /// and a lock here would only hide a threading mistake instead of the
    /// `RefCell` panicking on it.
    stream: RefCell<Option<VideoStream>>,
}

/// One running image stream's AVFoundation graph, owned by [`AvObjects`] and
/// therefore queue-confined like the rest of it.
struct VideoStream {
    /// The output added to the session; removed again by [`detach_stream`].
    output: Retained<AVCaptureVideoDataOutput>,
    /// The delegate the output calls. `AVCaptureVideoDataOutput` retains its
    /// sample-buffer delegate, but this crate keeps its own strong reference
    /// too so the object's lifetime is never inferred from framework
    /// behaviour.
    delegate: Retained<SampleBufferDelegate>,
    /// The serial queue frames are delivered on (module doc's *Threading*).
    /// Held so it outlives the delegate registration.
    queue: DispatchRetained<DispatchQueue>,
}

/// One open camera — the state [`AppleSession`] and the C export share.
struct SessionInner {
    /// This session's registry id (see [`SESSIONS`]).
    id: i32,
    /// The serial queue every session mutation runs on (module doc's
    /// *Threading*).
    queue: DispatchRetained<DispatchQueue>,
    /// The AVFoundation graph, queue-confined ([`QueueBound`]).
    objects: QueueBound<AvObjects>,
    /// The preview aspect ratio (width / height, rotation-applied), stored as
    /// `f32` bits so a later phase can refresh it without a lock.
    aspect_ratio: AtomicU32,
    /// Set by [`Self::close`]; every subsequent operation reports
    /// [`CameraError::SessionClosed`].
    closed: AtomicBool,
}

impl SessionInner {
    /// Run `body` on this session's serial queue and wait for it.
    ///
    /// `exec_sync` (not `exec_async`) because every caller needs the result:
    /// a configuration failure has to surface as a [`CameraError`], and the
    /// public API is documented blocking (module doc's *Threading*).
    fn on_queue<F: Send + FnOnce(&AvObjects)>(&self, body: F) {
        let objects = &self.objects;
        self.queue.exec_sync(move || body(objects.get()));
    }

    /// Start the configured session, **asynchronously** on its own serial
    /// queue.
    ///
    /// `startRunning` blocks until the capture pipeline is live (hundreds of
    /// milliseconds on a real device), so it is the one session operation
    /// [`AppleSession::open`] does not wait on: the public API documents
    /// session configuration as asynchronous, with
    /// [`preview_aspect_ratio`](crate::CameraSession::preview_aspect_ratio)
    /// and the preview slot as the readiness signals. Queued FIFO behind the
    /// configuration transaction and ahead of any later
    /// capture/[`close`](Self::close) on the same serial queue.
    fn start(self: &Arc<Self>) {
        let session = Arc::clone(self);
        self.queue.exec_async(move || {
            if session.closed.load(Ordering::Acquire) {
                // Closed before the start ever ran — nothing to start.
                return;
            }
            // SAFETY: a lifecycle message on a fully configured session, sent
            // from that session's own serial queue (module doc's *Threading*).
            unsafe { session.objects.get().session.startRunning() };
        });
    }

    /// The preview aspect ratio, or `0.0` before one is known.
    fn aspect_ratio(&self) -> f32 {
        f32::from_bits(self.aspect_ratio.load(Ordering::Relaxed))
    }

    /// See [`crate::CameraSession::take_picture`].
    ///
    /// Blocks the calling thread for up to [`CAPTURE_TIMEOUT`], so it refuses
    /// the main run loop outright ([`reject_on_main_thread`]) rather than
    /// freezing the frame it was called from.
    fn take_picture(&self, path: &Path) -> Result<(), CameraError> {
        reject_on_main_thread("take_picture")?;
        if self.closed.load(Ordering::Acquire) {
            return Err(CameraError::SessionClosed);
        }

        // Fail before firing a capture the delegate could not write anywhere:
        // a missing parent directory is the caller's error, and reporting it
        // here keeps it a synchronous `Io` rather than a timed-out capture.
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && !parent.is_dir()
        {
            return Err(CameraError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("capture directory {} does not exist", parent.display()),
            )));
        }

        let (tx, rx) = sync_channel::<CaptureOutcome>(1);
        // `+photoSettings` is documented to default to the JPEG codec in a
        // JFIF container (`AVVideoCodecTypeJPEG` / `AVFileTypeJPEG`), which is
        // exactly the JPEG-to-path contract this call promises — so no
        // format dictionary (and no `AVVideoSettings` feature) is needed.
        // A settings object may be used for one capture only, hence one fresh
        // instance per call.
        //
        // SAFETY: `objc2` marks every AVFoundation constructor `unsafe`; this
        // one takes no arguments and returns a fresh, owned instance.
        let settings = unsafe { AVCapturePhotoSettings::photoSettings() };
        let delegate = PhotoCaptureDelegate::new(path.to_path_buf(), tx);

        let request = QueueBound::new((settings, delegate));
        self.on_queue(|objects| {
            let (settings, delegate) = request.get();
            let delegate = ProtocolObject::from_ref(&**delegate);
            // SAFETY: both arguments are live, correctly-typed objects; the
            // delegate conforms to `AVCapturePhotoCaptureDelegate` (its
            // `define_class!` block below implements the protocol), and
            // `AVCapturePhotoOutput` retains it until the capture completes.
            // Called on the session's serial queue per the module doc.
            unsafe {
                objects
                    .photo_output
                    .capturePhotoWithSettings_delegate(settings, delegate);
            }
        });

        match rx.recv_timeout(CAPTURE_TIMEOUT) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(CaptureFailure::Io(err))) => Err(CameraError::Io(err)),
            Ok(Err(CaptureFailure::Platform(message))) => Err(CameraError::Platform(message)),
            Err(_) => Err(CameraError::Platform(format!(
                "apple camera backend: still capture did not complete within {}s",
                CAPTURE_TIMEOUT.as_secs()
            ))),
        }
    }

    /// See [`crate::CameraSession::start_image_stream`].
    ///
    /// # Close-deadline contract (the failure mode this backend can produce)
    ///
    /// The delegate locks the `CVPixelBuffer`, calls `on_frame`, and unlocks
    /// again the moment it returns; AVFoundation then recycles the buffer into
    /// its capture pool. A callback that keeps a plane slice past its return
    /// would be reading recycled memory, and one that simply *takes too long*
    /// blocks its serial delivery queue — with
    /// `alwaysDiscardsLateVideoFrames` (the lossy-latest analogue of Android's
    /// `STRATEGY_KEEP_ONLY_LATEST`) the frames that arrive meanwhile are
    /// dropped rather than queued. Copy or consume before returning; see
    /// [`crate::ImageFrame`]'s doc, which states the same contract on the
    /// public API.
    ///
    /// Blocks only for the configuration transaction on the session queue, not
    /// for any frame.
    fn start_image_stream(
        &self,
        format: ImageFormat,
        on_frame: Box<ImageFrameCallback>,
    ) -> Result<(), CameraError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(CameraError::SessionClosed);
        }

        // Allocation on the calling thread — no live session is touched (module
        // doc's *Threading*).
        // SAFETY: an argument-free constructor returning a fresh, owned
        // instance.
        let output = unsafe { AVCaptureVideoDataOutput::new() };
        let queue = DispatchQueue::new(FRAMES_QUEUE_LABEL, DispatchQueueAttr::SERIAL);
        let delegate = SampleBufferDelegate::new(format, on_frame);
        let staged = QueueBound::new((output, delegate, queue));

        let outcome: Mutex<Result<(), String>> = Mutex::new(Ok(()));
        {
            let outcome = &outcome;
            self.on_queue(move |objects| {
                let (output, delegate, queue) = staged.into_inner();
                *outcome.lock().unwrap_or_else(|e| e.into_inner()) =
                    attach_stream(objects, output, delegate, queue, pixel_format_for(format));
            });
        }
        outcome
            .into_inner()
            .unwrap_or_else(|e| e.into_inner())
            .map_err(CameraError::Platform)
    }

    /// See [`crate::CameraSession::stop_image_stream`] — the preview and the
    /// capture device are untouched. A no-op if no stream is running.
    fn stop_image_stream(&self) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        self.on_queue(detach_stream);
    }

    /// See [`crate::CameraSession::close`] — idempotent.
    fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        sessions().remove(&self.id);
        self.on_queue(|objects| {
            // Drop any running stream first: its delegate owns the app's frame
            // callback, which must not outlive the session that fed it.
            detach_stream(objects);
            // SAFETY: `stopRunning` is a plain session-lifecycle message on a
            // live session, sent from the session's own serial queue (the
            // module doc's threading rule).
            unsafe { objects.session.stopRunning() };
        });
        log::debug!("frust-camera: apple session {} closed", self.id);
    }
}

/// [`crate::Camera::request_permission`]'s Apple arm.
///
/// Fast path first (`authorizationStatusForMediaType:`, no dialog); only an
/// undecided status prompts, and that prompt's completion handler — which
/// AVFoundation runs on an arbitrary queue — is bridged onto a channel this
/// call blocks on (module doc's *Threading*).
///
/// The four `AVAuthorizationStatus` values collapse onto three
/// [`PermissionStatus`] variants, per the crate doc's mapping note:
/// `Authorized` → [`PermissionStatus::Granted`], `Denied`/`Restricted` →
/// [`PermissionStatus::Denied`], `NotDetermined` → whatever the prompt
/// resolves to. [`PermissionStatus::NeedsUi`] is Android-only (iOS needs no
/// Activity to prompt) and never returned here;
/// [`PermissionStatus::Pending`] is returned only if the prompt outlives
/// [`PERMISSION_TIMEOUT`].
pub(crate) fn request_permission() -> Result<PermissionStatus, CameraError> {
    let media_type = video_media_type()?;

    // SAFETY: `media_type` is `AVMediaTypeVideo`, one of the two media types
    // this class method accepts (any other throws `NSInvalidArgumentException`
    // — see the binding's own doc).
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) };
    match status {
        AVAuthorizationStatus::Authorized => return Ok(PermissionStatus::Granted),
        AVAuthorizationStatus::Denied | AVAuthorizationStatus::Restricted => {
            return Ok(PermissionStatus::Denied);
        }
        AVAuthorizationStatus::NotDetermined => {}
        // `AVAuthorizationStatus` is a non-exhaustive `NSInteger` newtype; a
        // value a future iOS adds is treated as "not granted" rather than
        // optimistically allowed.
        other => {
            log::warn!("frust-camera: unknown AVAuthorizationStatus {}", other.0);
            return Ok(PermissionStatus::Denied);
        }
    }

    // Only the prompt path blocks, so only the prompt path is guarded: the
    // three statuses above returned without waiting for anything and are safe
    // from any thread. From here the call parks until the user answers a
    // consent alert — and that alert needs the main thread to be free to
    // present it, so a main-thread caller waits 120 s for a dialog its own
    // wait is preventing. Refuse it with the same typed error the Android half
    // and `take_picture` use.
    reject_on_main_thread("request_permission")?;

    let (tx, rx) = sync_channel::<bool>(1);
    let handler = RcBlock::new(move |granted: Bool| {
        // A full channel means the receiver already timed out and moved on —
        // dropping the late answer is correct, and `try_send` keeps this
        // arbitrary-queue callback from ever blocking.
        let _ = tx.try_send(granted.as_bool());
    });
    // SAFETY: `media_type` is `AVMediaTypeVideo` (as above) and `handler` is a
    // live block of the `void (^)(BOOL)` shape this method calls. The block is
    // kept alive by `handler` until this function returns, which happens only
    // after the channel resolves or the timeout expires.
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &handler) };

    match rx.recv_timeout(PERMISSION_TIMEOUT) {
        Ok(true) => Ok(PermissionStatus::Granted),
        Ok(false) => Ok(PermissionStatus::Denied),
        Err(_) => Ok(PermissionStatus::Pending),
    }
}

/// The Apple [`SessionBackend`] — a handle onto one registered
/// [`SessionInner`].
pub(crate) struct AppleSession {
    inner: Arc<SessionInner>,
}

impl AppleSession {
    /// [`crate::Camera::open`]'s Apple arm: discover the device for `lens`,
    /// build and configure the session graph, then queue its start — every
    /// mutation on the session's own serial queue (module doc's *Threading*),
    /// and the start itself asynchronous ([`SessionInner::start`]) so this
    /// call never waits on the capture pipeline coming up.
    ///
    /// # Errors
    /// [`CameraError::PermissionDenied`] when access is already denied or
    /// restricted (a synchronous convenience — an undecided status is *not*
    /// an error: AVFoundation prompts when the device input is created).
    /// [`CameraError::Platform`] when no camera matches `lens` or the session
    /// rejects the input/output. [`CameraError::InUse`] when another session
    /// holds the device.
    pub(crate) fn open(lens: Lens, resolution: Resolution) -> Result<Self, CameraError> {
        let media_type = video_media_type()?;

        // SAFETY: `media_type` is `AVMediaTypeVideo` — see
        // `request_permission`'s identical call.
        let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) };
        if matches!(
            status,
            AVAuthorizationStatus::Denied | AVAuthorizationStatus::Restricted
        ) {
            return Err(CameraError::PermissionDenied);
        }

        let device = default_device(lens, media_type).ok_or_else(|| {
            CameraError::Platform(format!(
                "apple camera backend: no {lens:?}-facing capture device is available"
            ))
        })?;

        // SAFETY: `device` is a live capture device. This constructor opens
        // the device (taking exclusive control), returning the underlying
        // `NSError` rather than throwing on failure.
        let input = unsafe { AVCaptureDeviceInput::deviceInputWithDevice_error(&device) }
            .map_err(|error| device_input_error(&error))?;
        // SAFETY: argument-free constructors returning fresh, owned instances.
        let (session, photo_output) =
            unsafe { (AVCaptureSession::new(), AVCapturePhotoOutput::new()) };

        let objects = QueueBound::new(AvObjects {
            session,
            input,
            photo_output,
            stream: RefCell::new(None),
        });
        let queue = DispatchQueue::new(SESSION_QUEUE_LABEL, DispatchQueueAttr::SERIAL);

        // The configuration transaction, on the serial queue. `configure`
        // reports through this slot rather than a return value because
        // `exec_sync` bodies return nothing; its `Ok` payload is the preview
        // aspect ratio, read on the queue *after* the preset has settled the
        // device's active format.
        let outcome: Mutex<Result<f32, String>> = Mutex::new(Ok(0.0));
        {
            let objects = &objects;
            let outcome = &outcome;
            queue.exec_sync(move || {
                *outcome.lock().unwrap_or_else(|e| e.into_inner()) =
                    configure(objects.get(), resolution, lens);
            });
        }
        let aspect = outcome
            .into_inner()
            .unwrap_or_else(|e| e.into_inner())
            .map_err(CameraError::Platform)?;

        let id = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
        let inner = Arc::new(SessionInner {
            id,
            queue,
            objects,
            aspect_ratio: AtomicU32::new(aspect.to_bits()),
            closed: AtomicBool::new(false),
        });
        sessions().insert(id, Arc::clone(&inner));
        inner.start();
        log::debug!("frust-camera: apple session {id} open ({lens:?}, aspect {aspect})");

        Ok(Self { inner })
    }
}

impl Drop for AppleSession {
    /// Dropping the last handle releases the camera.
    ///
    /// Without this, an app that drops its `CameraSession` without calling
    /// [`crate::CameraSession::close`] would leave the capture device (and
    /// its registry entry) live for the rest of the process — the exact
    /// leak Flutter's "dispose on inactive" lesson is about (RESEARCH.md §1).
    /// [`SessionInner::close`] is idempotent, so an explicit `close()` first
    /// costs nothing here.
    fn drop(&mut self) {
        self.inner.close();
    }
}

impl SessionBackend for AppleSession {
    fn preview_view_type(&self) -> &'static str {
        PREVIEW_VIEW_TYPE
    }

    fn params_json(&self) -> String {
        // `session` (not Android's `sessionId`): each platform's factory owns
        // its own payload shape, and the iOS factory (task 08's
        // `CameraPreviewFactory.swift`) parses `{"session": N}` before calling
        // `frust_camera_session_handle(N)`.
        format!(r#"{{"session":{}}}"#, self.inner.id)
    }

    fn preview_aspect_ratio(&self) -> f32 {
        self.inner.aspect_ratio()
    }

    fn take_picture(&self, path: &Path) -> Result<(), CameraError> {
        self.inner.take_picture(path)
    }

    fn start_image_stream(
        &self,
        format: ImageFormat,
        on_frame: Box<ImageFrameCallback>,
    ) -> Result<(), CameraError> {
        self.inner.start_image_stream(format, on_frame)
    }

    fn stop_image_stream(&self) {
        self.inner.stop_image_stream();
    }

    fn close(&self) {
        self.inner.close();
    }
}

// --- The preview seam ------------------------------------------------------

/// `frust_camera_session_handle`: resolve a session id to its live
/// `AVCaptureSession`, for the Swift preview factory (task 08).
///
/// The id is the `"session"` value from
/// [`CameraSession::params_json`](crate::CameraSession::params_json), which
/// the platform-view host hands the factory as its `paramsJson` payload.
///
/// # Retain contract — ownership is TRANSFERRED at +1
///
/// The returned pointer carries a **retain the caller owns** (the
/// `CFBridgingRetain` idiom): this function takes an ObjC-level retain on the
/// session before returning, and the caller is responsible for releasing it
/// exactly once —
/// `Unmanaged<AVCaptureSession>.fromOpaque(ptr).takeRetainedValue()` from
/// Swift (ARC then owns it), or `CFRelease`/`release` from plain C/ObjC.
/// Dropping the pointer without releasing leaks the session object.
///
/// It is +1 rather than a +0 borrow because a +0 borrow is **not honourable
/// here**: `SessionInner::close`/`AppleSession::drop` can run on any thread
/// (an `on_cleanup`, a `spawn_blocking` worker, a drop inside a background
/// task), and the caller has no way to observe that close. A close landing
/// between this function's return and the caller's own retain would leave the
/// caller messaging a freed object — a use-after-free across the FFI
/// boundary. Taking the retain on this side, while the registry's
/// `Arc<SessionInner>` (and with it the session's own strong reference) is
/// still alive, closes that window entirely: a session closed a microsecond
/// later stops running, but the returned pointer stays a valid object until
/// the caller releases it.
///
/// Returns null for an unknown or already-closed id; the Swift side treats
/// null as "no preview yet" and re-asks on the next params update. A null
/// return owns nothing and must not be released.
///
/// # Symbol survival (UNVERIFIED — device gate owns this)
///
/// This is the repo's first Swift-called `#[unsafe(no_mangle)]` export
/// defined in a **dependency** crate rather than stamped into the app crate
/// by a macro (`ios_app!`). Whether it survives the release profile's
/// `lto = "fat"` + `strip = "symbols"` into the linked staticlib is untested
/// (PLAN.md's *Symbol survival* risk). [`ios_exports`] is the fallback an
/// app invokes if it does not.
///
/// # Safety
///
/// This function is safe to call with any `session` value; it is `extern "C"`
/// only so Swift can. It never panics across the FFI boundary (the body is
/// [`catch_unwind`](std::panic::catch_unwind)-wrapped). What the *caller* must
/// uphold is the release half of the retain contract above — the returned
/// pointer is owned, and leaking or double-releasing it is on that side of
/// the boundary.
#[unsafe(no_mangle)]
pub extern "C" fn frust_camera_session_handle(session: i32) -> *mut c_void {
    std::panic::catch_unwind(|| {
        // The registry clone is what keeps the session graph alive across the
        // retain below: a concurrent `close()` removes the registry entry, but
        // this `Arc` still holds `AvObjects` (and therefore the session's own
        // strong reference) until this function returns.
        let Some(inner) = sessions().get(&session).cloned() else {
            log::debug!("frust-camera: no live apple session {session}");
            return std::ptr::null_mut();
        };
        // SAFETY (the assertion, not a call): cloning a `Retained` is an
        // `objc_retain`, which is thread-safe on any ObjC object — the same
        // exemption `QueueBound`'s `# Safety` doc already grants release. No
        // AVFoundation *state* is touched, so the serial-queue confinement is
        // not weakened by doing this off the session queue.
        let retained: Retained<AVCaptureSession> = inner.objects.get().session.clone();
        // Hand the +1 out (the `CFBridgingRetain` idiom): `into_raw` forgets
        // the `Retained` without releasing, so the retain taken above is the
        // one the caller now owns per this function's retain contract.
        Retained::into_raw(retained).cast::<c_void>()
    })
    .unwrap_or_else(|_| {
        log::error!("frust-camera: panic in frust_camera_session_handle — returning null");
        std::ptr::null_mut()
    })
}

/// Force-retain this crate's iOS C exports from the app crate.
///
/// **Invoke this only if the device gate shows the direct export missing** —
/// [`frust_camera_session_handle`] is defined unconditionally above, and an
/// app that links it successfully needs nothing here.
///
/// The risk this exists for: [`frust_camera_session_handle`] lives in a
/// *dependency* crate, and nothing inside the Rust staticlib calls it (only
/// Swift does), so a release build's `lto = "fat"` may internalize and drop
/// it before the Swift side ever gets to link against it — the untested
/// caveat on that function. Expanding this macro in the **app crate** (the
/// staticlib root) plants a `#[used]` reference to the symbol, which keeps
/// LTO from treating it as dead and keeps the archive member it lives in
/// pulled in at final link.
///
/// It deliberately *references* rather than *re-defines* the export: two
/// definitions of one `#[unsafe(no_mangle)]` symbol in one link unit is a
/// hard duplicate-symbol error, so a define-shaped fallback could not
/// coexist with the direct export it is meant to back up.
///
/// ```ignore
/// // in the app crate's `lib.rs`, beside `frust::ios_app!(...)`:
/// #[cfg(target_vendor = "apple")]
/// frust_camera::ios_exports!();
/// ```
///
/// The `#[cfg]` is on the caller because this macro only exists on Apple
/// targets (it is declared in this target-gated module).
#[macro_export]
macro_rules! ios_exports {
    () => {
        const _: () = {
            /// `frust_camera_session_handle`'s C signature.
            type FrustCameraSessionHandle = unsafe extern "C" fn(i32) -> *mut ::core::ffi::c_void;

            unsafe extern "C" {
                fn frust_camera_session_handle(session: i32) -> *mut ::core::ffi::c_void;
            }

            /// A `#[used]` reference to every C export this plugin ships, so
            /// LTO cannot internalize one no Rust code calls.
            #[used]
            static FRUST_CAMERA_IOS_EXPORTS: [FrustCameraSessionHandle; 1] =
                [frust_camera_session_handle];
        };
    };
}

// --- The photo-capture delegate -------------------------------------------

/// What one still capture reports back to the blocked
/// [`SessionInner::take_picture`].
type CaptureOutcome = Result<(), CaptureFailure>;

/// A still capture's failure, kept typed so the caller can map it back onto
/// the right [`CameraError`] variant rather than flattening everything into
/// [`CameraError::Platform`].
enum CaptureFailure {
    /// Writing the flattened JPEG to the caller's path failed.
    Io(std::io::Error),
    /// AVFoundation reported a capture error, or produced no file data.
    Platform(String),
}

/// [`PhotoCaptureDelegate`]'s instance state.
struct PhotoDelegateIvars {
    /// Where to write the flattened JPEG.
    path: PathBuf,
    /// The waiting [`SessionInner::take_picture`] call. Taken (not cloned) on
    /// completion so a delegate that somehow fires twice reports once.
    tx: Mutex<Option<SyncSender<CaptureOutcome>>>,
}

define_class!(
    // SAFETY:
    // - `NSObject` has no subclassing requirements.
    // - This class implements `Drop` only through its ivars (no manual impl),
    //   so the macro's generated `dealloc` has nothing extra to uphold.
    #[unsafe(super(NSObject))]
    // AVFoundation invokes photo-capture callbacks on "a common dispatch
    // queue — not necessarily the main queue" (the protocol's own doc), so
    // this class must be usable from any thread.
    #[thread_kind = AnyThread]
    #[ivars = PhotoDelegateIvars]
    struct PhotoCaptureDelegate;

    unsafe impl NSObjectProtocol for PhotoCaptureDelegate {}

    // SAFETY: the one method implemented below is the protocol's own
    // `captureOutput:didFinishProcessingPhoto:error:`, with the signature the
    // binding declares. Every other member of the protocol is `@optional`.
    unsafe impl AVCapturePhotoCaptureDelegate for PhotoCaptureDelegate {
        #[unsafe(method(captureOutput:didFinishProcessingPhoto:error:))]
        fn did_finish_processing_photo(
            &self,
            _output: &AVCapturePhotoOutput,
            photo: &AVCapturePhoto,
            error: Option<&NSError>,
        ) {
            // This runs on an AVFoundation-owned queue: a panic here would
            // unwind into an Objective-C frame, which is undefined behavior
            // (`docs/CODE_STANDARDS.md`'s no-unwind-across-FFI rule).
            let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
                flatten_photo(photo, error).and_then(|bytes| {
                    std::fs::write(&self.ivars().path, bytes).map_err(CaptureFailure::Io)
                })
            }))
            .unwrap_or_else(|_| {
                Err(CaptureFailure::Platform(
                    "apple camera backend: panic while writing the captured photo".to_string(),
                ))
            });
            self.complete(outcome);
        }
    }
);

impl PhotoCaptureDelegate {
    /// A delegate for one capture, writing to `path` and reporting through
    /// `tx`.
    fn new(path: PathBuf, tx: SyncSender<CaptureOutcome>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(PhotoDelegateIvars {
            path,
            tx: Mutex::new(Some(tx)),
        });
        // SAFETY: `NSObject`'s designated initializer, called on a freshly
        // allocated instance whose ivars are already set.
        unsafe { msg_send![super(this), init] }
    }

    /// Report `outcome` to the waiting call, at most once.
    fn complete(&self, outcome: CaptureOutcome) {
        let sender = self
            .ivars()
            .tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        match sender {
            // A full/disconnected channel means the caller timed out — the
            // photo is already written either way, so the late answer is
            // simply dropped.
            Some(tx) => drop(tx.try_send(outcome)),
            None => log::warn!("frust-camera: duplicate photo-capture completion ignored"),
        }
    }
}

/// The delivered photo's file bytes (JPEG, per the settings
/// [`SessionInner::take_picture`] requests), or a typed failure.
fn flatten_photo(
    photo: &AVCapturePhoto,
    error: Option<&NSError>,
) -> Result<Vec<u8>, CaptureFailure> {
    if let Some(error) = error {
        return Err(CaptureFailure::Platform(format!(
            "apple camera backend: photo capture failed ({})",
            describe(error)
        )));
    }
    // SAFETY: `photo` is the live, non-null photo AVFoundation just handed
    // this delegate; `fileDataRepresentation` flattens it into the container
    // format the capture settings named, returning nil only on failure.
    let data = unsafe { photo.fileDataRepresentation() }.ok_or_else(|| {
        CaptureFailure::Platform(
            "apple camera backend: photo produced no file data representation".to_string(),
        )
    })?;
    Ok(data.to_vec())
}

// --- The image stream ------------------------------------------------------

/// The `CVPixelBuffer` format type a requested [`ImageFormat`] maps to.
///
/// `420f` (full-range bi-planar) rather than `420v` (video-range) for
/// [`ImageFormat::Yuv420`]: full range is what Android's `YUV_420_888` carries,
/// so one callback can treat both platforms' luma identically.
fn pixel_format_for(format: ImageFormat) -> u32 {
    match format {
        ImageFormat::Yuv420 => kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
        ImageFormat::Bgra => kCVPixelFormatType_32BGRA,
    }
}

/// The `videoSettings` dictionary that pins an output's delivered pixel
/// format: `{ kCVPixelBufferPixelFormatTypeKey: <pixel_format> }`.
fn video_settings(pixel_format: u32) -> Retained<NSDictionary<NSString, AnyObject>> {
    // SAFETY: reading an `extern` CoreVideo constant static (edition-2024
    // unsafe); linker-provided and non-null wherever CoreVideo is linked. The
    // `CFString` → `NSString` view is the toll-free bridge `objc2-foundation`
    // itself models as `AsRef`.
    let key: &NSString = AsRef::as_ref(unsafe { kCVPixelBufferPixelFormatTypeKey });
    let value = NSNumber::numberWithUnsignedInt(pixel_format);
    let value: &AnyObject = &value;
    NSDictionary::from_slices(&[key], &[value])
}

/// Wire `output` into the session and start delivering frames to `delegate` on
/// `queue`. Runs on the session's serial queue; replaces any stream already
/// running (a restart at another format is just another `start_image_stream`).
///
/// Returns the message for a [`CameraError::Platform`] on failure, in which
/// case nothing was left attached.
fn attach_stream(
    objects: &AvObjects,
    output: Retained<AVCaptureVideoDataOutput>,
    delegate: Retained<SampleBufferDelegate>,
    queue: DispatchRetained<DispatchQueue>,
    pixel_format: u32,
) -> Result<(), String> {
    detach_stream(objects);

    let session = &objects.session;
    // SAFETY: plain message sends on live, correctly typed objects, from the
    // session's own serial queue (module doc's *Threading*). `addOutput` is
    // guarded by `canAddOutput`, which is what keeps it from throwing.
    let added = unsafe {
        session.beginConfiguration();
        let added = session.canAddOutput(&output);
        if added {
            session.addOutput(&output);
        }
        session.commitConfiguration();
        added
    };
    if !added {
        return Err("apple camera backend: the session rejected the video data output".to_string());
    }

    // SAFETY: as above, plus: `videoSettings` is set *after* the output joined
    // the session (the point at which its supported pixel formats are known),
    // and both formats this crate offers are stock capture formats every iOS
    // device advertises. The delegate conforms to
    // `AVCaptureVideoDataOutputSampleBufferDelegate` (its `define_class!` block
    // below implements the protocol) and `queue` is a live serial queue, which
    // the setter requires whenever the delegate is non-nil.
    unsafe {
        // The lossy-latest policy: a frame that arrives while the delegate
        // queue is busy is dropped, never queued (`docs/ARCHITECTURE.md`'s
        // gRPC-stream conflation guidance, applied to frames).
        output.setAlwaysDiscardsLateVideoFrames(true);
        output.setVideoSettings(Some(&video_settings(pixel_format)));
        let protocol = ProtocolObject::from_ref(&*delegate);
        output.setSampleBufferDelegate_queue(Some(protocol), Some(&queue));
    }

    objects.stream.replace(Some(VideoStream {
        output,
        delegate,
        queue,
    }));
    Ok(())
}

/// Stop and remove a running stream, if any. Runs on the session's serial
/// queue; leaves the preview and photo output untouched.
///
/// Clearing the delegate first is what guarantees no *new* callback starts;
/// AVFoundation retains the delegate for the duration of one in-flight call,
/// so dropping this crate's own reference immediately afterwards is safe (and
/// avoids a barrier that would deadlock a callback stopping its own stream).
fn detach_stream(objects: &AvObjects) {
    let Some(stream) = objects.stream.borrow_mut().take() else {
        return;
    };
    // SAFETY: message sends on live objects from the session's serial queue;
    // `setSampleBufferDelegate:queue:` accepts a nil queue exactly when the
    // delegate is nil, which is the pair passed here.
    unsafe {
        stream.output.setSampleBufferDelegate_queue(None, None);
        objects.session.beginConfiguration();
        objects.session.removeOutput(&stream.output);
        objects.session.commitConfiguration();
    }
    // Release in teardown order: the output is detached from the session
    // above, then the delegate it called, then the queue those calls ran on.
    drop(stream.output);
    drop(stream.delegate);
    drop(stream.queue);
}

/// [`SampleBufferDelegate`]'s instance state.
struct SampleDelegateIvars {
    /// The format the stream was started with — the one
    /// [`attach_stream`] pinned via `videoSettings`, and therefore the one
    /// every delivered [`crate::ImageFrame`] reports.
    format: ImageFormat,
    /// The app's frame callback, invoked synchronously per frame.
    on_frame: Box<ImageFrameCallback>,
}

define_class!(
    // SAFETY:
    // - `NSObject` has no subclassing requirements.
    // - This class implements `Drop` only through its ivars (no manual impl),
    //   so the macro's generated `dealloc` has nothing extra to uphold.
    #[unsafe(super(NSObject))]
    // Sample buffers are delivered on the queue passed to
    // `setSampleBufferDelegate:queue:` — this plugin's own, never the main
    // one — so this class must be usable from any thread.
    #[thread_kind = AnyThread]
    #[ivars = SampleDelegateIvars]
    struct SampleBufferDelegate;

    unsafe impl NSObjectProtocol for SampleBufferDelegate {}

    // SAFETY: the one method implemented below is the protocol's own
    // `captureOutput:didOutputSampleBuffer:fromConnection:`, with the signature
    // the binding declares. Every member of the protocol is `@optional`.
    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for SampleBufferDelegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn did_output_sample_buffer(
            &self,
            _output: &AVCaptureOutput,
            sample_buffer: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            // This runs on a plugin-owned queue but is *called* from an
            // Objective-C frame: a panic here (including one out of the app's
            // own callback) would unwind into it, which is undefined behavior
            // (`docs/CODE_STANDARDS.md`'s no-unwind-across-FFI rule).
            let delivered = std::panic::catch_unwind(AssertUnwindSafe(|| {
                self.deliver(sample_buffer);
            }));
            if delivered.is_err() {
                log::error!("frust-camera: panic in an image-stream frame callback — frame lost");
            }
        }
    }
);

impl SampleBufferDelegate {
    /// A delegate delivering `format` frames to `on_frame`.
    fn new(format: ImageFormat, on_frame: Box<ImageFrameCallback>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(SampleDelegateIvars { format, on_frame });
        // SAFETY: `NSObject`'s designated initializer, called on a freshly
        // allocated instance whose ivars are already set.
        unsafe { msg_send![super(this), init] }
    }

    /// Lock one sample buffer's pixel buffer, hand its planes to the app
    /// callback, unlock. A frame carrying no image buffer (or one that cannot
    /// be locked) is dropped with a log line — never a panic on the delivery
    /// queue.
    fn deliver(&self, sample_buffer: &CMSampleBuffer) {
        // SAFETY: `sample_buffer` is the live buffer AVFoundation just handed
        // this delegate; `image_buffer` is a +1 accessor returning `None` for a
        // non-video sample.
        let Some(pixel_buffer) = (unsafe { sample_buffer.image_buffer() }) else {
            log::warn!("frust-camera: sample buffer carried no image buffer — frame dropped");
            return;
        };

        // SAFETY: a read-only lock on the live pixel buffer, released by the
        // guard below — including on an unwind out of the callback, which is
        // why this is a guard and not a bare unlock call.
        let status = unsafe {
            CVPixelBufferLockBaseAddress(&pixel_buffer, CVPixelBufferLockFlags::ReadOnly)
        };
        if status != 0 {
            log::warn!(
                "frust-camera: CVPixelBufferLockBaseAddress failed ({status}) — frame dropped"
            );
            return;
        }
        let _guard = PixelBufferLock {
            buffer: &pixel_buffer,
        };

        self.deliver_locked(&pixel_buffer);
    }

    /// The locked half of [`Self::deliver`]: build the borrowed plane views and
    /// run the app callback.
    fn deliver_locked(&self, buffer: &CVPixelBuffer) {
        let format = self.ivars().format;
        let planar_count = CVPixelBufferGetPlaneCount(buffer);
        let mut planes = [
            ImagePlane {
                data: &[],
                row_stride: 0,
                pixel_stride: 0,
            },
            ImagePlane {
                data: &[],
                row_stride: 0,
                pixel_stride: 0,
            },
            ImagePlane {
                data: &[],
                row_stride: 0,
                pixel_stride: 0,
            },
        ];

        // A non-planar buffer (BGRA) reports plane count 0 and answers the
        // whole-buffer accessors instead of the per-plane ones.
        let count = if planar_count == 0 {
            1
        } else {
            planar_count.min(MAX_PLANES)
        };
        for (index, plane) in planes.iter_mut().enumerate().take(count) {
            let (address, row_stride, rows) = if planar_count == 0 {
                (
                    CVPixelBufferGetBaseAddress(buffer),
                    CVPixelBufferGetBytesPerRow(buffer),
                    CVPixelBufferGetHeight(buffer),
                )
            } else {
                (
                    CVPixelBufferGetBaseAddressOfPlane(buffer, index),
                    CVPixelBufferGetBytesPerRowOfPlane(buffer, index),
                    CVPixelBufferGetHeightOfPlane(buffer, index),
                )
            };
            if address.is_null() {
                log::warn!("frust-camera: pixel buffer plane {index} has no base address");
                return;
            }
            // SAFETY: the base address and row stride come from CoreVideo for
            // this locked plane, so `row_stride * rows` bytes are readable
            // until the lock guard in `deliver` releases it — which happens
            // only after the app callback below has returned (the
            // close-deadline contract on `SessionInner::start_image_stream`).
            plane.data = unsafe {
                std::slice::from_raw_parts(address.cast::<u8>().cast_const(), row_stride * rows)
            };
            plane.row_stride = row_stride;
            plane.pixel_stride = plane_pixel_stride(format, index);
        }

        let frame = ImageFrame {
            format,
            width: CVPixelBufferGetWidth(buffer) as u32,
            height: CVPixelBufferGetHeight(buffer) as u32,
            rotation_degrees: STREAM_ROTATION_DEGREES,
            planes: &planes[..count],
        };
        (self.ivars().on_frame)(&frame);
    }
}

/// Releases a `CVPixelBuffer`'s read lock on drop, so an unwind out of the app
/// callback cannot leave the capture pool's buffer locked forever.
struct PixelBufferLock<'a> {
    /// The locked buffer.
    buffer: &'a CVPixelBuffer,
}

impl Drop for PixelBufferLock<'_> {
    fn drop(&mut self) {
        // SAFETY: symmetrical with the `ReadOnly` lock taken in
        // `SampleBufferDelegate::deliver` — CoreVideo requires the same flags
        // on both sides, and this guard exists only while that lock is held.
        unsafe { CVPixelBufferUnlockBaseAddress(self.buffer, CVPixelBufferLockFlags::ReadOnly) };
    }
}

/// The bytes between consecutive pixels within one plane's row.
///
/// CoreVideo has no per-plane pixel-stride accessor (unlike Android's
/// `Plane.pixelStride`), so it follows from the pinned pixel format:
/// `32BGRA` is 4 bytes per pixel; `420f`'s luma plane is 1, and its chroma
/// plane interleaves Cb and Cr, so 2 — the same values Android's
/// `YUV_420_888` reports for an NV12-packed image.
fn plane_pixel_stride(format: ImageFormat, plane: usize) -> usize {
    match (format, plane) {
        (ImageFormat::Bgra, _) => 4,
        (ImageFormat::Yuv420, 0) => 1,
        (ImageFormat::Yuv420, _) => 2,
    }
}

// --- Session configuration -------------------------------------------------

/// The `beginConfiguration`/`commitConfiguration` transaction, run on the
/// session's serial queue by [`AppleSession::open`].
///
/// Returns the configured session's preview aspect ratio, or the message for
/// a [`CameraError::Platform`] on failure; the transaction is always
/// committed, even on the failure path, so the session is never left
/// mid-configuration.
///
/// The media-type constant is re-read here rather than passed in: a `&'static
/// AVMediaType` is `!Send` in `objc2`'s type system, and this function is the
/// body of a `Send` `exec_sync` closure (module doc's *Threading*).
fn configure(objects: &AvObjects, resolution: Resolution, lens: Lens) -> Result<f32, String> {
    let media_type = video_media_type().map_err(|error| error.to_string())?;
    let session = &objects.session;
    let output = &objects.photo_output;
    let input = &objects.input;

    // SAFETY: every call below is a plain message send on live, correctly
    // typed objects, made inside one `beginConfiguration`/`commitConfiguration`
    // transaction on the session's own serial queue (module doc's
    // *Threading*). `addInput`/`addOutput` are each guarded by their
    // `canAdd*` predicate, which is what keeps them from throwing.
    let result = unsafe {
        session.beginConfiguration();

        let preset = session_preset(resolution);
        let mut result = Ok(());
        if session.canSetSessionPreset(preset) {
            session.setSessionPreset(preset);
        } else {
            log::debug!("frust-camera: requested session preset unsupported; keeping the default");
        }

        if session.canAddInput(input) {
            session.addInput(input);
        } else {
            result = Err("the session rejected the camera input".to_string());
        }

        if result.is_ok() {
            if session.canAddOutput(output) {
                session.addOutput(output);
            } else {
                result = Err("the session rejected the photo output".to_string());
            }
        }

        session.commitConfiguration();
        result
    };
    result?;

    // SAFETY: `connectionWithMediaType:` returns the video connection this
    // output just gained (or nil if the graph did not wire one), and the
    // orientation/mirroring properties it feeds are guarded by their own
    // support predicates inside `configure_connection`.
    match unsafe { output.connectionWithMediaType(media_type) } {
        Some(connection) => configure_connection(&connection, lens),
        None => log::warn!("frust-camera: photo output has no video connection to orient"),
    }

    // Read the geometry only now: `setSessionPreset:` above is what settles
    // the device's `activeFormat`, and this runs on the session's own queue.
    //
    // SAFETY: `device` is a read-only property of the input this session
    // already owns.
    let device = unsafe { input.device() };
    let aspect = aspect_ratio_of(&device);

    // `startRunning` is deliberately NOT called here — it is queued
    // separately by `SessionInner::start`, so `Camera::open` never waits on
    // the capture pipeline coming up.
    Ok(aspect)
}

/// Apply this backend's rotation and front-camera mirroring to a capture
/// connection.
///
/// Rotation goes through `videoRotationAngle` (iOS 17+, the physical-device
/// floor `docs/DEVELOPMENT.md` already states) and falls back to the
/// deprecated `videoOrientation` on anything older. The version check is a
/// `respondsToSelector:` probe rather than an OS-version parse — the
/// Objective-C-idiomatic availability test, and the only one available to a
/// plugin with no `UIDevice` dependency.
fn configure_connection(connection: &AVCaptureConnection, lens: Lens) {
    // SAFETY: `respondsToSelector:` is a runtime query on a live object; both
    // rotation paths below are guarded by their own support predicate
    // (`isVideoRotationAngleSupported:` / `isVideoOrientationSupported`),
    // which is what keeps the setters from throwing
    // `NSInvalidArgumentException`.
    unsafe {
        if connection.respondsToSelector(sel!(isVideoRotationAngleSupported:))
            && connection.isVideoRotationAngleSupported(PORTRAIT_ROTATION_ANGLE)
        {
            connection.setVideoRotationAngle(PORTRAIT_ROTATION_ANGLE);
        } else {
            set_legacy_video_orientation(connection);
        }

        // Front-camera mirroring matches what the user sees in the preview.
        // `automaticallyAdjustsVideoMirroring` must be cleared first —
        // `setVideoMirrored:` throws while it is on.
        if lens == Lens::Front && connection.isVideoMirroringSupported() {
            connection.setAutomaticallyAdjustsVideoMirroring(false);
            connection.setVideoMirrored(true);
        }
    }
}

/// The pre-iOS-17 rotation path (`videoOrientation`), kept in its own
/// function so the deprecation allowance is scoped to the two calls that
/// need it rather than the whole connection setup.
#[allow(deprecated)]
fn set_legacy_video_orientation(connection: &AVCaptureConnection) {
    use objc2_av_foundation::AVCaptureVideoOrientation;

    // SAFETY: guarded by the framework's own `isVideoOrientationSupported`
    // predicate, which is the documented precondition for the setter.
    unsafe {
        if connection.isVideoOrientationSupported() {
            connection.setVideoOrientation(AVCaptureVideoOrientation::Portrait);
        }
    }
}

/// The `AVCaptureSessionPreset` for a requested [`Resolution`].
///
/// [`Resolution::Auto`] maps to `AVCaptureSessionPresetPhoto` — the
/// still-capture preset (full sensor resolution), not `…PresetHigh`, because
/// this session's only v1 output is [`AVCapturePhotoOutput`]. An
/// [`Resolution::Explicit`] request picks the smallest stock preset that
/// covers both requested dimensions, falling back to the largest one when the
/// request exceeds them all; the platform may still deliver something else,
/// exactly as [`Resolution`]'s own doc warns.
fn session_preset(resolution: Resolution) -> &'static AVCaptureSessionPreset {
    // SAFETY: reading `extern` AVFoundation constant statics (edition-2024
    // unsafe); each is a linker-provided, non-null `&'static NSString`.
    unsafe {
        let Resolution::Explicit { width, height } = resolution else {
            return AVCaptureSessionPresetPhoto;
        };
        let (long_side, short_side) = if width >= height {
            (width, height)
        } else {
            (height, width)
        };
        // Stock presets, ascending — the only sizes AVFoundation names.
        const PRESET_SIZES: [(u32, u32); 4] = [(640, 480), (1280, 720), (1920, 1080), (3840, 2160)];
        let presets = [
            AVCaptureSessionPreset640x480,
            AVCaptureSessionPreset1280x720,
            AVCaptureSessionPreset1920x1080,
            AVCaptureSessionPreset3840x2160,
        ];
        for (index, (preset_long, preset_short)) in PRESET_SIZES.iter().enumerate() {
            if *preset_long >= long_side && *preset_short >= short_side {
                return presets[index];
            }
        }
        presets[presets.len() - 1]
    }
}

/// The default capture device for `lens`.
///
/// Wide-angle first (`AVCaptureDeviceTypeBuiltInWideAngleCamera` — the lens
/// every iPhone has, and what a "back"/"front" camera means to an app), then
/// the position-agnostic default as a fallback for a device with no
/// wide-angle camera at that position.
fn default_device(lens: Lens, media_type: &AVMediaType) -> Option<Retained<AVCaptureDevice>> {
    let position = match lens {
        Lens::Back => AVCaptureDevicePosition::Back,
        Lens::Front => AVCaptureDevicePosition::Front,
    };
    // SAFETY: reading an `extern` constant static (edition-2024 unsafe), then
    // two class-method lookups over live constants; both return `None` rather
    // than throwing when no device matches.
    unsafe {
        let wide_angle: &AVCaptureDeviceType = AVCaptureDeviceTypeBuiltInWideAngleCamera;
        AVCaptureDevice::defaultDeviceWithDeviceType_mediaType_position(
            wide_angle,
            Some(media_type),
            position,
        )
        .or_else(|| AVCaptureDevice::defaultDeviceWithMediaType(media_type))
    }
}

/// The preview aspect ratio (width / height) for `device`'s active format, or
/// `0.0` when the format reports no usable dimensions.
///
/// The sensor's own dimensions are landscape-major; [`configure_connection`]
/// rotates the connection to portrait, so the ratio is inverted to match what
/// the preview slot actually displays — an app sizes its `platform_view` from
/// this value (the Android backend's `previewAspectRatio` contract, kept
/// symmetric).
fn aspect_ratio_of(device: &AVCaptureDevice) -> f32 {
    // SAFETY: `activeFormat`/`formatDescription` are read-only property reads
    // on a live device; `CMVideoFormatDescriptionGetDimensions` is a plain C
    // getter over the returned, still-retained description.
    let dimensions = unsafe {
        let format = device.activeFormat();
        let description = format.formatDescription();
        CMVideoFormatDescriptionGetDimensions(&description)
    };
    if dimensions.width <= 0 || dimensions.height <= 0 {
        return 0.0;
    }
    // Portrait: the short sensor edge becomes the displayed width.
    dimensions.height as f32 / dimensions.width as f32
}

/// Refuse a blocking camera operation attempted on the main run loop.
///
/// The Apple half of the crate-wide main-thread rule (the module doc's
/// *Threading*; the Android half is the `Looper.myLooper() ==
/// Looper.getMainLooper()` check in the `android` backend). Both halves exist
/// for the same reason: an operation that parks its caller for seconds turns
/// into a frozen UI — or, on Android, an ANR — when that caller is the thread
/// the platform runs its frame loop on. Failing immediately makes the
/// misuse diagnosable at the call site instead of surfacing as a timeout
/// long after the fact.
///
/// `MainThreadMarker::new()` is `pthread_main_np()` underneath — a plain
/// thread-identity check, no message send and no allocation, so it is cheap
/// enough to sit in front of every blocking entry point.
///
/// The error is a [`CameraError::Platform`] rather than a variant of its own:
/// `plugins/camera/src/lib.rs` (the enum's home) is another task's file this
/// round, and the message below is the distinguishing payload. If the enum
/// later grows a dedicated main-thread variant, this function is the single
/// place the Apple side changes.
fn reject_on_main_thread(operation: &str) -> Result<(), CameraError> {
    if MainThreadMarker::new().is_none() {
        return Ok(());
    }
    // Same typed error as the Android half's `ensure_off_ui_thread`: the two
    // backends must report one main-thread refusal, not two shapes of it.
    // (f2 wrote `Platform` because `CameraError` lived in f1's file that round;
    // re-pointed by the conductor once f1's `UiThread` variant landed.)
    // `UiThread` carries the crate-wide message, so the operation name is not
    // part of the error; the parameter is kept because it documents each call
    // site and would be the payload if this ever needs a per-operation variant.
    let _ = operation;
    Err(CameraError::UiThread)
}

/// `AVMediaTypeVideo`, or a typed error if the framework constant is missing
/// (it never is on a real Apple target — this is the `Option` the binding
/// hands back, not a runtime condition worth a panic near FFI).
fn video_media_type() -> Result<&'static AVMediaType, CameraError> {
    // SAFETY: reading an `extern` AVFoundation constant static (edition-2024
    // unsafe); linker-provided and non-null wherever AVFoundation is linked.
    unsafe { AVMediaTypeVideo }.ok_or_else(|| {
        CameraError::Platform("apple camera backend: AVMediaTypeVideo is unavailable".to_string())
    })
}

/// Map an `AVCaptureDeviceInput` creation failure onto a [`CameraError`].
///
/// `AVErrorDeviceAlreadyUsedByAnotherSession` is the one code with a
/// dedicated variant ([`CameraError::InUse`]) — the distinction the crate doc
/// calls out as worth matching on (retry vs. prompt);
/// `AVErrorApplicationIsNotAuthorizedToUseDevice` maps to
/// [`CameraError::PermissionDenied`] for the same reason.
fn device_input_error(error: &NSError) -> CameraError {
    let code = error.code();
    if code == AVError::DeviceAlreadyUsedByAnotherSession.0 {
        return CameraError::InUse;
    }
    if code == AVError::ApplicationIsNotAuthorizedToUseDevice.0 {
        return CameraError::PermissionDenied;
    }
    CameraError::Platform(format!(
        "apple camera backend: could not open the capture device ({})",
        describe(error)
    ))
}

/// An `NSError` rendered for a [`CameraError::Platform`] message.
fn describe(error: &NSError) -> String {
    format!("{}, code {}", error.localizedDescription(), error.code())
}

#[cfg(test)]
mod tests {
    //! These are **compile-time** checks, not behavioral ones: every test
    //! below is exercised by `cargo check/clippy --target
    //! aarch64-apple-ios-sim --all-targets` on any host, while *running* them
    //! needs an Apple host (and, for anything touching a camera, a physical
    //! device — the Simulator has none). Behavior is device-gated by design
    //! (task 14).

    /// [`crate::ios_exports`] expands and type-checks.
    ///
    /// The macro is invoked here in the defining crate, where the symbol it
    /// references is also defined — so this pins the expansion's syntax and
    /// its `unsafe extern "C"` signature against the real
    /// [`super::frust_camera_session_handle`], which is exactly the drift a
    /// consuming app would only discover at link time on a mac.
    #[test]
    fn ios_exports_macro_expands() {
        crate::ios_exports!();
    }

    /// The main-thread guard lets a worker thread through.
    ///
    /// Only the permissive direction is assertable here: the test harness
    /// already runs each test on a spawned thread, and nothing in a `cargo
    /// test` process can *become* the main thread to check the rejecting one
    /// (that half is the device gate's, task 14). This still pins the guard's
    /// signature and its "off the main run loop is always fine" contract.
    #[test]
    fn main_thread_guard_admits_a_worker_thread() {
        std::thread::spawn(|| {
            assert!(super::reject_on_main_thread("take_picture").is_ok());
        })
        .join()
        .expect("worker thread panicked");
    }

    /// The preview `viewType` is the bare iOS runtime name — never Android's
    /// fully-qualified form (`docs/CODE_STANDARDS.md`'s factory `viewType`
    /// LAW; the platform-views W5 finding this crate's doc restates).
    #[test]
    fn preview_view_type_is_the_bare_objc_name() {
        assert_eq!(super::PREVIEW_VIEW_TYPE, "CameraPreviewFactory");
        assert!(!super::PREVIEW_VIEW_TYPE.contains('.'));
    }
}
