package dev.frust.camera

import android.app.Activity
import android.content.Context
import android.graphics.Color
import android.util.Log
import android.util.Size
import android.view.Gravity
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.View
import android.widget.FrameLayout
import androidx.camera.core.Preview
import androidx.camera.core.SurfaceRequest
import dev.frust.FrustPlatformViewFactory
import org.json.JSONException
import org.json.JSONObject
import kotlin.math.cos
import kotlin.math.max
import kotlin.math.sin

/**
 * The `frust-camera` plugin's platform-view factory: hosts a live CameraX
 * preview for a session opened through [FrustCameraHost].
 *
 * The fully-qualified class name `dev.frust.camera.CameraPreviewFactory` **is**
 * the `viewType` string — it is what `CameraSession::preview_view_type()`
 * returns on Android and what the embedding's `FrustViewHost` resolves through
 * the application classloader (`docs/CODE_STANDARDS.md`'s platform-view factory
 * LAW: a `dev.frust.`-prefixed FQCN, a public no-arg constructor, all methods
 * on the main thread and non-blocking).
 *
 * `paramsJson` carries the session id — `{"sessionId":N}`, the shape
 * `AndroidSession::params_json` (`plugins/camera/src/android.rs`) emits; a
 * `{"session":N}` spelling is accepted as an alias so the two sides can never
 * drift into a silent black preview.
 */
class CameraPreviewFactory : FrustPlatformViewFactory {
    override fun createView(activity: Activity, context: Context, paramsJson: String): View {
        // The only Activity handle this plugin ever gets — permission dialogs
        // and the pause/resume lifecycle hook both come from here.
        FrustCameraHost.cacheActivity(activity)
        val view = CameraPreviewView(context)
        view.bindSession(parseSessionId(paramsJson))
        return view
    }

    override fun updateParams(view: View, paramsJson: String) {
        (view as? CameraPreviewView)?.bindSession(parseSessionId(paramsJson))
    }

    override fun disposeView(view: View) {
        // Detaches the surface only: the session, its use cases and the open
        // camera device all survive, so a revived slot re-attaches without a
        // camera reopen.
        (view as? CameraPreviewView)?.release()
    }

    private fun parseSessionId(paramsJson: String): Int {
        if (paramsJson.isEmpty()) return NO_SESSION
        return try {
            val obj = JSONObject(paramsJson)
            when {
                obj.has(PARAM_SESSION_ID) -> obj.optInt(PARAM_SESSION_ID, NO_SESSION)
                obj.has(PARAM_SESSION) -> obj.optInt(PARAM_SESSION, NO_SESSION)
                else -> NO_SESSION
            }
        } catch (e: JSONException) {
            Log.w(TAG, "frust-camera: unparseable preview params '$paramsJson'", e)
            NO_SESSION
        }
    }

    private companion object {
        const val TAG = "frust"
        const val PARAM_SESSION_ID = "sessionId"
        const val PARAM_SESSION = "session"
    }
}

/** No session bound — the slot renders as an empty (black) rectangle. */
private const val NO_SESSION = -1

/**
 * The hosted view: a [FrameLayout] wrapping a plain [SurfaceView].
 *
 * **Why a wrapper.** The embedding's `FrustViewHost` owns the *hosted view's*
 * geometry — it writes `layoutParams`, `translationX/Y` and `clipBounds` on it
 * every frame — so the preview transform (rotation, crop, mirror, fill scale)
 * cannot live on the same view without the two fighting. The wrapper takes the
 * host's geometry; the inner [SurfaceView] takes the camera transform. This is
 * the same split `PreviewView` uses internally.
 *
 * **Why a plain SurfaceView, not `PreviewView`.** A `SurfaceView` is
 * behind-window by default, which is exactly Mode B's bottom layer: frust's own
 * translucent render surface calls `setZOrderOnTop(true)` and composites over
 * it (`docs/ARCHITECTURE.md`'s Platform-view flow). It also self-updates at
 * camera rate with zero frust frames.
 *
 * **Surface lifetime.** The inner surface comes and goes with the view's
 * attachment (scrolled away, backgrounded, slot culled) while the CameraX
 * session does not. The two are reconciled with the APIs documented for exactly
 * this case: `SurfaceRequest.invalidate()` when a provided surface is destroyed
 * (CameraX then issues a fresh request to the same provider, in place, with no
 * unbind and no camera reopen), and `Preview.setSurfaceProvider(null)` — via
 * [FrustCameraHost.detachPreview] — to pause preview when the slot is disposed.
 *
 * All callbacks land on the main thread ([FrustCameraHost.mainExecutor] runs
 * inline there and `Preview.setSurfaceProvider(provider)`'s single-argument
 * overload dispatches on it), so no synchronization is needed.
 */
internal class CameraPreviewView(context: Context) : FrameLayout(context), Preview.SurfaceProvider {
    private val surfaceView = SurfaceView(context)

    /** The session this slot is showing, or [NO_SESSION]. */
    private var sessionId: Int = NO_SESSION

