package dev.frust.camera

import android.Manifest
import android.app.Activity
import android.app.Application
import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.content.pm.PackageManager
import android.database.Cursor
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import androidx.camera.core.Camera
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageCapture
import androidx.camera.core.ImageCaptureException
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.core.app.ActivityCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.LifecycleRegistry
import java.io.File
import java.nio.ByteBuffer
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executor
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicInteger

/**
 * The `frust-camera` plugin's CameraX session owner — the Kotlin half of the
 * frozen Rust↔Kotlin contract reproduced in `plugins/camera/src/android.rs`'s
 * module doc.
 *
 * ## The contract (LAW — see `plugins/camera/src/android.rs`)
 *
 * Rust → Kotlin (static methods, resolved through the application classloader
 * exactly like `dev.frust.securestorage.FrustBiometric`):
 *
 * | Method | Signature | Notes |
 * |---|---|---|
 * | [requestPermission] | `() -> int` | 0 Granted / 1 Denied / 2 NeedsUi / 3 Pending |
 * | [openCamera] | `(int lensFacing) -> int` | ≥0 session id; <0 error code |
 * | [closeCamera] | `(int session) -> void` | |
 * | [takePicture] | `(int session, long requestId, String path) -> int` | 0 started; [nativeOnPictureTaken] echoes `requestId` back |
 * | [previewAspectRatio] | `(int session) -> float` | 0.0 until the first `TransformationInfo` |
 * | [startImageStream] | `(int session, int format) -> int` | 0 started; <0 refused |
 * | [stopImageStream] | `(int session) -> void` | unbinds the analyzer only |
 *
 * ### Additive entries (post-freeze)
 *
 * The table above is frozen verbatim; a capability added later arrives as an
 * **additional static**, never as a changed row (the same additive rule
 * [nativeOnImageFrame] followed). `plugins/camera/src/android.rs`'s module doc
 * carries the mirror of this section.
 *
 * | Method | Signature | Notes |
 * |---|---|---|
 * | [setTorch] | `(int session, boolean on) -> int` | [TORCH_SET] accepted (`enableTorch`'s `ListenableFuture` is deliberately **not** waited on) / [ERROR_UNKNOWN_SESSION] / [ERROR_NO_CAMERA] nothing bound yet / [ERROR_TORCH_FAILED] |
 * | [torchAvailable] | `(int session) -> int` | [TORCH_AVAILABLE] / [TORCH_UNAVAILABLE] / [ERROR_UNKNOWN_SESSION] / [ERROR_NO_CAMERA] |
 *
 * Kotlin → Rust ([nativeOnPermissionResult], [nativeOnCameraState],
 * [nativeOnPictureTaken], [nativeOnImageFrame] — the last one was added
 * after this contract was originally frozen; [nativeOnPictureTaken]'s
 * `requestId` is the one field widening since then — see *Correlating a
 * capture completion* below). The package is
 * baked into those symbols' mangled names
 * (`Java_dev_frust_camera_FrustCameraHost_native*`), so **this class may never
 * move once shipped** — same rule as `dev.frust.FrustSurfaceView`'s exports.
 * `dev.frust` itself belongs exclusively to the `frust-embedding` module; every
 * plugin takes a subpackage (`docs/CODE_STANDARDS.md`'s Plugin Conventions).
 *
 * ## Threading
 *
 * Every static above is called over JNI from a Rust background thread (the
 * plugin never-on-the-UI-thread invariant, which the Rust side now enforces
 * with a `Looper.myLooper() == Looper.getMainLooper()` guard) and **must not
 * block**. CameraX's binding APIs and [LifecycleRegistry] are main-thread-only,
 * so each static does its bookkeeping synchronously (allocating the session id,
 * reading a volatile) and posts the platform work to [mainExecutor]; results
 * come back asynchronously through the `nativeOn*` exports.
 *
 * **A completion must never be delivered on the main thread.** The Rust caller
 * of [takePicture] blocks until [nativeOnPictureTaken] answers it, so routing
 * that answer through [mainExecutor] would make the answer depend on the main
 * Looper being free — a self-deadlock for a (misbehaving, now refused)
 * main-thread caller, and needless coupling to UI-thread congestion for
 * everyone else. Still capture therefore runs on its own [captureExecutor]:
 * both the `takePicture` issue and the `OnImageSavedCallback` it hands CameraX,
 * matching what [analysisExecutor] already does for the image stream. Only work
 * CameraX genuinely requires on the main thread — `bindToLifecycle`,
 * [LifecycleRegistry] mutations, `setSurfaceProvider` — stays on
 * [mainExecutor].
 *
 * ## Correlating a capture completion (LAW)
 *
 * [takePicture] carries a Rust-minted `requestId` and [nativeOnPictureTaken]
 * **must echo back the id of the capture it belongs to** — never a "current"
 * id read from session state. Each call's `OnImageSavedCallback` is a fresh
 * closure, so it simply captures its own `requestId`. The Rust side drops any
 * completion whose id is not the one it is waiting for; without the echo, a
 * late completion for a timed-out capture would resolve the *next* one and a
 * caller would believe a photo was written when none was.
 *
 * ## Session lifetime is independent of any view
 *
 * A session owns a private [LifecycleRegistry]-backed [LifecycleOwner] rather
 * than borrowing the Activity's, so it survives a preview slot being culled and
 * revived (`docs/ARCHITECTURE.md`'s Platform-view flow — the frust differ
 * disposes a scrolled-away slot). A disposed slot only drops the *surface*
 * ([CameraPreviewFactory] calls `SurfaceRequest.invalidate()` /
 * `Preview.setSurfaceProvider(null)`); the use cases stay bound and the camera
 * device is never reopened. Only [closeCamera] — or the owning Activity being
 * destroyed — ends a session.
 *
 * Backgrounding is driven off the Activity's own lifecycle
 * ([activityCallbacks]): `onPause` drops every session's registry to
 * [Lifecycle.State.CREATED] (CameraX releases the camera device), `onResume`
 * lifts it back to [Lifecycle.State.RESUMED]. Baking that in here is deliberate
 * — an app must not have to remember to release the camera when it backgrounds.
 *
 * ## The image stream's close deadline (LAW)
 *
 * [startImageStream]'s analyzer calls [nativeOnImageFrame] **synchronously**
 * and closes the [ImageProxy] only after that call returns — which is what
 * makes the plane access on the Rust side zero-copy
 * (`GetDirectBufferAddress`). With
 * [ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST] one image may be in flight at a
 * time, so a Rust callback that returns late stalls every subsequent frame.
 * Never defer the `close()` out of the analyzer callback, and never hand the
 * plane buffers to another thread here.
 */
