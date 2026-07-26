//! The Android backend — CameraX, driven from a `dev.frust.camera.FrustCameraHost`
//! Kotlin helper (`plugins/camera/platform/android/`) over this crate's own
//! JNI surface. **Task 06** drove the real Rust→Kotlin calls and filled the
//! `nativeOn*` export bodies against the frozen contract task 02 fixed;
//! **task 09** (this module's current state) added the image stream —
//! [`AndroidSession::start_image_stream`]/[`AndroidSession::stop_image_stream`],
//! the host's `startImageStream`/`stopImageStream`, and the one contract
//! addition task 02 reserved,
//! [`Java_dev_frust_camera_FrustCameraHost_nativeOnImageFrame`]. Every
//! operation this backend exposes is now real.
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
//! | `startImageStream` | `(int session, int format) -> int` | `0` started (frames arrive via [`Java_dev_frust_camera_FrustCameraHost_nativeOnImageFrame`]); <0 refused. `format`: [`FORMAT_CODE_YUV420`] / [`FORMAT_CODE_BGRA`] (Android refuses the latter — see [`stream_format_code`]) |
//! | `stopImageStream` | `(int session) -> void` | Unbinds the `ImageAnalysis` use case only; the preview keeps running |
//!
//! ## Kotlin → Rust (this crate's own `#[unsafe(no_mangle)]` JNI exports —
//! package baked into the symbol names, so `dev.frust.camera.FrustCameraHost`
//! may never move once shipped)
//!
//! - [`Java_dev_frust_camera_FrustCameraHost_nativeOnPermissionResult`]`(env, class, granted: jboolean)`
//! - [`Java_dev_frust_camera_FrustCameraHost_nativeOnCameraState`]`(env, class, session: jint, state: jint)` — `0` Configuring / `1` Running / `2` Closed / `3` Error
//! - [`Java_dev_frust_camera_FrustCameraHost_nativeOnPictureTaken`]`(env, class, session: jint, ok: jboolean, path: JString)`
//! - [`Java_dev_frust_camera_FrustCameraHost_nativeOnImageFrame`]`(env, class, session: jint, format: jint, width: jint, height: jint, rotationDegrees: jint, planeCount: jint, plane0: JByteBuffer, rowStride0: jint, pixelStride0: jint, plane1: …, plane2: …)` — **the task-09 contract addition.** Three fixed plane slots (never an array: an array would allocate on the Kotlin side once per frame at camera rate); slots past `planeCount` are null. Called on the host's own analyzer executor, and `ImageProxy.close()` runs only after it returns — see [`AndroidSession::start_image_stream`]'s close-deadline contract.
//!
//! Each export upgrades its [`jni::EnvUnowned`] via
//! [`jni::EnvUnowned::with_env`], which wraps the body in `catch_unwind` — the
//! crate's own no-unwind-across-FFI guarantee (`docs/CODE_STANDARDS.md`'s
//! Language Idioms), matching the pattern `frust-shell-android`'s own JNI
//! exports use. The **one** `unsafe` block in this module is the image
//! stream's zero-copy plane view ([`plane_data`]) — the sanctioned
//! `GetDirectBufferAddress` pattern, `# Safety`-noted there; every other
//! `unsafe` token is an export's `#[unsafe(no_mangle)]` attribute.
//!
//! # Which calls block, and on what
//!
//! Three of the contract's calls are answered asynchronously by a `nativeOn*`
//! callback. This backend resolves that split **once**, here, so an app never
//! has to:
//!
//! | Call | Blocking? |
//! |---|---|
//! | [`request_permission`] | **Blocks** on [`PERMISSION_TIMEOUT`] when the host reports `3` (dialog shown), waking on `nativeOnPermissionResult`; a timeout reports [`PermissionStatus::Denied`]. `0`/`1`/`2` return immediately. |
//! | [`AndroidSession::open`] | **Never blocks.** `openCamera` returns the session id and this returns immediately with a live [`AndroidSession`]; CameraX configuration continues on the host's main thread and lands via `nativeOnCameraState`. A caller reads [`AndroidSession::preview_aspect_ratio`] (`0.0` until the first `TransformationInfo`) to learn when geometry is ready — this is the same contract [`crate::Camera::open`]'s doc already states, and the one task 11's catalog page consumes. |
//! | [`AndroidSession::take_picture`] | **Blocks** on [`PICTURE_TIMEOUT`] until `nativeOnPictureTaken` answers the capture it started (`spawn_blocking`-paired by convention — the crate doc's *Blocking API*). |
//!
//! Every blocking wait happens **outside** the scoped JNI attachment
//! ([`frust_plugin::android::with_jni_env`] returns before the wait starts),
//! so a blocked caller never pins a JVM attachment and the callback thread is
//! free to deliver its answer.
//!
//! # Session state
//!
//! [`SESSIONS`] is the process-wide `session id -> `[`SessionState`] map the
//! exports write and the blocking waits read, woken by [`SESSIONS_UPDATED`].
//! Entries are created by [`AndroidSession::open`] and dropped by
//! [`AndroidSession::close`] or a `Closed` state callback — an
//! [`AndroidSession`] itself holds only the integer id, so it stays trivially
//! `Send + Sync` and holds no JNI reference across calls (the
//! `frust-secure-storage` per-call-attachment shape).
//!
//! A running image stream's callback lives in a **second**, deliberately
//! separate map ([`STREAMS`]): `nativeOnImageFrame` holds the callback's lock
//! for the whole user callback, and routing that through [`SESSIONS`] would
//! block `preview_aspect_ratio`/`take_picture` bookkeeping behind every frame.
//!
//! # Fail-soft, never a panic
//!
//! Every entry point routes through [`with_host`], which checks
//! `frust-plugin`'s ready flag first: before the host shell installs the
//! `(JavaVM, Context)` handles (an old scaffold predating
//! `nativeInitPlatform`) every call reports
//! [`CameraError::PlatformNotInitialized`]. Any JNI failure — a missing
//! `FrustCameraHost` class (the plugin's Gradle module isn't wired in), a
//! thrown exception, a null return — maps to [`CameraError::Platform`] with
//! the failing operation named, never a panic across the FFI boundary.
//!
//! # What the contract does *not* carry
//!
//! `openCamera` takes a lens facing and nothing else, so
//! [`crate::Resolution`] is **not** forwarded to CameraX on Android v1: the
//! host uses CameraX's own un-configured resolution strategy and an app reads
//! the delivered geometry back via [`AndroidSession::preview_aspect_ratio`],
//! exactly as [`crate::Resolution::Explicit`]'s "best-effort, always read it
//! back" doc allows. Widening the contract with a resolution parameter means
//! updating tasks 02/05/06 together, not improvising here.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use jni::errors::LogErrorAndDefault;
use jni::objects::{JByteBuffer, JClass, JObject, JString, JValue};
use jni::refs::Global;
use jni::sys::{jboolean, jint};
use jni::{Env, EnvUnowned, jni_sig, jni_str};

