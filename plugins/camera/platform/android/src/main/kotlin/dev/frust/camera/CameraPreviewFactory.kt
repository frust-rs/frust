package dev.frust.camera

import android.app.Activity
import android.content.Context
import android.graphics.Color
import android.graphics.Matrix
import android.graphics.SurfaceTexture
import android.util.Log
import android.util.Size
import android.view.Surface
import android.view.TextureView
import android.view.View
import android.widget.FrameLayout
import androidx.camera.core.Preview
import androidx.camera.core.SurfaceRequest
import dev.frust.FrustPlatformViewFactory
import org.json.JSONException
import org.json.JSONObject
import kotlin.math.max

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
 * The hosted view: a [FrameLayout] wrapping a [TextureView].
 *
 * **Why a wrapper.** The embedding's `FrustViewHost` owns the *hosted view's*
 * geometry — it writes `layoutParams`, `translationX/Y` and `clipBounds` on it
 * every frame — so the preview transform (rotation, crop, mirror, fill scale)
 * cannot live on the same view without the two fighting. The wrapper takes the
 * host's geometry; the inner [TextureView] takes the camera transform.
 *
 * **Why a TextureView, and why the crop is a content transform.** The preview
 * must never paint outside its slot, and a `SurfaceView` cannot promise that:
 * its pixels live in a compositor layer of its own whose on-screen rect follows
 * the view's own frame, so no ancestor's `clipBounds`/`clipChildren` trims
 * them. Centre-cropping a `SurfaceView` means oversizing it past the slot and
 * trusting that clip — which real devices do not honour, leaving the preview
 * spilling over surrounding chrome by the overflow of its buffer aspect. A
 * [TextureView] draws through the view hierarchy instead, so its content is
 * bounded by its own frame: it is laid out `MATCH_PARENT` here (frame == slot,
 * always) and the entire crop is a content matrix ([applyTransform]). That is
 * the shape the iOS side has always had — an `AVCaptureVideoPreviewLayer`
 * pinned to the view's bounds with `resizeAspectFill` — and it makes the
 * host's per-frame `clipBounds` real rather than advisory, so a slot half
 * scrolled out of a viewport now clips too.
 *
 * Mode B is unaffected: frust's translucent render surface still composites on
 * top and punches the slot (`docs/ARCHITECTURE.md`'s Platform-view flow), and
 * what shows through is the app window's own pixels rather than a layer below
 * it — one *fewer* transparent layer in the chain. The cost is a GPU composite
 * of the slot per camera frame inside the app window instead of a
 * SurfaceFlinger overlay, and a self-update now redraws that window region
 * rather than nothing at all — still zero *frust* frames, which is the
 * property the render loop's frame gate depends on. `PreviewView` remains
 * unused: it owns its own view hierarchy and transform policy, and defaults
 * back to a `SurfaceView`. A [TextureView] needs a hardware-accelerated
 * window; every frust Activity has one.
 *
 * **Surface lifetime.** The inner texture comes and goes with the view's
 * attachment (scrolled away, backgrounded, slot culled) while the CameraX
 * session does not. The two are reconciled with the APIs documented for exactly
 * this case: `SurfaceRequest.invalidate()` when a provided surface is destroyed
 * (CameraX then issues a fresh request to the same provider, in place, with no
 * unbind and no camera reopen), and `Preview.setSurfaceProvider(null)` — via
 * [FrustCameraHost.detachPreview] — to pause preview when the slot is disposed.
 * Ownership differs from a `SurfaceView`'s holder-owned surface: the [Surface]
 * handed to CameraX is built here over the [TextureView]'s [SurfaceTexture], so
 * this view releases it when the hand-over completes — and releases the texture
 * too if the [TextureView] gave it up while CameraX still held it.
 *
 * **Hand-over contract.** At most one hand-over is in flight: [providedSurface],
 * its [providedRequest] and the [providedTexture] it was built over move as one
 * triple, and [tryProvideSurface] starts no second one — so one texture never
 * carries two producers, and a [Surface] is offered to exactly one request. A
 * request that arrives mid-flight waits in [pendingRequest]; the result callback
 * releases the old [Surface] and then re-drives [tryProvideSurface], and being
 * the only thing that clears the gate is what keeps every arrival order from
 * wedging the slot black. Releases are identity-gated: only [providedTexture]
 * can still be read by CameraX, so it alone survives its destroy callback (as
 * [detachedTexture], freed by that same result callback) while every other
 * texture goes straight back to the [TextureView]. [requestedResolution] is
 * pinned at hand-over rather than at request arrival, so [applyTransform] undoes
 * the stretch of the buffer actually on screen — and it is only one of that
 * transform's three inputs. The other two are the request's
 * `TransformationInfo` and a non-zero slot size, which a hand-over does not
 * imply: the host mounts every slot 0×0 and publishes real geometry in a later
 * update, so geometry readiness is routinely the last input to land. The three
 * settle in any order, and each setter re-drives [applyTransform] — but a
 * setter running while another input is still missing would otherwise be the
 * last word, leaving the slot black with frames streaming into it. A miss
 * therefore arms [transformPending] and the per-frame texture-update tick
 * retries it, so the transform converges within one camera frame of the third
 * input becoming available, whatever the order, and costs one boolean read per
 * frame once it has.
 *
 * **First draw.** A [TextureView] gets its first camera frame onto the screen
 * through exactly one edge: the [SurfaceTexture]'s frame-available callback
 * marks the view's layer dirty and calls `invalidate()`, and the *window
 * traversal* that follows is what runs `updateTexImage` — and only then reports
 * [TextureView.SurfaceTextureListener.onSurfaceTextureUpdated]. `View.invalidate()`
 * is single-shot: a view already flagged invalidated returns without touching
 * its parent at all, so if that one traversal never happens, no *later* camera
 * frame can ask for another one either. The slot then stays black permanently
 * while the session streams into it perfectly happily — which is exactly the
 * shape this preview hit on device, hand-over complete and not one frame drawn.
 * An ordinary app never notices the fragility, because its own scrolls, ripples
 * and animations redraw the window constantly; frust does, because it paints
 * into its own surface on its own frame loop and never touches the view
 * hierarchy, so once a slot is mounted the window has *no other reason to
 * redraw* and a single lost redraw is forever. [armFirstFrameKick] is the belt:
 * a bounded main-thread watchdog that damages the window itself (via the
 * parent's `onDescendantInvalidated`, which — unlike `invalidate()` — is not
 * gated on the view's own dirty flags) until a frame is proven drawn, then
 * retires. It is armed per hand-over, so a re-attach gets a fresh one, and it
 * costs nothing on the happy path: the first frame normally lands inside its
 * grace period and disarms it before it ever ticks.
 *
 * All callbacks land on the main thread ([FrustCameraHost.mainExecutor] runs
 * inline there, `Preview.setSurfaceProvider(provider)`'s single-argument
 * overload dispatches on it, a [TextureView.SurfaceTextureListener] is called
 * from the view hierarchy, and [armFirstFrameKick] posts through this view), so
 * no synchronization is needed.
 */
internal class CameraPreviewView(context: Context) : FrameLayout(context), Preview.SurfaceProvider {
    private val textureView = TextureView(context)

    /** The session this slot is showing, or [NO_SESSION]. */
    private var sessionId: Int = NO_SESSION

    /** A request whose surface has not been handed over yet; it waits for one. */
    private var pendingRequest: SurfaceRequest? = null

    /** The request currently holding our surface. */
    private var providedRequest: SurfaceRequest? = null

    /** The [Surface] handed to CameraX — built here, so released here. */
    private var providedSurface: Surface? = null

    /** The texture [providedSurface] was built over: what gates its release. */
    private var providedTexture: SurfaceTexture? = null

    /** Buffer size of the handed-over surface; pinned onto its texture at hand-over. */
    private var requestedResolution: Size? = null

    /** The [TextureView]'s live texture, or null while it has none. */
    private var surfaceTexture: SurfaceTexture? = null

    /**
     * The [providedTexture] of an in-flight hand-over that the [TextureView]
     * handed back: the destroy callback answered `false`, taking on its release
     * once that hand-over completes. One slot suffices because only one
     * hand-over — hence only one texture — is ever in flight.
     */
    private var detachedTexture: SurfaceTexture? = null

    /** Latest transform info, re-applied whenever this slot is resized. */
    private var transformationInfo: SurfaceRequest.TransformationInfo? = null

    /**
     * An apply is owed: [applyTransform] last ran without all of its inputs, or
     * the buffer it computed for is gone. Only an apply that reaches
     * `setTransform` clears it, and only a camera frame on [providedTexture]
     * retries it — see the Hand-over contract.
     */
    private var transformPending = false

    /** Reused by [applyTransform]; every op below is a `set`/`post`, never additive. */
    private val transformMatrix = Matrix()

    /**
     * Whether the [TextureView] has drawn a camera frame for the CURRENT
     * hand-over. Cleared at every hand-over and set by the first
     * `onSurfaceTextureUpdated` — the only callback that proves a buffer
     * actually reached the screen, and what retires [firstFrameKick].
     */
    private var firstFrameDrawn = false

    /** Kicks [firstFrameKick] has left before it gives up; 0 = not armed. */
    private var firstFrameKicksLeft = 0

    /**
     * The first-draw watchdog (class doc's *First draw*). Each tick asks the
     * window for a redraw the [TextureView]'s own `invalidate()` can no longer
     * ask for, so a frame already sitting in the texture is consumed on the next
     * traversal instead of never.
     */
    private val firstFrameKick = object : Runnable {
        override fun run() {
            // Disarmed by everything that ends a hand-over, so these are belts
            // against a tick already in the queue when that happened.
            if (firstFrameDrawn || providedTexture == null) return
            if (firstFrameKicksLeft <= 0) {
                // Not the invalidate path failing, then: nothing is arriving on
                // this texture at all. The hand-over and transform lines above
                // say which of the two halves got as far as it should have.
                Log.w(
                    TAG,
                    "frust-camera: preview slot drew no camera frame within " +
                        "${FIRST_FRAME_GRACE_MS + FIRST_FRAME_KICKS * FIRST_FRAME_KICK_MS}ms " +
                        "of hand-over (buffer $requestedResolution, slot " +
                        "${this@CameraPreviewView.width}x${this@CameraPreviewView.height})",
                )
                return
            }
            firstFrameKicksLeft--
            // Flag the TextureView so the traversal re-records it (its draw is
            // what calls `updateTexImage`)...
            textureView.invalidate()
            // ...then damage the window unconditionally, so that traversal
            // happens. `invalidate()` alone cannot: a view already flagged
            // invalidated — precisely the wedged state — drops the call.
            this@CameraPreviewView.parent?.onDescendantInvalidated(
                this@CameraPreviewView,
                textureView,
            )
            this@CameraPreviewView.postDelayed(this, FIRST_FRAME_KICK_MS)
        }
    }

    private val textureListener = object : TextureView.SurfaceTextureListener {
        override fun onSurfaceTextureAvailable(surface: SurfaceTexture, width: Int, height: Int) {
            // Once per texture, i.e. once per attach — the head of the four-line
            // trail (available → handed over → transform applied → first frame
            // drawn) that says how far a black slot got.
            Log.d(TAG, "frust-camera: preview texture available (${width}x$height)")
            surfaceTexture = surface
            tryProvideSurface()
        }

        override fun onSurfaceTextureSizeChanged(surface: SurfaceTexture, width: Int, height: Int) {
            // A TextureView re-pins its texture's DEFAULT buffer size to the
            // view size on every resize. Once camera2 has connected it sets the
            // buffer dimensions explicitly and the default is inert, but a
            // resize landing between hand-over and that connection would leave
            // CameraX a wrongly-sized buffer — so restate the camera's own
            // resolution on the handed-over texture. Any other texture is
            // (re-)pinned by `tryProvideSurface` at its own hand-over.
            if (surface === providedTexture) {
                requestedResolution?.let { surface.setDefaultBufferSize(it.width, it.height) }
            }
        }

        override fun onSurfaceTextureDestroyed(surface: SurfaceTexture): Boolean {
            surfaceTexture = null
            val provided = providedRequest
            providedRequest = null
            // The documented path for "the view destroyed its texture while the
            // session lives on": CameraX re-issues a fresh SurfaceRequest to
            // this same provider, in place — no unbind, no camera reopen. Any
            // request outstanding here (this fresh one included) stays pending
            // until a texture returns to serve it.
            provided?.invalidate()
            // Identity, not presence: CameraX can only still be reading the
            // Surface built over THIS texture, and `detachFromSession` clears
            // the request while that read is live. Answering `false` for it
            // alone keeps it alive and hands its release to that hand-over's
            // result callback; a texture never handed over goes straight back.
            if (surface === providedTexture) {
                detachedTexture = surface
                return false
            }
            return true
        }

        override fun onSurfaceTextureUpdated(surface: SurfaceTexture) {
            // Per camera frame, and deliberately almost empty: the TextureView
            // has already scheduled its own redraw and nothing here depends on
            // frame timing. Both flags are tested first, so a converged frame
            // costs two bool reads.
            //
            // Reaching here at all is the proof the class doc's *First draw*
            // wants — `updateTexImage` ran, so a buffer is on screen — and it
            // is what retires the watchdog.
            if (!firstFrameDrawn && surface === providedTexture) {
                firstFrameDrawn = true
                disarmFirstFrameKick()
                Log.d(
                    TAG,
                    "frust-camera: first preview frame drawn (buffer $requestedResolution, " +
                        "slot ${this@CameraPreviewView.width}x${this@CameraPreviewView.height})",
                )
            }
            // The other thing that depends on this tick is a transform still
            // owed — the only tick guaranteed to keep coming while the inputs it
            // needs settle, so it retries until the transform applies.
            if (transformPending && surface === providedTexture) applyTransform()
        }
    }

    init {
        // Anything the camera does not cover reads as frame, not as a hole —
        // and in Mode B an unpainted region shows raw OS content
        // (`docs/CODE_STANDARDS.md`'s Mode B paint contract).
        setBackgroundColor(Color.BLACK)
        // MATCH_PARENT is load-bearing: the preview's frame is exactly this
        // slot, and every crop/rotation/mirror is a content transform inside
        // those bounds — never an oversized view.
        addView(textureView, LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT))
        // A transform can leave part of the bounds uncovered for a frame (before
        // the first `TransformationInfo` lands), and Android's own guidance is
        // to mark such a TextureView non-opaque; that makes the gap read as the
        // black above rather than as undefined layer content.
        textureView.setOpaque(false)
        textureView.surfaceTextureListener = textureListener
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
        // Every input this session's transform needs is still to come, and the
        // matrix on screen (if any) belongs to the previous one.
        transformPending = true
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
        // Any not-yet-served request is superseded by this one. Its resolution
        // is deliberately NOT recorded here: until the hand-over happens, the
        // buffer on screen is still the previous surface's.
        pendingRequest?.willNotProvideSurface()
        pendingRequest = request
        request.setTransformationInfoListener(FrustCameraHost.mainExecutor) { info ->
            transformationInfo = info
            applyTransform()
        }
        request.addRequestCancellationListener(FrustCameraHost.mainExecutor) {
            if (pendingRequest === request) pendingRequest = null
        }
        tryProvideSurface()
    }

    // --- Surface plumbing --------------------------------------------------

    private fun tryProvideSurface() {
        val request = pendingRequest ?: return
        val texture = surfaceTexture ?: return
        // One hand-over at a time: re-pointing a slot can raise the next request
        // before CameraX has finished with (and let us release) the previous
        // Surface, and two live producers on one texture is a stream
        // configuration failure. The result callback below re-drives this, so
        // the request waiting here is served the moment the gate clears.
        if (providedSurface != null) return

        // Identity, checked rather than assumed: the whole hand-over is
        // pointless if the camera streams into a texture the view has stopped
        // displaying, and that failure is invisible — a permanently black slot
        // with a healthy CameraX session behind it. `getSurfaceTexture` is a
        // plain field read, so this rides every hand-over rather than a debug
        // flag. Diverging is a bug in the destroy/re-provide bookkeeping below,
        // not something to paper over here, so it logs and proceeds.
        if (textureView.surfaceTexture !== texture) {
            Log.w(TAG, "frust-camera: preview hand-over texture is not the one on display")
        }

        // The camera writes at ITS resolution, so the texture must agree before
        // the Surface is built (`onSurfaceTextureSizeChanged` restates it) —
        // handing over a wrongly-sized surface is the classic
        // stretched-first-frame bug, or a rejected stream configuration.
        val resolution = request.resolution
        texture.setDefaultBufferSize(resolution.width, resolution.height)
        val surface = Surface(texture)

        // Every field settles BEFORE provideSurface: on an already-terminated
        // request the result callback runs inline from inside that call, and it
        // clears exactly what is set here.
        pendingRequest = null
        providedRequest = request
        providedSurface = surface
        providedTexture = texture
        // The buffer on screen is this one from now on, so the transform that
        // undoes its stretch is recomputed against it.
        requestedResolution = resolution
        // Same rule: armed before the call, so an inline re-drive re-arms it for
        // whatever hand-over replaces this one rather than losing the watchdog.
        armFirstFrameKick()
        Log.d(
            TAG,
            "frust-camera: preview surface handed to CameraX " +
                "(buffer ${resolution.width}x${resolution.height}, slot ${width}x$height)",
        )
        applyTransform()
        request.provideSurface(surface, FrustCameraHost.mainExecutor) { result ->
            // This view built the Surface, so this view releases it — the
            // mirror image of the SurfaceView case, where the holder owned it.
            // The texture underneath is only ours to free when the TextureView
            // already gave it up (see the destroy callback).
            surface.release()
            if (providedSurface === surface) providedSurface = null
            if (providedRequest === request) providedRequest = null
            if (providedTexture === texture) {
                providedTexture = null
                // The buffer the current matrix was computed for is gone, and
                // the retry only fires for the texture actually handed over, so
                // whatever surface replaces this one starts owing an apply.
                transformPending = true
                // Nothing left to watch for; the re-drive below arms the next
                // hand-over's own watchdog.
                disarmFirstFrameKick()
            }
            if (detachedTexture === texture) {
                detachedTexture = null
                texture.release()
            }
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
            // The gate is clear and the old texture is gone: serve whatever
            // arrived mid-flight. Without this the slot stays black forever on
            // every interleaving that raises a request before this callback —
            // a foregrounded preview, a re-pointed slot. It terminates because
            // each pass consumes `pendingRequest`.
            tryProvideSurface()
        }
    }

    private fun detachFromSession() {
        val id = sessionId
        if (id != NO_SESSION) FrustCameraHost.detachPreview(id)
        // `invalidate` runs the hand-over's result callback, which is what frees
        // the Surface (and any detached texture) — never released inline here.
        providedRequest?.invalidate()
        providedRequest = null
        pendingRequest?.willNotProvideSurface()
        pendingRequest = null
        requestedResolution = null
        transformationInfo = null
        // Nothing is owed with no inputs left to compute from; `bindSession`
        // re-arms for the next session, so the flag never crosses one.
        transformPending = false
        // Same: no hand-over left to watch, and the next one arms its own.
        disarmFirstFrameKick()
    }

    // --- First-draw watchdog -----------------------------------------------

    /**
     * Start watching this hand-over for its first drawn frame (class doc's
     * *First draw*). The grace period is what keeps the happy path free: a
     * camera that starts streaming normally reports its first frame — and
     * disarms this — before the first kick is ever due.
     */
    private fun armFirstFrameKick() {
        firstFrameDrawn = false
        firstFrameKicksLeft = FIRST_FRAME_KICKS
        removeCallbacks(firstFrameKick)
        postDelayed(firstFrameKick, FIRST_FRAME_GRACE_MS)
    }

    /** Stop the watchdog: the frame arrived, or there is no hand-over left. */
    private fun disarmFirstFrameKick() {
        firstFrameKicksLeft = 0
        removeCallbacks(firstFrameKick)
    }

    override fun onDetachedFromWindow() {
        // Nothing can draw while detached, and a re-attach destroys the texture
        // and re-hands-over — which arms a fresh watchdog of its own.
        //
        // BEFORE super, not after: `removeCallbacks` can only reach an already
        // posted tick through the view's attach info, and the detach dispatch
        // clears that on the way out. Disarming afterwards would leave the tick
        // in the main queue to fire against a detached view.
        disarmFirstFrameKick()
        super.onDetachedFromWindow()
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
     * it for a front lens, and re-centre the crop — as a [TextureView] content
     * matrix, so the fill's overflow is simply not drawn instead of spilling
     * past the slot.
     *
     * A [TextureView] draws its texture stretched across its own bounds, and
     * `setTransform`'s matrix is then applied in view coordinates. So the
     * matrix undoes that stretch first (step 1), which leaves the remaining
     * steps working in plain buffer pixels; each is a `post` op, i.e. applied
     * in the order written. Content mapped outside the view's bounds is not
     * drawn, which is exactly the centre-crop trim.
     *
     * Geometry check against a 4:3 buffer, portrait device (`rotationDegrees`
     * 90, upright 3:4): a 1:1 slot fills on width and overflows ~33% of its
     * height, trimmed evenly top and bottom; a 3:4 slot maps 1:1 with no trim;
     * a 9:16 slot fills on height and trims the sides. Landscape (rotation 0,
     * upright 4:3) mirrors that — a 16:9 slot fills on width and trims top and
     * bottom. No slot aspect letterboxes, and none can paint outside the slot.
     *
     * The matrix shape follows CameraX's own `PreviewView` transform and the
     * platform `TextureView` preview recipe; correctness lands at the device
     * gate (both orientations + front-camera mirroring).
     */
    private fun applyTransform() {
        // Armed ahead of the guards, cleared only once `setTransform` has run:
        // every return below leaves the transform owed, and the frame tick is
        // what comes back for it. An input setter therefore just calls this,
        // in whatever order its input happened to arrive.
        transformPending = true
        val info = transformationInfo ?: return
        val resolution = requestedResolution ?: return
        val slotWidth = width
        val slotHeight = height
        if (slotWidth == 0 || slotHeight == 0) return
        if (resolution.width <= 0 || resolution.height <= 0) return

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

        // 1. Undo the default buffer→bounds stretch: view px become buffer px.
        transformMatrix.setScale(
            resolution.width.toFloat() / slotWidth.toFloat(),
            resolution.height.toFloat() / slotHeight.toFloat(),
        )
        // 2. Origin on the crop centre, so every step below is about the crop.
        transformMatrix.postTranslate(-cropCenterX, -cropCenterY)
        // 3. Mirror a front lens, then rotate the buffer upright — both in
        //    buffer space, the order the previous view-transform used.
        if (info.isMirroring) transformMatrix.postScale(-1.0f, 1.0f)
        transformMatrix.postRotate(rotationDegrees.toFloat())
        // 4. Fill the slot (centre-crop) and re-centre on it.
        transformMatrix.postScale(scale, scale)
        transformMatrix.postTranslate(slotWidth / 2.0f, slotHeight / 2.0f)
        textureView.setTransform(transformMatrix)
        transformPending = false
        // `setTransform` only marks the matrix dirty and damages this view's
        // PARENT; the layer picks the matrix up on the TextureView's own next
        // draw, which — class doc's *First draw* — is exactly what nothing else
        // here guarantees. Bounded: an apply that reaches this line clears
        // `transformPending`, so the retry stops driving it.
        textureView.invalidate()
        // The inputs of the matrix actually on screen, once per convergence.
        Log.d(
            TAG,
            "frust-camera: preview transform applied (buffer " +
                "${resolution.width}x${resolution.height}, crop $crop, rotation " +
                "$rotationDegrees, mirrored ${info.isMirroring}, slot ${slotWidth}x$slotHeight)",
        )

        if (sessionId != NO_SESSION) {
            FrustCameraHost.setPreviewAspectRatio(
                sessionId,
                uprightWidth.toFloat() / uprightHeight.toFloat(),
            )
        }
    }

    private companion object {
        const val TAG = "frust"

        /**
         * How long a hand-over is left alone before the first-draw watchdog
         * starts kicking. A camera that streams normally puts its first frame up
         * well inside this, so the watchdog costs a healthy preview nothing at
         * all; it is long enough that a slow first frame does not buy a burst of
         * pointless window redraws either.
         */
        const val FIRST_FRAME_GRACE_MS = 250L

        /** Roughly two display frames between kicks — a redraw ask, not a poll. */
        const val FIRST_FRAME_KICK_MS = 33L

        /**
         * Kicks before the watchdog gives up and logs. With the two above that
         * is ~2s of watching, past which the cause is no longer a lost redraw:
         * no camera frame is reaching this texture at all.
         */
        const val FIRST_FRAME_KICKS = 53
    }
}