object FrustCameraHost {
    // --- Contract constants ------------------------------------------------

    /** [requestPermission]: the app already holds `android.permission.CAMERA`. */
    private const val PERMISSION_GRANTED = 0

    /**
     * [requestPermission]: permission was requested before and refused in a way
     * a further request cannot change (the system dialog would no-op) — the
     * caller must send the user to app settings.
     */
    private const val PERMISSION_DENIED = 1

    /**
     * [requestPermission]: no Activity is cached yet, so no dialog can be
     * shown. The Activity arrives with the first
     * [CameraPreviewFactory.createView]; a caller retries after that (or shows
     * its own pre-permission UI).
     */
    private const val PERMISSION_NEEDS_UI = 2

    /**
     * [requestPermission]: the system dialog is up; the answer arrives via
     * [nativeOnPermissionResult].
     */
    private const val PERMISSION_PENDING = 3

    /** [nativeOnCameraState] `state`: configuration started. */
    private const val STATE_CONFIGURING = 0

    /** [nativeOnCameraState] `state`: use cases bound, camera streaming. */
    private const val STATE_RUNNING = 1

    /** [nativeOnCameraState] `state`: session torn down, id retired. */
    private const val STATE_CLOSED = 2

    /** [nativeOnCameraState] `state`: configuration failed; the session is dead. */
    private const val STATE_ERROR = 3

    /**
     * [openCamera] error: no application [Context] — the module's
     * [FrustCameraInitProvider] never ran (an app that strips the merged
     * provider out of its manifest). Unrecoverable.
     */
    private const val ERROR_NO_CONTEXT = -1

    /**
     * [openCamera] error: `android.permission.CAMERA` is not granted. Callers
     * pair [requestPermission] with [openCamera]; binding without the
     * permission would throw a `SecurityException` deep inside CameraX instead.
     */
    private const val ERROR_PERMISSION_DENIED = -2

    /**
     * [takePicture]/[startImageStream]/[setTorch]/[torchAvailable] error:
     * unknown or already-closed session id.
     */
    private const val ERROR_UNKNOWN_SESSION = -1

    /**
     * [setTorch]/[torchAvailable] error: the session is live but CameraX has
     * not finished its first `bindToLifecycle`, so [Session.camera] is still
     * null and there is no `CameraControl` to drive.
     *
     * Transient by nature — the Rust side reports a retry-worded
     * `CameraError::Platform` rather than inventing a wait here, since the
     * whole torch contract is non-blocking.
     */
    private const val ERROR_NO_CAMERA = -3

    /** [setTorch] error: `CameraControl.enableTorch` itself threw. */
    private const val ERROR_TORCH_FAILED = -5

    /**
     * [startImageStream] error: the requested [FORMAT_BGRA] has no CameraX
     * equivalent (see that constant). The Rust backend refuses `Bgra` before it
     * ever reaches this seam; this code exists so a version-skewed caller gets a
     * typed refusal rather than an unrelated format.
     */
    private const val ERROR_UNSUPPORTED_FORMAT = -4

    /** [startImageStream]: the stream request was accepted. */
    private const val IMAGE_STREAM_STARTED = 0

    /**
     * [setTorch]: the request was handed to `CameraControl.enableTorch`.
     *
     * Accepted, not applied — the returned `ListenableFuture` is deliberately
     * dropped so this static never blocks its JNI caller (class doc's
     * *Threading*).
     */
    private const val TORCH_SET = 0

    /** [torchAvailable]: the bound camera reports `CameraInfo.hasFlashUnit()`. */
    private const val TORCH_AVAILABLE = 1

    /** [torchAvailable]: the bound camera has no flash unit (most front lenses). */
    private const val TORCH_UNAVAILABLE = 0

    /**
     * [startImageStream] `format`: `ImageAnalysis.OUTPUT_IMAGE_FORMAT_YUV_420_888`
     * — three planes (Y/U/V), the format both platforms deliver.
     */
    private const val FORMAT_YUV_420_888 = 0

    /**
     * [startImageStream] `format`: packed 32-bit BGRA — **iOS only**. CameraX's
     * one packed output is `OUTPUT_IMAGE_FORMAT_RGBA_8888` (byte order
     * `R,G,B,A`), so this backend refuses the code with
     * [ERROR_UNSUPPORTED_FORMAT] rather than delivering mislabelled bytes.
     */
    private const val FORMAT_BGRA = 1

    /**
     * How many plane slots [nativeOnImageFrame] carries. Three covers
     * `YUV_420_888`; unused slots are passed as null. A fixed argument list
     * rather than an array keeps the per-frame path allocation-free.
     */
    private const val MAX_PLANES = 3

    /** [ActivityCompat.requestPermissions] request code — module-local, arbitrary. */
    private const val PERMISSION_REQUEST_CODE = 0x6672 // 'f','r'

    private const val TAG = "frust"

    // --- Process state -----------------------------------------------------

    private val mainHandler = Handler(Looper.getMainLooper())

    /**
     * Runs inline when already on the main thread, else posts. Used for every
     * main-thread-only CameraX call (`bindToLifecycle`, `unbind`,
     * `setSurfaceProvider`), every [LifecycleRegistry] mutation, and as the
     * callback executor for the preview seam (`provideSurface`,
     * `setTransformationInfoListener`).
     *
     * **Not** for still capture or image analysis: both deliver into a blocked
     * or app-visible Rust callback, so they run on [captureExecutor] /
     * [analysisExecutor] instead (class doc's *Threading*).
     */
    internal val mainExecutor: Executor = Executor { command ->
        if (Looper.myLooper() == Looper.getMainLooper()) {
            command.run()
        } else {
            mainHandler.post(command)
        }
    }