use crate::{
    CameraError, ImageFormat, ImageFrame, ImageFrameCallback, ImagePlane, Lens, PermissionStatus,
    Resolution, SessionBackend,
};

/// The `viewType` this crate's Android preview slot resolves to — the
/// fully-qualified class name the embedding module's `FrustViewHost` looks
/// up via the app classloader (`docs/CODE_STANDARDS.md`'s platform-view
/// factory `viewType` LAW). Shipped by `plugins/camera/platform/android/`'s
/// `CameraPreviewFactory.kt` (task 05).
const PREVIEW_VIEW_TYPE: &str = "dev.frust.camera.CameraPreviewFactory";

/// The Kotlin host's fully-qualified class name in **binary/dotted** form, as
/// `ClassLoader.loadClass` expects (**not** the slash form `FindClass` wants)
/// — the module doc's frozen contract, and the package baked into every JNI
/// export symbol below. Looked up through the application classloader (see
/// [`load_host_class`]).
const HOST_CLASS_BINARY: &str = "dev.frust.camera.FrustCameraHost";

/// `CameraSelector.LENS_FACING_FRONT` — a published `androidx.camera.core`
/// constant (see [`Lens`]'s doc), passed straight to `openCamera`.
const LENS_FACING_FRONT: i32 = 0;
/// `CameraSelector.LENS_FACING_BACK` — a published `androidx.camera.core`
/// constant (see [`Lens`]'s doc).
const LENS_FACING_BACK: i32 = 1;

/// `FrustCameraHost.requestPermission()`'s "already granted" return code.
const PERMISSION_CODE_GRANTED: i32 = 0;
/// `requestPermission()`'s "denied" return code.
const PERMISSION_CODE_DENIED: i32 = 1;
/// `requestPermission()`'s "no Activity cached yet" return code.
const PERMISSION_CODE_NEEDS_UI: i32 = 2;
/// `requestPermission()`'s "system dialog shown" return code — the answer
/// arrives via [`Java_dev_frust_camera_FrustCameraHost_nativeOnPermissionResult`].
const PERMISSION_CODE_PENDING: i32 = 3;

/// `nativeOnCameraState`'s `state` codes (module doc's contract table).
const STATE_CODE_CONFIGURING: i32 = 0;
/// See [`STATE_CODE_CONFIGURING`].
const STATE_CODE_RUNNING: i32 = 1;
/// See [`STATE_CODE_CONFIGURING`].
const STATE_CODE_CLOSED: i32 = 2;
/// See [`STATE_CODE_CONFIGURING`].
const STATE_CODE_ERROR: i32 = 3;

/// `FrustCameraHost.takePicture`'s "capture started" return code; anything
/// else is a rejected request.
const TAKE_PICTURE_STARTED: i32 = 0;

/// `FrustCameraHost.startImageStream`'s "stream started" return code; anything
/// else is a rejected request (the host's `ERROR_UNKNOWN_SESSION` /
/// `ERROR_UNSUPPORTED_FORMAT` / `ERROR_NOT_CONFIGURED`).
const START_IMAGE_STREAM_STARTED: i32 = 0;

/// `startImageStream`'s `format` code for [`ImageFormat::Yuv420`] — CameraX's
/// `ImageAnalysis.OUTPUT_IMAGE_FORMAT_YUV_420_888` (contract table above).
const FORMAT_CODE_YUV420: i32 = 0;

/// `startImageStream`'s `format` code for [`ImageFormat::Bgra`].
///
/// Reserved in the contract for symmetry with the Apple backend and
/// **refused** by this one — see [`stream_format_code`] for why CameraX
/// cannot deliver it.
const FORMAT_CODE_BGRA: i32 = 1;

/// The number of plane slots the `nativeOnImageFrame` contract carries — 3,
/// which covers `ImageFormat.YUV_420_888`'s Y/U/V and leaves room for a
/// single-plane packed format. Slots past the reported `planeCount` are null.
const MAX_PLANES: usize = 3;

/// How long [`request_permission`] waits for `nativeOnPermissionResult` after
/// the host reports [`PERMISSION_CODE_PENDING`], before reporting
/// [`PermissionStatus::Denied`].
///
/// **Not a platform value** — the system permission dialog has no published
/// deadline (a user may leave it on screen indefinitely). Two minutes is long
/// enough that a real answer is never cut short, short enough that a caller
/// paired with `frust_reactive::spawn_blocking` cannot pin a blocking-pool
/// thread forever if the host never relays a result (e.g. the Activity was
/// destroyed mid-dialog). Timing out reports `Denied` rather than an error:
/// "we did not obtain permission" is exactly what the caller must act on, and
/// re-requesting is always valid.
const PERMISSION_TIMEOUT: Duration = Duration::from_secs(120);

/// How long [`AndroidSession::take_picture`] waits for `nativeOnPictureTaken`
/// before reporting [`CameraError::Platform`].
///
/// **Not a platform value** — `ImageCapture.takePicture`'s own completion has
/// no published bound. Fifteen seconds comfortably covers a slow
/// low-light/HDR capture plus the file write while still failing a caller
/// whose completion callback never arrives.
const PICTURE_TIMEOUT: Duration = Duration::from_secs(15);

