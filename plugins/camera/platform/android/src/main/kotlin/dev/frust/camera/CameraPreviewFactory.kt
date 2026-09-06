package dev.frust.camera

import android.app.Activity
import android.content.Context
import android.graphics.Color
import android.hardware.display.DisplayManager
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
import kotlin.math.min
import kotlin.math.roundToInt

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
 * every frame — so the preview's own sizing cannot live on the same view
 * without the two fighting. The wrapper takes the host's geometry; the inner
 * [SurfaceView] takes the preview's.
 *
 * **Why a SurfaceView.** Its pixels live in a compositor layer of its own,
 * composited by the system *beside* the app window rather than inside it, so
 * nothing about the window's own drawing can lose them. That is the property
 * this preview needs. A `TextureView` was tried in this slot and draws through
 * the view hierarchy instead — into a window frust configures unusually (a
 * translucent window over a full-screen render surface that punches its own
 * hole through it) — and a vendor compositor was observed dropping that
 * window's TextureView content on most frames while the camera streamed into
 * it perfectly happily: a black slot with a healthy session behind it, and no
 * view-level knob (z, opacity, forced redraws) reaching the layer that was
 * being dropped. A separate compositor layer is the only shape here that does
 * not depend on the window's composition at all. `PreviewView` remains unused
 * for its own reasons: it owns a view hierarchy and a transform policy of its
 * own, and its fill scale types rely on exactly the clipping the *Geometry*
 * note below rules out.
 *
 * **Z-order: media overlay.** [SurfaceView.setZOrderMediaOverlay] puts this
 * layer above other behind-window surfaces and still below the app window —
 * the one placement that composites in both of frust's surface arrangements.
 * With an opaque render surface the frust surface is itself behind the window
 * (punching a full-screen hole through it) and `FrustViewHost` adds hosted
 * views *above* it, so the preview has to be above that surface: media-overlay
 * is. With a translucent render surface the frust surface is on top of the
 * window and hosted views go below it: media-overlay is below both, and
 * frust's own alpha hole is what reveals the preview. The default (no call)
 * would leave this layer at the same level as an opaque-mode frust surface
 * with no defined order between the two — composited or hidden by chance.
 * `setZOrderOnTop` is the other wrong answer: it lifts the preview above the
 * window *and* above a translucent frust surface, hiding whatever chrome the
 * app paints over the slot.
 *
 * **Geometry: the preview is fitted, never cropped — and that is forced.** The
 * system positions and scales this layer from the view's own frame: the frame
 * becomes the layer's on-screen rect, and the buffer is mapped onto it with a
 * positive, axis-aligned scale. Three consequences follow.
 *  1. The layer's on-screen rect **is** the view frame. No ancestor's
 *     `clipBounds`/`clipChildren` trims it — a preview laid out past its slot
 *     was observed on device spilling over the surrounding chrome. Containment
 *     has to be structural, so this inner view is never laid out larger than
 *     the slot in either axis.
 *  2. The two axes scale independently, so a frame whose aspect differs from
 *     the displayed buffer's stretches the image. Uniform scale exists only
 *     where the two agree.
 *  3. There is no content transform: a rotation, a mirror or a matrix set on
 *     the view never reaches the layer — only the frame and that positive
 *     scale do.
 *
 * (1) and (2) leave exactly one mapping that is both undistorted and
 * contained: fit the displayed buffer inside the slot and centre it — the
 * largest slot-inscribed rect carrying the displayed aspect, which is what
 * [updatePreviewLayout] computes. A centre-cropped fill needs a frame larger
 * than the slot on one axis, and (1) says nothing will trim the overflow.
 *
 * CameraX's `ViewPort` cannot rescue the fill either, because a crop rect is
 * **metadata, not a crop of the stream**: without a `CameraEffect` (whose
 * surface processor is what actually renders a cropped, upright output buffer,
 * and which this plugin does not build) the preview buffer carries the full
 * field of view and `TransformationInfo.cropRect` merely says which part of it
 * a consumer *should* show — which by (3) this consumer cannot. Every use case
 * therefore keeps its default full-field crop rect, and that is coherent by
 * construction: a still capture and an analysis frame cover exactly what the
 * preview shows. An app sizes its slot from
 * `CameraSession::preview_aspect_ratio` (published from here through
 * [FrustCameraHost.setPreviewAspectRatio]) — a slot at that aspect fits
 * exactly and shows no letterbox; any other aspect letterboxes against this
 * view's black background rather than distorting or overflowing.
 *
 * **Rotation.** The camera stream carries its own buffer transform and the
 * system compositor applies it — the same transform a `SurfaceTexture` hands a
 * `TextureView`, which device readings showed carrying the *full*
 * `TransformationInfo.rotationDegrees` and tracking whatever rotation the
 * display is currently at. It is derived from the use case's `targetRotation`,
 * which is per-session state on [FrustCameraHost] rather than something
 * CameraX re-reads on its own, so keeping that value current is the whole of
 * this view's rotation handling: [displayListener] re-pushes it for as long as
 * this view stays attached, and the fresh `TransformationInfo` that follows
 * re-runs [applyGeometry], which re-fits the inner view to the swapped
 * extents. [onSizeChanged] and [onAttachedToWindow] are belts for a rotation
 * that arrives as a resize or across a re-attach, and all of them share
 * [maybeSyncTargetRotation]'s last-sent compare, so the redundancy costs a
 * field read rather than a host round trip. `hasCameraTransform()` false —
 * only behind an effect pipeline, which this plugin never builds — would mean
 * no transform is applied at all; [applyGeometry] then fits the buffer as it
 * actually stands instead of pretending it was uprighted.
 *
 * **What this shape does not do.** A front lens reports
 * `TransformationInfo.isMirroring` and the preview convention is to mirror it;
 * consequence (3) says this view cannot, so a front preview shows unmirrored
 * (iOS's `AVCaptureVideoPreviewLayer` does mirror, so the platforms differ
 * here). The host's per-frame `clipBounds` is advisory for the same reason: a
 * slot scrolled half out of its viewport keeps painting its whole preview.
 * Both are the price of the layer that makes the preview visible at all.
 *
 * **Surface lifetime.** The inner surface comes and goes with the view's
 * attachment (scrolled away, backgrounded, slot culled) while the CameraX
 * session does not. The two are reconciled with the APIs documented for
 * exactly this case: `SurfaceRequest.invalidate()` when a provided surface is
 * destroyed (CameraX then issues a fresh request to the same provider, in
 * place, with no unbind and no camera reopen), and
 * `Preview.setSurfaceProvider(null)` — via [FrustCameraHost.detachPreview] —
 * to pause preview when the slot is disposed. The [SurfaceHolder] owns the
 * `Surface`, so this view never releases one: it only stops offering it.
 *
 * **Hand-over contract.** At most one hand-over is live: [providedRequest]
 * gates [tryProvideSurface], and only its result callback — CameraX reporting
 * it has finished with the surface — clears that gate and re-drives, so one
 * surface never carries two producers and a request that arrives mid-flight is
 * served the moment the previous one ends rather than wedging the slot black.
 * A request arriving while the surface is unusable simply waits in
 * [pendingRequest]; `surfaceChanged` re-drives it.
 *
 * **Buffer size.** `SurfaceHolder.setFixedSize` pins the buffer to the
 * resolution CameraX asked for, and the hand-over waits for `surfaceChanged`
 * to report *that exact size*: offering a surface whose buffer is still the
 * previous size is the classic stretched-first-frame bug, and camera2 refuses
 * a stream configured at a size it never offered. The frame that buffer is
 * scaled onto is a separate matter, owned by [updatePreviewLayout].
 *
 * **Diagnostics.** The trail is surface created → buffer pinned and handed to
 * CameraX → geometry applied, and there is deliberately no first-frame line:
 * this layer's frames travel from the camera to the compositor without passing
 * through this process, so nothing here could honestly claim one was drawn
 * (`surfaceChanged` reports configuration, not content). A slot still black
 * after the geometry line therefore has a live, correctly-sized surface, and
 * the fault is below this view — composition or the session itself — which is
 * the split those lines exist to make.
 *
 * All callbacks land on the main thread ([FrustCameraHost.mainExecutor] runs
 * inline there, `Preview.setSurfaceProvider(provider)`'s single-argument
 * overload dispatches on it, a [SurfaceHolder.Callback] is called from the view
 * hierarchy, and [displayListener] is registered against this view's own
 * [Handler][android.os.Handler]), so no synchronization is needed.
 */