    /**
     * The plugin-owned thread still capture runs on: [takePicture]'s own
     * issue-side work (creating the parent directory — disk I/O that has no
     * business on the main thread — and building the
     * [ImageCapture.OutputFileOptions]) *and* the executor CameraX delivers
     * that capture's [ImageCapture.OnImageSavedCallback] on.
     *
     * The callback half is the load-bearing one: it is what makes a
     * [nativeOnPictureTaken] answer independent of the main Looper being free,
     * so a blocked Rust caller is woken by this thread rather than by a queue
     * it might itself be sitting in (class doc's *Threading*).
     *
     * CameraX's `ImageCapture.takePicture` re-posts itself to the main thread
     * internally when called from anywhere else (verified in camera-core
     * 1.6.1), so the *initiation* is main-thread-bound by CameraX no matter
     * which executor issues it — that post returns immediately and never
     * occupies this thread, which is why one shared single-thread executor is
     * enough. Same shape as [analysisExecutor]: lazy, single-threaded, daemon.
     */
    private val captureExecutor: ExecutorService by lazy {
        Executors.newSingleThreadExecutor { runnable ->
            Thread(runnable, "frust-camera-capture").apply { isDaemon = true }
        }
    }

    /**
     * The plugin-owned thread every [ImageAnalysis] analyzer runs on — never
     * the main thread (a frame callback crosses into Rust and runs app code)
     * and never a CameraX-internal one.
     *
     * One shared single-thread executor for the whole process: two concurrently
     * streaming sessions is not a v1 shape, and serializing them is preferable
     * to spawning a thread per session. Created lazily so a process that never
     * streams never pays for the thread; daemon, so it cannot keep the JVM
     * alive.
     */
    private val analysisExecutor: ExecutorService by lazy {
        Executors.newSingleThreadExecutor { runnable ->
            Thread(runnable, "frust-camera-analysis").apply { isDaemon = true }
        }
    }

    /**
     * Set once [nativeOnImageFrame] has proven unavailable (an app taking this
     * Gradle module without the `frust-camera` Rust crate), so the per-frame
     * guard logs once instead of once per frame at camera rate.
     */
    @Volatile
    private var imageFrameNativeMissing: Boolean = false

    /**
     * The application [Context], installed at process start by
     * [FrustCameraInitProvider]. Needed by
     * `ProcessCameraProvider.getInstance(context)` and
     * `checkSelfPermission` — both of which must work before any view (and
     * therefore any Activity) exists, since a session id is what a preview
     * slot's `params_json` carries.
     */
    @Volatile
    private var appContext: Context? = null

    /**
     * The hosting Activity. Learned from **either** the process-wide lifecycle
     * callbacks (registered at process start — [ensureLifecycleCallbacks]) or
     * the platform-view factory seam ([CameraPreviewFactory.createView]).
     * Permission dialogs need it; [requestPermission] reports
     * [PERMISSION_NEEDS_UI] while it is null.
     *
     * The callbacks path exists because the factory seam alone deadlocks: it
     * cannot fire until a preview slot mounts, and a host page will not mount
     * one until permission is granted.
     */
    @Volatile
    private var activity: Activity? = null

    /** Whether [activity] is currently between `onResume` and `onPause`. */
    @Volatile
    private var activityResumed: Boolean = true

    /** Guards a single [Application.registerActivityLifecycleCallbacks] call. */
    private var lifecycleCallbacksRegistered: Boolean = false

    /** A permission dialog is up; the next `onResume` relays its outcome. */
    @Volatile
    private var permissionRequestPending: Boolean = false

    /** A permission request has already come back refused at least once. */
    @Volatile
    private var permissionRefusedOnce: Boolean = false

    private val nextSessionId = AtomicInteger(0)

    /** Live sessions by id. Removed by [closeCamera] — ids are never reused. */
    private val sessions = ConcurrentHashMap<Int, Session>()

    // --- Rust -> Kotlin: the frozen contract -------------------------------

    /**
     * `requestPermission() -> int`. Returns [PERMISSION_GRANTED],
     * [PERMISSION_DENIED], [PERMISSION_NEEDS_UI] or [PERMISSION_PENDING];
     * never blocks on the dialog (a [PERMISSION_PENDING] answer arrives through
     * [nativeOnPermissionResult]).
     */
    @JvmStatic
    fun requestPermission(): Int {
        val context = appContext ?: activity
        if (context != null && hasCameraPermission(context)) return PERMISSION_GRANTED
        val act = activity ?: return PERMISSION_NEEDS_UI
        // A refusal plus "no rationale to show" is Android's only signal for
        // "the system will not show this dialog again" — re-requesting would
        // silently deliver an immediate denial, so report it as a terminal
        // Denied and let the caller route the user to app settings.
        if (permissionRefusedOnce &&
            !ActivityCompat.shouldShowRequestPermissionRationale(act, Manifest.permission.CAMERA)
        ) {
            return PERMISSION_DENIED
        }
        permissionRequestPending = true
        mainExecutor.execute {
            try {
                ActivityCompat.requestPermissions(
                    act,
                    arrayOf(Manifest.permission.CAMERA),
                    PERMISSION_REQUEST_CODE,
                )
            } catch (e: Throwable) {
                Log.w(TAG, "frust-camera: requestPermissions failed", e)
                permissionRequestPending = false
                notifyPermissionResult(false)
            }
        }
        return PERMISSION_PENDING
    }

    /**
     * `openCamera(int lensFacing) -> int`. Returns the new session id (≥0)
     * immediately; configuration continues on the main thread and reports
     * [STATE_CONFIGURING] → [STATE_RUNNING] (or [STATE_ERROR]) through
     * [nativeOnCameraState]. `lensFacing` is a `CameraSelector.LENS_FACING_*`
     * value.
     *
     * The frozen contract carries no resolution argument, so CameraX's own
     * default resolution selection applies; the Rust API's `Resolution` is not
     * threaded across this seam in v1.
     */
    @JvmStatic
    fun openCamera(lensFacing: Int): Int {
        val context = appContext
        if (context == null) {
            Log.w(TAG, "frust-camera: openCamera before the init provider ran — no app Context")
            return ERROR_NO_CONTEXT
        }
        if (!hasCameraPermission(context)) {
            Log.w(TAG, "frust-camera: openCamera without CAMERA permission")
            return ERROR_PERMISSION_DENIED
        }
        val session = Session(nextSessionId.getAndIncrement(), lensFacing)
        sessions[session.id] = session
        notifyCameraState(session.id, STATE_CONFIGURING)
        mainExecutor.execute { configure(context, session) }
        return session.id
    }