/// The cached `dev.frust.camera.FrustCameraHost` class reference.
///
/// **One** global reference for the whole process (plus the method ids ART
/// caches behind `call_static_method`), never a per-call one: ART's global-ref
/// table is a hard-capped budget (`JNI ERROR: global reference table
/// overflow`), and a per-call `Global` would burn it on the image-stream path.
/// A local `JClass` cannot be cached instead — locals die with their JNI
/// frame.
static HOST_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

/// The process-wide permission-request slot: `nativeOnPermissionResult`
/// carries no request id, so the answer is matched to the caller by
/// [`PermissionSlot::generation`] (bumped by every request), which keeps a
/// late answer to an abandoned request from resolving the next one.
struct PermissionSlot {
    /// Bumped by each [`request_permission`] call that arms a wait.
    generation: u64,
    /// The answer `nativeOnPermissionResult` delivered for [`Self::generation`].
    result: Option<bool>,
}

impl PermissionSlot {
    /// The pre-request state: nothing armed, no answer.
    const fn new() -> Self {
        Self {
            generation: 0,
            result: None,
        }
    }
}

/// See [`PermissionSlot`].
static PERMISSION: Mutex<PermissionSlot> = Mutex::new(PermissionSlot::new());
/// Woken by `nativeOnPermissionResult`; waited on by [`request_permission`].
static PERMISSION_UPDATED: Condvar = Condvar::new();

/// A session's lifecycle phase, as `nativeOnCameraState` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SessionPhase {
    /// CameraX is still binding the use cases — the session's initial state.
    #[default]
    Configuring,
    /// The camera is live.
    Running,
    /// The host reported a failure; every blocking wait on this session
    /// aborts with [`CameraError::Platform`].
    Error,
}

/// One live session's callback-delivered state (see [`SESSIONS`]).
#[derive(Debug, Default)]
struct SessionState {
    /// The latest phase `nativeOnCameraState` reported. A `Closed` state is
    /// not a phase: it removes the entry outright (see the module doc).
    phase: SessionPhase,
    /// Bumped by each [`AndroidSession::take_picture`] call, so a completion
    /// is matched to the caller that started it rather than to a previous,
    /// timed-out one.
    capture_generation: u64,
    /// `(generation, ok)` as `nativeOnPictureTaken` delivered it.
    capture_result: Option<(u64, bool)>,
}

/// Every open session's callback state, keyed by the host's session id.
///
/// `HashMap::new` is not `const`, so this is a [`LazyLock`] rather than a bare
/// `static Mutex<HashMap<..>>`. Entries are inserted by
/// [`AndroidSession::open`] and removed by [`AndroidSession::close`] or a
/// `Closed` state callback; the `nativeOn*` exports only ever update an
/// **existing** entry, so a callback for an unknown/already-closed session is
/// logged and dropped instead of resurrecting a map entry.
static SESSIONS: LazyLock<Mutex<HashMap<i32, SessionState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// Woken by `nativeOnCameraState`/`nativeOnPictureTaken` and by
/// [`AndroidSession::close`]; waited on by [`await_capture`].
static SESSIONS_UPDATED: Condvar = Condvar::new();

/// One running image stream's user callback.
///
/// The [`Mutex`] is **not** contention control — delivery is serialized by the
/// host's single-threaded analyzer executor already. It is what makes the slot
/// `Sync`, so an `Arc` of it can live in the [`STREAMS`] static: a bare
/// `Box<dyn Fn(..) + Send>` is `Send` but not `Sync`, and `Arc<T>` is only
/// `Send` for `T: Send + Sync`.
struct StreamSlot {
    /// Invoked once per delivered frame by
    /// [`Java_dev_frust_camera_FrustCameraHost_nativeOnImageFrame`].
    on_frame: Mutex<Box<ImageFrameCallback>>,
}

/// Every running image stream's callback, keyed by session id (module doc's
/// *Session state*).
///
/// [`Java_dev_frust_camera_FrustCameraHost_nativeOnImageFrame`] clones the
/// `Arc` under a **short** lock and releases the map before invoking the
/// callback, so a stream that stops itself from inside its own callback
/// (removing the entry) cannot deadlock — the in-flight frame simply finishes
/// against the `Arc` it already holds, and the callback drops afterwards.
static STREAMS: LazyLock<Mutex<HashMap<i32, Arc<StreamSlot>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Lock a module-global mutex, recovering from poisoning instead of
/// panicking: a panic caught by an export's `catch_unwind` must not turn every
/// later camera call into a panic near the FFI boundary
/// (`docs/CODE_STANDARDS.md`'s no-unwind rule). The guarded data is plain
/// bookkeeping, so a poisoned view is still coherent enough to use.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// --- JNI plumbing -----------------------------------------------------------

/// Run `f` with a live [`Env`] and the cached `FrustCameraHost` class inside a
/// scoped JNI attachment, flattening the two error layers: a missing platform
/// handle → [`CameraError::PlatformNotInitialized`] (the ready flag is checked
/// *first*, before any JNI work), a JVM attach failure →
/// [`CameraError::Platform`]. `f` itself already returns a [`CameraError`].
fn with_host<T>(
    f: impl FnOnce(&mut Env<'_>, &Global<JClass<'static>>) -> Result<T, CameraError>,
) -> Result<T, CameraError> {
    let attached = frust_plugin::android::with_jni_env(|env, context| {
        let class = host_class(env, context)?;
        f(env, class)
    });
    match attached {
        Ok(inner) => inner,
        Err(frust_plugin::PlatformHandleError::NotInitialized) => {
            Err(CameraError::PlatformNotInitialized)
        }
        Err(other) => Err(CameraError::Platform(format!(
            "android camera backend: platform handle error: {other}"
        ))),
    }
}

/// The cached [`HOST_CLASS`], loading it on first use. A racing loser's
/// reference is dropped immediately (`Global`'s own `Drop` releases it), so at
/// most one global ref survives.
fn host_class(
    env: &mut Env<'_>,
    context: &JObject,
) -> Result<&'static Global<JClass<'static>>, CameraError> {
    if let Some(class) = HOST_CLASS.get() {
        return Ok(class);
    }
    let class = load_host_class(env, context)?;
    Ok(HOST_CLASS.get_or_init(|| class))
}

