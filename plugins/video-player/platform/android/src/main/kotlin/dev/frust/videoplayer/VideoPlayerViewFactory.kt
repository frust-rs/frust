package dev.frust.videoplayer

import android.app.Activity
import android.content.Context
import android.graphics.Color
import android.util.Log
import android.view.Gravity
import android.view.SurfaceView
import android.view.View
import android.widget.FrameLayout
import dev.frust.FrustPlatformViewFactory
import org.json.JSONException
import org.json.JSONObject
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/**
 * The `frust-video-player` plugin's platform-view factory: hosts the picture of
 * a session opened through [FrustVideoPlayerHost].
 *
 * The fully-qualified class name `dev.frust.videoplayer.VideoPlayerViewFactory`
 * **is** the `viewType` string — it is what `PlayerSession::view_type()`
 * returns on Android (`contract::ANDROID_VIEW_TYPE` in
 * `plugins/video-player/src/lib.rs`) and what the embedding's `FrustViewHost`
 * resolves through the application classloader
 * (`docs/PLUGINS_CODE_STANDARDS.md`'s platform-view factory LAW: a
 * `dev.frust.`-prefixed FQCN, a public no-arg constructor, all methods on the
 * main thread and non-blocking).
 *
 * `paramsJson` carries the session id and the fit —
 * `{"sessionId":N,"fit":"contain"|"cover"}`, the shape
 * `PlayerSession::params_json` emits; a `{"session":N}` spelling is accepted as
 * an alias so the two sides can never drift into a silent black picture, and an
 * absent or unrecognised `fit` reads as `contain` (the Rust-side default).
 *
 * No method here throws: a parse failure logs and yields an unbound slot (a
 * plain black rectangle), which is also what an unknown session id produces.
 */
class VideoPlayerViewFactory : FrustPlatformViewFactory {
    override fun createView(activity: Activity, context: Context, paramsJson: String): View {
        // Only used as a fallback source of the application Context; the
        // Activity itself is not retained (FrustVideoPlayerHost's class doc).
        FrustVideoPlayerHost.cacheActivity(activity)
        return try {
            val view = VideoPlayerView(context)
            val params = parseParams(paramsJson)
            view.bind(params.sessionId, params.fit)
            view
        } catch (e: Throwable) {
            Log.w(TAG, "frust-video-player: createView failed, showing a placeholder", e)
            placeholder(context)
        }
    }

    override fun updateParams(view: View, paramsJson: String) {
        val target = view as? VideoPlayerView ?: return
        try {
            val params = parseParams(paramsJson)
            target.bind(params.sessionId, params.fit)
        } catch (e: Throwable) {
            Log.w(TAG, "frust-video-player: updateParams failed", e)
        }
    }

    override fun disposeView(view: View) {
        // Detaches the surface only: the session, its ExoPlayer and its audio
        // all survive (FrustVideoPlayerHost's *Slot lifetime vs session
        // lifetime*), so a revived slot re-attaches without reopening anything.
        try {
            (view as? VideoPlayerView)?.release()
        } catch (e: Throwable) {
            Log.w(TAG, "frust-video-player: disposeView failed", e)
        }
    }

    /** What a slot needs from `paramsJson`: which session, shown how. */
    private class Params(val sessionId: Int, val fit: VideoFit)

    private fun parseParams(paramsJson: String): Params {
        if (paramsJson.isEmpty()) return Params(NO_SESSION, VideoFit.CONTAIN)
        return try {
            val obj = JSONObject(paramsJson)
            val sessionId = when {
                obj.has(PARAM_SESSION_ID) -> obj.optInt(PARAM_SESSION_ID, NO_SESSION)
                obj.has(PARAM_SESSION) -> obj.optInt(PARAM_SESSION, NO_SESSION)
                else -> NO_SESSION
            }
            Params(sessionId, VideoFit.parse(obj.optString(PARAM_FIT)))
        } catch (e: JSONException) {
            Log.w(TAG, "frust-video-player: unparseable params '$paramsJson'", e)
            Params(NO_SESSION, VideoFit.CONTAIN)
        }
    }

    /** The view a slot gets when nothing else can be built: an opaque hole. */
    private fun placeholder(context: Context): View = View(context).apply {
        setBackgroundColor(Color.BLACK)
    }

    private companion object {
        const val TAG = "frust"
        const val PARAM_SESSION_ID = "sessionId"
        const val PARAM_SESSION = "session"
        const val PARAM_FIT = "fit"
    }
}