    /**
     * `closeCamera(int session) -> void`. Unbinds this session's use cases and
     * destroys its lifecycle owner; the id is retired. Reports [STATE_CLOSED].
     * Unknown/already-closed ids are ignored.
     */
    @JvmStatic
    fun closeCamera(session: Int) {
        val entry = sessions.remove(session) ?: return
        entry.closed = true
        entry.streamFormat = null
        mainExecutor.execute {
            try {
                entry.preview.setSurfaceProvider(null)
                unbindImageAnalysis(entry)
                // `unbind(useCases)`, never `unbindAll()`: another session may
                // be bound to the same process-wide provider.
                entry.provider?.unbind(entry.preview, entry.imageCapture)
                // The unbound `Camera` handle can control nothing any more —
                // drop it with the use cases (the torch goes out with the
                // device, which is `CameraSession::close`'s documented
                // contract on the Rust side).
                entry.camera = null
                entry.setState(Lifecycle.State.DESTROYED)
            } catch (e: Throwable) {
                Log.w(TAG, "frust-camera: closeCamera($session) failed", e)
            }
            notifyCameraState(session, STATE_CLOSED)
        }
    }

    /**
     * `takePicture(int session, long requestId, String path) -> int`. Returns
     * `0` once the capture has been *accepted* (its completion arrives via
     * [nativeOnPictureTaken], stamped with this exact [requestId]) or
     * [ERROR_UNKNOWN_SESSION].
     *
     * [requestId] is minted by the Rust caller and is opaque here: this method
     * only has to carry it into the per-call [ImageCapture.OnImageSavedCallback]
     * closure and hand it back unchanged (class doc's *Correlating a capture
     * completion* LAW).
     *
     * Everything below runs on [captureExecutor], never [mainExecutor] — the
     * completion must not depend on the main Looper being free (class doc's
     * *Threading*).
     */
    @JvmStatic
    fun takePicture(session: Int, requestId: Long, path: String): Int {
        val entry = sessions[session] ?: return ERROR_UNKNOWN_SESSION
        captureExecutor.execute {
            val file = File(path)
            try {
                file.parentFile?.mkdirs()
                val options = ImageCapture.OutputFileOptions.Builder(file).build()
                entry.imageCapture.takePicture(
                    options,
                    captureExecutor,
                    object : ImageCapture.OnImageSavedCallback {
                        override fun onImageSaved(results: ImageCapture.OutputFileResults) {
                            notifyPictureTaken(session, requestId, true, path)
                        }

                        override fun onError(exception: ImageCaptureException) {
                            Log.w(TAG, "frust-camera: takePicture($session) failed", exception)
                            notifyPictureTaken(session, requestId, false, path)
                        }
                    },
                )
            } catch (e: Throwable) {
                Log.w(TAG, "frust-camera: takePicture($session) could not start", e)
                notifyPictureTaken(session, requestId, false, path)
            }
        }
        return 0
    }

    /**
     * `previewAspectRatio(int session) -> float`. The displayed preview's
     * width/height, `0.0` until the first `SurfaceRequest.TransformationInfo`
     * arrives (or for an unknown session) — the contract's documented initial
     * value, which an app uses to size the slot.
     */
    @JvmStatic
    fun previewAspectRatio(session: Int): Float = sessions[session]?.aspectRatio ?: 0.0f

    /**
     * `startImageStream(int session, int format) -> int`. Binds an
     * [ImageAnalysis] use case beside the already-bound preview/capture pair and
     * starts delivering frames through [nativeOnImageFrame].
     *
     * Returns [IMAGE_STREAM_STARTED] once the request is accepted — the bind
     * itself happens on the main thread, and for a session still configuring it
     * happens as part of [configure]'s own bind (the requested format is
     * remembered on the session). Backpressure is
     * [ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST]: a slow consumer drops frames
     * rather than queueing them — and stalls the stream outright if it misses
     * the close deadline (class doc's LAW).
     *
     * Calling it twice on one session re-binds the analyzer with the new format.
     */
    @JvmStatic
    fun startImageStream(session: Int, format: Int): Int {
        val entry = sessions[session] ?: return ERROR_UNKNOWN_SESSION
        if (outputImageFormatFor(format) == null) {
            Log.w(TAG, "frust-camera: startImageStream($session) unsupported format $format")
            return ERROR_UNSUPPORTED_FORMAT
        }
        entry.streamFormat = format
        mainExecutor.execute {
            // A session that has not resolved its provider yet binds the
            // analyzer as part of `configure`; nothing to do here.
            if (entry.provider != null) bindImageAnalysis(entry, format)
        }
        return IMAGE_STREAM_STARTED
    }

    /**
     * `stopImageStream(int session) -> void`. Unbinds this session's
     * [ImageAnalysis] use case — **the preview and the camera device are
     * untouched** — and clears its analyzer. A no-op for an unknown session or
     * one with no stream running.
     */
    @JvmStatic
    fun stopImageStream(session: Int) {
        val entry = sessions[session] ?: return
        entry.streamFormat = null
        mainExecutor.execute { unbindImageAnalysis(entry) }
    }