/// `context.getClassLoader().loadClass("dev.frust.camera.FrustCameraHost")`,
/// promoted to a process-lifetime global reference.
///
/// The application `Context`'s classloader is the only loader that can see
/// app-defined classes — a bare `FindClass` on a JNI worker thread sees the
/// bootstrap loader only, which is exactly why this explicit path exists (the
/// `FrustBiometric` mechanism, `plugins/secure-storage/src/android.rs`). A
/// `ClassNotFoundException` here means the plugin's Android Gradle module
/// isn't wired into the app.
fn load_host_class(
    env: &mut Env<'_>,
    context: &JObject,
) -> Result<Global<JClass<'static>>, CameraError> {
    run_jni(
        env,
        "loading dev.frust.camera.FrustCameraHost (is the plugin's Android Gradle module wired \
         into the app?)",
        |env| {
            let loader = env
                .call_method(
                    context,
                    jni_str!("getClassLoader"),
                    jni_sig!("()Ljava/lang/ClassLoader;"),
                    &[],
                )?
                .l()?;
            let name = env.new_string(HOST_CLASS_BINARY)?;
            let class = env
                .call_method(
                    &loader,
                    jni_str!("loadClass"),
                    jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
                    &[JValue::Object(&name)],
                )?
                .l()?;
            let class = env.cast_local::<JClass>(class)?;
            env.new_global_ref(class)
        },
    )
}

/// Run a sequence of JNI calls, converting any pending Java exception into a
/// typed [`CameraError::Platform`] naming `op`.
///
/// `jni` 0.22 returns `Err(Error::JavaException)` and leaves the exception
/// **pending** — undefined behaviour for the next JNI call — so we always
/// check/clear it here before returning, whatever `f` reported (the
/// `frust-secure-storage` `run_jni` shape). The camera contract has no
/// exception taxonomy to distinguish (unlike Keystore's
/// `KeyPermanentlyInvalidatedException`), so every throw maps to one variant
/// with the class and message preserved in the string.
fn run_jni<'local, T>(
    env: &mut Env<'local>,
    op: &str,
    f: impl FnOnce(&mut Env<'local>) -> Result<T, jni::errors::Error>,
) -> Result<T, CameraError> {
    let result = f(env);
    if env.exception_check() {
        return Err(take_pending_exception(env, op));
    }
    result.map_err(|e| CameraError::Platform(format!("android camera backend: {op}: {e}")))
}

/// Extract, **clear**, and describe the pending Java exception. Clears first
/// (mirroring `jni`'s own `exception_catch`) so the subsequent class/message
/// queries run without a pending exception; a defensive final clear covers the
/// unlikely case one of those queries itself throws.
fn take_pending_exception(env: &mut Env<'_>, op: &str) -> CameraError {
    let Some(throwable) = env.exception_occurred() else {
        env.exception_clear();
        return CameraError::Platform(format!(
            "android camera backend: {op}: JNI reported an exception with no throwable"
        ));
    };
    env.exception_clear();

    let class_name = match env.get_object_class(&throwable) {
        Ok(class) => match class.get_name(env) {
            Ok(name) => name.to_string(),
            Err(_) => "<unknown exception class>".to_string(),
        },
        Err(_) => "<unknown exception class>".to_string(),
    };
    let message = match throwable.get_message(env) {
        Ok(msg) => msg.to_string(),
        Err(_) => "<no message>".to_string(),
    };

    // Defensive: don't leave a second exception pending for the next JNI call.
    if env.exception_check() {
        env.exception_clear();
    }

    CameraError::Platform(format!(
        "android camera backend: {op}: {class_name}: {message}"
    ))
}

// --- Permission -------------------------------------------------------------

/// [`crate::Camera::request_permission`]'s Android arm.
///
/// Calls `FrustCameraHost.requestPermission()` (module doc's contract table).
/// A [`PERMISSION_CODE_PENDING`] answer means the system dialog is on screen,
/// so this **blocks** for up to [`PERMISSION_TIMEOUT`] on
/// `nativeOnPermissionResult` — pair with `frust_reactive::spawn_blocking`,
/// never call it on the UI thread (the crate doc's *Blocking API*).
///
/// # Errors
/// [`CameraError::PlatformNotInitialized`] before the host shell installs the
/// `(JavaVM, Context)` handles; [`CameraError::Platform`] on any JNI failure
/// or an unrecognised return code.
pub(crate) fn request_permission() -> Result<PermissionStatus, CameraError> {
    // Arm the wait *before* the JNI call so a result relayed while
    // `requestPermission` is still returning cannot be missed.
    let generation = {
        let mut slot = lock(&PERMISSION);
        slot.generation = slot.generation.wrapping_add(1);
        slot.result = None;
        slot.generation
    };

    let code = with_host(|env, class| {
        run_jni(env, "FrustCameraHost.requestPermission", |env| {
            env.call_static_method(class, jni_str!("requestPermission"), jni_sig!("()I"), &[])?
                .i()
        })
    })?;

    match code {
        PERMISSION_CODE_GRANTED => Ok(PermissionStatus::Granted),
        PERMISSION_CODE_DENIED => Ok(PermissionStatus::Denied),
        PERMISSION_CODE_NEEDS_UI => Ok(PermissionStatus::NeedsUi),
        // The dialog is showing — block outside the JNI attachment (which
        // `with_host` already dropped) until the relay answers.
        PERMISSION_CODE_PENDING => Ok(await_permission(generation)),
        other => Err(CameraError::Platform(format!(
            "android camera backend: FrustCameraHost.requestPermission returned an unknown code \
             {other}"
        ))),
    }
}