/** No session bound — the slot renders as an empty (black) rectangle. */
private const val NO_SESSION = -1

/**
 * How the picture fills its slot. The spellings are frozen on the Rust side
 * (`VideoFit::as_str` in `plugins/video-player/src/lib.rs`); an unrecognised or
 * absent value reads as [CONTAIN], the Rust-side default.
 */
internal enum class VideoFit {
    /** Fit the whole picture inside the slot, letterboxing the remainder. */
    CONTAIN,

    /** Fill the slot completely, overflowing whichever axis is short. */
    COVER,
    ;

    internal companion object {
        internal fun parse(value: String?): VideoFit =
            if (value.equals("cover", ignoreCase = true)) COVER else CONTAIN
    }
}

/**
 * The hosted view: a [FrameLayout] wrapping a plain [SurfaceView] that
 * [FrustVideoPlayerHost] hands to ExoPlayer with `setVideoSurfaceView`.
 *
 * **Why a wrapper.** The embedding's `FrustViewHost` owns the *hosted view's*
 * geometry — it writes `layoutParams`, `translationX/Y` and `clipBounds` on it
 * every frame — so the picture's own sizing cannot live on the same view
 * without the two fighting. The wrapper takes the host's geometry; the inner
 * [SurfaceView] takes the picture's.
 *
 * **Why a SurfaceView.** Its pixels live in a compositor layer of its own,
 * composited by the system *beside* the app window rather than inside it, so
 * nothing about the window's own drawing can lose them — and a video decoder
 * can write into it directly. A `TextureView` draws through the view hierarchy
 * instead, into a window frust configures unusually (a translucent window over
 * a full-screen render surface that punches its own hole through it), and the
 * sibling camera plugin recorded a vendor compositor dropping that window's
 * TextureView content on most frames. `media3-ui`'s `PlayerView` is unused for
 * its own reasons (`build.gradle.kts`): it owns a view hierarchy, transport
 * controls and a resize policy of its own, and its fill modes rely on exactly
 * the ancestor clipping the *Geometry* note below cannot count on.
 *
 * **Z-order: media overlay.** [SurfaceView.setZOrderMediaOverlay] puts this
 * layer above other behind-window surfaces and still below the app window — the
 * one placement that composites in both of frust's surface arrangements, as
 * `CameraPreviewFactory`'s class doc derives at length. With an opaque render
 * surface the frust surface is itself behind the window and hosted views go
 * above it; with a translucent one the frust surface is on top and frust's own
 * alpha hole reveals this layer. `setZOrderOnTop` would lift the picture above
 * the window and hide any chrome the app paints over the slot.
 *
 * **Geometry.** The system maps the decoded buffer onto this view's frame with
 * a positive, axis-aligned scale, independently per axis — so an undistorted
 * picture requires a frame whose aspect matches the decoded size reported by
 * `onVideoSizeChanged`. [VideoFit.CONTAIN] is the largest slot-inscribed rect
 * at that aspect, centred, letterboxing against this view's black background;
 * [VideoFit.COVER] is the smallest slot-covering rect at that aspect, centred,
 * so the long axis overflows the slot and is meant to be clipped. Before any
 * size is known the inner view simply fills the slot, which is also what a
 * session with no picture (audio-only) leaves in place.
 *
 * **COVER's clipping caveat (device-gate item).** A `SurfaceView`'s compositor
 * layer takes its on-screen rect from the view's own frame, and the camera
 * plugin recorded on device that an ancestor's `clipBounds`/`clipChildren` did
 * **not** trim such a layer in frust's window arrangement — a preview laid out
 * past its slot spilled over the surrounding chrome. `clipChildren` is left on
 * here (it is the default) and the overflow is centred, but a device run must
 * confirm whether COVER actually crops on this path; if it does not, COVER on
 * Android is a fill-and-spill rather than a fill-and-crop, and the honest fix
 * would be an effect/transform pipeline this plugin does not build. CONTAIN —
 * the Rust-side default — is contained by construction and carries no such
 * caveat.
 *
 * All entry points run on the main thread: the factory methods by the
 * `FrustPlatformViewFactory` contract, [onVideoSize] from
 * [FrustVideoPlayerHost]'s Media3 listener (which lives on the main Looper),
 * and the layout callbacks from the view hierarchy. No synchronization is
 * needed.
 */
internal class VideoPlayerView(context: Context) : FrameLayout(context) {
    /** The surface ExoPlayer renders into; handed over by the host. */
    internal val videoSurface = SurfaceView(context)