    /**
     * `setTorch(int session, boolean on) -> int` (class doc's *Additive
     * entries*). Drives `CameraControl.enableTorch` on the [Camera] handle
     * `bindToLifecycle` returned.
     *
     * **The returned `ListenableFuture` is deliberately dropped.** Waiting on
     * it would mean either blocking this JNI caller (the one thing every
     * static here must not do) or inventing a completion callback the frozen
     * contract has no `nativeOn*` entry for; the Rust API documents the torch
     * as non-blocking and callable from any thread, and CameraX applies the
     * change on its own executor either way. A refusal that *is* knowable
     * synchronously — no session, nothing bound yet, a throwing call — comes
     * back as a return code.
     *
     * Runs entirely on the calling (JNI) thread, unlike every
     * [mainExecutor]-posted static above: this one has a return code to
     * produce, and posting would leave nothing to report. `CameraControl` is
     * asynchronous by construction (it answers with a `ListenableFuture` and
     * applies the change on CameraX's own camera executor — verified against
     * camera-core 1.6.1's signature), so the call itself neither blocks here
     * nor needs the main thread the way `bindToLifecycle` does.
     */
    @JvmStatic
    fun setTorch(session: Int, on: Boolean): Int {
        val entry = sessions[session] ?: return ERROR_UNKNOWN_SESSION
        val camera = entry.camera ?: return ERROR_NO_CAMERA
        return try {
            camera.cameraControl.enableTorch(on)
            TORCH_SET
        } catch (e: Throwable) {
            Log.w(TAG, "frust-camera: setTorch($session, $on) failed", e)
            ERROR_TORCH_FAILED
        }
    }

    /**
     * `torchAvailable(int session) -> int` (class doc's *Additive entries*).
     * [TORCH_AVAILABLE] / [TORCH_UNAVAILABLE], or a negative code when there
     * is nothing to ask ([ERROR_UNKNOWN_SESSION] / [ERROR_NO_CAMERA]) — the
     * Rust side reports every non-[TORCH_AVAILABLE] answer as `false`.
     *
     * `CameraInfo.hasFlashUnit()` is a fixed property of the bound lens (it
     * answers immediately and never changes for that camera), so this is a
     * plain synchronous read on the calling thread like [setTorch].
     */
    @JvmStatic
    fun torchAvailable(session: Int): Int {
        val entry = sessions[session] ?: return ERROR_UNKNOWN_SESSION
        val camera = entry.camera ?: return ERROR_NO_CAMERA
        return try {
            if (camera.cameraInfo.hasFlashUnit()) TORCH_AVAILABLE else TORCH_UNAVAILABLE
        } catch (e: Throwable) {
            Log.w(TAG, "frust-camera: torchAvailable($session) failed", e)
            TORCH_UNAVAILABLE
        }
    }

    // --- Kotlin -> Rust: this plugin's own JNI exports ---------------------
    //
    // Implemented in `plugins/camera/src/android.rs` as
    // `Java_dev_frust_camera_FrustCameraHost_native*`. They resolve against the
    // app's already-loaded native library (the embedding's `FrustActivity`
    // loads it before any frust view exists, and every call below is downstream
    // of a Rust-initiated one), so no `System.loadLibrary` happens here.

    /** Delivers the outcome of a [PERMISSION_PENDING] request. */
    @JvmStatic
    external fun nativeOnPermissionResult(granted: Boolean)

    /**
     * Reports a session's state: [STATE_CONFIGURING] / [STATE_RUNNING] /
     * [STATE_CLOSED] / [STATE_ERROR].
     */
    @JvmStatic
    external fun nativeOnCameraState(session: Int, state: Int)

    /**
     * Reports a [takePicture] completion, echoing back the `requestId` **that
     * capture was started with** — class doc's *Correlating a capture
     * completion* LAW.
     */
    @JvmStatic
    external fun nativeOnPictureTaken(
        session: Int,
        requestId: Long,
        ok: Boolean,
        path: String,
    )

    /**
     * Delivers one [ImageAnalysis] frame, on [analysisExecutor].
     *
     * Called synchronously from the analyzer with the [ImageProxy] still open,
     * so the Rust side can read the plane buffers zero-copy
     * (`GetDirectBufferAddress`); the proxy is closed as soon as this returns
     * (class doc's close-deadline LAW).
     *
     * [MAX_PLANES] fixed plane slots rather than arrays: an array per frame
     * would allocate at camera rate. Slots past [planeCount] are null.
     */
    @JvmStatic
    external fun nativeOnImageFrame(
        session: Int,
        format: Int,
        width: Int,
        height: Int,
        rotationDegrees: Int,
        planeCount: Int,
        plane0: ByteBuffer?,
        rowStride0: Int,
        pixelStride0: Int,
        plane1: ByteBuffer?,
        rowStride1: Int,
        pixelStride1: Int,
        plane2: ByteBuffer?,
        rowStride2: Int,
        pixelStride2: Int,
    )

    // --- Module-internal seam (NOT part of the JNI contract) ---------------

    /**
     * Install the application [Context] — called once at process start by
     * [FrustCameraInitProvider], before any Activity or view exists. Idempotent
     * and first-writer-wins: [cacheActivity] installs the same context as a
     * fallback if the provider was somehow stripped from the merged manifest.
     *
     * This also registers the process-wide lifecycle callbacks, which is what
     * lets [requestPermission] work before any preview slot has ever mounted —
     * see [ensureLifecycleCallbacks].
     */
    internal fun installApplicationContext(context: Context) {
        if (appContext == null) appContext = context
        ensureLifecycleCallbacks(context)
    }

    /**
     * Register the process-wide [Application.ActivityLifecycleCallbacks] once.
     *
     * These callbacks do three jobs: drive session pause/resume, relay a
     * pending permission answer on the next resume, and — since a
     * device-gate run surfaced the permission deadlock below —
     * **discover the foreground Activity in the first place**.
     *
     * Registering them at process start (from [installApplicationContext],
     * i.e. [FrustCameraInitProvider.onCreate]) rather than from
     * [cacheActivity] is what breaks the permission deadlock the gate found:
     * the Activity used to be observable *only* through the platform-view
     * factory seam, but that seam only fires once a preview slot mounts, and a
     * host page only mounts its slot once permission is already granted. No
     * preview → no Activity → `PERMISSION_NEEDS_UI` → no grant → no preview.
     * A `ContentProvider` runs before any Activity is created, so the callbacks
     * are in place for the very first `onActivityCreated` and the cycle cannot
     * form.
     *
     * [Context.getApplicationContext] is the `Application` instance on every
     * supported API level; the `as?` keeps a non-`Application` context (a test
     * double, an oddly-wrapped host) from throwing at process start — the
     * module then degrades to the old factory-seam path rather than crashing
     * the app in a provider.
     */
    private fun ensureLifecycleCallbacks(context: Context) {
        if (lifecycleCallbacksRegistered) return
        val app = context.applicationContext as? Application ?: return
        lifecycleCallbacksRegistered = true
        app.registerActivityLifecycleCallbacks(activityCallbacks)
    }