/// Block until `nativeOnPermissionResult` answers request `generation`, or
/// [`PERMISSION_TIMEOUT`] elapses (→ [`PermissionStatus::Denied`], see that
/// constant's doc).
///
/// A concurrent [`request_permission`] that bumps the generation ends this
/// wait too: the other caller now owns the dialog's answer, so this one
/// reports [`PermissionStatus::Pending`] — the "ask again / read it from the
/// other call" status, not a fabricated grant or denial.
fn await_permission(generation: u64) -> PermissionStatus {
    let slot = lock(&PERMISSION);
    let (slot, _timeout) = PERMISSION_UPDATED
        .wait_timeout_while(slot, PERMISSION_TIMEOUT, |slot| {
            slot.generation == generation && slot.result.is_none()
        })
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    if slot.generation != generation {
        log::debug!(
            "frust-camera: permission request superseded by a newer one — reporting Pending"
        );
        return PermissionStatus::Pending;
    }

    match slot.result {
        Some(true) => PermissionStatus::Granted,
        Some(false) => PermissionStatus::Denied,
        None => {
            log::warn!(
                "frust-camera: permission dialog did not report a result within {}s — reporting \
                 Denied",
                PERMISSION_TIMEOUT.as_secs()
            );
            PermissionStatus::Denied
        }
    }
}

// --- Session ----------------------------------------------------------------

/// The Android [`SessionBackend`] — one CameraX session owned by
/// `FrustCameraHost.kt`, referenced here by its integer session id.
///
/// Holds no JNI reference (each operation re-attaches through [`with_host`]),
/// so it is trivially `Send + Sync`; the callback-delivered state lives in
/// [`SESSIONS`] under [`Self::session_id`].
pub(crate) struct AndroidSession {
    /// The id `FrustCameraHost.openCamera` returned.
    session_id: i32,
    /// Set by the first [`Self::close`] so a second close (and the [`Drop`]
    /// safety net) is a no-op and later operations report
    /// [`CameraError::SessionClosed`].
    closed: AtomicBool,
}

impl AndroidSession {
    /// [`crate::Camera::open`]'s Android arm — `FrustCameraHost.openCamera`.
    ///
    /// **Returns as soon as the host hands back a session id**, without
    /// waiting for `nativeOnCameraState` to report past `Configuring` (the
    /// module doc's blocking table, and the contract
    /// [`crate::Camera::open`]'s doc already states): CameraX binds on the
    /// host's main thread, so blocking here would stall a caller for a whole
    /// bind cycle to learn something [`Self::preview_aspect_ratio`] reports
    /// anyway. A caller sizes its preview slot off that ratio once it turns
    /// non-zero.
    ///
    /// `resolution` is accepted and **not forwarded** — the frozen
    /// `openCamera(int lensFacing)` contract carries no resolution parameter
    /// (module doc's *What the contract does not carry*).
    ///
    /// # Errors
    /// [`CameraError::PlatformNotInitialized`] before the host shell installs
    /// the platform handles; [`CameraError::Platform`] if `openCamera`
    /// reports a negative error code or the JNI call fails.
    pub(crate) fn open(lens: Lens, resolution: Resolution) -> Result<Self, CameraError> {
        let _ = resolution;
        let lens_facing = match lens {
            Lens::Back => LENS_FACING_BACK,
            Lens::Front => LENS_FACING_FRONT,
        };

        let session_id = with_host(|env, class| {
            run_jni(env, "FrustCameraHost.openCamera", |env| {
                env.call_static_method(
                    class,
                    jni_str!("openCamera"),
                    jni_sig!("(I)I"),
                    &[JValue::Int(lens_facing)],
                )?
                .i()
            })
        })?;

        if session_id < 0 {
            // The contract fixes only the sign (≥0 id / <0 error); it defines
            // no finer code taxonomy, so the code travels in the message
            // rather than being mapped to an invented variant. A post-open
            // failure arrives instead as `nativeOnCameraState`'s `Error`.
            return Err(CameraError::Platform(format!(
                "android camera backend: FrustCameraHost.openCamera failed (code {session_id})"
            )));
        }

        lock(&SESSIONS).insert(session_id, SessionState::default());

        Ok(Self {
            session_id,
            closed: AtomicBool::new(false),
        })
    }

    /// `Ok(())` while this session is usable, [`CameraError::SessionClosed`]
    /// once [`Self::close`] ran (or a `Closed` state callback dropped the
    /// entry).
    fn ensure_open(&self) -> Result<(), CameraError> {
        if self.closed.load(Ordering::Acquire) || !lock(&SESSIONS).contains_key(&self.session_id) {
            return Err(CameraError::SessionClosed);
        }
        Ok(())
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
        // the contract table's own documented initial value. A closed session
        // or a JNI failure reports the same `0.0` rather than an error: this
        // accessor is infallible by API shape, so a caller sizing a preview
        // slot simply keeps waiting.
        if self.ensure_open().is_err() {
            return 0.0;
        }
        let ratio = with_host(|env, class| {
            run_jni(env, "FrustCameraHost.previewAspectRatio", |env| {
                env.call_static_method(
                    class,
                    jni_str!("previewAspectRatio"),
                    jni_sig!("(I)F"),
                    &[JValue::Int(self.session_id)],
                )?
                .f()
            })
        });
        match ratio {
            Ok(ratio) => ratio,
            Err(err) => {
                log::warn!("frust-camera: preview_aspect_ratio failed: {err}");
                0.0
            }
        }
    }