    /** The session this slot is showing, or [NO_SESSION]. */
    private var sessionId: Int = NO_SESSION

    /** How the picture fills this slot. */
    private var fit: VideoFit = VideoFit.CONTAIN

    /** Last decoded width reported for the bound session; 0 until one is. */
    private var videoWidth: Int = 0

    /** Height half of [videoWidth]'s pair; the two are always set together. */
    private var videoHeight: Int = 0

    init {
        // Anything the picture does not cover reads as frame, not as a hole —
        // and in Mode B an unpainted region shows raw OS content
        // (`docs/CODE_STANDARDS.md`'s Mode B paint contract). It is also what
        // CONTAIN's letterbox shows.
        setBackgroundColor(Color.BLACK)
        // Before the surface exists, which is what this call requires: the
        // layer's level is fixed at creation (class doc's *Z-order*).
        videoSurface.setZOrderMediaOverlay(true)
        // Starts slot-sized so a surface exists before any picture size is
        // known; CENTER is the gravity every later size relies on.
        addView(
            videoSurface,
            LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT, Gravity.CENTER),
        )
    }

    /**
     * Point this slot at session [id], shown with [newFit] — a `createView` or
     * `updateParams` call. Re-pointing detaches the previous session's surface
     * first; a fit-only change just re-lays out, which is what makes a fit
     * toggle a params update rather than a slot teardown.
     */
    fun bind(id: Int, newFit: VideoFit) {
        if (id == sessionId) {
            if (newFit != fit) {
                fit = newFit
                updateVideoLayout()
            }
            return
        }
        detachFromSession()
        fit = newFit
        sessionId = id
        if (id == NO_SESSION) return
        if (!FrustVideoPlayerHost.attachSurface(id, this)) {
            Log.w(TAG, "frust-video-player: slot bound to unknown session $id")
            sessionId = NO_SESSION
        }
    }

    /** Slot disposed: drop the surface, leave the session playing. */
    fun release() {
        detachFromSession()
        sessionId = NO_SESSION
    }

    /**
     * The bound session's decoded picture size, from
     * [FrustVideoPlayerHost]'s `onVideoSizeChanged` (and replayed on attach for
     * a slot created after the fact). Never `0×0` — the host filters those.
     */
    internal fun onVideoSize(width: Int, height: Int) {
        if (width == videoWidth && height == videoHeight) return
        videoWidth = width
        videoHeight = height
        updateVideoLayout()
    }

    private fun detachFromSession() {
        val id = sessionId
        if (id != NO_SESSION) FrustVideoPlayerHost.detachSurface(id, this)
        // The next session publishes its own size; until it does the inner view
        // falls back to filling the slot (class doc's *Geometry*).
        videoWidth = 0
        videoHeight = 0
        updateVideoLayout()
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        updateVideoLayout()
    }

    /**
     * Size the inner [SurfaceView] to the fitted rect for the current
     * [fit] and picture size, centred — the mapping the class doc's *Geometry*
     * note derives. A no-op before the slot has been measured.
     */
    private fun updateVideoLayout() {
        val slotWidth = width
        val slotHeight = height
        if (slotWidth <= 0 || slotHeight <= 0) return

        val viewWidth: Int
        val viewHeight: Int
        if (videoWidth > 0 && videoHeight > 0) {
            val horizontal = slotWidth.toFloat() / videoWidth.toFloat()
            val vertical = slotHeight.toFloat() / videoHeight.toFloat()
            // CONTAIN takes the smaller scale (the picture fits inside the
            // slot), COVER the larger (the slot is covered and one axis
            // overflows) — class doc's *Geometry*.
            val scale = when (fit) {
                VideoFit.CONTAIN -> min(horizontal, vertical)
                VideoFit.COVER -> max(horizontal, vertical)
            }
            viewWidth = (videoWidth * scale).roundToInt().coerceAtLeast(1)
            viewHeight = (videoHeight * scale).roundToInt().coerceAtLeast(1)
        } else {
            viewWidth = slotWidth
            viewHeight = slotHeight
        }

        val lp = videoSurface.layoutParams as LayoutParams
        if (lp.width != viewWidth || lp.height != viewHeight) {
            lp.width = viewWidth
            lp.height = viewHeight
            // Assigning is what schedules the layout pass; the compare above is
            // what keeps a steadily-playing slot free of one.
            videoSurface.layoutParams = lp
        }
    }

    private companion object {
        const val TAG = "frust"
    }
}