internal class CameraPreviewView(context: Context) : FrameLayout(context), Preview.SurfaceProvider {
    private val surfaceView = SurfaceView(context)

    /** The session this slot is showing, or [NO_SESSION]. */
    private var sessionId: Int = NO_SESSION

    /**
     * Non-null while attached ([onAttachedToWindow] registers [displayListener]
     * against it, [onDetachedFromWindow] unregisters and clears it) — the
     * source [displayListener] rides for the rest of this slot's rotation
     * tracking.
     */
    private var displayManager: DisplayManager? = null

    /**
     * The last `Surface.ROTATION_*` value [maybeSyncTargetRotation] sent to
     * [FrustCameraHost] for the [sessionId] currently bound, or null before
     * anything has been sent for it. Every rotation-tracking path (bind,
     * attach, [displayListener], a resize) compares against this rather than
     * resending unconditionally.
     */
    private var lastSentRotation: Int? = null

    /** A request whose surface has not been handed over yet; it waits for one. */
    private var pendingRequest: SurfaceRequest? = null

    /** The request currently holding our surface. */
    private var providedRequest: SurfaceRequest? = null

    /**
     * Size the holder last reported. The surface is only handed over once this
     * matches the buffer size pinned for the request being served (class doc's
     * *Buffer size*).
     */
    private var surfaceSize: Size? = null