    fn take_picture(&self, path: &Path) -> Result<(), CameraError> {
        self.ensure_open()?;

        let path_str = path.to_str().ok_or_else(|| {
            CameraError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "camera capture path is not valid UTF-8 (it must cross JNI as a Java \
                     String): {}",
                    path.display()
                ),
            ))
        })?;

        // Arm the completion slot *before* the JNI call so a capture that
        // completes while `takePicture` is still returning cannot be missed.
        let generation = {
            let mut sessions = lock(&SESSIONS);
            let Some(state) = sessions.get_mut(&self.session_id) else {
                return Err(CameraError::SessionClosed);
            };
            state.capture_generation = state.capture_generation.wrapping_add(1);
            state.capture_result = None;
            state.capture_generation
        };

        let code = with_host(|env, class| {
            run_jni(env, "FrustCameraHost.takePicture", |env| {
                let path_jstr = env.new_string(path_str)?;
                env.call_static_method(
                    class,
                    jni_str!("takePicture"),
                    jni_sig!("(ILjava/lang/String;)I"),
                    &[JValue::Int(self.session_id), JValue::Object(&path_jstr)],
                )?
                .i()
            })
        })?;

        if code != TAKE_PICTURE_STARTED {
            return Err(CameraError::Platform(format!(
                "android camera backend: FrustCameraHost.takePicture refused the request (code \
                 {code})"
            )));
        }

        // Block outside the JNI attachment (`with_host` already dropped it)
        // so the callback thread can attach and deliver the completion.
        await_capture(self.session_id, generation)
    }

    /// `FrustCameraHost.startImageStream` — binds an `ImageAnalysis` use case
    /// beside the already-bound preview/capture pair.
    ///
    /// # Close-deadline contract (the failure mode this backend can produce)
    ///
    /// The host analyzer calls `nativeOnImageFrame` **synchronously** and only
    /// then `ImageProxy.close()`, so `on_frame`'s return *is* the close
    /// deadline. With `STRATEGY_KEEP_ONLY_LATEST` exactly one image may be in
    /// flight, so a callback that blocks (or squirrels the borrowed planes
    /// away and returns late) **stalls every subsequent frame** — the stream
    /// goes quiet rather than dropping frames. Copy or consume before
    /// returning; see [`crate::ImageFrame`]'s doc, which states the same
    /// contract on the public API.
    ///
    /// Never blocks: the bind itself happens on the host's main thread and
    /// frames start arriving once it lands.
    ///
    /// # Errors
    /// [`CameraError::SessionClosed`] after [`Self::close`];
    /// [`CameraError::Platform`] for [`ImageFormat::Bgra`] (see
    /// [`stream_format_code`]), a host refusal, or a JNI failure.
    fn start_image_stream(
        &self,
        format: ImageFormat,
        on_frame: Box<ImageFrameCallback>,
    ) -> Result<(), CameraError> {
        self.ensure_open()?;
        let format_code = stream_format_code(format)?;

        // Register *before* the JNI call: the analyzer can deliver its first
        // frame while `startImageStream` is still returning, and a frame with
        // no registered slot is dropped.
        lock(&STREAMS).insert(
            self.session_id,
            Arc::new(StreamSlot {
                on_frame: Mutex::new(on_frame),
            }),
        );

        let code = with_host(|env, class| {
            run_jni(env, "FrustCameraHost.startImageStream", |env| {
                env.call_static_method(
                    class,
                    jni_str!("startImageStream"),
                    jni_sig!("(II)I"),
                    &[JValue::Int(self.session_id), JValue::Int(format_code)],
                )?
                .i()
            })
        });

        match code {
            Ok(START_IMAGE_STREAM_STARTED) => Ok(()),
            Ok(other) => {
                lock(&STREAMS).remove(&self.session_id);
                Err(CameraError::Platform(format!(
                    "android camera backend: FrustCameraHost.startImageStream refused the request \
                     (code {other})"
                )))
            }
            Err(err) => {
                lock(&STREAMS).remove(&self.session_id);
                Err(err)
            }
        }
    }

    /// `FrustCameraHost.stopImageStream` — unbinds the analyzer only; the
    /// preview slot and the camera device are untouched.
    ///
    /// Dropping the [`STREAMS`] entry first means no frame delivered between
    /// here and the host's main-thread unbind reaches the callback.
    fn stop_image_stream(&self) {
        if lock(&STREAMS).remove(&self.session_id).is_none() {
            // No stream running — `CameraSession::stop_image_stream`'s
            // documented no-op, and no reason to cross JNI for it.
            return;
        }

        let stopped = with_host(|env, class| {
            run_jni(env, "FrustCameraHost.stopImageStream", |env| {
                env.call_static_method(
                    class,
                    jni_str!("stopImageStream"),
                    jni_sig!("(I)V"),
                    &[JValue::Int(self.session_id)],
                )?
                .v()
            })
        });
        if let Err(err) = stopped {
            // Infallible by API shape; our own bookkeeping is already dropped,
            // so the callback is unreachable either way.
            log::warn!(
                "frust-camera: stop_image_stream(session_id={}) failed: {err}",
                self.session_id
            );
        }
    }

    fn close(&self) {
        // First close wins; a second (including the `Drop` safety net) is a
        // no-op, per `CameraSession::close`'s documented contract.
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }

        // Drop any running stream's callback first — `closeCamera` unbinds the
        // analyzer host-side, and a frame already queued for delivery must not
        // reach a callback the app considers dead.
        lock(&STREAMS).remove(&self.session_id);

        let closed = with_host(|env, class| {
            run_jni(env, "FrustCameraHost.closeCamera", |env| {
                env.call_static_method(
                    class,
                    jni_str!("closeCamera"),
                    jni_sig!("(I)V"),
                    &[JValue::Int(self.session_id)],
                )?
                .v()
            })
        });
        if let Err(err) = closed {
            // Close is infallible by API shape; report and still drop our own
            // bookkeeping so blocked callers wake with `SessionClosed`.
            log::warn!(
                "frust-camera: close(session_id={}) failed: {err}",
                self.session_id
            );
        }

        lock(&SESSIONS).remove(&self.session_id);
        SESSIONS_UPDATED.notify_all();
    }
}

impl Drop for AndroidSession {
    /// Release the camera if the app dropped its [`crate::CameraSession`]
    /// without calling [`crate::CameraSession::close`] — a hardware resource
    /// no other owner would ever free. Explicit `close` remains the documented
    /// path; this is only the safety net (and a no-op after it).
    fn drop(&mut self) {
        SessionBackend::close(self);
    }
}

/// Block until `nativeOnPictureTaken` answers capture `generation` on
/// `session_id`, the session ends, or [`PICTURE_TIMEOUT`] elapses.
fn await_capture(session_id: i32, generation: u64) -> Result<(), CameraError> {
    let sessions = lock(&SESSIONS);
    let (sessions, _timeout) = SESSIONS_UPDATED
        .wait_timeout_while(sessions, PICTURE_TIMEOUT, |sessions| {
            !capture_settled(sessions, session_id, generation)
        })
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let Some(state) = sessions.get(&session_id) else {
        // The entry is gone: `close` ran, or the host reported `Closed`.
        return Err(CameraError::SessionClosed);
    };
    match state.capture_result {
        Some((delivered, true)) if delivered == generation => Ok(()),
        Some((delivered, false)) if delivered == generation => Err(CameraError::Platform(
            "android camera backend: take_picture failed (the host reported an unsuccessful \
             capture)"
                .to_string(),
        )),
        // No answer for *our* generation: either the session errored or we
        // hit the deadline.
        _ if state.phase == SessionPhase::Error => Err(CameraError::Platform(
            "android camera backend: take_picture aborted — the session reported an error state"
                .to_string(),
        )),
        _ => Err(CameraError::Platform(format!(
            "android camera backend: take_picture did not complete within {}s",
            PICTURE_TIMEOUT.as_secs()
        ))),
    }
}

