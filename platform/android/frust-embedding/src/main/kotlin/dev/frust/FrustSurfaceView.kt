package dev.frust

import android.app.Activity
import android.content.Context
import android.content.res.Configuration
import android.database.ContentObserver
import android.graphics.PixelFormat
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.text.Editable
import android.text.InputType
import android.text.Selection
import android.text.SpannableStringBuilder
import android.view.Choreographer
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.Surface
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import androidx.core.graphics.Insets
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import org.json.JSONException
import org.json.JSONObject

/**
 * The Frust Android render surface. Package
 * `dev.frust` is fixed across every app — it's what keeps the JNI export
 * names (`Java_dev_frust_FrustSurfaceView_native*`) stable, so do not
 * rename or move this file. The hosting activity (normally [FrustActivity],
 * in the app's own package) owns the activity lifecycle and forwards
 * `onResume`/`onPause`/`onDestroy` here.
 *
 * The native library is NOT loaded by constructing this view: a host must
 * call [loadNativeLibrary] first (as [FrustActivity.onCreate] does), since
 * the library's name is the app's, not the framework's.
 *
 * Soft-keyboard text input uses the Flutter-proven
 * **state-sync** contract, not op-forwarding: [FrustInputConnection] owns
 * composition mechanics against a mirror [Editable], pushes the whole editing
 * state into Rust via `nativeImeApply`, then pulls the reconciled state back via
 * `nativeImeState` to keep the `InputMethodManager` synchronised.
 *
 * `androidx.core` backs the inset listener
 * ([dispatchInsets]) and status/nav-bar icon contrast
 * ([updateSystemBarsAppearance]) below — the only AndroidX dependency this
 * view uses.
 */