    /**
     * Cache the hosting Activity, handed to us through the platform-view
     * factory seam ([CameraPreviewFactory.createView]).
     *
     * No longer the *only* way this module learns its Activity — the lifecycle
     * callbacks registered at process start ([ensureLifecycleCallbacks]) adopt
     * whichever Activity is resumed. This path is kept because it is exact (the
     * Activity actually hosting the slot) and because it still works if the
     * merged provider was stripped.
     */
    internal fun cacheActivity(activity: Activity) {
        this.activity = activity
        if (appContext == null) appContext = activity.applicationContext
        ensureLifecycleCallbacks(activity)
    }

    /**
     * Point session [sessionId]'s `Preview` at [provider]. Called on the main
     * thread when a preview slot is created or re-pointed at another session.
     *
     * Swapping a provider on an already-bound `Preview` is supported in place:
     * CameraX detaches the old surface and issues a brand-new `SurfaceRequest`
     * to the new provider without unbinding — no camera reopen. Returns false
     * for an unknown session (the caller renders an empty slot).
     */
    internal fun attachPreview(sessionId: Int, provider: Preview.SurfaceProvider): Boolean {
        val entry = sessions[sessionId] ?: return false
        entry.preview.setSurfaceProvider(provider)
        return true
    }

    /**
     * Drop session [sessionId]'s surface provider (slot disposed). This pauses
     * preview — the CameraX-sanctioned way — while leaving the use cases bound
     * and the camera device open, which is what makes a revived slot cheap.
     */
    internal fun detachPreview(sessionId: Int) {
        val entry = sessions[sessionId] ?: return
        entry.preview.setSurfaceProvider(null)
        // The last published aspect ratio deliberately SURVIVES a detach: the
        // stream configuration has not changed, and zeroing it would collapse
        // the app's slot sizing every time a preview scrolls out of view.
    }

    /**
     * Publish the resolved preview aspect ratio for [previewAspectRatio] — the
     * width/height of what the preview slot actually displays.
     *
     * An app is expected to size its slot from it (the Rust
     * `CameraSession::preview_aspect_ratio` contract): the Android preview
     * fits its content inside the slot rather than cropping to fill it, so a
     * slot at this ratio is exactly filled and any other ratio letterboxes —
     * see [CameraPreviewView]'s *Geometry* note for why a compositor-layer
     * preview has no third option.
     */
    internal fun setPreviewAspectRatio(sessionId: Int, ratio: Float) {
        sessions[sessionId]?.aspectRatio = ratio
    }

    /**
     * Track the display rotation the preview is being shown at (a
     * `Surface.ROTATION_*` value), so CameraX's `TransformationInfo` reports
     * the rotation that makes the buffer upright *for this display*. Sessions
     * open before any view exists, so this cannot be set at build time.
     *
     * This is the **whole** of the preview's rotation handling: the stream's
     * own buffer transform is derived from `targetRotation` and the system
     * compositor is what applies it, and a `SurfaceView`-hosted preview has no
     * content transform of its own to correct with ([CameraPreviewView]'s
     * *Rotation* note). A `targetRotation` left stale therefore shows as a
     * sideways preview, not as a slightly-off one.
     */
    internal fun setTargetRotation(sessionId: Int, rotation: Int) {
        val entry = sessions[sessionId] ?: return
        entry.targetRotation = rotation
        entry.preview.targetRotation = rotation
        entry.imageCapture.targetRotation = rotation
        // A running analyzer's frames carry `rotationDegrees` derived from this,
        // so the stream tracks the display exactly like the preview does.
        entry.imageAnalysis?.targetRotation = rotation
    }

    // --- Internals ---------------------------------------------------------

    /**
     * One camera session: its own [LifecycleOwner] (so its lifetime is
     * independent of both the Activity and any preview slot), its use cases,
     * and the last-published preview aspect ratio.
     *
     * Constructed on the calling (JNI) thread — [LifecycleRegistry]'s
     * constructor takes no main-thread lock — but [setState] and every CameraX
     * call are main-thread-only.
     */
    private class Session(val id: Int, val lensFacing: Int) : LifecycleOwner {
        private val registry = LifecycleRegistry(this)

        override val lifecycle: Lifecycle
            get() = registry

        val preview: Preview = Preview.Builder().build()
        val imageCapture: ImageCapture = ImageCapture.Builder().build()

        /** Read from any thread by `previewAspectRatio`. */
        @Volatile
        var aspectRatio: Float = 0.0f

        /** The process-wide provider, once resolved. */
        var provider: ProcessCameraProvider? = null

        /**
         * The [Camera] `bindToLifecycle` returned — this session's handle onto
         * `CameraControl`/`CameraInfo`, and the only way to reach the torch.
         *
         * Written on the main thread at **every** bind site ([configure]'s
         * initial bind and [bindImageAnalysis]'s analyzer re-bind, which
         * returns a handle for the same device rather than reopening it) and
         * read from a JNI thread by [setTorch]/[torchAvailable], hence
         * `@Volatile`. Null until the first bind completes — the
         * [ERROR_NO_CAMERA] path, not an error state.
         */
        @Volatile
        var camera: Camera? = null

        /**
         * The bound `ImageAnalysis` use case while a stream is running, else
         * null. Main thread only.
         */
        var imageAnalysis: ImageAnalysis? = null

        /**
         * The format `startImageStream` asked for, or null when no stream is
         * wanted. Read on the main thread by [configure] (a stream requested
         * while the session was still configuring) and by `bindImageAnalysis`.
         */
        @Volatile
        var streamFormat: Int? = null

        /**
         * The last display rotation `setTargetRotation` reported, replayed onto
         * an analyzer bound after that call. `ROTATION_0` until a preview slot
         * exists — the same default CameraX's use cases start at.
         */
        @Volatile
        var targetRotation: Int = android.view.Surface.ROTATION_0

        /** Set by `closeCamera` before the main-thread teardown runs. */
        @Volatile
        var closed: Boolean = false