/// Whether [`await_capture`]'s wait is over: the session ended, it reported an
/// error, or the completion for `generation` landed.
fn capture_settled(
    sessions: &HashMap<i32, SessionState>,
    session_id: i32,
    generation: u64,
) -> bool {
    match sessions.get(&session_id) {
        None => true,
        Some(state) => {
            state.phase == SessionPhase::Error
                || matches!(state.capture_result, Some((delivered, _)) if delivered == generation)
        }
    }
}

// --- Image stream -----------------------------------------------------------

/// The contract's `format` code for a requested [`ImageFormat`].
///
/// [`ImageFormat::Bgra`] is **refused here**, before any JNI call: CameraX's
/// only packed-32-bit `ImageAnalysis` output is
/// `OUTPUT_IMAGE_FORMAT_RGBA_8888`, whose bytes are `R,G,B,A` — not the BGRA
/// order [`ImageFormat::Bgra`] promises — and this backend will not silently
/// hand a caller mislabelled bytes or pay for a per-frame channel swizzle in
/// the zero-copy path. Flutter's `camera` plugin draws the same platform line
/// (its `bgra8888` group is iOS-only). Callers that need one code path on both
/// platforms request [`ImageFormat::Yuv420`], which both backends deliver.
fn stream_format_code(format: ImageFormat) -> Result<i32, CameraError> {
    match format {
        ImageFormat::Yuv420 => Ok(FORMAT_CODE_YUV420),
        ImageFormat::Bgra => Err(CameraError::Platform(
            "android camera backend: ImageFormat::Bgra is not available on Android — CameraX's \
             ImageAnalysis delivers RGBA_8888 (byte order R,G,B,A), never BGRA. Request \
             ImageFormat::Yuv420, which both backends support."
                .to_string(),
        )),
    }
}

/// The [`ImageFormat`] a contract `format` code names, or `None` for a code
/// this backend never sends (a host/Rust version skew).
fn image_format_from_code(code: i32) -> Option<ImageFormat> {
    match code {
        FORMAT_CODE_YUV420 => Some(ImageFormat::Yuv420),
        FORMAT_CODE_BGRA => Some(ImageFormat::Bgra),
        _ => None,
    }
}

/// One delivered frame's scalar header — the `nativeOnImageFrame` arguments
/// that aren't plane triples, bundled so [`deliver_image_frame`] keeps a
/// readable signature.
struct FrameHeader {
    /// The session the frame belongs to (its [`STREAMS`] key).
    session: i32,
    /// The contract format code — see [`image_format_from_code`].
    format: i32,
    /// `ImageProxy.getWidth()`.
    width: i32,
    /// `ImageProxy.getHeight()`.
    height: i32,
    /// `ImageProxy.getImageInfo().getRotationDegrees()`.
    rotation_degrees: i32,
    /// How many of the three plane slots carry a buffer.
    plane_count: i32,
}

/// The zero-copy byte view of one plane's direct `ByteBuffer`.
///
/// `GetDirectBufferAddress`/`GetDirectBufferCapacity` (the sanctioned
/// `ImageAnalysis` pattern — the plan's RESEARCH §5) hand back the buffer's
/// own memory, so the returned slice aliases the platform's in-flight image
/// with **no copy**. `None` if the buffer is null or not direct.
///
/// # Safety
///
/// The returned slice is valid only until `ImageProxy.close()` runs, which the
/// host defers until [`Java_dev_frust_camera_FrustCameraHost_nativeOnImageFrame`]
/// returns. Callers must therefore keep the slice (and anything derived from
/// it by reference) inside that export's body — which is exactly what
/// [`crate::ImageFrame`]'s close-deadline contract passes on to the user
/// callback. Android's plane buffers are freshly positioned at 0, so the
/// capacity is the whole plane.
unsafe fn plane_data<'frame>(env: &mut Env<'_>, buffer: &JByteBuffer<'_>) -> Option<&'frame [u8]> {
    let address = env.get_direct_buffer_address(buffer).ok()?;
    let capacity = env.get_direct_buffer_capacity(buffer).ok()?;
    if address.is_null() {
        return None;
    }
    // SAFETY: `address`/`capacity` describe one direct `ByteBuffer`'s own
    // allocation, kept alive by the un-closed `ImageProxy` per this function's
    // `# Safety` contract; the bytes are only read, never written, and no Java
    // code touches the image while the analyzer holds it.
    Some(unsafe { std::slice::from_raw_parts(address.cast_const(), capacity) })
}