class FrustSurfaceView(
    context: Context,
    /**
     * Host opt-in for a **translucent** (Mode B) render surface, where Frust
     * paints an alpha channel over native sibling views hosted BELOW the
     * surface (platform-views feature). Pass `true` for an app that composites
     * Frust content over native `View`s ([FrustViewHost] reads this to pick the
     * host z-order); the default `false` keeps the opaque (Mode A) surface
     * every app has always had. [FrustActivity.translucentSurface] is the
     * override point an app flips.
     *
     * When `true`, [surfaceCreated] latches the alpha surface on the Rust
     * side via `nativeSetSurfaceMode(true)` BEFORE `nativeInit`, and the
     * [init] block configures the `SurfaceHolder` for the translucent
     * recipe (`PixelFormat.TRANSLUCENT` + `setZOrderOnTop(true)`). The
     * on-top z-order (surface above the window) is needed for native
     * sibling views to composite; media-overlay below-window arrangement
     * erased them. Input routing is unchanged — device-verified: a
     * SurfaceView does not consume input, so taps still reach the frust
     * content painted over a hosted slot.
     *
     * **This parameter is the whole Mode B switch: it must drive the
     * `SurfaceHolder` pixel format, the [FrustViewHost] z-order
     * arrangement, AND the `nativeSetSurfaceMode` declaration together.**
     * Flipping only some of them is a defect — the Rust side trusts
     * `nativeSetSurfaceMode(true)` as proof the window is *already*
     * translucent; calling it without the matching `PixelFormat.TRANSLUCENT`
     * clears to a transparent base and punches an otherwise-opaque
     * swapchain, painting black rectangles where the hole should show
     * through instead. It must also stay paired with the Rust-side
     * `declare_host_translucent_surface` latch: the shell reads that latch
     * once, at surface-creation time, so a mismatch is a startup-visible
     * failure rather than a recoverable one. This is also why there is no
     * app-Rust equivalent call — only a host may declare translucency.
     */
    val translucentSurface: Boolean = false,
) : SurfaceView(context),
    SurfaceHolder.Callback,
    Choreographer.FrameCallback {

    companion object {
        /**
         * Latched by the first successful [loadNativeLibrary] call — the library
         * name is the app's, not the framework's, so this module cannot load it
         * from a static initializer the way a per-app generated copy of this file
         * used to.
         *
         * Plain Boolean (not AtomicBoolean) since the only access path
         * ([loadNativeLibrary]) is @Synchronized; the monitor lock provides the
         * necessary exclusion, and AtomicBoolean's CAS would be redundant.
         */
        private var libraryLoaded = false

        /**
         * Load the app's Rust native library (the `.so` carrying the
         * `Java_dev_frust_FrustSurfaceView_native*` exports) by `name` — the
         * `System.loadLibrary` base name, i.e. `libfoo.so` is `"foo"`.
         *
         * Idempotent; safe to call from any thread. Loads the library on the
         * first successful call and leaves the flag latched, so a second call
         * after success is a silent no-op and does not re-invoke
         * `System.loadLibrary`. A load failure propagates (an
         * `UnsatisfiedLinkError`) and leaves the flag clear, so a later retry
         * (e.g. after fixing a missing `.so` or ABI mismatch) re-attempts the
         * load rather than silently succeeding.
         *
         * **Single-library shape**: a caller that invokes this with different
         * `name` values will load the first, ignore the rest. This is the only
         * supported configuration today; multi-library hosting is not
         * implemented.
         *
         * Must run before any native method on this view is reached;
         * [FrustActivity.onCreate] calls it before constructing the view.
         * `@Synchronized` so a concurrent second caller cannot return while the
         * first is still inside `System.loadLibrary`.
         */
        @JvmStatic
        @Synchronized
        fun loadNativeLibrary(name: String) {
            if (!libraryLoaded) {
                System.loadLibrary(name)
                libraryLoaded = true
            }
        }

        /**
         * How many frames to keep re-polling the IME surface after an editor
         * action, so [doFrame] catches the submit's next-frame rebuild (which
         * clears/replaces the field) and reseeds the IME mirror. A small budget
         * (a couple of frames) covers the one-frame rebuild latency with slack.
         */
        private const val IME_RESYNC_FRAMES = 3

        /**
         * Decode the low byte of `nativeSystemUiState`'s packed `u64` (task
         * 03's `system_ui::encoded_state()` doc comment:
         * `(generation << 8) | mode_bits`, low byte: `0..=4` is the mode
         * discriminant, bit 4 (`0x10`) = Manual's `top`, bit 5 (`0x20`) =
         * Manual's `bottom`) into a [SystemUiMode]. `low` must already be
         * masked to the low byte by the caller.
         */
        private fun decodeSystemUiMode(low: Long): SystemUiMode =
            when ((low and 0x0FL).toInt()) {
                0 -> SystemUiMode.EdgeToEdge
                1 -> SystemUiMode.Immersive
                2 -> SystemUiMode.ImmersiveSticky
                3 -> SystemUiMode.LeanBack
                else -> SystemUiMode.Manual(
                    top = (low and 0x10L) != 0L,
                    bottom = (low and 0x20L) != 0L,
                )
            }
    }

    /**
     * Decoded mirror of `frust_shell_common::system_ui::SystemUiMode`
     * (Flutter `SystemUiMode` parity) — the vocabulary
     * [pollSystemUiState] decodes `nativeSystemUiState`'s packed `u64`
     * into for [onSystemUiModeChanged]. `MainActivity` applies
     * one of these via `WindowInsetsControllerCompat`. Declared in the class
     * body, NOT the companion object: Kotlin resolves companion members
     * (`FrustSurfaceView.decodeSystemUiMode`) through the outer class name,
     * but never companion-nested *types* — `FrustSurfaceView.SystemUiMode`
     * only compiles with the declaration here.
     */
    sealed class SystemUiMode {
        object EdgeToEdge : SystemUiMode()
        object Immersive : SystemUiMode()
        object ImmersiveSticky : SystemUiMode()
        object LeanBack : SystemUiMode()
        data class Manual(val top: Boolean, val bottom: Boolean) : SystemUiMode()
    }

    // JNI exports implemented by `frust-shell-android` — these names and
    // signatures are load-bearing, matched exactly by
    // `#[no_mangle] extern "system" fn Java_dev_frust_*`.
    // `cacheDir` is the app's `context.cacheDir.absolutePath` — the Rust side
    // persists the wgpu pipeline cache under it (`<cacheDir>/frust/`) so a
    // warm start skips Vulkan shader-pipeline compilation. The JNI symbol name is
    // unchanged (it doesn't encode params); this signature and the Rust
    // `native_init` gained the parameter together.
    private external fun nativeInit(surface: Surface, scaleFactor: Float, cacheDir: String): Long

    // `density` is `resources.displayMetrics.density`
    // for the current configuration — a BREAKING signature change from the
    // pre-parity three-arg form; the Rust `native_on_surface_changed` gained
    // the same trailing parameter in lockstep.
    private external fun nativeOnSurfaceChanged(
        handle: Long,
        surface: Surface,
        width: Int,
        height: Int,
        density: Float,
    )

    private external fun nativeOnSurfaceDestroyed(handle: Long)

    // Returns whether the framework is still healthy: `false` signals a FATAL,
    // unrecoverable render-thread failure (the first surface install could not
    // succeed — an incapable GPU/driver), on which [doFrame] stops the
    // Choreographer loop instead of driving doomed frames against a permanent
    // black screen. The JNI symbol name is unchanged; this `Boolean` return moves
    // in lockstep with the Rust `native_on_frame` signature.
    private external fun nativeOnFrame(handle: Long, frameTimeNanos: Long): Boolean

    // Touch delivery. `action` is a fixed numeric ABI shared with the
    // Rust `nativeOnTouch` glue — DO NOT renumber without changing both sides:
    //   0 = down (ACTION_DOWN / ACTION_POINTER_DOWN)
    //   1 = move (ACTION_MOVE)
    //   2 = up   (ACTION_UP / ACTION_POINTER_UP)
    //   3 = cancel (ACTION_CANCEL)
    // `x`/`y` are physical, view-local pixels (`MotionEvent.x`/`.y`); the Rust
    // side divides by the display density to get logical coordinates.
    private external fun nativeOnTouch(handle: Long, action: Int, x: Float, y: Float)

    private external fun nativeOnResume(handle: Long)

    private external fun nativeOnPause(handle: Long)

    private external fun nativeOnDestroy(handle: Long)

    // IME state-sync exports. Indices are UTF-16 code units
    // (Java-native), passed through the Rust seam unchanged.
    private external fun nativeImeApply(
        handle: Long,
        text: String,
        selBase: Int,
        selExt: Int,
        compBase: Int,
        compExt: Int,
    )

    // Returns JSON (`{"active":..,"text":..,"selBase":..,...}`) or null when there
    // is no live native handle.
    private external fun nativeImeState(handle: Long): String?

    private external fun nativeImeAction(handle: Long, action: Int)

    // Appearance: flip the app's theme brightness between
    // light and dark. `dark` mirrors Configuration.UI_MODE_NIGHT_YES — see
    // [isDarkMode]. Called once right after `nativeInit` returns a handle and
    // again on every `onConfigurationChanged` (the manifest declares `uiMode`
    // in `android:configChanges` so a system dark-mode toggle reaches here
    // instead of recreating the activity).
    private external fun nativeSetAppearance(handle: Long, dark: Boolean)

    // The read half of the appearance seam: whether the APP's currently
    // active theme is dark right now — NOT a re-read of `Configuration.
    // uiMode`. Kotlin trusted the device's own dark-mode preference for
    // status-bar icon contrast ([updateSystemBarsAppearance]); that silently
    // disagrees with the app's actual theme whenever an app-forced
    // `frust::set_app_theme` override (or a design system's seeded default)
    // is in play, painting invisible (light-on-light or dark-on-dark) icons.
    // Called right after every [nativeSetAppearance] ([surfaceCreated],
    // [onConfigurationChanged]) and once per frame ([pollAppBrightness]) —
    // the per-frame poll is what picks up a runtime `set_app_theme`/
    // `clear_app_theme` call, which has no `Configuration` event of its own
    // to ride in on.
    private external fun nativeAppIsDark(handle: Long): Boolean

    // Reduced motion: apply the platform's reduce-motion
    // accessibility preference to the app's motion tokens. `reduce` mirrors
    // `Settings.Global.ANIMATOR_DURATION_SCALE == 0f` — see
    // [reduceMotionEnabled]. Called once right after `nativeInit` returns a
    // handle, again from [onResume] (the setting is toggled in Settings, i.e.
    // while this app is backgrounded), and on every [reduceMotionObserver]
    // fire. NOT driven by `onConfigurationChanged`: animation scale is not a
    // `Configuration` field and that callback never fires for it.
    private external fun nativeSetReduceMotion(handle: Long, reduce: Boolean)

    // Deep links. `url` is the raw `Intent.data` Uri's `toString()`,
    // forwarded to `frust_reactive::push_deep_link` on the Rust side.
    private external fun nativeOnDeepLink(handle: Long, url: String)

    // Accessibility. Attaches the accesskit Android adapter
    // to this view. `view` is the accessibility host — always `this`
    // (`FrustSurfaceView` IS a `View`); the adapter installs a
    // `View.AccessibilityDelegate` + `OnHoverListener` on it (posted to the UI
    // thread) and depends on the bundled `dev.accesskit.android.Delegate` class.
    // Best-effort: the Rust side isolates any init failure so a11y never blocks
    // startup. Called once, right after `nativeInit` returns a live handle.
    private external fun nativeInitAccessibility(handle: Long, view: View)

    // Insets. The eight floats are physical px: `viewPadding` (system-bar/cutout
    // occlusion, vp*) then `viewInsets` (the IME area, vi*), each l/t/r/b —
    // see [dispatchInsets].
    private external fun nativeOnInsetsChanged(
        handle: Long,
        vpLeft: Float,
        vpTop: Float,
        vpRight: Float,
        vpBottom: Float,
        viLeft: Float,
        viTop: Float,
        viRight: Float,
        viBottom: Float,
    )

    // Android back. Returns whether the framework consumed the press (it
    // will pop on the next rebuild) — see [dispatchBackPress].
    private external fun nativeOnBackPress(handle: Long): Boolean

    // System UI / SystemChrome: returns the process-wide `frust::set_system_ui_mode`
    // override slot's packed `(generation, mode)` state
    // (`frust_shell_common::system_ui::encoded_state()`'s doc comment has the
    // exact bit layout) for [pollSystemUiState] to decode.
    private external fun nativeSystemUiState(handle: Long): Long

    // Plugin platform initialization: deliver the
    // application Context to the native side so `frust-plugin`'s handles
    // can be initialized. Called once from [surfaceCreated] with the
    // application context (never the Activity), strictly before `nativeInit`.
    private external fun nativeInitPlatform(context: Context)

    // Platform views: latch a translucent (Mode B)
    // GPU surface BEFORE `nativeInit` creates it — a process-wide, pre-init-only
    // one-way opt-in (a call after a surface already exists is a no-op on the
    // Rust side). Takes no handle by design. Called from [surfaceCreated] only
    // when [FRUST_TRANSLUCENT_SURFACE] is true.
    private external fun nativeSetSurfaceMode(translucent: Boolean)

    // Platform views: the native-sibling-compositor command backlog (the
    // Rust-side differ) as JSON for [FrustViewHost] to apply, or null on the
    // no-change fast path (or a dead handle) — Kotlin treats null
    // as "nothing to do", keeping a steady frame allocation-free (no JSON parse).
    // `ackGeneration` round-trips [FrustViewHost.ackedGeneration] so the differ
    // can compact acknowledged commands. Rects in the JSON are physical px.
    private external fun nativePlatformViewCommands(handle: Long, ackGeneration: Long): String?

    // Platform-view scroll sync: push this tick's Choreographer
    // frame-timeline delta (`expectedPresentationTimeNanos - frameTimeNanos`) to
    // the native scroll-sync tail, which derives from it how many display frames
    // a geometry batch must be held so a hosted native view lands WITH the frust
    // content it is pinned to. `0` means "no sample" and leaves the native side
    // gate-only (its shipped behaviour). This is the one signal Rust cannot read
    // for itself — `Choreographer.postVsyncCallback` is JVM-only, API 33+.
    private external fun nativeSetFrameTimeline(handle: Long, expectedPresentDeltaNanos: Long)

    /** `0` means "no native side yet" — every native call is guarded on this. */
    private var handle: Long = 0

    /** Whether the Choreographer frame loop should keep re-posting itself. */
    private var running = false

    /**
     * A deep link delivered (by `MainActivity`) before [nativeInit] has
     * returned a non-zero handle. Cold-start links routinely arrive this
     * early — `MainActivity.onCreate` calls [onDeepLink] right after
     * constructing this view, well before `surfaceCreated` (and therefore
     * `nativeInit`) ever runs. Held here and flushed the moment `handle`
     * becomes non-zero (queue-until-handle-ready).
     */
    private var pendingDeepLink: String? = null

    /**
     * The last IME surface Rust published (from `nativeImeState`). Seeds a freshly
     * created [FrustInputConnection]'s mirror and `EditorInfo.initialSel*`, and
     * lets [pollImeAfterDispatch] detect active-state edges.
     */
    private var lastKnownState: ImeWireState? = null

    /** Whether the soft keyboard is currently requested (edge-detects show/hide). */
    private var imeActive = false

    /**
     * Frames remaining to re-poll the IME surface after an editor action
     * ([performEditorAction] / a hardware Enter). A submit clears/replaces the
     * field's text through the framework's *next-frame* rebuild — not
     * synchronously — so the [InputConnection]'s mirror [Editable] would keep the
     * pre-submit text and the next keystroke would append to it. Counting a few
     * frames lets [doFrame] observe the post-rebuild state and reseed the mirror
     * (see [resyncImeMirror]).
     */
    private var imeResyncFrames = 0

    /**
     * The live [FrustInputConnection] Gboard is bound to (set in
     * [onCreateInputConnection]). Held so a framework-side edit (a submit
     * clearing the field) can reseed its mirror [Editable] directly, instead of
     * only reacting to IME-originated calls.
     */
    private var activeConnection: FrustInputConnection? = null

    /**
     * The last `WindowInsetsCompat` this view received. Re-applied to
     * [dispatchInsets] whenever a new native handle is created
     * ([surfaceCreated]) so a recreated handle sees the current insets
     * immediately instead of waiting on the next system dispatch (which may
     * not come at all if nothing about the insets actually changed).
     */
    private var lastInsets: WindowInsetsCompat? = null

    /**
     * The last system-UI slot generation observed by [pollSystemUiState]
     * (the `frust::set_system_ui_mode` override). Starts at `0`,
     * matching the slot's initial (never-requested) generation, so an app
     * that never calls the API never fires [onSystemUiModeChanged].
     */
    private var lastSystemUiGeneration: Long = 0

    /**
     * The last brightness [updateSystemBarsAppearance] was actually applied
     * with, so [pollAppBrightness] only touches
     * `WindowInsetsControllerCompat` on a real change instead of re-setting
     * the same two booleans every Choreographer tick. `null` until the first
     * call ([surfaceCreated]), matching "nothing applied yet" distinctly from
     * either boolean value.
     */
    private var lastAppliedDark: Boolean? = null

    /**
     * Set by `MainActivity` (which owns the `Window` a
     * `WindowInsetsControllerCompat` needs) to receive a decoded
     * [SystemUiMode] whenever [pollSystemUiState] observes a fresh
     * `frust::set_system_ui_mode` call. `null` until the Activity
     * wires it up in `onCreate`; a mode observed before that is dropped —
     * mirrors this file's other startup-race cases (e.g. [pendingDeepLink]),
     * though in practice `onCreate` wires this listener up well before the
     * first Choreographer frame can observe a real generation change.
     */
    var onSystemUiModeChanged: ((SystemUiMode) -> Unit)? = null

    /**
     * The native-view host `MainActivity` wires up in `onCreate` (it owns the
     * [FrameLayout][android.widget.FrameLayout] root the host adds sibling
     * views to). `doFrame` polls `nativePlatformViewCommands` and hands each
     * non-null batch here (platform-views feature). The generated `MainActivity`
     * always wires this up; the nullability is a guard for a custom shell that
     * doesn't (and for the pre-`onCreate` window). Even an app that never hosts
     * a platform view keeps this set — the per-frame poll then just takes the
     * Rust side's null-on-no-change fast path (one cheap JNI call, no parse).
     */
    var platformViewHost: FrustViewHost? = null

    /**
     * Mode B input forwarding: the interactive
     * sibling view that owns the in-flight gesture, decided at touch-DOWN
     * (null = frust owns it). Cleared on UP/CANCEL.
     */
    private var platformViewForwardTarget: View? = null

    /**
     * The latest `expectedPresentationTimeNanos - frameTimeNanos` read off the
     * Choreographer frame timeline (API 33+) — how far ahead of *now* a window
     * frame committed on this tick is expected to reach the screen, which is
     * exactly the landing time a hosted native view's geometry has to match.
     * Pushed into Rust by [sampleFrameTimeline]; `0` until the first sample (and
     * again whenever sampling stops).
     *
     * Written by [frameTimelineCallback] and read by [sampleFrameTimeline], both
     * on the main thread (a `Choreographer` callback is dispatched on the thread
     * that posted it — the same thread [doFrame] runs on), so no synchronization
     * is needed.
     */
    private var expectedPresentDeltaNanos = 0L

    /**
     * One-shot frame-timeline sampler, re-posted by [sampleFrameTimeline] on
     * every frame that hosts a native sibling. `null` below API 33 — where the
     * `Choreographer.VsyncCallback` API does not exist and the scroll-sync tail
     * stays gate-only, which is already strictly better than the ungated
     * behaviour on every measured device.
     */
    private val frameTimelineCallback: Choreographer.VsyncCallback? =
        if (Build.VERSION.SDK_INT >= 33) {
            Choreographer.VsyncCallback { data ->
                expectedPresentDeltaNanos =
                    data.preferredFrameTimeline.expectedPresentationTimeNanos - data.frameTimeNanos
            }
        } else {
            null
        }

    private val imm: InputMethodManager
        get() = context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager

    private val scaleFactor: Float
        get() = resources.displayMetrics.density

    /** Whether the platform currently reports a dark appearance preference. */
    private val isDarkMode: Boolean
        get() = (resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) ==
            Configuration.UI_MODE_NIGHT_YES

    /**
     * Whether the platform currently reports a reduce-motion preference.
     *
     * Android has no single "reduce motion" switch: the user-facing controls
     * (Settings > Accessibility > *Remove animations*, and Developer options >
     * *Animator duration scale: off*) both land on
     * `Settings.Global.ANIMATOR_DURATION_SCALE`, and a scale of exactly `0`
     * means "no animations" — the same signal the platform's own
     * `ValueAnimator.areAnimatorsEnabled()` consults. Read with a default of
     * `1f` (the untouched-setting value), so a device that has never written
     * the row reports "animate normally" rather than "reduced".
     *
     * Deliberately NOT sourced from `Configuration`/`onConfigurationChanged`:
     * animation scale is not a configuration field, so that callback never
     * fires for it — hence [reduceMotionObserver].
     */
    private val reduceMotionEnabled: Boolean
        get() = Settings.Global.getFloat(
            context.contentResolver,
            Settings.Global.ANIMATOR_DURATION_SCALE,
            1f,
        ) == 0f

    /**
     * Change notification for [reduceMotionEnabled]. Registered on
     * [onResume] and unregistered on [onPause], so a backgrounded app holds no
     * resolver registration; the [onResume] re-read below covers the far more
     * common path — the user leaves the app to flip the setting and comes back
     * — while this observer covers a toggle made with the app still visible
     * (split screen, a quick-settings tile).
     *
     * Constructed against the main `Looper` so `onChange` is dispatched on the
     * same thread every other native call here runs on; the native side is not
     * thread-safe.
     */
    private val reduceMotionObserver = object : ContentObserver(Handler(Looper.getMainLooper())) {
        override fun onChange(selfChange: Boolean) {
            pushReduceMotion()
        }
    }

    /** Whether [reduceMotionObserver] is currently registered (idempotence guard). */
    private var reduceMotionObserverRegistered = false

    /**
     * Push the current [reduceMotionEnabled] value to the native side. A
     * missing handle is a no-op — [surfaceCreated] pushes once the handle
     * exists, mirroring `nativeSetAppearance`'s seeding.
     */
    private fun pushReduceMotion() {
        if (handle == 0L) return
        nativeSetReduceMotion(handle, reduceMotionEnabled)
    }

    private fun registerReduceMotionObserver() {
        if (reduceMotionObserverRegistered) return
        context.contentResolver.registerContentObserver(
            Settings.Global.getUriFor(Settings.Global.ANIMATOR_DURATION_SCALE),
            false,
            reduceMotionObserver,
        )
        reduceMotionObserverRegistered = true
    }

    private fun unregisterReduceMotionObserver() {
        if (!reduceMotionObserverRegistered) return
        context.contentResolver.unregisterContentObserver(reduceMotionObserver)
        reduceMotionObserverRegistered = false
    }

    init {
        holder.addCallback(this)
        // Required for a custom editor view to receive IME focus + text input.
        isFocusable = true
        isFocusableInTouchMode = true
        if (translucentSurface) {
            // Mode B (platform-views): an alpha render surface z-ordered on top
            // of the window, so native sibling views hosted in the FrameLayout
            // root BELOW this view show through the regions Frust paints
            // transparent. Paired with `nativeSetSurfaceMode(true)` before
            // `nativeInit` in [surfaceCreated].
            holder.setFormat(PixelFormat.TRANSLUCENT)
            setZOrderOnTop(true)
        }
        // Insets. Fires on attach and on every later system-bar/cutout/IME change.
        // This view is the FrameLayout root's render-surface child; any native
        // sibling views (platform-views feature) are positioned by Frust in
        // absolute paint coordinates and do not consume insets, so the original
        // `windowInsets` is returned unconsumed — every child still sees them.
        ViewCompat.setOnApplyWindowInsetsListener(this) { _, windowInsets ->
            lastInsets = windowInsets
            dispatchInsets(windowInsets)
            windowInsets
        }
    }

    /**
     * Compute and forward the platform window insets to `nativeOnInsetsChanged`
     * (physical px): `viewPadding` is the system-bar +
     * display-cutout occlusion, max-merged per edge (mirrors Flutter's
     * `FlutterView.onApplyWindowInsets`, `FlutterView.java:751-793`);
     * `viewInsets` is the IME area. A missing handle is a no-op — the insets
     * are still cached in [lastInsets] and re-dispatched once one exists (see
     * [surfaceCreated]).
     *
     * Pre-API-30 devices have no native `Type.ime()` insets; `WindowInsetsCompat`
     * falls back to its own best-effort IME detection there — an accepted
     * degradation, not a bug.
     */
    private fun dispatchInsets(windowInsets: WindowInsetsCompat) {
        if (handle == 0L) return
        val systemBars = windowInsets.getInsets(WindowInsetsCompat.Type.systemBars())
        val cutout = windowInsets.getInsets(WindowInsetsCompat.Type.displayCutout())
        val viewPadding = Insets.max(systemBars, cutout)
        val viewInsets = windowInsets.getInsets(WindowInsetsCompat.Type.ime())
        nativeOnInsetsChanged(
            handle,
            viewPadding.left.toFloat(),
            viewPadding.top.toFloat(),
            viewPadding.right.toFloat(),
            viewPadding.bottom.toFloat(),
            viewInsets.left.toFloat(),
            viewInsets.top.toFloat(),
            viewInsets.right.toFloat(),
            viewInsets.bottom.toFloat(),
        )
    }

    /**
     * Android back, called by `MainActivity`'s `onBackPressedDispatcher`
     * callback. Wraps `nativeOnBackPress`: `true` means the framework consumed the press
     * (it will pop on the next rebuild); `false` means `MainActivity` should
     * fall through to its default (finish) behavior. A missing handle
     * (native side not up yet) never claims the press.
     */
    fun dispatchBackPress(): Boolean {
        if (handle == 0L) return false
        return nativeOnBackPress(handle)
    }

    /**
     * Status/nav-bar icon contrast: light icons on a dark theme and vice
     * versa, the one `WindowInsetsControllerCompat` use that's real in Flutter's embedder
     * (`setSystemUIOverlayStyle` is not deprecated, but Frust uses the
     * AndroidX compat surface instead). `WindowCompat.getInsetsController`
     * (not the deprecated `ViewCompat.getWindowInsetsController(View)`)
     * needs the hosting `Activity`'s `Window` — always available here since
     * `MainActivity` is this view's sole constructor caller (see
     * `dev.frust.FrustSurfaceView`'s class doc).
     *
     * `dark` must be the APP's resolved brightness ([nativeAppIsDark]), not
     * the device's raw [isDarkMode] — the two legitimately disagree once an
     * app-forced `frust::set_app_theme` override is active (FINDINGS #43: the
     * device-sourced call this used to receive produced invisible
     * light-on-light or dark-on-dark icons whenever they did). Called from
     * [surfaceCreated], [onConfigurationChanged], and every frame via
     * [pollAppBrightness] (change-gated by [lastAppliedDark] there).
     */
    private fun updateSystemBarsAppearance(dark: Boolean) {
        lastAppliedDark = dark
        val window = (context as? Activity)?.window ?: return
        WindowCompat.getInsetsController(window, this).apply {
            isAppearanceLightStatusBars = !dark
            isAppearanceLightNavigationBars = !dark
        }
    }

    /**
     * Per-frame poll of the app's resolved brightness ([nativeAppIsDark]),
     * applying [updateSystemBarsAppearance] only on an actual change (mirrors
     * [pollSystemUiState]'s generation-gated shape, minus the generation — a
     * cheap direct `Boolean` compare is enough here since the value itself,
     * not a monotonic counter, is what Kotlin polls).
     *
     * This is what reaches a runtime `frust::set_app_theme`/`clear_app_theme`
     * call: unlike the device's `uiMode`, an app theme swap has no
     * `Configuration` event of its own to ride in on, so without this poll
     * the status bar would only catch up at the next unrelated
     * `onConfigurationChanged` (or never, if the device config never
     * changes). Called only while `handle != 0L` (see [doFrame]) — before
     * that, the native side has nothing to report, and the system bars are
     * simply left at the platform default until [surfaceCreated] seeds them.
     */
    private fun pollAppBrightness() {
        val dark = nativeAppIsDark(handle)
        if (dark == lastAppliedDark) return
        updateSystemBarsAppearance(dark)
    }

    /**
     * Set the frame rate hint on API 30+, requesting the display's maximum
     * refresh rate. This is a HINT only — OEM policy and device capabilities
     * override it. The Choreographer already follows the display's active rate
     * regardless of this hint; this call just signals the platform that our
     * surface can benefit from high-refresh rendering.
     */
    private fun setFrameRateHint(holder: SurfaceHolder) {
        if (Build.VERSION.SDK_INT < 30) return

        val display = display ?: return
        val modes = display.supportedModes.ifEmpty { return }
        val maxRefreshRate = modes.maxOf { it.refreshRate }

        if (Build.VERSION.SDK_INT >= 31) {
            holder.surface.setFrameRate(
                maxRefreshRate,
                Surface.FRAME_RATE_COMPATIBILITY_DEFAULT,
                Surface.CHANGE_FRAME_RATE_ALWAYS
            )
        } else {
            holder.surface.setFrameRate(
                maxRefreshRate,
                Surface.FRAME_RATE_COMPATIBILITY_DEFAULT
            )
        }
    }

    override fun surfaceCreated(holder: SurfaceHolder) {
        setFrameRateHint(holder)
        if (handle == 0L) {
            if (translucentSurface) {
                // Latch the alpha surface config on the Rust side BEFORE the
                // surface is created — a one-way, pre-init-only opt-in
                // paired with the holder's translucent format configured in
                // [init].
                nativeSetSurfaceMode(true)
            }
            // Initialize plugin platform handles before native init.
            // The application context is stable across activity recreation and is the
            // only context-like parameter plugins need.
            nativeInitPlatform(context.applicationContext)
            handle = nativeInit(holder.surface, scaleFactor, context.cacheDir.absolutePath)
            if (handle != 0L) {
                nativeSetAppearance(handle, isDarkMode)
                // Seed system-bar icon contrast from the APP's just-resolved
                // brightness, not the device's `isDarkMode` we just fed in —
                // with no override active yet they agree, but sourcing this
                // from the app keeps `surfaceCreated` on the same one true
                // path [pollAppBrightness]/[onConfigurationChanged] use (see
                // [updateSystemBarsAppearance]'s doc).
                updateSystemBarsAppearance(nativeAppIsDark(handle))
                // Seed the reduce-motion accessibility preference beside the
                // appearance: [onResume] already ran (and re-runs on every
                // foreground), but it fires before this handle exists on a cold
                // start, so its push was a no-op.
                pushReduceMotion()
                // Attach the accesskit accessibility adapter to this view.
                // Best-effort: the native side isolates any
                // failure in its own guard, so a missing delegate class or JNI
                // hiccup degrades to "no a11y" rather than blocking startup.
                nativeInitAccessibility(handle, this)
                // Re-dispatch the last known insets: a recreated
                // handle must not stay stale until the next system dispatch,
                // which may never come if nothing about the insets changed.
                lastInsets?.let { dispatchInsets(it) }
                // Flush a deep link that arrived before this handle existed
                // (see `pendingDeepLink`'s doc comment) — a cold-start link
                // must not be silently dropped just because it raced ahead
                // of `nativeInit`.
                pendingDeepLink?.let { url ->
                    pendingDeepLink = null
                    nativeOnDeepLink(handle, url)
                }
            }
        } else {
            nativeOnSurfaceChanged(handle, holder.surface, width, height, scaleFactor)
        }
    }

    /**
     * Deliver a platform deep link — called by `MainActivity` for both the
     * cold-start link (`onCreate`'s `intent?.data`) and a running-app link
     * (`onNewIntent`) uniformly. Queues until [nativeInit] has returned a
     * handle if the link arrives first (see [pendingDeepLink]); a `null`
     * `url` (no data on the intent) is a no-op.
     */
    fun onDeepLink(url: String?) {
        if (url == null) return
        if (handle == 0L) {
            pendingDeepLink = url
        } else {
            nativeOnDeepLink(handle, url)
        }
    }

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
        setFrameRateHint(holder)
        if (handle != 0L) {
            nativeOnSurfaceChanged(handle, holder.surface, width, height, scaleFactor)
        }
    }

    /**
     * The `android:configChanges` manifest entry includes `uiMode`, so a
     * system light/dark toggle reaches here instead of recreating the
     * activity — re-seed the theme's brightness from the fresh configuration.
     *
     * Only appearance: the reduce-motion preference is a `Settings.Global`
     * row, not a `Configuration` field, so it never reaches this callback —
     * see [reduceMotionObserver].
     */
    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        if (handle != 0L) {
            nativeSetAppearance(handle, isDarkMode)
            // Re-derive system-bar icon contrast from the APP's just-resolved
            // brightness, not `isDarkMode` directly: if an app-forced
            // `frust::set_app_theme` override is active, `nativeSetAppearance`
            // above is a no-op on `theme.brightness` (the override-wins
            // rule — see `frust_shell_common::theme_override`), so re-reading
            // the device here would have flipped the icons to disagree with
            // what the override is actually painting. `nativeAppIsDark`
            // reports the resolved value either way.
            updateSystemBarsAppearance(nativeAppIsDark(handle))
        }
    }

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        if (handle != 0L) {
            nativeOnSurfaceDestroyed(handle)
        }
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (handle == 0L) {
            return false
        }
        // Mode B input forwarding: touch-DOWN decides
        // ownership for the WHOLE gesture — a down inside an interactive
        // slot's rect (outside its z-shields) hands this and every subsequent
        // event of the gesture to the native sibling; frust never sees any of
        // it. No mid-gesture handoff in v1.
        if (event.actionMasked == MotionEvent.ACTION_DOWN) {
            platformViewForwardTarget = platformViewHost?.interactiveTargetAt(event.x, event.y)
        }
        val forwardTarget = platformViewForwardTarget
        if (forwardTarget != null) {
            val copy = MotionEvent.obtain(event)
            // Sibling views are positioned via translationX/Y in the shared
            // FrameLayout root, which this surface view fills at (0,0) — so
            // window-space and surface-space coincide, and the child's local
            // space is a pure translation.
            copy.offsetLocation(-forwardTarget.translationX, -forwardTarget.translationY)
            forwardTarget.dispatchTouchEvent(copy)
            copy.recycle()
            if (event.actionMasked == MotionEvent.ACTION_UP ||
                event.actionMasked == MotionEvent.ACTION_CANCEL
            ) {
                platformViewForwardTarget = null
            }
            return true
        }
        // Map Android's masked action to our fixed 0..3 ABI (see `nativeOnTouch`).
        // Single-pointer in v1: we forward the primary pointer's location only.
        val action = when (event.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> 0
            MotionEvent.ACTION_MOVE -> 1
            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP -> 2
            MotionEvent.ACTION_CANCEL -> 3
            else -> return false
        }
        nativeOnTouch(handle, action, event.x, event.y)
        // The tree may have taken/dropped focus in response to this contact; poll
        // the IME surface and show/hide the keyboard accordingly. Polling after
        // the dispatch keeps the JNI ABI one-directional (no native up-calls).
        pollImeAfterDispatch()
        return true
    }

    /**
     * Forward a hardware / injected **Enter** to the focused editable as the
     * field's editor action (submit), the same path Gboard's Done key drives
     * through [FrustInputConnection.performEditorAction].
     *
     * Soft-keyboard text (including its own Enter) arrives through the
     * [InputConnection]; but a *physical* keyboard — and `adb shell input
     * keyevent 66` — dispatches key events straight to the focused view, which
     * an `InputConnection`-only editor would otherwise drop. Single-line v1
     * fields treat Enter as submit (`imeOptions = IME_ACTION_DONE`), so route it
     * to the existing `nativeImeAction` seam rather than inserting a newline.
     * Only Enter is intercepted while a field is focused; every other key falls
     * through to the default handling (soft-keyboard input stays the IME's job).
     */
    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        if (handle != 0L && imeActive &&
            (keyCode == KeyEvent.KEYCODE_ENTER || keyCode == KeyEvent.KEYCODE_NUMPAD_ENTER)
        ) {
            nativeImeAction(handle, EditorInfo.IME_ACTION_DONE)
            pollImeAfterDispatch()
            imeResyncFrames = IME_RESYNC_FRAMES
            return true
        }
        return super.onKeyDown(keyCode, event)
    }

    /** A custom editor view MUST return true here to be offered an InputConnection. */
    override fun onCheckIsTextEditor(): Boolean = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
        outAttrs.inputType = InputType.TYPE_CLASS_TEXT
        outAttrs.imeOptions = EditorInfo.IME_ACTION_DONE
        val state = lastKnownState
        outAttrs.initialSelStart = state?.selBase ?: -1
        outAttrs.initialSelEnd = state?.selExt ?: -1
        return FrustInputConnection().also {
            it.seed(state)
            activeConnection = it
        }
    }

    /**
     * After every native dispatch, reconcile the soft keyboard with the focused
     * widget's IME surface: newly-active ⇒ take focus + show the keyboard (and
     * restart input so a fresh [FrustInputConnection] is seeded from the new
     * state); newly-inactive ⇒ hide it.
     */
    private fun pollImeAfterDispatch() {
        if (handle == 0L) return
        val state = parseImeState(nativeImeState(handle)) ?: return
        lastKnownState = state
        if (state.active && !imeActive) {
            imeActive = true
            requestFocus()
            // Rebuild the InputConnection so its mirror starts from `state`.
            imm.restartInput(this)
            imm.showSoftInput(this, 0)
        } else if (!state.active && imeActive) {
            imeActive = false
            imm.hideSoftInputFromWindow(windowToken, 0)
        } else if (state.active) {
            // Steady active (no show/hide edge): reconcile the live mirror to the
            // focused field's published state — a caret moved by a tap, or a
            // whole-field text change from a field switch or a submit-clear.
            // `reconcileTo` compares against the LIVE editable (not the racing
            // `lastKnownState`), so a pre-advanced snapshot can't hide a change.
            activeConnection?.reconcileTo(state)
        }
    }

    /**
     * Per-frame poll of the process-wide system-UI override slot
     * (`frust::set_system_ui_mode`) — decodes
     * `nativeSystemUiState`'s packed `(generation, mode)` `u64` and hands a
     * decoded [SystemUiMode] to [onSystemUiModeChanged] only when the
     * generation has advanced since the last poll (mirrors
     * [pollImeAfterDispatch]'s per-frame shape: a cheap JNI long read, the
     * proven idiom in this exact callback). Generation `0` (never called)
     * applies nothing — platform defaults stand until the app calls the API.
     */
    private fun pollSystemUiState() {
        val encoded = nativeSystemUiState(handle)
        val generation = encoded ushr 8
        if (generation == lastSystemUiGeneration) return
        lastSystemUiGeneration = generation
        if (generation == 0L) return
        onSystemUiModeChanged?.invoke(decodeSystemUiMode(encoded and 0xFFL))
    }

    private fun parseImeState(json: String?): ImeWireState? {
        if (json == null) return null
        return try {
            val obj = JSONObject(json)
            ImeWireState(
                active = obj.optBoolean("active", false),
                text = obj.optString("text", ""),
                selBase = obj.optInt("selBase", -1),
                selExt = obj.optInt("selExt", -1),
                compBase = obj.optInt("compBase", -1),
                compExt = obj.optInt("compExt", -1),
            )
        } catch (e: JSONException) {
            null
        }
    }

    /** A plain snapshot of the editing state exchanged with the Rust seam. */
    private data class ImeWireState(
        val active: Boolean,
        val text: String,
        val selBase: Int,
        val selExt: Int,
        val compBase: Int,
        val compExt: Int,
    )

    override fun doFrame(frameTimeNanos: Long) {
        if (!running) {
            return
        }
        // `onResume` starts this loop before the surface exists, so the first
        // frames run with `handle == 0` (the native side isn't created until
        // `surfaceCreated`). Keep re-posting regardless — only skip the native
        // frame call itself while there's no handle — otherwise the loop would
        // terminate before `surfaceCreated` ever ran and nothing would render.
        // Once the surface is lost the handle stays valid and the native frame
        // is a cheap no-op, so re-posting is always correct while `running`.
        if (handle != 0L) {
            // Platform-view scroll sync: hand the native side
            // this tick's frame-timeline sample BEFORE `nativeOnFrame`, which is
            // where the scroll-sync tail advances. Inert (one `Long` field
            // compare) unless a native sibling is actually on screen.
            sampleFrameTimeline()
            // A `false` return is a FATAL, unrecoverable render-thread failure
            // (first-surface install could not succeed) — stop the loop rather
            // than drive doomed frames against a permanent black screen. This is
            // the Android counterpart to iOS's `initFailed` displayLink
            // invalidation; a new branch beside the `handle == 0L` guard above.
            if (!nativeOnFrame(handle, frameTimeNanos)) {
                running = false
                return
            }
            // Per-frame IME reconcile: pick up caret moves, field switches, and
            // submit-clears that no InputConnection callback originated.
            // `pollImeAfterDispatch` reconciles the live mirror to the focused
            // field's published state; a no-op when they already match, so normal
            // typing never triggers a spurious restart.
            pollImeAfterDispatch()
            // Per-frame system-UI poll: cheap generation-gated
            // JNI read, applied only on an actual `set_system_ui_mode` change.
            pollSystemUiState()
            // Per-frame app-brightness poll: cheap `Boolean`-gated
            // JNI read, applied only on an actual change — this is what picks
            // up a runtime `frust::set_app_theme`/`clear_app_theme` call
            // (see [pollAppBrightness]'s doc for why that needs a per-frame
            // poll rather than riding on `onConfigurationChanged`).
            pollAppBrightness()
            // Per-frame platform-view command poll:
            // the null-return fast path keeps a no-change frame allocation-free
            // (a null string means "nothing to do" — no JSON parse); only a
            // non-null batch reaches the host applier. An app with no platform
            // views still wires the host, so this runs every frame — but it's
            // just one cheap JNI call returning null (the `nativeSystemUiState`
            // cost bar above).
            platformViewHost?.let { host ->
                val commands = nativePlatformViewCommands(handle, host.ackedGeneration)
                if (commands != null) {
                    host.applyCommands(commands)
                }
            }
        }
        Choreographer.getInstance().postFrameCallback(this)
    }

    /**
     * Push this tick's frame-timeline sample to the native scroll-sync tail and
     * arm the next one.
     *
     * Sampling is **scoped to frames that actually host a native sibling**: the
     * tail exists only to align a hosted view's geometry with the Frust content
     * behind it, so an app with no platform view — the overwhelming majority —
     * posts no vsync callback and makes no extra JNI call beyond the single
     * `0`-valued push that stands the tail down when the last slot goes away.
     *
     * Below API 33 ([frameTimelineCallback] `== null`) there is no timeline to
     * read and the native side stays gate-only, which measured strictly ahead
     * of the ungated behaviour on every device tested.
     */
    private fun sampleFrameTimeline() {
        val sampling = frameTimelineCallback != null && platformViewHost?.hasHostedViews == true
        if (!sampling) {
            // Stand the tail down exactly once when sampling stops (the last
            // hosted view was disposed, or this app never had one): a stale
            // delta must not keep deriving a hold for geometry nobody is
            // showing.
            if (expectedPresentDeltaNanos != 0L) {
                expectedPresentDeltaNanos = 0L
                nativeSetFrameTimeline(handle, 0L)
            }
            return
        }
        nativeSetFrameTimeline(handle, expectedPresentDeltaNanos)
        if (Build.VERSION.SDK_INT >= 33) {
            Choreographer.getInstance().postVsyncCallback(frameTimelineCallback!!)
        }
    }

    /**
     * Reconcile the IME with the focused widget's *current* published editing
     * state after a framework-initiated edit (a submit clearing the field) that
     * the [InputConnection] did not originate. If the text changed since the IME
     * last knew it, its mirror [Editable] is stale, so `restartInput` rebuilds
     * the connection — reseeding the mirror from the fresh state
     * ([onCreateInputConnection] seeds from [lastKnownState]). A pure
     * selection/caret move (same text) needs no restart.
     */
    private fun resyncImeMirror() {
        if (handle == 0L) return
        val state = parseImeState(nativeImeState(handle)) ?: return
        val prev = lastKnownState
        lastKnownState = state
        if (state.active && prev != null && prev.text != state.text) {
            // Reseed the live connection's mirror to match the widget, then tell
            // the IMM the text/selection changed under it and restart input so
            // Gboard re-reads from the fresh (e.g. cleared) editable.
            activeConnection?.seed(state)
            imm.updateSelection(this, state.selBase, state.selExt, state.compBase, state.compExt)
            imm.restartInput(this)
        }
    }

    /** Called by `MainActivity.onResume` — starts the Choreographer loop. */
    fun onResume() {
        running = true
        if (handle != 0L) {
            nativeOnResume(handle)
        }
        // Reduce motion: re-read on every foreground and (re)arm the observer.
        // The setting lives in Settings/Developer options, so the user is
        // necessarily out of this app while flipping it — this re-read, not the
        // observer, is the path that normally catches the change.
        registerReduceMotionObserver()
        pushReduceMotion()
        Choreographer.getInstance().postFrameCallback(this)
    }

    /** Called by `MainActivity.onPause` — stops the Choreographer loop. */
    fun onPause() {
        running = false
        Choreographer.getInstance().removeFrameCallback(this)
        // Hold no resolver registration while backgrounded; [onResume] re-arms
        // it and re-reads the value that may have changed in between.
        unregisterReduceMotionObserver()
        if (handle != 0L) {
            nativeOnPause(handle)
        }
    }

    /** Called by `MainActivity.onDestroy` — releases the native handle. */
    fun onDestroy() {
        // Belt-and-suspenders: [onPause] normally precedes destruction, but a
        // ContentObserver outliving its view would keep calling into a dead
        // handle.
        unregisterReduceMotionObserver()
        if (handle != 0L) {
            nativeOnDestroy(handle)
            handle = 0
        }
    }

    /**
     * A [BaseInputConnection] subclass over a mirror [Editable] (the Flutter
     * `InputConnectionAdaptor` model). `super` performs the composition mechanics
     * against [editable]; each override then [sync]s the whole editing state into
     * Rust and reconciles the state Rust returns back into the mirror + `IMM`.
     *
     * `fullEditor = true` so `BaseInputConnection` edits our own [getEditable]
     * rather than dispatching key events to the (non-existent) view text machinery.
     */
    private inner class FrustInputConnection :
        BaseInputConnection(this@FrustSurfaceView, true) {

        private val editable = SpannableStringBuilder()

        /** Nesting depth of `beginBatchEdit`/`endBatchEdit`; sync once at depth 0. */
        private var batchDepth = 0

        /**
         * The last state we pushed to Rust. Breaks the state-sync loop
         * (IMM update → IME reacts → apply → update …) by short-circuiting a
         * [sync] whose editable snapshot is unchanged (Flutter's batch-edit
         * final-state model).
         */
        private var lastPushed: ImeWireState? = null

        override fun getEditable(): Editable = editable

        /** Initialise the mirror from a Rust-published state (fresh connection). */
        fun seed(state: ImeWireState?) {
            editable.clear()
            lastPushed = null
            if (state == null) return
            editable.append(state.text)
            val len = editable.length
            val selStart = state.selBase.coerceIn(0, len)
            val selEnd = state.selExt.coerceIn(0, len)
            if (state.selBase >= 0 && state.selExt >= 0) {
                Selection.setSelection(editable, selStart, selEnd)
            }
        }

        /**
         * Reconcile the mirror to a Rust-published state the IME did not
         * originate: a tap that moved the caret (selection-only), or a whole-field
         * text change from a field switch / submit-clear. Compares against the
         * LIVE editable (not `lastKnownState`) so a pre-advanced snapshot cannot
         * hide a divergence. Selection-only changes move the IMM cursor without a
         * `restartInput` (which would drop any active composition).
         */
        fun reconcileTo(state: ImeWireState) {
            val curText = editable.toString()
            val curStart = Selection.getSelectionStart(editable)
            val curEnd = Selection.getSelectionEnd(editable)
            if (curText != state.text) {
                seed(state)
                imm.updateSelection(this@FrustSurfaceView, state.selBase, state.selExt, state.compBase, state.compExt)
                imm.restartInput(this@FrustSurfaceView)
            } else if (curStart != state.selBase || curEnd != state.selExt) {
                val len = editable.length
                if (state.selBase in 0..len && state.selExt in 0..len) {
                    Selection.setSelection(editable, state.selBase, state.selExt)
                }
                imm.updateSelection(this@FrustSurfaceView, state.selBase, state.selExt, state.compBase, state.compExt)
                // A later identical `sync()` must not short-circuit on a stale snapshot.
                lastPushed = null
            }
        }

        override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
            val handled = super.commitText(text, newCursorPosition)
            sync()
            return handled
        }

        override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean {
            val handled = super.setComposingText(text, newCursorPosition)
            sync()
            return handled
        }

        override fun setComposingRegion(start: Int, end: Int): Boolean {
            val handled = super.setComposingRegion(start, end)
            sync()
            return handled
        }

        override fun finishComposingText(): Boolean {
            val handled = super.finishComposingText()
            sync()
            return handled
        }

        override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
            val handled = super.deleteSurroundingText(beforeLength, afterLength)
            sync()
            return handled
        }

        override fun deleteSurroundingTextInCodePoints(
            beforeLength: Int,
            afterLength: Int,
        ): Boolean {
            val handled = super.deleteSurroundingTextInCodePoints(beforeLength, afterLength)
            sync()
            return handled
        }

        override fun setSelection(start: Int, end: Int): Boolean {
            val handled = super.setSelection(start, end)
            sync()
            return handled
        }

        override fun sendKeyEvent(event: KeyEvent): Boolean {
            // `BaseInputConnection.sendKeyEvent` does NOT edit the mirror for
            // KEYCODE_DEL/FORWARD_DEL — it only dispatches the key to the view.
            // Many soft keyboards (e.g. Gboard) deliver backspace this way rather
            // than via `deleteSurroundingText`, so perform the edit on the mirror
            // ourselves (at its current selection), then let `sync()` push it.
            if (event.action == KeyEvent.ACTION_DOWN &&
                (event.keyCode == KeyEvent.KEYCODE_DEL ||
                    event.keyCode == KeyEvent.KEYCODE_FORWARD_DEL)
            ) {
                val selStart = Selection.getSelectionStart(editable)
                val selEnd = Selection.getSelectionEnd(editable)
                val lo = minOf(selStart, selEnd)
                val hi = maxOf(selStart, selEnd)
                if (lo != hi) {
                    editable.delete(lo, hi)
                } else if (event.keyCode == KeyEvent.KEYCODE_DEL && lo > 0) {
                    editable.delete(lo - 1, lo)
                } else if (event.keyCode == KeyEvent.KEYCODE_FORWARD_DEL && lo < editable.length) {
                    editable.delete(lo, lo + 1)
                }
                sync()
                return true
            }
            // Other keys (hardware ENTER/etc.): let `super` dispatch, then sync.
            val handled = super.sendKeyEvent(event)
            if (event.action == KeyEvent.ACTION_DOWN) {
                sync()
            }
            return handled
        }

        override fun beginBatchEdit(): Boolean {
            batchDepth++
            return super.beginBatchEdit()
        }

        override fun endBatchEdit(): Boolean {
            val handled = super.endBatchEdit()
            if (batchDepth > 0) {
                batchDepth--
            }
            if (batchDepth == 0) {
                sync()
            }
            return handled
        }

        override fun performEditorAction(actionCode: Int): Boolean {
            if (handle != 0L) {
                nativeImeAction(handle, actionCode)
                // A submit clears the field on the next-frame rebuild; re-poll for
                // a few frames so the mirror is reseeded (see [resyncImeMirror]).
                imeResyncFrames = IME_RESYNC_FRAMES
            }
            return true
        }

        /**
         * Push the whole mirror editing state to Rust, then reconcile the state
         * Rust returns back into the mirror and the `IMM`.
         *
         * No-op inside a batch edit (synced once at depth 0) or when the state is
         * unchanged since the last push (loop guard). Otherwise it always calls
         * `imm.updateSelection` — skipping it breaks composition/prediction — and
         * `imm.restartInput` when Rust replaced the text wholesale.
         */
        private fun sync() {
            if (batchDepth > 0 || handle == 0L) return

            val text = editable.toString()
            val selStart = Selection.getSelectionStart(editable)
            val selEnd = Selection.getSelectionEnd(editable)
            val compStart = getComposingSpanStart(editable)
            val compEnd = getComposingSpanEnd(editable)
            val snapshot = ImeWireState(true, text, selStart, selEnd, compStart, compEnd)
            if (snapshot == lastPushed) return
            lastPushed = snapshot

            nativeImeApply(handle, text, selStart, selEnd, compStart, compEnd)

            val state = parseImeState(nativeImeState(handle)) ?: return
            lastKnownState = state

            val wholesale = state.text != text
            if (wholesale) {
                // Rust replaced the text: rebuild the mirror to match.
                editable.replace(0, editable.length, state.text)
            }
            val len = editable.length
            if (state.selBase in 0..len && state.selExt in 0..len) {
                Selection.setSelection(editable, state.selBase, state.selExt)
            }
            // Sync the IMM on every (state-changing) sync — `updateSelection`'s
            // last two args are the *candidates* (composing) range, not named
            // composing params (a common miscoding).
            imm.updateSelection(
                this@FrustSurfaceView,
                state.selBase,
                state.selExt,
                state.compBase,
                state.compExt,
            )
            if (wholesale) {
                imm.restartInput(this@FrustSurfaceView)
            }
        }
    }
}
