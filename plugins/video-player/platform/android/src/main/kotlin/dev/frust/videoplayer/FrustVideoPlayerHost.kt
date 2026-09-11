package dev.frust.videoplayer

import android.app.Activity
import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.database.Cursor
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.util.Log
import androidx.annotation.OptIn
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.VideoSize
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.ExoPlayer
import java.io.File
import java.io.IOException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicInteger

/**
 * The `frust-video-player` plugin's ExoPlayer session owner — the Kotlin half
 * of the frozen Rust↔Kotlin contract mirrored in
 * `plugins/video-player/src/android.rs`'s module doc.
 *
 * ## The contract (LAW — mirrored in `plugins/video-player/src/android.rs`)
 *
 * Kotlin `object dev.frust.videoplayer.FrustVideoPlayerHost` exposes
 * `@JvmStatic` methods Rust calls by name through the application classloader:
 * `openPlayer(sourceKind: Int /*0 File (absolute path), 1 Asset (assets/
 * relative path), 2 Url (http(s)/HLS)*/, source: String, autoplay: Boolean,
 * looping: Boolean, volume: Float): Int` → ≥0 session id or a negative error
 * code; `play(session: Int): Int`, `pause(session: Int): Int`,
 * `seekTo(session: Int, positionMs: Long): Int`, `setRate(session: Int, rate:
 * Float): Int`, `setVolume(session: Int, volume: Float): Int`,
 * `setLooping(session: Int, looping: Boolean): Int`, `close(session: Int): Int`
 * — each returns 0 ok / -1 unknown session, each posts its work to the main
 * Looper (`Handler(Looper.getMainLooper())`) and returns immediately (if
 * already on the main thread, run inline). Error codes: -1 unknown session, -2
 * unreadable/unsupported source, -3 host not initialized (no Context captured
 * yet), -4 network, -5 decoder. Exports Rust implements (`external fun`, all
 * invoked ON THE MAIN THREAD): `nativeOnState(session: Int, state: Int)` with 0
 * Idle / 1 Loading / 2 Paused / 3 Playing / 4 Buffering / 5 Ended / 6 Error;
 * `nativeOnPosition(session: Int, positionMs: Long, durationMs: Long /* -1
 * unknown */)`; `nativeOnVideoSize(session: Int, width: Int, height: Int)`;
 * `nativeOnError(session: Int, code: Int, message: String)`. State mapping from
 * Media3: STATE_IDLE→Idle (or Error after onPlayerError),
 * STATE_BUFFERING→Loading before the first READY else Buffering, STATE_READY +
 * isPlaying→Playing, STATE_READY + !isPlaying→Paused, STATE_ENDED→Ended (with
 * looping via `REPEAT_MODE_ONE` the player never reaches ENDED). Position
 * cadence: a main-Looper `Runnable` at 250 ms while Playing, plus one emission
 * on every state change and after every seek (post-seek emission on
 * `onPositionDiscontinuity` or after `seekTo` returns). `nativeOnVideoSize`
 * from `onVideoSizeChanged` (skip 0×0). Every `nativeOn*` call is wrapped so an
 * exception never crosses JNI.
 *
 * ### The contract as a table
 *
 * | Rust → Kotlin | Signature | Returns |
 * |---|---|---|
 * | [openPlayer] | `(int sourceKind, String source, boolean autoplay, boolean looping, float volume) -> int` | ≥0 session id; [ERROR_BAD_SOURCE] / [ERROR_NO_CONTEXT] |
 * | [play] | `(int session) -> int` | [OK] / [ERROR_UNKNOWN_SESSION] |
 * | [pause] | `(int session) -> int` | [OK] / [ERROR_UNKNOWN_SESSION] |
 * | [seekTo] | `(int session, long positionMs) -> int` | [OK] / [ERROR_UNKNOWN_SESSION] |
 * | [setRate] | `(int session, float rate) -> int` | [OK] / [ERROR_UNKNOWN_SESSION] |
 * | [setVolume] | `(int session, float volume) -> int` | [OK] / [ERROR_UNKNOWN_SESSION] |
 * | [setLooping] | `(int session, boolean looping) -> int` | [OK] / [ERROR_UNKNOWN_SESSION] |
 * | [close] | `(int session) -> int` | [OK] / [ERROR_UNKNOWN_SESSION] |
 *
 * | Kotlin → Rust | Signature |
 * |---|---|
 * | [nativeOnState] | `(int session, int state)` |
 * | [nativeOnPosition] | `(int session, long positionMs, long durationMs)` |
 * | [nativeOnVideoSize] | `(int session, int width, int height)` |
 * | [nativeOnError] | `(int session, int code, String message)` |
 *
 * The package is baked into those exports' mangled names
 * (`Java_dev_frust_videoplayer_FrustVideoPlayerHost_native*`), so **this class
 * may never move once shipped** — the same rule `dev.frust.FrustSurfaceView`
 * and `dev.frust.camera.FrustCameraHost` carry. `dev.frust` itself belongs
 * exclusively to the `frust-embedding` module; every plugin takes a subpackage
 * (`docs/PLUGINS_CODE_STANDARDS.md`'s Plugin Conventions).
 *
 * ## Threading
 *
 * Every static above is called over JNI from a Rust thread and **must not
 * block**. ExoPlayer is single-threaded on the application `Looper` it was
 * built with — every call on a player instance must come from that thread —
 * so this host builds and drives every player on the **main Looper**: each
 * static does only its bookkeeping synchronously (registry lookup, session-id
 * allocation, source validation) and posts the player work through [onMain],
 * which runs inline when the caller is already on the main thread and posts
 * otherwise. Results come back through the `nativeOn*` exports, which are
 * therefore all delivered on the main thread (the Rust side publishes into its
 * snapshot and never blocks on them). No other thread and no executor exists in
 * this module.
 *
 * ## A session id outlives player creation
 *
 * [openPlayer] allocates and returns the session id **synchronously** while the
 * [ExoPlayer] itself is created on the main Looper, possibly after the caller
 * has already issued [play] or [setVolume] against that id. A command arriving
 * before its player exists is queued on the session ([Session.pending]) and
 * replayed, in order, the moment creation finishes. Main-Looper FIFO ordering
 * already gives the same answer for a command posted after the creation post;
 * the queue is what makes it true regardless of where the caller was and
 * removes any dependence on that ordering.
 *
 * ## Slot lifetime vs session lifetime
 *
 * A picture slot ([VideoPlayerViewFactory]) and a session are independent: a
 * disposed slot detaches its `SurfaceView` from the player
 * ([detachSurface]) and the player **keeps running** (audio continues, the
 * position keeps advancing), so a scrolled-away-and-back slot re-attaches
 * without reopening anything. Only [close] — or the Rust `PlayerSession` being
 * dropped, which calls it — ends a session.
 *
 * ## No Activity is retained
 *
 * Unlike `FrustCameraHost` this host holds **no Activity reference at all**: it
 * shows no dialog, asks for no runtime permission and touches no
 * Activity-scoped API, so the `Activity` handed to
 * [VideoPlayerViewFactory.createView] is used only as a fallback source of the
 * application `Context` ([cacheActivity]) if the merged manifest's
 * [FrustVideoPlayerInitProvider] was somehow stripped. Nothing here can leak an
 * Activity, and no `ActivityLifecycleCallbacks` are registered.
 *
 * Playback is deliberately **not** paused when the app backgrounds: audio-only
 * continuation is a legitimate app choice, and the app drives
 * [pause]/[play] from its own lifecycle through the Rust API.
 */
@OptIn(UnstableApi::class)
object FrustVideoPlayerHost {
    // --- Contract constants ------------------------------------------------

    /** [openPlayer] `sourceKind`: `source` is an absolute filesystem path. */
    private const val SOURCE_FILE = 0

    /**
     * [openPlayer] `sourceKind`: `source` is a path relative to the app's
     * `assets/` directory, resolved as an `asset:///…` URI (Media3's
     * `DefaultDataSource` routes that scheme through `AssetDataSource`, so no
     * extra Media3 artifact is needed).
     */
    private const val SOURCE_ASSET = 1

    /**
     * [openPlayer] `sourceKind`: `source` is an `http(s)` URL — a progressive
     * download or an HLS playlist. `DefaultMediaSourceFactory` detects an
     * `.m3u8` playlist and reflectively loads `HlsMediaSource.Factory` from
     * `media3-exoplayer-hls`, which `build.gradle.kts` puts on the runtime
     * classpath for exactly that reason.
     */
    private const val SOURCE_URL = 2

    /** [nativeOnState] `state`: nothing loaded, or the session was closed. */
    private const val STATE_IDLE = 0

    /** [nativeOnState] `state`: preparing; no frame available yet. */
    private const val STATE_LOADING = 1

    /** [nativeOnState] `state`: ready and not advancing. */
    private const val STATE_PAUSED = 2

    /** [nativeOnState] `state`: advancing. */
    private const val STATE_PLAYING = 3

    /** [nativeOnState] `state`: refilling the buffer after the first READY. */
    private const val STATE_BUFFERING = 4

    /**
     * [nativeOnState] `state`: played to the end. Unreachable while looping —
     * `REPEAT_MODE_ONE` restarts the item instead of ending it (class doc).
     */
    private const val STATE_ENDED = 5

    /** [nativeOnState] `state`: failed; [nativeOnError] carries the reason. */
    private const val STATE_ERROR = 6

    /** Every command static's success answer. */
    private const val OK = 0

    /** Unknown or already-closed session id. */
    private const val ERROR_UNKNOWN_SESSION = -1

    /**
     * [openPlayer]: the source is unreadable or unsupported — an unknown
     * `sourceKind`, an empty `source`, a [SOURCE_FILE] path that does not exist
     * or cannot be read, or an unparseable URL. Also the [nativeOnError] code
     * for a playback failure that is neither network nor decoder (a malformed
     * container, an unsupported DRM scheme).
     */
    private const val ERROR_BAD_SOURCE = -2

    /**
     * [openPlayer]: no application [Context] — this module's
     * [FrustVideoPlayerInitProvider] never ran (an app that strips the merged
     * provider out of its manifest) and no view has handed one over either.
     * Unrecoverable.
     */
    private const val ERROR_NO_CONTEXT = -3

    /** [nativeOnError] code: the media could not be fetched (I/O, HTTP, DNS). */
    private const val ERROR_NETWORK = -4

    /** [nativeOnError] code: a decoder failed to initialize or to decode. */
    private const val ERROR_DECODER = -5

    // --- Tunables ----------------------------------------------------------

    /** The contract's position cadence while [STATE_PLAYING] (class doc). */
    private const val POSITION_INTERVAL_MS = 250L

    /** [nativeOnPosition] `durationMs` for an item of unknown length. */
    private const val DURATION_UNKNOWN = -1L

    /**
     * Floor [setRate] clamps to. `Player.setPlaybackSpeed` throws on a
     * non-positive speed, and the frozen contract gives [setRate] no way to
     * report a rejected argument (0 ok / -1 unknown session only), so a
     * non-positive rate is clamped and logged rather than thrown into the main
     * Looper. Pausing is [pause]'s job.
     */
    private const val MIN_RATE = 0.01f

    /**
     * Media3 groups `PlaybackException.errorCode` values in thousands (1xxx
     * runtime/miscellaneous, 2xxx I/O, 3xxx container/manifest parsing, 4xxx
     * decoding, 5xxx audio renderer, 6xxx DRM). [errorCodeFor] switches on the
     * group rather than on individual constants so a code added in a later
     * Media3 release still maps to the right contract code.
     */
    private const val ERROR_CODE_GROUP = 1000

    private const val TAG = "frust"

    // --- Process state -----------------------------------------------------

    private val mainHandler = Handler(Looper.getMainLooper())

    /**
     * The application [Context], installed at process start by
     * [FrustVideoPlayerInitProvider]. `ExoPlayer.Builder(context)` needs one,
     * and it must be available before any view exists: the frozen
     * [openPlayer] contract takes no Context, and the picture slot's
     * `params_json` carries the session id [openPlayer] returned, so the slot
     * (and its Activity) necessarily arrive later.
     */
    @Volatile
    private var appContext: Context? = null

    /** Session ids start at 1 and are never reused; 0 is never a session. */
    private val nextSessionId = AtomicInteger(1)

    /** Live sessions by id. Removed by [close]. */
    private val sessions = ConcurrentHashMap<Int, Session>()

    /**
     * Latched once [nativeOnPosition] has proven unavailable (an app taking
     * this Gradle module without the `frust-video-player` Rust crate), so the
     * ticker logs once instead of four times a second.
     */
    @Volatile
    private var positionNativeMissing: Boolean = false

    // --- Rust -> Kotlin: the frozen contract -------------------------------

    /**
     * `openPlayer(int sourceKind, String source, boolean autoplay, boolean
     * looping, float volume) -> int`. Returns the new session id (≥0)
     * immediately, or [ERROR_BAD_SOURCE] / [ERROR_NO_CONTEXT].
     *
     * `sourceKind` is [SOURCE_FILE], [SOURCE_ASSET] or [SOURCE_URL]. The
     * player itself is built on the main Looper right after this returns;
     * state reports start with [STATE_LOADING] and arrive through
     * [nativeOnState] (class doc's *A session id outlives player creation*).
     *
     * `volume` is clamped to `0.0..=1.0`. A [SOURCE_FILE] source is stat-ed on
     * the calling thread — a few microseconds, and the only way to answer the
     * contract's synchronous [ERROR_BAD_SOURCE] at all.
     */
    @JvmStatic
    fun openPlayer(
        sourceKind: Int,
        source: String,
        autoplay: Boolean,
        looping: Boolean,
        volume: Float,
    ): Int {
        val context = appContext
        if (context == null) {
            Log.w(TAG, "frust-video-player: openPlayer before the init provider ran — no Context")
            return ERROR_NO_CONTEXT
        }
        val uri = resolveSource(sourceKind, source)
        if (uri == null) {
            Log.w(TAG, "frust-video-player: openPlayer refused source kind=$sourceKind '$source'")
            return ERROR_BAD_SOURCE
        }
        val session = Session(
            id = nextSessionId.getAndIncrement(),
            autoplay = autoplay,
            looping = looping,
            volume = volume.coerceIn(0f, 1f),
        )
        sessions[session.id] = session
        onMain { createPlayer(context, session, uri) }
        return session.id
    }

    /** `play(int session) -> int`. [OK] / [ERROR_UNKNOWN_SESSION]. */
    @JvmStatic
    fun play(session: Int): Int {
        val entry = sessions[session] ?: return ERROR_UNKNOWN_SESSION
        withPlayer(entry) { it.play() }
        return OK
    }

    /** `pause(int session) -> int`. [OK] / [ERROR_UNKNOWN_SESSION]. */
    @JvmStatic
    fun pause(session: Int): Int {
        val entry = sessions[session] ?: return ERROR_UNKNOWN_SESSION
        withPlayer(entry) { it.pause() }
        return OK
    }

    /**
     * `seekTo(int session, long positionMs) -> int`. [OK] /
     * [ERROR_UNKNOWN_SESSION].
     *
     * ExoPlayer clamps the target to the item's range. One [nativeOnPosition]
     * is emitted as soon as the seek has been issued, which is the contract's
     * "after every seek" emission for the case where the player reports no
     * discontinuity (a seek inside the current buffer of a paused item);
     * `onPositionDiscontinuity` emits the other one.
     */
    @JvmStatic
    fun seekTo(session: Int, positionMs: Long): Int {
        val entry = sessions[session] ?: return ERROR_UNKNOWN_SESSION
        withPlayer(entry) { player ->
            player.seekTo(positionMs.coerceAtLeast(0L))
            emitPosition(entry)
        }
        return OK
    }

    /**
     * `setRate(int session, float rate) -> int`. [OK] /
     * [ERROR_UNKNOWN_SESSION]. A non-positive rate is clamped to [MIN_RATE]
     * (see that constant); a rate on a paused player starts it, which is
     * `Player.setPlaybackSpeed`'s own documented behaviour and what the Rust
     * API promises.
     */
    @JvmStatic
    fun setRate(session: Int, rate: Float): Int {
        val entry = sessions[session] ?: return ERROR_UNKNOWN_SESSION
        if (rate < MIN_RATE) {
            Log.w(TAG, "frust-video-player: setRate($session, $rate) clamped to $MIN_RATE")
        }
        val clamped = rate.coerceAtLeast(MIN_RATE)
        withPlayer(entry) { it.setPlaybackSpeed(clamped) }
        return OK
    }

    /**
     * `setVolume(int session, float volume) -> int`. [OK] /
     * [ERROR_UNKNOWN_SESSION]. Clamped to `0.0..=1.0` — the range
     * `Player.setVolume` accepts.
     */
    @JvmStatic
    fun setVolume(session: Int, volume: Float): Int {
        val entry = sessions[session] ?: return ERROR_UNKNOWN_SESSION
        val clamped = volume.coerceIn(0f, 1f)
        withPlayer(entry) { it.volume = clamped }
        return OK
    }

    /**
     * `setLooping(int session, boolean looping) -> int`. [OK] /
     * [ERROR_UNKNOWN_SESSION]. `REPEAT_MODE_ONE` restarts the item instead of
     * ending it, so a looping session never reports [STATE_ENDED] (class doc).
     */
    @JvmStatic
    fun setLooping(session: Int, looping: Boolean): Int {
        val entry = sessions[session] ?: return ERROR_UNKNOWN_SESSION
        val mode = if (looping) Player.REPEAT_MODE_ONE else Player.REPEAT_MODE_OFF
        withPlayer(entry) { it.repeatMode = mode }
        return OK
    }

    /**
     * `close(int session) -> int`. [OK] / [ERROR_UNKNOWN_SESSION].
     *
     * Retires the id immediately (a later command on it answers
     * [ERROR_UNKNOWN_SESSION]), then releases the player on the main Looper and
     * reports a final [STATE_IDLE] through [nativeOnState]. Idempotent by
     * construction: the second call finds nothing in the registry.
     */
    @JvmStatic
    fun close(session: Int): Int {
        val entry = sessions.remove(session) ?: return ERROR_UNKNOWN_SESSION
        entry.closed = true
        onMain {
            stopTicker(entry)
            entry.pending.clear()
            val player = entry.player
            entry.player = null
            entry.view = null
            try {
                player?.release()
            } catch (e: Throwable) {
                Log.w(TAG, "frust-video-player: release($session) failed", e)
            }
            // Unconditional, not routed through `publishState`: the session is
            // out of the registry and this is the contract's final report.
            notifyState(session, STATE_IDLE)
        }
        return OK
    }

    // --- Kotlin -> Rust: the frozen exports --------------------------------
    //
    // Implemented in `plugins/video-player/src/android.rs` as
    // `Java_dev_frust_videoplayer_FrustVideoPlayerHost_native*`. They resolve
    // against the app's already-loaded native library (the embedding's
    // `FrustActivity` loads it before any frust view exists, and every call
    // below is downstream of a Rust-initiated one), so no `System.loadLibrary`
    // happens here. All four are invoked on the main thread.

    /** Reports a session's playback state, [STATE_IDLE]..[STATE_ERROR]. */
    @JvmStatic
    external fun nativeOnState(session: Int, state: Int)

    /**
     * Reports the playhead and the item's length, `durationMs`
     * [DURATION_UNKNOWN] while unknown (a live stream, or before the item is
     * prepared).
     */
    @JvmStatic
    external fun nativeOnPosition(session: Int, positionMs: Long, durationMs: Long)

    /** Reports the decoded picture size; `0×0` is never sent. */
    @JvmStatic
    external fun nativeOnVideoSize(session: Int, width: Int, height: Int)

    /**
     * Reports a playback failure: [ERROR_NETWORK], [ERROR_DECODER] or
     * [ERROR_BAD_SOURCE], with Media3's own message. Always preceded by a
     * [STATE_ERROR] [nativeOnState].
     */
    @JvmStatic
    external fun nativeOnError(session: Int, code: Int, message: String)

    // --- Module-internal seam (NOT part of the JNI contract) ---------------

    /**
     * Install the application [Context] — called once at process start by
     * [FrustVideoPlayerInitProvider], before any Activity or view exists.
     * Idempotent and first-writer-wins.
     */
    internal fun installApplicationContext(context: Context) {
        if (appContext == null) appContext = context
    }

    /**
     * Take the application [Context] from the Activity the platform-view
     * factory seam hands over, as a fallback for an app whose merged manifest
     * lost this module's provider. The [Activity] itself is **not** retained
     * (class doc's *No Activity is retained*).
     */
    internal fun cacheActivity(activity: Activity) {
        if (appContext == null) appContext = activity.applicationContext
    }

    /**
     * Point session [sessionId]'s player at [view]'s `SurfaceView`. Main
     * thread. Returns false for an unknown session (the slot renders empty).
     *
     * Also re-delivers the last known picture size to [view], so a slot created
     * after `onVideoSizeChanged` already fired is fitted on its first layout
     * instead of waiting for a size change that may never come again.
     */
    internal fun attachSurface(sessionId: Int, view: VideoPlayerView): Boolean {
        val session = sessions[sessionId] ?: return false
        session.view = view
        if (session.videoWidth > 0 && session.videoHeight > 0) {
            view.onVideoSize(session.videoWidth, session.videoHeight)
        }
        withPlayer(session) { it.setVideoSurfaceView(view.videoSurface) }
        return true
    }

    /**
     * Drop session [sessionId]'s surface (slot disposed or re-pointed), leaving
     * the player running — class doc's *Slot lifetime vs session lifetime*.
     * Main thread; a no-op if [view] is not the surface currently attached.
     */
    internal fun detachSurface(sessionId: Int, view: VideoPlayerView) {
        val session = sessions[sessionId] ?: return
        if (session.view !== view) return
        session.view = null
        withPlayer(session) { it.clearVideoSurfaceView(view.videoSurface) }
    }

    // --- Session bookkeeping -----------------------------------------------

    /**
     * One open player. Every field except [closed] is touched **only on the
     * main Looper**, which is what keeps this class free of synchronization;
     * [closed] is written by [close] from the JNI caller's thread and only ever
     * read as a short-circuit.
     */
    private class Session(
        val id: Int,
        val autoplay: Boolean,
        val looping: Boolean,
        val volume: Float,
    ) {
        /** Null until [createPlayer] runs, and again after [close]. */
        var player: ExoPlayer? = null

        /** Commands that arrived before [player] existed, in arrival order. */
        val pending: ArrayDeque<(ExoPlayer) -> Unit> = ArrayDeque()

        /** Retired by [close]; short-circuits queued and in-flight work. */
        @Volatile
        var closed: Boolean = false

        /** Last state handed to [nativeOnState]; -1 before the first one. */
        var lastState: Int = -1

        /** Whether the player has reached `STATE_READY` at least once. */
        var sawReady: Boolean = false

        /** Set by `onPlayerError`; turns a following `STATE_IDLE` into Error. */
        var failed: Boolean = false

        /** Whether the position ticker is currently scheduled. */
        var ticking: Boolean = false

        /** The scheduled ticker, so [stopTicker] can remove exactly it. */
        var ticker: Runnable? = null

        /** The slot currently showing this session, if any. */
        var view: VideoPlayerView? = null

        /** Last non-zero decoded width, replayed to a late-arriving slot. */
        var videoWidth: Int = 0

        /** Height half of [videoWidth]'s pair; the two are always set together. */
        var videoHeight: Int = 0
    }

    /**
     * Run [block] on the main Looper — inline when the caller is already there,
     * posted otherwise. The single marshalling point this module has (class
     * doc's *Threading*).
     */
    private fun onMain(block: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) block() else mainHandler.post(block)
    }

    /**
     * Run [command] against [session]'s player on the main Looper, queueing it
     * on the session if the player has not been built yet (class doc's *A
     * session id outlives player creation*).
     *
     * A throwing command is logged, never propagated: this runs on the main
     * Looper, where an escaping exception would take the app's UI thread down.
     */
    private fun withPlayer(session: Session, command: (ExoPlayer) -> Unit) {
        onMain {
            if (session.closed) return@onMain
            val player = session.player
            if (player == null) {
                session.pending.addLast(command)
                return@onMain
            }
            runCommand(session, player, command)
        }
    }

    private fun runCommand(session: Session, player: ExoPlayer, command: (ExoPlayer) -> Unit) {
        try {
            command(player)
        } catch (e: Throwable) {
            Log.w(TAG, "frust-video-player: command on session ${session.id} failed", e)
        }
    }

    /**
     * Build the session's [ExoPlayer], configure it from the options
     * [openPlayer] captured, attach the listener, prepare, and replay anything
     * that queued up meanwhile. Main Looper only.
     */
    private fun createPlayer(context: Context, session: Session, uri: Uri) {
        if (session.closed) return
        val player = try {
            ExoPlayer.Builder(context).build()
        } catch (e: Throwable) {
            Log.w(TAG, "frust-video-player: ExoPlayer creation failed for session ${session.id}", e)
            sessions.remove(session.id)
            session.closed = true
            publishState(session, STATE_ERROR)
            notifyError(session.id, ERROR_DECODER, "ExoPlayer could not be created: ${e.message}")
            return
        }
        session.player = player
        player.repeatMode = if (session.looping) Player.REPEAT_MODE_ONE else Player.REPEAT_MODE_OFF
        player.volume = session.volume
        player.playWhenReady = session.autoplay
        player.addListener(listenerFor(session))
        // `DefaultMediaSourceFactory` picks the media source from the URI:
        // `asset:///…` through `AssetDataSource`, an `.m3u8` playlist through
        // the reflectively-loaded `HlsMediaSource.Factory`, everything else
        // progressive (`build.gradle.kts`'s dependency notes).
        player.setMediaItem(MediaItem.fromUri(uri))
        // Published before `prepare()` so the Rust snapshot leaves Idle the
        // moment the session is real, without waiting for Media3's first
        // STATE_BUFFERING callback.
        publishState(session, STATE_LOADING)
        player.prepare()
        // In order, and cleared as they run: a command queued by `withPlayer`
        // before this point is the caller's, and must not be replayed twice.
        while (session.pending.isNotEmpty()) {
            runCommand(session, player, session.pending.removeFirst())
        }
    }

    /**
     * The per-session [Player.Listener] implementing the contract's state
     * mapping, the picture-size report and the seek-time position emission.
     * Every callback lands on the main Looper (the thread the player was built
     * on), so it reads and writes [Session] fields directly.
     */
    private fun listenerFor(session: Session): Player.Listener = object : Player.Listener {
        override fun onPlaybackStateChanged(playbackState: Int) {
            val player = session.player ?: return
            when (playbackState) {
                // Idle after a failure is the failure, not a fresh player:
                // Media3 drops to STATE_IDLE right after `onPlayerError`.
                Player.STATE_IDLE ->
                    publishState(session, if (session.failed) STATE_ERROR else STATE_IDLE)
                // Before the first READY the item is still being prepared
                // (Loading); afterwards the same callback means a rebuffer.
                Player.STATE_BUFFERING ->
                    publishState(session, if (session.sawReady) STATE_BUFFERING else STATE_LOADING)
                Player.STATE_READY -> {
                    session.sawReady = true
                    publishState(session, if (player.isPlaying) STATE_PLAYING else STATE_PAUSED)
                }
                Player.STATE_ENDED -> publishState(session, STATE_ENDED)
                else -> Log.w(TAG, "frust-video-player: unknown Media3 state $playbackState")
            }
            syncTicker(session)
            emitPosition(session)
        }

        override fun onIsPlayingChanged(isPlaying: Boolean) {
            val player = session.player ?: return
            // Only READY distinguishes Playing from Paused; while buffering or
            // ended the state published by `onPlaybackStateChanged` stands.
            if (player.playbackState == Player.STATE_READY) {
                publishState(session, if (isPlaying) STATE_PLAYING else STATE_PAUSED)
            }
            syncTicker(session)
            emitPosition(session)
        }

        override fun onPlayerError(error: PlaybackException) {
            session.failed = true
            publishState(session, STATE_ERROR)
            syncTicker(session)
            val message = error.message ?: error.errorCodeName
            Log.w(TAG, "frust-video-player: session ${session.id} failed (${error.errorCodeName})")
            notifyError(session.id, errorCodeFor(error), message)
        }

        override fun onVideoSizeChanged(videoSize: VideoSize) {
            val width = videoSize.width
            val height = videoSize.height
            // 0×0 is Media3's "no picture yet" report and is never forwarded:
            // the contract's consumers size a slot from this value.
            if (width <= 0 || height <= 0) return
            session.videoWidth = width
            session.videoHeight = height
            session.view?.onVideoSize(width, height)
            notifyVideoSize(session.id, width, height)
        }

        override fun onPositionDiscontinuity(
            oldPosition: Player.PositionInfo,
            newPosition: Player.PositionInfo,
            reason: Int,
        ) {
            // The contract's post-seek emission (a loop restart and an ad-free
            // item transition arrive here too — all of them move the playhead).
            emitPosition(session)
        }
    }

    /**
     * Map a Media3 failure onto the contract's error codes by the documented
     * code group (see [ERROR_CODE_GROUP]): I/O → [ERROR_NETWORK], decoding and
     * audio-renderer → [ERROR_DECODER], everything else (runtime, container or
     * manifest parsing, DRM) → [ERROR_BAD_SOURCE].
     */
    private fun errorCodeFor(error: PlaybackException): Int =
        when (error.errorCode / ERROR_CODE_GROUP) {
            2 -> ERROR_NETWORK
            4, 5 -> ERROR_DECODER
            else -> ERROR_BAD_SOURCE
        }

    /**
     * Turn `source` into the URI Media3 should play, or null for anything the
     * contract calls unreadable/unsupported ([ERROR_BAD_SOURCE]).
     *
     * [SOURCE_ASSET] deliberately strips a leading `/`: the contract's asset
     * spelling is a path *relative to* `assets/`, and `asset:///a/b.mp4` is the
     * URI `AssetDataSource` resolves to `assets/a/b.mp4`. The asset is opened
     * and closed here purely to answer the contract's synchronous
     * [ERROR_BAD_SOURCE] for a name that is not packaged — symmetric with the
     * [SOURCE_FILE] stat, and the only point at which a missing asset can be
     * reported as a *refused open* rather than as a later I/O failure.
     *
     * Both checks are TOCTOU-shaped by nature (a file can vanish between the
     * stat and the load): they exist to give the common mistake — a typo in a
     * path — the typed refusal the contract defines, not to guarantee the load
     * will succeed. A source that passes here and fails later still arrives as
     * [nativeOnError].
     */
    private fun resolveSource(sourceKind: Int, source: String): Uri? {
        if (source.isEmpty()) return null
        return when (sourceKind) {
            SOURCE_FILE -> {
                val file = File(source)
                if (!file.isFile || !file.canRead()) null else Uri.fromFile(file)
            }
            SOURCE_ASSET -> {
                val path = source.trimStart('/')
                if (!assetExists(path)) null else Uri.parse("asset:///" + path)
            }
            SOURCE_URL -> {
                val uri = try {
                    Uri.parse(source)
                } catch (e: Throwable) {
                    Log.w(TAG, "frust-video-player: unparseable url '$source'", e)
                    null
                }
                if (uri?.scheme == null) null else uri
            }
            else -> null
        }
    }

    /**
     * Whether [path] names a packaged asset — an `AssetManager` open/close
     * against the application [Context]. False for a directory, an absent name,
     * or a host with no Context yet (which [openPlayer] has already refused).
     */
    private fun assetExists(path: String): Boolean {
        val assets = appContext?.assets ?: return false
        return try {
            assets.open(path).close()
            true
        } catch (e: IOException) {
            Log.w(TAG, "frust-video-player: asset '$path' is not packaged", e)
            false
        }
    }

    // --- State, position and the ticker ------------------------------------

    /**
     * Report [state] unless it is the one already reported — the contract's
     * "every state change" emission, deduplicated so a redundant Media3
     * callback (or [createPlayer]'s explicit [STATE_LOADING]) cannot produce a
     * repeat.
     */
    private fun publishState(session: Session, state: Int) {
        if (session.lastState == state) return
        session.lastState = state
        notifyState(session.id, state)
    }

    /** Start or stop the position ticker to match the player's actual state. */
    private fun syncTicker(session: Session) {
        if (session.player?.isPlaying == true) startTicker(session) else stopTicker(session)
    }

    /**
     * Schedule the contract's [POSITION_INTERVAL_MS] position emission on the
     * main Looper. Idempotent: a second call while ticking does nothing, so the
     * several triggers (`onPlaybackStateChanged`, `onIsPlayingChanged`) cannot
     * stack two tickers on one session.
     */
    private fun startTicker(session: Session) {
        if (session.ticking) return
        session.ticking = true
        val ticker = object : Runnable {
            override fun run() {
                if (!session.ticking || session.closed) return
                emitPosition(session)
                mainHandler.postDelayed(this, POSITION_INTERVAL_MS)
            }
        }
        session.ticker = ticker
        mainHandler.postDelayed(ticker, POSITION_INTERVAL_MS)
    }

    /** Unschedule the ticker; safe to call when none is scheduled. */
    private fun stopTicker(session: Session) {
        session.ticking = false
        session.ticker?.let { mainHandler.removeCallbacks(it) }
        session.ticker = null
    }

    /**
     * Emit one [nativeOnPosition] for [session]. `C.TIME_UNSET` (and any
     * negative duration) becomes the contract's [DURATION_UNKNOWN]; the
     * playhead is floored at 0, which is what an unprepared player reports as a
     * negative "unset" position.
     */
    private fun emitPosition(session: Session) {
        val player = session.player ?: return
        val duration = player.duration
        val reported = if (duration == C.TIME_UNSET || duration < 0L) DURATION_UNKNOWN else duration
        notifyPosition(session.id, player.currentPosition.coerceAtLeast(0L), reported)
    }

    // --- Export guards -----------------------------------------------------
    //
    // Every native call is guarded: a missing symbol (an app that depends on
    // this Gradle module without the `frust-video-player` Rust crate) must
    // degrade to a log line, never an UnsatisfiedLinkError thrown into a Media3
    // callback or the main Looper.

    private fun notifyState(session: Int, state: Int) {
        try {
            nativeOnState(session, state)
        } catch (e: Throwable) {
            Log.w(TAG, "frust-video-player: nativeOnState unavailable", e)
        }
    }

    /**
     * The ticking guard. Unlike its siblings this one latches
     * [positionNativeMissing] on the first failure: at the contract's cadence
     * an unresolvable symbol would otherwise log four times a second.
     */
    private fun notifyPosition(session: Int, positionMs: Long, durationMs: Long) {
        if (positionNativeMissing) return
        try {
            nativeOnPosition(session, positionMs, durationMs)
        } catch (e: Throwable) {
            positionNativeMissing = true
            Log.w(TAG, "frust-video-player: nativeOnPosition unavailable", e)
        }
    }

    private fun notifyVideoSize(session: Int, width: Int, height: Int) {
        try {
            nativeOnVideoSize(session, width, height)
        } catch (e: Throwable) {
            Log.w(TAG, "frust-video-player: nativeOnVideoSize unavailable", e)
        }
    }

    private fun notifyError(session: Int, code: Int, message: String) {
        try {
            nativeOnError(session, code, message)
        } catch (e: Throwable) {
            Log.w(TAG, "frust-video-player: nativeOnError unavailable", e)
        }
    }
}

/**
 * Installs the application [Context] into [FrustVideoPlayerHost] at process
 * start.
 *
 * A `ContentProvider` declared in this module's own `AndroidManifest.xml` is
 * created by the system before `Application.onCreate`, which is the standard
 * self-initialization mechanism for an Android library that needs a `Context`
 * with no app-side code (`androidx.startup`'s `InitializationProvider`,
 * Firebase's `FirebaseInitProvider`). `frust-video-player` needs one because
 * the frozen `openPlayer(...)` contract passes no `Context` and a session must
 * be openable *before* its picture slot exists — the slot's `params_json`
 * carries the session id `openPlayer` returned, so the Activity from the
 * platform-view factory seam arrives strictly too late.
 *
 * It provides no data: every `ContentProvider` operation returns null/0. It is
 * `exported="false"` and its authority is `${applicationId}`-scoped, so nothing
 * outside the app can reach it.
 */
class FrustVideoPlayerInitProvider : ContentProvider() {
    override fun onCreate(): Boolean {
        context?.let { FrustVideoPlayerHost.installApplicationContext(it.applicationContext) }
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