        /** Main thread only. */
        fun setState(state: Lifecycle.State) {
            if (registry.currentState == Lifecycle.State.DESTROYED) return
            registry.currentState = state
        }

        /** Main thread only. */
        fun isDestroyed(): Boolean = registry.currentState == Lifecycle.State.DESTROYED
    }

    /** Main thread. Resolves the provider, then binds this session's use cases. */
    private fun configure(context: Context, session: Session) {
        val future = ProcessCameraProvider.getInstance(context)
        future.addListener(
            {
                if (session.closed) return@addListener
                try {
                    val provider = future.get()
                    session.provider = provider
                    // Reach RESUMED (or CREATED while the app is backgrounded)
                    // BEFORE binding: CameraX opens the device as soon as the
                    // bound lifecycle is at least STARTED.
                    session.setState(targetState())
                    // The returned `Camera` is this session's `CameraControl`/
                    // `CameraInfo` handle — captured, not discarded, because
                    // `setTorch`/`torchAvailable` have no other way in.
                    session.camera = provider.bindToLifecycle(
                        session,
                        selectorFor(session.lensFacing),
                        session.preview,
                        session.imageCapture,
                    )
                    // A stream requested while this session was still
                    // configuring binds now, as part of the same bring-up.
                    session.streamFormat?.let { bindImageAnalysis(session, it) }
                    notifyCameraState(session.id, STATE_RUNNING)
                } catch (e: Throwable) {
                    Log.w(TAG, "frust-camera: session ${session.id} failed to configure", e)
                    sessions.remove(session.id)
                    notifyCameraState(session.id, STATE_ERROR)
                }
            },
            mainExecutor,
        )
    }

    private fun selectorFor(lensFacing: Int): CameraSelector = when (lensFacing) {
        CameraSelector.LENS_FACING_FRONT -> CameraSelector.DEFAULT_FRONT_CAMERA
        else -> CameraSelector.DEFAULT_BACK_CAMERA
    }

    /**
     * The `ImageAnalysis.OUTPUT_IMAGE_FORMAT_*` value a contract format code
     * names, or null for one CameraX cannot deliver ([FORMAT_BGRA]).
     */
    private fun outputImageFormatFor(format: Int): Int? = when (format) {
        FORMAT_YUV_420_888 -> ImageAnalysis.OUTPUT_IMAGE_FORMAT_YUV_420_888
        else -> null
    }

    /**
     * Main thread. Bind (or re-bind) session [entry]'s analyzer at [format].
     *
     * `bindToLifecycle` is called again with only the new use case — CameraX
     * combines it with the preview/capture pair already bound to the same
     * lifecycle owner, so the camera device is never reopened and the preview
     * never blinks.
     */
    private fun bindImageAnalysis(entry: Session, format: Int) {
        if (entry.closed || entry.isDestroyed()) return
        val provider = entry.provider ?: return
        val outputFormat = outputImageFormatFor(format) ?: return

        // Re-binding replaces any analyzer already running for this session.
        unbindImageAnalysis(entry)

        try {
            val analysis = ImageAnalysis.Builder()
                .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                .setOutputImageFormat(outputFormat)
                .build()
            analysis.targetRotation = entry.targetRotation
            analysis.setAnalyzer(analysisExecutor) { proxy ->
                // `use`-style: the proxy closes as soon as the native call
                // returns, whatever it did — the KEEP_ONLY_LATEST slot must not
                // be held by a failed delivery either.
                try {
                    deliverImageFrame(entry.id, format, proxy)
                } finally {
                    proxy.close()
                }
            }
            // Re-assign the handle: this bind returns a `Camera` for the same
            // already-open device (CameraX combines the new use case with the
            // preview/capture pair), so torch control stays reachable across
            // a `startImageStream` re-bind rather than going stale.
            entry.camera = provider.bindToLifecycle(
                entry,
                selectorFor(entry.lensFacing),
                analysis,
            )
            entry.imageAnalysis = analysis
        } catch (e: Throwable) {
            Log.w(TAG, "frust-camera: startImageStream(${entry.id}) failed to bind", e)
            entry.streamFormat = null
        }
    }

    /** Main thread. Drop session [entry]'s analyzer, leaving the preview bound. */
    private fun unbindImageAnalysis(entry: Session) {
        val analysis = entry.imageAnalysis ?: return
        entry.imageAnalysis = null
        try {
            analysis.clearAnalyzer()
            entry.provider?.unbind(analysis)
        } catch (e: Throwable) {
            Log.w(TAG, "frust-camera: stopImageStream(${entry.id}) failed to unbind", e)
        }
    }

    /**
     * [analysisExecutor]. Hand one open [ImageProxy]'s planes to Rust.
     *
     * Reads nothing out of the buffers itself — the whole point is that the
     * Rust callback sees the platform's own memory (class doc's LAW).
     */
    private fun deliverImageFrame(session: Int, format: Int, proxy: ImageProxy) {
        val planes = proxy.planes
        val count = if (planes.size < MAX_PLANES) planes.size else MAX_PLANES
        notifyImageFrame(
            session,
            format,
            proxy.width,
            proxy.height,
            proxy.imageInfo.rotationDegrees,
            count,
            if (count > 0) planes[0].buffer else null,
            if (count > 0) planes[0].rowStride else 0,
            if (count > 0) planes[0].pixelStride else 0,
            if (count > 1) planes[1].buffer else null,
            if (count > 1) planes[1].rowStride else 0,
            if (count > 1) planes[1].pixelStride else 0,
            if (count > 2) planes[2].buffer else null,
            if (count > 2) planes[2].rowStride else 0,
            if (count > 2) planes[2].pixelStride else 0,
        )
    }

    /** The lifecycle state a live session should sit at right now. */
    private fun targetState(): Lifecycle.State =
        if (activityResumed) Lifecycle.State.RESUMED else Lifecycle.State.CREATED

    private fun hasCameraPermission(context: Context): Boolean =
        context.checkPermission(
            Manifest.permission.CAMERA,
            android.os.Process.myPid(),
            android.os.Process.myUid(),
        ) == PackageManager.PERMISSION_GRANTED

