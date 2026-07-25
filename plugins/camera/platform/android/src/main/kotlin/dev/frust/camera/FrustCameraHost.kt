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
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageCapture
import androidx.camera.core.ImageCaptureException
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.core.app.ActivityCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.LifecycleRegistry
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executor
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
 * | [takePicture] | `(int session, String path) -> int` | 0 started |
 * | [previewAspectRatio] | `(int session) -> float` | 0.0 until the first `TransformationInfo` |
 * | [startImageStream] | `(int session, int format) -> int` | task 09 |
 * | [stopImageStream] | `(int session) -> void` | task 09 |
 *
 * Kotlin → Rust ([nativeOnPermissionResult], [nativeOnCameraState],
 * [nativeOnPictureTaken]; task 09 adds `nativeOnImageFrame`). The package is
 * baked into those symbols' mangled names
 * (`Java_dev_frust_camera_FrustCameraHost_native*`), so **this class may never
 * move once shipped** — same rule as `dev.frust.FrustSurfaceView`'s exports.
 * `dev.frust` itself belongs exclusively to the `frust-embedding` module; every
 * plugin takes a subpackage (`docs/CODE_STANDARDS.md`'s Plugin Conventions).
 *
 * ## Threading
 *
 * Every static above is called over JNI from a Rust background thread (the
 * plugin never-on-the-UI-thread invariant) and **must not block**. CameraX's
 * binding APIs and [LifecycleRegistry] are main-thread-only, so each static
 * does its bookkeeping synchronously (allocating the session id, reading a
 * volatile) and posts the platform work to [mainExecutor]; results come back
 * asynchronously through the `nativeOn*` exports.
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

    /** [takePicture]/[startImageStream] error: unknown or already-closed session id. */
    private const val ERROR_UNKNOWN_SESSION = -1

    /** [startImageStream] error: not implemented yet (task 09 wires the stream). */
    private const val ERROR_NOT_IMPLEMENTED = -3

    /** [ActivityCompat.requestPermissions] request code — module-local, arbitrary. */
    private const val PERMISSION_REQUEST_CODE = 0x6672 // 'f','r'

    private const val TAG = "frust"

    // --- Process state -----------------------------------------------------

    private val mainHandler = Handler(Looper.getMainLooper())

    /**
     * Runs inline when already on the main thread, else posts. Used for every
     * CameraX call, every [LifecycleRegistry] mutation, and as the callback
     * executor handed to CameraX (`provideSurface`,
     * `setTransformationInfoListener`, `takePicture`).
     */
    internal val mainExecutor: Executor = Executor { command ->
        if (Looper.myLooper() == Looper.getMainLooper()) {
            command.run()
        } else {
            mainHandler.post(command)
        }
    }

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
     * The hosting Activity, cached from the platform-view factory seam
     * ([CameraPreviewFactory.createView]) — the only Activity handle this
     * module is given. Permission dialogs need it; [requestPermission] reports
     * [PERMISSION_NEEDS_UI] while it is null.
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
        mainExecutor.execute {
            try {
                entry.preview.setSurfaceProvider(null)
                // `unbind(useCases)`, never `unbindAll()`: another session may
                // be bound to the same process-wide provider.
                entry.provider?.unbind(entry.preview, entry.imageCapture)
                entry.setState(Lifecycle.State.DESTROYED)
            } catch (e: Throwable) {
                Log.w(TAG, "frust-camera: closeCamera($session) failed", e)
            }
            notifyCameraState(session, STATE_CLOSED)
        }
    }

    /**
     * `takePicture(int session, String path) -> int`. Returns `0` once the
     * capture has been started (completion arrives via [nativeOnPictureTaken])
     * or [ERROR_UNKNOWN_SESSION].
     */
    @JvmStatic
    fun takePicture(session: Int, path: String): Int {
        val entry = sessions[session] ?: return ERROR_UNKNOWN_SESSION
        mainExecutor.execute {
            val file = File(path)
            try {
                file.parentFile?.mkdirs()
                val options = ImageCapture.OutputFileOptions.Builder(file).build()
                entry.imageCapture.takePicture(
                    options,
                    mainExecutor,
                    object : ImageCapture.OnImageSavedCallback {
                        override fun onImageSaved(results: ImageCapture.OutputFileResults) {
                            notifyPictureTaken(session, true, path)
                        }

                        override fun onError(exception: ImageCaptureException) {
                            Log.w(TAG, "frust-camera: takePicture($session) failed", exception)
                            notifyPictureTaken(session, false, path)
                        }
                    },
                )
            } catch (e: Throwable) {
                Log.w(TAG, "frust-camera: takePicture($session) could not start", e)
                notifyPictureTaken(session, false, path)
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
     * `startImageStream(int session, int format) -> int`. **Task 09** wires the
     * real `ImageAnalysis` use case; the signature is fixed here so the
     * contract's shape is complete.
     */
    @JvmStatic
    fun startImageStream(session: Int, format: Int): Int {
        if (!sessions.containsKey(session)) return ERROR_UNKNOWN_SESSION
        Log.d(TAG, "frust-camera: startImageStream($session, $format) — not implemented (task 09)")
        return ERROR_NOT_IMPLEMENTED
    }

    /**
     * `stopImageStream(int session) -> void`. **Task 09** fills this alongside
     * [startImageStream].
     */
    @JvmStatic
    fun stopImageStream(session: Int) {
        Log.d(TAG, "frust-camera: stopImageStream($session) — not implemented (task 09)")
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

    /** Reports a [takePicture] completion. */
    @JvmStatic
    external fun nativeOnPictureTaken(session: Int, ok: Boolean, path: String)

    // --- Module-internal seam (NOT part of the JNI contract) ---------------

    /**
     * Install the application [Context] — called once at process start by
     * [FrustCameraInitProvider], before any Activity or view exists. Idempotent
     * and first-writer-wins: [cacheActivity] installs the same context as a
     * fallback if the provider was somehow stripped from the merged manifest.
     */
    internal fun installApplicationContext(context: Context) {
        if (appContext == null) appContext = context
    }

    /**
     * Cache the hosting Activity, handed to us through the platform-view
     * factory seam ([CameraPreviewFactory.createView]) — the only Activity this
     * module ever sees. The first call also registers the process-wide
     * [Application.ActivityLifecycleCallbacks] that drive session
     * pause/resume and relay a pending permission answer.
     */
    internal fun cacheActivity(activity: Activity) {
        this.activity = activity
        if (appContext == null) appContext = activity.applicationContext
        if (!lifecycleCallbacksRegistered) {
            lifecycleCallbacksRegistered = true
            activity.application.registerActivityLifecycleCallbacks(activityCallbacks)
        }
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

    /** Publish the resolved preview aspect ratio for [previewAspectRatio]. */
    internal fun setPreviewAspectRatio(sessionId: Int, ratio: Float) {
        sessions[sessionId]?.aspectRatio = ratio
    }

    /**
     * Track the display rotation the preview is being shown at (a
     * `Surface.ROTATION_*` value), so CameraX's `TransformationInfo` reports
     * the rotation that makes the buffer upright *for this display*. Sessions
     * open before any view exists, so this cannot be set at build time.
     */
    internal fun setTargetRotation(sessionId: Int, rotation: Int) {
        val entry = sessions[sessionId] ?: return
        entry.preview.targetRotation = rotation
        entry.imageCapture.targetRotation = rotation
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
                    provider.bindToLifecycle(
                        session,
                        selectorFor(session.lensFacing),
                        session.preview,
                        session.imageCapture,
                    )
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
        override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {}

        override fun onActivityStarted(activity: Activity) {}

        override fun onActivityResumed(activity: Activity) {
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

    private fun notifyPictureTaken(session: Int, ok: Boolean, path: String) {
        try {
            nativeOnPictureTaken(session, ok, path)
        } catch (e: Throwable) {
            Log.w(TAG, "frust-camera: nativeOnPictureTaken unavailable", e)
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