/// Resolve one frame's planes and hand it to the session's registered
/// callback. A frame for a session with no running stream (a late delivery
/// racing [`AndroidSession::stop_image_stream`]) is dropped.
fn deliver_image_frame(
    env: &mut Env<'_>,
    header: &FrameHeader,
    planes: &[(JByteBuffer<'_>, jint, jint); MAX_PLANES],
) {
    // Clone the `Arc` out and release the map immediately — see [`STREAMS`].
    let Some(slot) = lock(&STREAMS).get(&header.session).cloned() else {
        return;
    };
    let Some(format) = image_format_from_code(header.format) else {
        log::warn!(
            "frust-camera: nativeOnImageFrame(session={}) reported unknown format code {} — frame \
             dropped",
            header.session,
            header.format
        );
        return;
    };

    let count = header.plane_count.clamp(0, MAX_PLANES as i32) as usize;
    let mut frame_planes = [
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
    for (index, plane) in frame_planes.iter_mut().enumerate().take(count) {
        let (buffer, row_stride, pixel_stride) = &planes[index];
        // SAFETY: the slice is used only below, inside this call — the export
        // returns (and only then does the host close the `ImageProxy`) after
        // the user callback has returned. See `plane_data`'s `# Safety`.
        let Some(data) = (unsafe { plane_data(env, buffer) }) else {
            log::warn!(
                "frust-camera: nativeOnImageFrame(session={}) plane {index} is not a direct \
                 ByteBuffer — frame dropped",
                header.session
            );
            return;
        };
        plane.data = data;
        plane.row_stride = (*row_stride).max(0) as usize;
        plane.pixel_stride = (*pixel_stride).max(0) as usize;
    }

    let frame = ImageFrame {
        format,
        width: header.width.max(0) as u32,
        height: header.height.max(0) as u32,
        rotation_degrees: header.rotation_degrees,
        planes: &frame_planes[..count],
    };
    (lock(&slot.on_frame))(&frame);
}

// --- Kotlin -> Rust JNI exports (contract table above) ---------------------

/// `Java_dev_frust_camera_FrustCameraHost_nativeOnPermissionResult` — fires
/// once the system permission dialog resolves
/// ([`PermissionStatus::Pending`]'s eventual answer), waking the
/// [`request_permission`] call blocked in [`await_permission`].
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_camera_FrustCameraHost_nativeOnPermissionResult<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    granted: jboolean,
) {
    env.with_env(|_env| {
        log::debug!("frust-camera: nativeOnPermissionResult(granted={granted})");
        lock(&PERMISSION).result = Some(granted);
        PERMISSION_UPDATED.notify_all();
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `Java_dev_frust_camera_FrustCameraHost_nativeOnCameraState` — `state` is
/// `0` Configuring / `1` Running / `2` Closed / `3` Error (contract table
/// above).
///
/// A `Closed` state **removes** the session's [`SESSIONS`] entry (it is the
/// host's own teardown notification, and the common case is that it arrives
/// after [`AndroidSession::close`] already removed it); every other state
/// updates an existing entry only, so a callback for an unknown session is
/// logged and dropped rather than resurrecting a map entry.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_camera_FrustCameraHost_nativeOnCameraState<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    session: jint,
    state: jint,
) {
    env.with_env(|_env| {
        log::debug!("frust-camera: nativeOnCameraState(session={session}, state={state})");
        {
            let mut sessions = lock(&SESSIONS);
            match state {
                STATE_CODE_CLOSED => {
                    sessions.remove(&session);
                }
                STATE_CODE_CONFIGURING | STATE_CODE_RUNNING | STATE_CODE_ERROR => {
                    let phase = match state {
                        STATE_CODE_RUNNING => SessionPhase::Running,
                        STATE_CODE_ERROR => SessionPhase::Error,
                        _ => SessionPhase::Configuring,
                    };
                    match sessions.get_mut(&session) {
                        Some(entry) => entry.phase = phase,
                        None => log::debug!(
                            "frust-camera: nativeOnCameraState for unknown session {session} — \
                             dropped"
                        ),
                    }
                }
                unknown => log::warn!(
                    "frust-camera: nativeOnCameraState(session={session}) reported unknown state \
                     {unknown} — ignored"
                ),
            }
        }
        SESSIONS_UPDATED.notify_all();
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `Java_dev_frust_camera_FrustCameraHost_nativeOnPictureTaken` — completion
/// callback for [`AndroidSession::take_picture`] (contract table above),
/// waking the caller blocked in [`await_capture`]. The completion is tagged
/// with the session's current capture generation, so a late answer to a
/// timed-out capture can never resolve the next one.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_camera_FrustCameraHost_nativeOnPictureTaken<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    session: jint,
    ok: jboolean,
    path: JString<'local>,
) {
    env.with_env(|_env| {
        log::debug!("frust-camera: nativeOnPictureTaken(session={session}, ok={ok}, path={path})");
        {
            let mut sessions = lock(&SESSIONS);
            match sessions.get_mut(&session) {
                Some(state) => {
                    let generation = state.capture_generation;
                    state.capture_result = Some((generation, ok));
                }
                None => log::debug!(
                    "frust-camera: nativeOnPictureTaken for unknown session {session} — dropped"
                ),
            }
        }
        SESSIONS_UPDATED.notify_all();
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}

/// `Java_dev_frust_camera_FrustCameraHost_nativeOnImageFrame` — one
/// `ImageAnalysis` frame, delivered on the host's analyzer executor (contract
/// table above; **the task-09 contract addition**).
///
/// Runs the user callback **synchronously**: the host closes the `ImageProxy`
/// only after this returns, which is what makes the plane views zero-copy and
/// what makes a slow callback stall the stream — see
/// [`AndroidSession::start_image_stream`]'s close-deadline contract. A panic
/// inside the callback is caught by [`jni::EnvUnowned::with_env`] rather than
/// unwinding into the JVM frame below it.
///
/// The three fixed plane slots (rather than a `ByteBuffer[]`) keep the Kotlin
/// side allocation-free at camera rate; slots past `plane_count` are null and
/// never read.
// The argument list *is* the frozen JNI contract — one flat signature per the
// module doc's table, not a shape clippy's 7-argument heuristic can improve.
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_camera_FrustCameraHost_nativeOnImageFrame<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    session: jint,
    format: jint,
    width: jint,
    height: jint,
    rotation_degrees: jint,
    plane_count: jint,
    plane0: JByteBuffer<'local>,
    row_stride0: jint,
    pixel_stride0: jint,
    plane1: JByteBuffer<'local>,
    row_stride1: jint,
    pixel_stride1: jint,
    plane2: JByteBuffer<'local>,
    row_stride2: jint,
    pixel_stride2: jint,
) {
    env.with_env(|env| {
        let header = FrameHeader {
            session,
            format,
            width,
            height,
            rotation_degrees,
            plane_count,
        };
        let planes = [
            (plane0, row_stride0, pixel_stride0),
            (plane1, row_stride1, pixel_stride1),
            (plane2, row_stride2, pixel_stride2),
        ];
        deliver_image_frame(env, &header, &planes);
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}