    /** Whether the holder currently has a live surface. */
    private var surfaceValid: Boolean = false

    /** Latest transform info; [applyGeometry]'s first input. */
    private var transformationInfo: SurfaceRequest.TransformationInfo? = null

    /** The buffer size [transformationInfo]'s crop rect is expressed in. */
    private var infoResolution: Size? = null

    /**
     * Extents of what the compositor actually displays — the crop rect swapped
     * by the rotation the buffer transform applies — or `0` before the first
     * `TransformationInfo`. [updatePreviewLayout] fits these inside the slot.
     */
    private var contentWidth: Int = 0

    /** Height half of [contentWidth]'s pair; the two are always set together. */
    private var contentHeight: Int = 0

    /**
     * Registered against [displayManager] for the lifetime of this view's
     * attach (class doc's *Rotation*). `registerDisplayListener` is
     * process-wide, not scoped to one display, so every callback re-checks
     * `displayId` against [display] before doing anything — a change on some
     * other display (a different window, a detached HDMI output) is not this
     * slot's rotation.
     */
    private val displayListener = object : DisplayManager.DisplayListener {
        override fun onDisplayAdded(displayId: Int) = Unit

        override fun onDisplayRemoved(displayId: Int) = Unit

        override fun onDisplayChanged(displayId: Int) {
            if (displayId == display?.displayId) maybeSyncTargetRotation()
        }
    }

    private val holderCallback = object : SurfaceHolder.Callback {
        override fun surfaceCreated(holder: SurfaceHolder) {
            // Nothing usable yet: `surfaceChanged` always follows with the real
            // size, and only a size matching the requested resolution is one
            // this view may hand over. Head of the class doc's diagnostic trail.
            Log.d(
                TAG,
                "frust-camera: preview surface created (slot ${width}x$height, view " +
                    "${surfaceView.width}x${surfaceView.height})",
            )
        }

        override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
            surfaceValid = true
            surfaceSize = Size(width, height)
            tryProvideSurface()
        }