    /** A request whose surface has not been handed over yet. */
    private var pendingRequest: SurfaceRequest? = null

    /** The request currently holding our surface. */
    private var providedRequest: SurfaceRequest? = null

    /** Buffer size CameraX asked for (the inner view is laid out 1:1 with it). */
    private var requestedResolution: Size? = null

    /** Size the holder last reported; the surface is only usable when it matches. */
    private var surfaceSize: Size? = null

    private var surfaceValid: Boolean = false

    /** Latest transform info, re-applied whenever this slot is resized. */
    private var transformationInfo: SurfaceRequest.TransformationInfo? = null

    private val holderCallback = object : SurfaceHolder.Callback {
        override fun surfaceCreated(holder: SurfaceHolder) {
            // Nothing yet: `surfaceChanged` always follows with the real size,
            // and only a size matching the requested resolution is usable.
        }

        override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
            surfaceValid = true
            surfaceSize = Size(width, height)
            tryProvideSurface()
        }

        override fun surfaceDestroyed(holder: SurfaceHolder) {
            surfaceValid = false
            surfaceSize = null
            val provided = providedRequest
            providedRequest = null
            if (provided != null) {
                // The documented path for "the SurfaceView destroyed its
                // surface while the session lives on": CameraX re-issues a
                // fresh SurfaceRequest to this same provider, in place — no
                // unbind, no camera reopen. Held pending until a surface
                // returns.
                provided.invalidate()
            } else {
                pendingRequest?.willNotProvideSurface()
                pendingRequest = null
            }
        }
    }

    init {
        // Anything the camera does not cover reads as frame, not as a hole —
        // and in Mode B an unpainted region shows raw OS content
        // (`docs/CODE_STANDARDS.md`'s Mode B paint contract).
        setBackgroundColor(Color.BLACK)
        addView(
            surfaceView,
            LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT, Gravity.CENTER),
        )
        surfaceView.holder.addCallback(holderCallback)
    }

    /**
     * Point this slot at [id] (a `createView`/`updateParams` params change).
     * Re-pointing detaches the old session's provider first; CameraX then
     * issues a fresh `SurfaceRequest` for the new one without unbinding either.
     */
    fun bindSession(id: Int) {
        if (id == sessionId) return
        detachFromSession()
        sessionId = id
        if (id == NO_SESSION) return
        // A session opened before any view existed has no idea what display it
        // will be shown on; tell it now (and again on every resize).
        display?.let { FrustCameraHost.setTargetRotation(id, it.rotation) }
        if (!FrustCameraHost.attachPreview(id, this)) {
            Log.w(TAG, "frust-camera: preview slot bound to unknown session $id")
            sessionId = NO_SESSION
        }
    }

    /** Slot disposed: drop the surface, leave the session running. */
    fun release() {
        detachFromSession()
        sessionId = NO_SESSION
    }

    // --- Preview.SurfaceProvider -------------------------------------------

    override fun onSurfaceRequested(request: SurfaceRequest) {
        // Any not-yet-served request is superseded by this one.
        pendingRequest?.willNotProvideSurface()
        pendingRequest = request
        requestedResolution = request.resolution
        request.setTransformationInfoListener(FrustCameraHost.mainExecutor) { info ->
            transformationInfo = info
            applyTransform()
        }
        request.addRequestCancellationListener(FrustCameraHost.mainExecutor) {
            if (pendingRequest === request) pendingRequest = null
        }
        applyResolution(request.resolution)
        tryProvideSurface()
    }

    // --- Surface plumbing --------------------------------------------------

    /**
     * Lay the inner [SurfaceView] out 1:1 with the camera buffer and pin the
     * buffer size, so the transform math below can treat view-local coordinates
     * as buffer coordinates.
     */
    private fun applyResolution(resolution: Size) {
        val lp = surfaceView.layoutParams as LayoutParams
        if (lp.width != resolution.width || lp.height != resolution.height) {
            lp.width = resolution.width
            lp.height = resolution.height
            lp.gravity = Gravity.CENTER
            surfaceView.layoutParams = lp
        }
        surfaceView.holder.setFixedSize(resolution.width, resolution.height)
    }

    private fun tryProvideSurface() {
        val request = pendingRequest ?: return
        val resolution = requestedResolution ?: return
        val size = surfaceSize
        if (!surfaceValid || size == null) return
        // Wait for the holder to actually report the fixed size we asked for —
        // handing over a surface whose buffer is still the old size is the
        // classic stretched-first-frame bug.
        if (size.width != resolution.width || size.height != resolution.height) return
        val surface = surfaceView.holder.surface
        if (surface == null || !surface.isValid) return

        pendingRequest = null
        providedRequest = request
        request.provideSurface(surface, FrustCameraHost.mainExecutor) { result ->
            if (providedRequest === request) providedRequest = null
            // The SurfaceView owns this Surface — never release it here,
            // whatever the outcome. All five documented codes are handled.
            when (result.resultCode) {
                SurfaceRequest.Result.RESULT_SURFACE_USED_SUCCESSFULLY ->
                    Log.d(TAG, "frust-camera: preview surface released by CameraX (used)")
                SurfaceRequest.Result.RESULT_REQUEST_CANCELLED ->
                    Log.d(TAG, "frust-camera: preview surface request cancelled")
                SurfaceRequest.Result.RESULT_INVALID_SURFACE ->
                    Log.w(TAG, "frust-camera: preview surface rejected as invalid")
                SurfaceRequest.Result.RESULT_SURFACE_ALREADY_PROVIDED ->
                    Log.w(TAG, "frust-camera: preview surface already provided to this request")
                SurfaceRequest.Result.RESULT_WILL_NOT_PROVIDE_SURFACE ->
                    Log.d(TAG, "frust-camera: preview surface will not be provided")
                else ->
                    Log.w(TAG, "frust-camera: unknown SurfaceRequest result ${result.resultCode}")
            }
        }
    }

    private fun detachFromSession() {
        val id = sessionId
        if (id != NO_SESSION) FrustCameraHost.detachPreview(id)
        providedRequest?.invalidate()
        providedRequest = null
        pendingRequest?.willNotProvideSurface()
        pendingRequest = null
        requestedResolution = null
        transformationInfo = null
    }

    // --- Transform ---------------------------------------------------------

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        // A slot resize is also how a device rotation reaches us (the host
        // rewrites every slot's layoutParams from the new frust layout).
        if (sessionId != NO_SESSION) {
            display?.let { FrustCameraHost.setTargetRotation(sessionId, it.rotation) }
        }
        applyTransform()
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        if (sessionId != NO_SESSION) {
            display?.let { FrustCameraHost.setTargetRotation(sessionId, it.rotation) }
        }
    }

    /**
     * Map the camera buffer's crop rect onto this slot: rotate it upright,
     * scale it to FILL the slot (centre-crop, the preview convention), mirror
     * it for a front lens, and re-centre the crop.
     *
     * The inner [SurfaceView] is laid out at exactly the buffer size and
     * centred, so its local coordinates *are* buffer pixels and its pivot sits
     * on the slot's centre. Android composes a view's transform as
     * `translate ∘ rotate ∘ scale` about that pivot, so mapping the crop centre
     * `c` onto the slot centre needs `translation = −R·S·(c − bufferCentre)`.
     *
     * ⚠️ The documented `setPolyToPoly` recipe covers `ImageAnalysis` →
     * `PreviewView` only; for a plain `SurfaceView` the reference is CameraX's
     * own core test app. Correctness lands at the device gate (both
     * orientations + front-camera mirroring).
     */
    private fun applyTransform() {
        val info = transformationInfo ?: return
        val resolution = requestedResolution ?: return
        val slotWidth = width
        val slotHeight = height
        if (slotWidth == 0 || slotHeight == 0) return

        val crop = info.cropRect
        val cropWidth = if (crop.width() > 0) crop.width() else resolution.width
        val cropHeight = if (crop.height() > 0) crop.height() else resolution.height
        val cropCenterX =
            if (crop.width() > 0) crop.exactCenterX() else resolution.width / 2.0f
        val cropCenterY =
            if (crop.height() > 0) crop.exactCenterY() else resolution.height / 2.0f

        val rotationDegrees = ((info.rotationDegrees % 360) + 360) % 360
        val quarterTurned = rotationDegrees == 90 || rotationDegrees == 270
        val uprightWidth = if (quarterTurned) cropHeight else cropWidth
        val uprightHeight = if (quarterTurned) cropWidth else cropHeight
        if (uprightWidth <= 0 || uprightHeight <= 0) return

        val scale = max(
            slotWidth.toFloat() / uprightWidth.toFloat(),
            slotHeight.toFloat() / uprightHeight.toFloat(),
        )
        val mirror = if (info.isMirroring) -1.0f else 1.0f

        // Offset of the crop centre from the buffer centre, in buffer px,
        // scaled and mirrored (scale is applied before rotation).
        val offsetX = scale * mirror * (cropCenterX - resolution.width / 2.0f)
        val offsetY = scale * (cropCenterY - resolution.height / 2.0f)
        val radians = Math.toRadians(rotationDegrees.toDouble())
        val cosR = cos(radians).toFloat()
        val sinR = sin(radians).toFloat()

        surfaceView.pivotX = resolution.width / 2.0f
        surfaceView.pivotY = resolution.height / 2.0f
        surfaceView.rotation = rotationDegrees.toFloat()
        surfaceView.scaleX = scale * mirror
        surfaceView.scaleY = scale
        surfaceView.translationX = -(offsetX * cosR - offsetY * sinR)
        surfaceView.translationY = -(offsetX * sinR + offsetY * cosR)

        if (sessionId != NO_SESSION) {
            FrustCameraHost.setPreviewAspectRatio(
                sessionId,
                uprightWidth.toFloat() / uprightHeight.toFloat(),
            )
        }
    }

    private companion object {
        const val TAG = "frust"
    }
}