    /**
     * Pause/resume every live session with the hosting Activity, and relay a
     * pending permission answer.
     *
     * The permission dialog runs in its own Activity, so the host pauses while
     * it is up and resumes when it is dismissed — re-checking the grant on
     * resume is what turns [ActivityCompat.requestPermissions] into a relayable
     * result without requiring a consuming app to forward
     * `onRequestPermissionsResult` (a plugin cannot subclass the app's
     * Activity).
     */
    private val activityCallbacks = object : Application.ActivityLifecycleCallbacks {
        /**
         * Adopt `activity` as the host if none is cached yet.
         *
         * This closes the permission deadlock: the module used
         * to learn its Activity only from the platform-view factory seam, which
         * cannot fire before permission is granted (see
         * [ensureLifecycleCallbacks]). Adoption is deliberately
         * first-resumed-wins and never *replaces* a live cached Activity —
         * [cacheActivity]'s handle is the exact slot host and stays
         * authoritative, and [onActivityDestroyed] is what clears the slot.
         */
        private fun adoptIfUnset(activity: Activity) {
            if (this@FrustCameraHost.activity == null) {
                this@FrustCameraHost.activity = activity
            }
        }

        override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {
            adoptIfUnset(activity)
        }

        override fun onActivityStarted(activity: Activity) {
            adoptIfUnset(activity)
        }

        override fun onActivityResumed(activity: Activity) {
            adoptIfUnset(activity)
            if (activity !== this@FrustCameraHost.activity) return
            activityResumed = true
            for (session in sessions.values) {
                if (!session.isDestroyed()) session.setState(Lifecycle.State.RESUMED)
            }
            if (permissionRequestPending) {
                permissionRequestPending = false
                val granted = hasCameraPermission(activity)
                if (!granted) permissionRefusedOnce = true
                notifyPermissionResult(granted)
            }
        }

        override fun onActivityPaused(activity: Activity) {
            if (activity !== this@FrustCameraHost.activity) return
            activityResumed = false
            for (session in sessions.values) {
                if (!session.isDestroyed()) session.setState(Lifecycle.State.CREATED)
            }
        }

        override fun onActivityStopped(activity: Activity) {}

        override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}

        override fun onActivityDestroyed(activity: Activity) {
            if (activity !== this@FrustCameraHost.activity) return
            this@FrustCameraHost.activity = null
        }
    }

    // Every native call is guarded: a missing symbol (an app that depends on
    // this Gradle module without the `frust-camera` Rust crate) must degrade to
    // a log line, never an UnsatisfiedLinkError thrown into a CameraX callback
    // or the Activity lifecycle.

    private fun notifyPermissionResult(granted: Boolean) {
        try {
            nativeOnPermissionResult(granted)
        } catch (e: Throwable) {
            Log.w(TAG, "frust-camera: nativeOnPermissionResult unavailable", e)
        }
    }

    private fun notifyCameraState(session: Int, state: Int) {
        try {
            nativeOnCameraState(session, state)
        } catch (e: Throwable) {
            Log.w(TAG, "frust-camera: nativeOnCameraState unavailable", e)
        }
    }

    private fun notifyPictureTaken(session: Int, requestId: Long, ok: Boolean, path: String) {
        try {
            nativeOnPictureTaken(session, requestId, ok, path)
        } catch (e: Throwable) {
            Log.w(TAG, "frust-camera: nativeOnPictureTaken unavailable", e)
        }
    }

    /**
     * The per-frame guard. Unlike its siblings this one latches
     * [imageFrameNativeMissing] on the first failure: at camera rate an
     * unresolvable symbol would otherwise log 30 times a second.
     */
    private fun notifyImageFrame(
        session: Int,
        format: Int,
        width: Int,
        height: Int,
        rotationDegrees: Int,
        planeCount: Int,
        plane0: ByteBuffer?,
        rowStride0: Int,
        pixelStride0: Int,
        plane1: ByteBuffer?,
        rowStride1: Int,
        pixelStride1: Int,
        plane2: ByteBuffer?,
        rowStride2: Int,
        pixelStride2: Int,
    ) {
        if (imageFrameNativeMissing) return
        try {
            nativeOnImageFrame(
                session,
                format,
                width,
                height,
                rotationDegrees,
                planeCount,
                plane0,
                rowStride0,
                pixelStride0,
                plane1,
                rowStride1,
                pixelStride1,
                plane2,
                rowStride2,
                pixelStride2,
            )
        } catch (e: UnsatisfiedLinkError) {
            imageFrameNativeMissing = true
            Log.w(TAG, "frust-camera: nativeOnImageFrame unavailable — stream frames dropped", e)
        } catch (e: Throwable) {
            Log.w(TAG, "frust-camera: nativeOnImageFrame($session) failed", e)
        }
    }
}

/**
 * Installs the application [Context] into [FrustCameraHost] at process start.
 *
 * A `ContentProvider` declared in this module's own `AndroidManifest.xml` is
 * created by the system before `Application.onCreate`, which is the standard
 * self-initialization mechanism for an Android library that needs a `Context`
 * with no app-side code (`androidx.startup`'s `InitializationProvider`,
 * Firebase's `FirebaseInitProvider`). `frust-camera` needs one because the
 * frozen `openCamera(int lensFacing)` contract passes no `Context` and a
 * session must be openable *before* its preview slot exists — the slot's
 * `params_json` carries the session id `openCamera` returned, so the Activity
 * from the platform-view factory seam arrives strictly too late.
 *
 * It provides no data: every `ContentProvider` operation returns null/0. It is
 * `exported="false"` and its authority is `${applicationId}`-scoped, so nothing
 * outside the app can reach it.
 */
class FrustCameraInitProvider : ContentProvider() {
    override fun onCreate(): Boolean {
        context?.let { FrustCameraHost.installApplicationContext(it.applicationContext) }
        return true
    }

    override fun query(
        uri: Uri,
        projection: Array<out String>?,
        selection: String?,
        selectionArgs: Array<out String>?,
        sortOrder: String?,
    ): Cursor? = null

    override fun getType(uri: Uri): String? = null

    override fun insert(uri: Uri, values: ContentValues?): Uri? = null

    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int = 0

    override fun update(
        uri: Uri,
        values: ContentValues?,
        selection: String?,
        selectionArgs: Array<out String>?,
    ): Int = 0
}