        override fun surfaceDestroyed(holder: SurfaceHolder) {
            surfaceValid = false
            surfaceSize = null
            // The documented path for "the view lost its surface while the
            // session lives on": CameraX re-issues a fresh SurfaceRequest to
            // this same provider, in place — no unbind, no camera reopen. Any
            // request outstanding here (that fresh one included) waits in
            // `pendingRequest` until a surface returns to serve it, which is
            // what makes a scrolled-away-and-back slot cheap.
            val provided = providedRequest
            providedRequest = null
            provided?.invalidate()
        }
    }

    init {
        // Anything the camera does not cover reads as frame, not as a hole —
        // and in Mode B an unpainted region shows raw OS content
        // (`docs/CODE_STANDARDS.md`'s Mode B paint contract). It is also what
        // the fitted preview's letterbox shows (class doc's *Geometry*).
        setBackgroundColor(Color.BLACK)
        // Before the surface exists, which is what this call requires: the
        // layer's level is fixed at creation (class doc's *Z-order*).
        surfaceView.setZOrderMediaOverlay(true)
        // Starts slot-sized so a surface exists (and a hand-over can complete)
        // before any geometry is known; the first `TransformationInfo` fits it
        // properly. CENTER is the gravity every later size relies on.
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
        // will be shown on; tell it now (and again for as long as this slot
        // stays attached — see `maybeSyncTargetRotation`). Forced: the session
        // just bound has its own server-side `targetRotation`, defaulted to
        // `ROTATION_0` independent of whatever this view last sent for a
        // previous one, so a rebind must resend even when the display's own
        // rotation hasn't changed.
        maybeSyncTargetRotation(force = true)
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

    // --- Rotation tracking ---------------------------------------------------

    /**
     * Re-read [display]'s rotation and, if it differs from [lastSentRotation]
     * (or [force] skips that compare), forward it to
     * [FrustCameraHost.setTargetRotation] for the currently bound session.
     * A no-op with nothing bound ([NO_SESSION]) or no known display yet.
     *
     * Every caller shares this one compare so the several redundant triggers
     * (class doc's *Rotation*: [displayListener], [onSizeChanged],
     * [onAttachedToWindow], [bindSession]) collapse into at most one host round
     * trip per actual rotation change.
     */
    private fun maybeSyncTargetRotation(force: Boolean = false) {
        val id = sessionId
        if (id == NO_SESSION) return
        val rotation = display?.rotation ?: return
        if (!force && rotation == lastSentRotation) return
        lastSentRotation = rotation
        FrustCameraHost.setTargetRotation(id, rotation)
    }

    // --- Preview.SurfaceProvider -------------------------------------------

    override fun onSurfaceRequested(request: SurfaceRequest) {
        // Any not-yet-served request is superseded by this one.
        pendingRequest?.willNotProvideSurface()
        pendingRequest = request
        request.setTransformationInfoListener(FrustCameraHost.mainExecutor) { info ->
            transformationInfo = info
            // The buffer this info's crop rect is expressed in — taken from the
            // request that carried it rather than from whatever is handed over,
            // so the two can never be read against each other.
            infoResolution = request.resolution
            applyGeometry()
        }
        request.addRequestCancellationListener(FrustCameraHost.mainExecutor) {
            if (pendingRequest === request) pendingRequest = null
        }
        // The camera writes at ITS resolution, so the buffer is pinned before
        // any hand-over can match it (class doc's *Buffer size*).
        surfaceView.holder.setFixedSize(request.resolution.width, request.resolution.height)
        tryProvideSurface()
    }

    // --- Surface plumbing --------------------------------------------------

    private fun tryProvideSurface() {
        val request = pendingRequest ?: return
        // One hand-over at a time: re-pointing a slot can raise the next
        // request before CameraX has finished with the previous surface, and
        // two live producers on one surface is a stream configuration failure.
        // The result callback below re-drives this, so the request waiting here
        // is served the moment the gate clears.
        if (providedRequest != null) return
        val size = surfaceSize ?: return
        if (!surfaceValid) return
        val resolution = request.resolution
        // Wait for the holder to actually report the fixed size asked for
        // (class doc's *Buffer size*).
        if (size.width != resolution.width || size.height != resolution.height) return
        val surface = surfaceView.holder.surface
        if (surface == null || !surface.isValid) return

        // Every field settles BEFORE provideSurface: on an already-terminated
        // request the result callback runs inline from inside that call, and it
        // clears exactly what is set here.
        pendingRequest = null
        providedRequest = request
        Log.d(
            TAG,
            "frust-camera: preview surface handed to CameraX " +
                "(buffer ${resolution.width}x${resolution.height}, slot ${width}x$height, " +
                "view ${surfaceView.width}x${surfaceView.height})",
        )
        request.provideSurface(surface, FrustCameraHost.mainExecutor) { result ->
            // The holder owns this Surface — never released here, whatever the
            // outcome (class doc's *Surface lifetime*).
            if (providedRequest === request) providedRequest = null
            // All five documented codes are handled.
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
            // The gate is clear: serve whatever arrived mid-flight. Without
            // this the slot stays black forever on every interleaving that
            // raises a request before this callback — a foregrounded preview, a
            // re-pointed slot. It terminates because each pass consumes
            // `pendingRequest`.
            tryProvideSurface()
        }
    }

    private fun detachFromSession() {
        val id = sessionId
        if (id != NO_SESSION) FrustCameraHost.detachPreview(id)
        providedRequest?.invalidate()
        providedRequest = null
        pendingRequest?.willNotProvideSurface()
        pendingRequest = null
        transformationInfo = null
        infoResolution = null
        // The next session publishes its own extents; until it does, the inner
        // view falls back to the slot's own size (class doc's *Geometry*).
        contentWidth = 0
        contentHeight = 0
    }

    // --- Geometry ----------------------------------------------------------

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        // A slot resize is also how a device rotation reaches us on layouts
        // that actually resize on one (the host rewrites every slot's
        // layoutParams from the new frust layout) — a belt beside
        // `displayListener`, which is what covers a slot whose bounds never
        // change size on a rotation (a square slot).
        maybeSyncTargetRotation()
        updatePreviewLayout()
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        // Registered for as long as this view stays attached (class doc's
        // *Rotation*) — the cover for a rotation with no accompanying resize,
        // which nothing else re-reads `display.rotation` for. Passing this
        // view's own Handler keeps the callback on the main thread this whole
        // class already assumes.
        val manager = context.getSystemService(DisplayManager::class.java)
        displayManager = manager
        manager?.registerDisplayListener(displayListener, handler)
        // A session opened before any view existed has no idea what display it
        // will be shown on, and a re-attach (e.g. this slot scrolled off-screen
        // and back) may follow a rotation that happened while `displayListener`
        // was unregistered — forced for the same reason `bindSession` forces
        // it.
        maybeSyncTargetRotation(force = true)
    }

    override fun onDetachedFromWindow() {
        // Balanced with `onAttachedToWindow`'s register: a detached slot has no
        // bounds to be rotated in, and leaving this registered would leak one
        // listener per detach/reattach cycle (class doc's *Rotation*).
        displayManager?.unregisterDisplayListener(displayListener)
        displayManager = null
        super.onDetachedFromWindow()
    }

    /**
     * Recompute what the compositor will actually put on screen from the
     * latest `TransformationInfo`, re-fit the inner [SurfaceView] to it, and
     * publish the displayed aspect ratio for `previewAspectRatio`.
     *
     * The rotation term is the one the buffer transform already applies (class
     * doc's *Rotation*): a quarter turn swaps the crop's extents on screen, and
     * nothing here can add to or undo it. With no camera transform on the
     * buffer at all — only behind an effect pipeline, which this plugin never
     * builds — the buffer is displayed as it stands and the crop's own extents
     * are what shows.
     */
    private fun applyGeometry() {
        val info = transformationInfo ?: return
        val resolution = infoResolution ?: return
        if (resolution.width <= 0 || resolution.height <= 0) return

        val crop = info.cropRect
        val cropWidth = if (crop.width() > 0) crop.width() else resolution.width
        val cropHeight = if (crop.height() > 0) crop.height() else resolution.height

        val rotationDegrees = normalizedDegrees(info.rotationDegrees)
        val streamDegrees = if (info.hasCameraTransform()) rotationDegrees else 0
        val quarterTurned = streamDegrees == 90 || streamDegrees == 270
        val displayedWidth = if (quarterTurned) cropHeight else cropWidth
        val displayedHeight = if (quarterTurned) cropWidth else cropHeight
        if (displayedWidth <= 0 || displayedHeight <= 0) return

        contentWidth = displayedWidth
        contentHeight = displayedHeight
        updatePreviewLayout()

        // The inputs AND the resulting fit, once per convergence: `displayed`
        // is what a wrongly-placed preview is measured against, `view` is the
        // frame the compositor scales the buffer onto, and `display` is the
        // rotation the buffer transform is tracking — the value a frozen
        // `targetRotation` would show staying constant across a live rotation.
        Log.d(
            TAG,
            "frust-camera: preview geometry (buffer ${resolution.width}x${resolution.height}, " +
                "crop $crop, rotation $rotationDegrees of which the stream applies " +
                "$streamDegrees, mirroring ${info.isMirroring} (not applied), displayed " +
                "${displayedWidth}x$displayedHeight, view " +
                "${surfaceView.layoutParams.width}x${surfaceView.layoutParams.height}, slot " +
                "${width}x$height, display=${display?.rotation})",
        )

        if (sessionId != NO_SESSION) {
            FrustCameraHost.setPreviewAspectRatio(
                sessionId,
                displayedWidth.toFloat() / displayedHeight.toFloat(),
            )
        }
    }

    /**
     * Size the inner [SurfaceView] to the largest slot-inscribed rect carrying
     * the displayed content's aspect ratio, centred — the fit the class doc's
     * *Geometry* note derives. Before any geometry is known the slot's own size
     * is used, which is what lets a surface exist (and a hand-over complete)
     * ahead of the first `TransformationInfo`.
     *
     * Neither extent can exceed the slot's, so this view's frame — which is
     * also its compositor layer's on-screen rect — is contained by
     * construction.
     */
    private fun updatePreviewLayout() {
        val slotWidth = width
        val slotHeight = height
        if (slotWidth <= 0 || slotHeight <= 0) return

        val viewWidth: Int
        val viewHeight: Int
        if (contentWidth > 0 && contentHeight > 0) {
            val scale = min(
                slotWidth.toFloat() / contentWidth.toFloat(),
                slotHeight.toFloat() / contentHeight.toFloat(),
            )
            viewWidth = (contentWidth * scale).roundToInt().coerceIn(1, slotWidth)
            viewHeight = (contentHeight * scale).roundToInt().coerceIn(1, slotHeight)
        } else {
            viewWidth = slotWidth
            viewHeight = slotHeight
        }

        val lp = surfaceView.layoutParams as LayoutParams
        if (lp.width != viewWidth || lp.height != viewHeight) {
            lp.width = viewWidth
            lp.height = viewHeight
            // Assigning is what schedules the layout pass; the compare above is
            // what keeps a steady preview free of one.
            surfaceView.layoutParams = lp
        }
    }

    /** Clockwise degrees in `[0, 360)`, for a value that may be either sign. */
    private fun normalizedDegrees(degrees: Int): Int = ((degrees % 360) + 360) % 360

    private companion object {
        const val TAG = "frust"
    }
}
