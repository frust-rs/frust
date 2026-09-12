package dev.frust

import android.app.Activity
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.res.Configuration
import android.database.ContentObserver
import android.graphics.PixelFormat
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.Settings
import android.text.Editable
import android.text.InputType
import android.text.Selection
import android.text.SpannableStringBuilder
import android.util.Log
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
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
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
 * Clipboard access is this side's job (`ClipboardManager` is a system service
 * with no Rust-reachable counterpart): the framework publishes copy/paste
 * intent, [syncClipboard] drains it once per frame, and every inbound verb —
 * the IME's own actions ([FrustInputConnection.performContextMenuAction]), a
 * hardware `Ctrl` chord ([onKeyDown]), and the answer to a paste request —
 * arrives through the one `nativeEditCommand` seam.
 *
 * **The selection toolbar is the framework's, not the platform's.** Frust draws
 * its own bar over a selection
 * (`frust_core::SelectionToolbarPolicy::Framework`, the default), so this view
 * deliberately installs **no `ActionMode.Callback` and no `GestureDetector`**.
 * Adding either would raise a second, competing toolbar over the framework's
 * own and take the long-press that summons it — the same choice Flutter makes
 * on this platform.
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
         * How long a URI-backed clipboard resolution's answer stays wanted,
         * in milliseconds of `SystemClock.uptimeMillis` (which does not
         * advance in deep sleep, so a device suspended mid-read does not
         * burn the budget).
         *
         * `ClipData.Item.coerceToText` reaches into the CLIP OWNER's process,
         * so how long it takes — or whether it returns at all — is that app's
         * decision, not this one's. Two seconds is well past any healthy
         * `ContentProvider` round-trip and well short of the user having
         * moved on and forgotten they asked. An answer arriving later is
         * dropped rather than pasted: see [readClipboardTextAsync].
         */
        private const val CLIPBOARD_RESOLUTION_TIMEOUT_MS = 2_000L

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
         * Wire values `nativeImeState`'s `"contentType"` field carries
         * (encoded on the Rust side by
         * `frust_shell_android::jni_glue::content_type_wire` from
         * `frust_core::event::ImeContentType`). `"password"` has no named
         * constant here — every unrecognized/unhandled value, including it,
         * falls through to [applyImeContentType]'s fail-closed `else` arm.
         */
        private const val CONTENT_TYPE_NORMAL = "normal"
        private const val CONTENT_TYPE_NO_SUGGESTIONS = "noSuggestions"
        private const val CONTENT_TYPE_TERMINAL = "terminal"

        /**
         * `nativeEditCommand`'s fixed command ABI, decoded on the Rust side by
         * `frust_shell_android::ffi_support::edit_command_from_code` — DO NOT
         * renumber without changing both sides. An unrecognised code is
         * dropped there rather than defaulted to a verb, since every verb here
         * edits the focused field's document.
         */
        private const val EDIT_COMMAND_COPY = 0
        private const val EDIT_COMMAND_CUT = 1
        private const val EDIT_COMMAND_PASTE = 2
        private const val EDIT_COMMAND_SELECT_ALL = 3

        /** logcat tag for this view's best-effort clipboard diagnostics. */
        private const val TAG = "frust"

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

    // Clipboard exports. The host clipboard is the JVM's
    // (`ClipboardManager` is a system service with no Rust-reachable
    // counterpart), so the framework publishes intent and this side performs
    // the actual read/write — see [syncClipboard].

    // Drains (and clears) the text a focused editable asked to put on the
    // system clipboard, or null when nothing was copied / there is no live
    // handle. A one-shot edge: a drained value that is dropped here is lost.
    private external fun nativeTakeClipboardWrite(handle: Long): String?

    // Drains (and clears) whether a focused editable asked this side to read
    // the system clipboard back to it. The answer is delivered as a separate
    // [nativeEditCommand] paste call rather than a return value, because the
    // read happens here and may legitimately yield nothing.
    private external fun nativeTakePasteRequest(handle: Long): Boolean

    // One decoded clipboard/selection verb for the focused editable. `cmd` is a
    // fixed numeric ABI shared with the Rust `nativeEditCommand` glue — DO NOT
    // renumber without changing both sides:
    //   0 = copy
    //   1 = cut
    //   2 = paste (`text` carries the clipboard content)
    //   3 = select all
    // `text` is read only for a paste and is null for every other verb. An
    // unrecognised code is dropped on the Rust side, never defaulted to a verb.
    // See [EDIT_COMMAND_COPY] and friends for the constants to pass.
    private external fun nativeEditCommand(handle: Long, cmd: Int, text: String?)

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

    /**
     * The framework's focus/IME session generation, used to bind an async
     * URI-backed clipboard resolution to the session that asked for it
     * ([readClipboardTextAsync]). A cheap `jlong` read, the same shape
     * [nativeSystemUiState] above uses. `0` means no live native handle, and
     * no real generation is ever `0`.
     */
    private external fun nativeFocusGeneration(handle: Long): Long

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

    /**
     * The system clipboard, resolved per access like [imm] rather than cached:
     * both are system services whose instance is not this view's to own across
     * a configuration change, and both are only touched on user-driven paths
     * (never per frame — [syncClipboard]'s per-frame poll is a JNI call, and it
     * only reaches this property when the framework actually asked for a copy
     * or a paste).
     */
    private val clipboard: ClipboardManager
        get() = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager

    /**
     * Main-thread handle used to hop a URI-backed clipboard resolution's
     * answer back onto the UI thread — see [readClipboardTextAsync]. A
     * dedicated field (rather than an inline `Handler(Looper.getMainLooper())`
     * per call, the way [reduceMotionObserver] constructs its own) since this
     * one is posted to repeatedly over the view's lifetime.
     */
    private val mainHandler = Handler(Looper.getMainLooper())

    /**
     * The single background thread a URI-backed clipboard item's
     * [ClipData.Item.coerceToText] round-trip runs on — never one thread per
     * paste. Lazily created by [clipboardWorker] on first use and torn down by
     * [shutdownClipboardExecutor] at every point a live handle stops being
     * trustworthy ([surfaceDestroyed], [onPause], [onDestroy]), so no thread
     * outlives this view. `null` until the first URI-backed paste is asked
     * for; most apps that never copy a URI-backed clip never create it.
     */
    private var clipboardExecutor: ExecutorService? = null

    /**
     * Bumped by [shutdownClipboardExecutor] every time it runs. A resolution
     * captures the epoch it started under ([readClipboardTextAsync]); if the
     * epoch has moved by the time the answer reaches [mainHandler], the view
     * tore down while the read was in flight and the answer is dropped
     * instead of dispatched, even though [shutdownClipboardExecutor]'s
     * `shutdownNow()` cannot guarantee the underlying `ContentProvider` call
     * actually honours the interrupt.
     */
    private var clipboardResolutionEpoch = 0L

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
        // Stop (and forget) any in-flight URI-backed clipboard resolution
        // before anything else here: the render surface going away is one of
        // the points nothing should still land a late paste into.
        shutdownClipboardExecutor()
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
     *
     * The hardware **clipboard chords** (`Ctrl+C`/`X`/`V`/`A`) are intercepted
     * on the same terms and decoded here into [nativeEditCommand] verbs, so the
     * framework never has to know which chord means copy on which host. A
     * *soft* keyboard never sends these — Gboard drives copy/paste through
     * [FrustInputConnection.performContextMenuAction] (or plain `commitText`)
     * instead — so this path exists exclusively for a physical keyboard and for
     * injected `adb shell input keyevent` presses.
     *
     * Only Enter and those four chords are intercepted while a field is
     * focused; every other key falls through to the default handling
     * (soft-keyboard input stays the IME's job). The `imeActive` gate is what
     * keeps a chord from being swallowed when no editable is focused: the
     * framework routes an edit command to the focus path, so with nothing
     * focused there would be nobody to answer it, and consuming the press would
     * take it away from the host activity for nothing.
     */
    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        if (handle != 0L && imeActive &&
            (keyCode == KeyEvent.KEYCODE_ENTER || keyCode == KeyEvent.KEYCODE_NUMPAD_ENTER)
        ) {
            nativeImeAction(handle, EditorInfo.IME_ACTION_DONE)
            pollImeAfterDispatch()
            return true
        }
        if (handle != 0L && imeActive && event.isCtrlPressed) {
            val command = when (keyCode) {
                KeyEvent.KEYCODE_C -> EDIT_COMMAND_COPY
                KeyEvent.KEYCODE_X -> EDIT_COMMAND_CUT
                KeyEvent.KEYCODE_V -> EDIT_COMMAND_PASTE
                KeyEvent.KEYCODE_A -> EDIT_COMMAND_SELECT_ALL
                else -> null
            }
            if (command != null) {
                dispatchEditCommand(command)
                return true
            }
        }
        return super.onKeyDown(keyCode, event)
    }

    /**
     * Send one decoded clipboard/selection verb to the focused editable and
     * reconcile the IME mirror in the same pass — the shared body behind both
     * host-driven routes ([onKeyDown]'s hardware chords and
     * [FrustInputConnection.performContextMenuAction]'s IME menu actions).
     *
     * A paste reads the clipboard here, on the JVM side that owns it, and is
     * simply not sent when the read yields nothing ([readClipboardText]
     * documents when that happens) — including the case where the clip is
     * URI-backed and [readClipboardText] has instead handed the item to
     * [readClipboardTextAsync], which dispatches on its own once (and if) the
     * text arrives. The press is still reported consumed by the callers in
     * that case: the user asked a focused field to paste, and letting the
     * chord fall through to the platform afterwards would be a second,
     * unrelated interpretation of the same press.
     *
     * The synchronous [pollImeAfterDispatch] is the same two-step shape
     * [FrustInputConnection.performEditorAction] uses: the verb changed the
     * field's text or selection *during this event pass*, and without the poll
     * the mirror [Editable] would keep the pre-edit state until some other IME
     * callback happened to push it back to Rust.
     */
    private fun dispatchEditCommand(command: Int) {
        if (command == EDIT_COMMAND_PASTE) {
            val text = readClipboardText()
            if (text.isNullOrEmpty()) {
                return
            }
            nativeEditCommand(handle, command, text)
        } else {
            nativeEditCommand(handle, command, null)
        }
        pollImeAfterDispatch()
    }

    /** A custom editor view MUST return true here to be offered an InputConnection. */
    override fun onCheckIsTextEditor(): Boolean = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
        val state = lastKnownState
        applyImeContentType(outAttrs, state?.contentType ?: CONTENT_TYPE_NORMAL)
        outAttrs.initialSelStart = state?.selBase ?: -1
        outAttrs.initialSelEnd = state?.selExt ?: -1
        return FrustInputConnection().also {
            it.seed(state)
            activeConnection = it
        }
    }

    /**
     * Map the wire `"contentType"` string (`"normal"` / `"password"` /
     * `"noSuggestions"` / `"terminal"` — encoded on the Rust side by
     * `frust_shell_android::jni_glue::content_type_wire` from
     * `frust_core::event::ImeContentType`) onto the `EditorInfo`/`InputType`
     * flags that actually close FINDINGS #31: typing into a `Password` field
     * must not surface the composed text in Gboard's suggestion strip, nor
     * let Gboard commit it to its learned-word dictionary.
     *
     * `TYPE_TEXT_VARIATION_PASSWORD` is what switches to secure entry;
     * `TYPE_TEXT_FLAG_NO_SUGGESTIONS` and `IME_FLAG_NO_PERSONALIZED_LEARNING`
     * are set alongside it, never omitted — the former suppresses the
     * suggestion strip, the latter stops the IME persisting the secret into
     * its learned word list, the *persistent* half of the leak.
     * `"noSuggestions"` is the non-secret sibling: entry stays visible, but
     * `TYPE_TEXT_FLAG_NO_SUGGESTIONS` still switches off the suggestion strip
     * and autocorrect, and `IME_FLAG_NO_PERSONALIZED_LEARNING` still stops
     * the typed text feeding the IME's learned-word dictionary (no secure-
     * entry masking — this is not a secret field).
     *
     * `"terminal"` (task F1) is a raw byte-entry surface: same non-secret
     * flags as `"noSuggestions"` (`TYPE_TEXT_FLAG_NO_SUGGESTIONS` +
     * `IME_FLAG_NO_PERSONALIZED_LEARNING`) — Android's `InputType`/`EditorInfo`
     * vocabulary has no separate "no smart punctuation" bit the way iOS's
     * `UITextInputTraits` does (`smartQuotesType`/`smartDashesType`), so
     * `TYPE_TEXT_FLAG_NO_SUGGESTIONS` is already the whole available lever.
     * Deliberately does **not** add `TYPE_TEXT_VARIATION_VISIBLE_PASSWORD`: an
     * S2-spike device check found it can disable swipe typing on some IMEs,
     * and zero-composing was already verified without it — reach for that
     * variation as a per-IME fallback only if a specific keyboard is still
     * seen suggesting into a `"terminal"` field on a device gate.
     *
     * Any wire value this `when` doesn't recognize — including an absent
     * `lastKnownState` (nothing focused/published yet) — falls through to
     * the `"password"` arm: the same fail-closed rule
     * `content_type_wire`'s Rust-side doc comment states, applied here at the
     * boundary that actually renders it. A field state-sync forgot to
     * classify becomes stricter than intended, never a secret field that got
     * de-classified into a plaintext keyboard. `onCreateInputConnection`
     * passes [CONTENT_TYPE_NORMAL] explicitly for the "nothing published
     * yet" case instead, since no field is even focused there.
     */
    private fun applyImeContentType(outAttrs: EditorInfo, contentType: String) {
        when (contentType) {
            CONTENT_TYPE_NORMAL -> {
                outAttrs.inputType = InputType.TYPE_CLASS_TEXT
                outAttrs.imeOptions = EditorInfo.IME_ACTION_DONE
            }
            CONTENT_TYPE_NO_SUGGESTIONS -> {
                outAttrs.inputType =
                    InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
                outAttrs.imeOptions =
                    EditorInfo.IME_ACTION_DONE or EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING
            }
            CONTENT_TYPE_TERMINAL -> {
                outAttrs.inputType =
                    InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
                outAttrs.imeOptions =
                    EditorInfo.IME_ACTION_DONE or EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING or
                        EditorInfo.IME_FLAG_NO_EXTRACT_UI
            }
            else -> { // "password", or any unrecognized/future value — fail closed.
                outAttrs.inputType =
                    InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD or
                        InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
                outAttrs.imeOptions =
                    EditorInfo.IME_ACTION_DONE or EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING
            }
        }
    }

    /**
     * After every native dispatch, reconcile the soft keyboard with the focused
     * widget's IME surface: newly-active ⇒ take focus + show the keyboard (and
     * restart input so a fresh [FrustInputConnection] is seeded from the new
     * state); newly-inactive ⇒ hide it.
     *
     * A **steady-active** field whose [ImeWireState.contentType] changed since
     * the last poll (e.g. a field that starts `"normal"` and flips to
     * `"password"` while still focused, without ever losing/regaining focus)
     * also forces [InputMethodManager.restartInput]: `EditorInfo`/`InputType`
     * are fixed at [onCreateInputConnection] time and Android never re-queries
     * them on an already-bound `InputConnection`, so without this restart a
     * field that becomes secret *after* the IME already connected would keep
     * leaking into the suggestion strip under the stale, non-secure
     * `EditorInfo` (the exact gap this task closes — see `onCreateInputConnection`'s
     * doc for how the fresh `EditorInfo` is derived).
     */
    private fun pollImeAfterDispatch() {
        if (handle == 0L) return
        val state = parseImeState(nativeImeState(handle)) ?: return
        val previous = lastKnownState
        lastKnownState = state
        val contentTypeChanged = previous != null && previous.contentType != state.contentType
        if (state.active && !imeActive) {
            imeActive = true
            requestFocus()
            // Rebuild the InputConnection so its mirror starts from `state`.
            imm.restartInput(this)
            imm.showSoftInput(this, 0)
        } else if (!state.active && imeActive) {
            imeActive = false
            imm.hideSoftInputFromWindow(windowToken, 0)
        } else if (state.active && contentTypeChanged) {
            // Same field, no show/hide edge, but the content-type hint
            // changed under it — force a fresh EditorInfo/InputType (see this
            // function's doc). `restartInput` re-invokes
            // `onCreateInputConnection`, which reseeds the mirror from the
            // already-updated `lastKnownState`, so no separate `reconcileTo`
            // call is needed on this path.
            imm.restartInput(this)
        } else if (state.active) {
            // Steady active (no show/hide edge, same content type): reconcile
            // the live mirror to the focused field's published state — a
            // caret moved by a tap, or a whole-field text change from a field
            // switch or a submit-clear. `reconcileTo` compares against the
            // LIVE editable (not the racing `lastKnownState`), so a
            // pre-advanced snapshot can't hide a change.
            activeConnection?.reconcileTo(state)
        }
    }

    /**
     * Drain the framework's two clipboard slots and act on them — the JVM half
     * of the clipboard channel, called once per frame from [doFrame] directly
     * after the post-dispatch IME reconcile.
     *
     * Both drains are one-shot edges on the Rust side, so a frame that skips
     * this would lose a copy outright; both are also plain JNI reads that
     * answer null/false on an idle frame, the same cost bar
     * [pollSystemUiState] already sets. `ClipboardManager` itself is only
     * touched when a drain actually reports something.
     *
     * The paste direction is deliberately a *second* dispatch rather than a
     * return value: the framework asks ([nativeTakePasteRequest]), this side
     * reads the clipboard, and the text goes back in through
     * [nativeEditCommand]. A read that yields nothing — an empty clipboard, or
     * the Android 10+ focus gate in [readClipboardText] — simply dispatches
     * nothing, which is why the request does not have to be remembered
     * anywhere: it is answered or it is dropped, and the framework's own
     * request slot is already clear either way.
     *
     * **No ActionMode and no GestureDetector anywhere in this view**: the
     * selection toolbar over a selection is drawn by the framework
     * (`frust_core::SelectionToolbarPolicy::Framework`, the default), which is
     * what publishes the copy/paste intent this method drains. Do not add a
     * native `ActionMode.Callback` or a `GestureDetector` long-press here —
     * they would present a *second*, competing toolbar over the framework's own
     * and steal the gesture that raises it.
     */
    private fun syncClipboard() {
        nativeTakeClipboardWrite(handle)?.let { writeClipboardText(it) }
        if (nativeTakePasteRequest(handle)) {
            // The same body the hardware Ctrl+V and the IME's own paste action
            // take: read here, dispatch only when there is something to insert,
            // then reconcile the mirror in this pass.
            dispatchEditCommand(EDIT_COMMAND_PASTE)
        }
    }

    /**
     * Put `text` on the system clipboard as a plain-text clip.
     *
     * Best-effort by design: `setPrimaryClip` is a binder call into another
     * process, and a failing one (a clip too large for the transaction buffer,
     * a clipboard service killed under memory pressure, an OEM policy refusing
     * the write) must not take down the Choreographer callback this runs on.
     * The failure is logged without the text — a copied password is exactly as
     * sensitive as a pasted one.
     *
     * The clip is labelled `"text"` and written as plain text: this side never
     * has styled content to preserve, since the framework hands over a plain
     * `String`. On Android 13+ the system itself shows a "copied" confirmation
     * for this write — nothing here suppresses or duplicates it.
     */
    private fun writeClipboardText(text: String) {
        try {
            clipboard.setPrimaryClip(ClipData.newPlainText("text", text))
        } catch (e: RuntimeException) {
            Log.w(TAG, "clipboard write failed", e)
        }
    }

    /**
     * Read the system clipboard's primary clip as plain text, or null when
     * there is nothing usable to paste **this pass**.
     *
     * Three platform behaviours this deliberately lives with:
     *
     * * **The focus gate (Android 10+).** `getPrimaryClip` returns null unless
     *   the calling app currently has input focus. A paste attempted while the
     *   window is not focused legitimately yields nothing; it is not an error
     *   state to report to the user, and a null return is indistinguishable
     *   here from an empty clipboard on purpose.
     * * **The read toast (Android 12+).** The system shows a
     *   "<app> pasted from <app>" toast the first time an app reads *another*
     *   app's clip through `getPrimaryClip`. That is by design and not worth
     *   engineering around: this method is only ever reached from a
     *   user-initiated paste. `hasPrimaryClip` (and `getPrimaryClipDescription`)
     *   do NOT raise it, which is why the emptiness gate runs first: the cheap
     *   check costs the user nothing, and only the real read — which happens
     *   solely because a paste was asked for — ever crosses that line.
     * * **A refused read.** A `SecurityException` (a device policy or profile
     *   restriction) is caught rather than propagated: the paste simply does
     *   not happen, and the frame loop survives.
     *
     * A URI-backed clip (a photo, file, or contact copied from another app)
     * still pastes as its human-readable text rather than silently refusing —
     * but resolving that text is `ClipData.Item.coerceToText`'s synchronous
     * `ContentResolver` round-trip into the CLIP OWNER's own process, which can
     * block for as long as that app's `ContentProvider` takes to answer, or
     * hang outright against a frozen/ANRed source app. Every caller of this
     * method runs on the UI thread (the hardware `Ctrl+V` chord, the IME's own
     * paste action, and the per-frame paste-request drain), so this method
     * itself must never be the one to make that call:
     *
     * * When the item already carries a `CharSequence` — `item.text`, the
     *   overwhelmingly common case, including every clip this view itself
     *   writes ([writeClipboardText] always uses `ClipData.newPlainText`) —
     *   it is returned directly, synchronously, in this same pass. A paste
     *   that can answer now has to land now: [dispatchEditCommand]'s
     *   synchronous [pollImeAfterDispatch] afterwards is what keeps the mirror
     *   `Editable` in step, and that only runs for a same-pass answer.
     * * When the item has no text and no `Uri` either (HTML-only, or an
     *   `Intent`-only clip), `coerceToText` degrades to `Html.fromHtml` or
     *   `Intent.toUri` — no `ContentResolver` call — so it stays synchronous
     *   here too.
     * * Only when the item has no text and does have a `Uri` is the
     *   potentially-blocking half reached, and it is handed off to
     *   [readClipboardTextAsync] to run off this thread instead. This method
     *   returns null for that pass; [readClipboardTextAsync] dispatches the
     *   paste itself once (and if) the text arrives — see its doc.
     */
    private fun readClipboardText(): String? {
        return try {
            if (!clipboard.hasPrimaryClip()) {
                return null
            }
            val clip = clipboard.primaryClip
            if (clip == null || clip.itemCount == 0) {
                return null
            }
            val item = clip.getItemAt(0) ?: return null
            val text = item.text
            if (text != null) {
                return text.toString()
            }
            if (item.uri == null) {
                // No direct text and nothing to resolve through a
                // `ContentResolver`: whatever `coerceToText` produces (HTML,
                // an `Intent`'s URI, or `""`) is cheap and synchronous.
                return item.coerceToText(context)?.toString()
            }
            // URI-backed with no direct text: `coerceToText` may block on a
            // `ContentResolver` round-trip into another app's process — never
            // call it here. Hand the item to the background resolver and
            // answer nothing for this pass.
            readClipboardTextAsync(item)
            null
        } catch (e: SecurityException) {
            Log.w(TAG, "clipboard read refused", e)
            null
        } catch (e: RuntimeException) {
            Log.w(TAG, "clipboard read failed", e)
            null
        }
    }

    /**
     * Resolve a URI-backed clip item's text off the UI thread and dispatch
     * the paste once (and if) it arrives — the half of [readClipboardText]
     * that can block, moved off the thread every caller of this method runs
     * on (see that doc comment for why).
     *
     * Runs `item.coerceToText(context)` on [clipboardWorker]'s single
     * background thread — never one thread per paste — then hops back to the
     * UI thread via [mainHandler] to actually dispatch.
     *
     * On arrival the answer must still belong where it was asked for, and
     * FOUR things are checked, because the framework routes
     * `EditCommand::Paste` down whatever focus path is live at dispatch time
     * and has no per-request identity of its own:
     *
     * * [clipboardResolutionEpoch] is unmoved — the view did not tear down
     *   ([shutdownClipboardExecutor] is its only writer).
     * * `handle` is still live.
     * * The framework's focus/IME session generation ([nativeFocusGeneration])
     *   is unmoved. This is the one that stops a resolved paste landing in a
     *   DIFFERENT field: [imeActive] is a single view-wide flag that only
     *   toggles on the active/inactive edge, so a move from one field to
     *   another never clears it and cannot be used for this.
     * * The text is non-empty.
     *
     * Any of those failing DROPS the paste silently: a paste landing in a
     * field the user has since moved away from is worse than one that never
     * lands, and a dropped one is recoverable by asking again.
     *
     * The generation check is deliberately CONSERVATIVE — it moves on any
     * real change of the focus flag or the published IME surface, so an edit
     * or a caret move while the provider is still answering also drops the
     * paste, not only a move to another field. Re-publishing an identical
     * surface is not an edge, so an ordinary wait does not trip it.
     *
     * The wait is also BOUNDED. `coerceToText` returns when the clip's owning
     * app decides to answer — or never — so a deadline is stamped at request
     * time and an answer arriving past it is dropped. `shutdownNow()` cannot
     * interrupt a thread already blocked inside another process's
     * `ContentProvider`, so the deadline and the epoch are what actually stop
     * a late answer from being delivered.
     *
     * When it does land, it calls the exact same
     * `nativeEditCommand`/[pollImeAfterDispatch] pair the synchronous fast
     * path in [dispatchEditCommand] uses.
     */
    private fun readClipboardTextAsync(item: ClipData.Item) {
        val epoch = clipboardResolutionEpoch
        // Snapshot the session this paste was asked for, and the point past
        // which its answer is no longer wanted. Both are read here, on the UI
        // thread, while the asking press is still the current truth.
        val requestedInSession = if (handle != 0L) nativeFocusGeneration(handle) else 0L
        val deadlineUptimeMillis = SystemClock.uptimeMillis() + CLIPBOARD_RESOLUTION_TIMEOUT_MS
        if (requestedInSession == 0L) {
            return
        }
        try {
            clipboardWorker().execute {
                val text = try {
                    item.coerceToText(context)?.toString()
                } catch (e: SecurityException) {
                    Log.w(TAG, "clipboard read refused", e)
                    null
                } catch (e: RuntimeException) {
                    Log.w(TAG, "clipboard read failed", e)
                    null
                }
                mainHandler.post {
                    if (epoch != clipboardResolutionEpoch ||
                        handle == 0L ||
                        text.isNullOrEmpty()
                    ) {
                        return@post
                    }
                    if (SystemClock.uptimeMillis() > deadlineUptimeMillis) {
                        Log.w(TAG, "clipboard read answered too late; dropping the paste")
                        return@post
                    }
                    // The session must be the one that asked. `imeActive` cannot
                    // answer this: it is view-wide and does not toggle when focus
                    // moves from one field to another.
                    if (nativeFocusGeneration(handle) != requestedInSession) {
                        return@post
                    }
                    nativeEditCommand(handle, EDIT_COMMAND_PASTE, text)
                    pollImeAfterDispatch()
                }
            }
        } catch (e: RejectedExecutionException) {
            // The executor was torn down between the epoch snapshot above and
            // this submit (the view is tearing down right now) — the
            // resolution simply does not happen.
            Log.w(TAG, "clipboard read dropped: view is tearing down", e)
        }
    }

    /**
     * Lazily create (or reuse) the single background thread [readClipboardTextAsync]
     * runs on. Never spawned per paste: the same executor answers every
     * URI-backed paste for as long as the view stays resumed, and
     * [shutdownClipboardExecutor] tears it down (and this getter re-creates
     * it on the next ask) at every point a live handle stops being
     * trustworthy.
     */
    private fun clipboardWorker(): ExecutorService {
        var executor = clipboardExecutor
        if (executor == null || executor.isShutdown) {
            executor = Executors.newSingleThreadExecutor { runnable ->
                Thread(runnable, "frust-clipboard").apply { isDaemon = true }
            }
            clipboardExecutor = executor
        }
        return executor
    }

    /**
     * Stop the background clipboard-coercion thread (if one was ever created)
     * and bump [clipboardResolutionEpoch] so a resolution that finished on
     * that thread just as the view tore down is dropped on arrival instead of
     * dispatching into a view nobody should still be pasting into. Idempotent
     * — safe to call from every teardown path ([surfaceDestroyed], [onPause],
     * [onDestroy]) whether or not a resolution was ever started.
     *
     * `shutdownNow()` rather than `shutdown()`: the one task this executor can
     * be running is blocked inside another app's `ContentProvider` query
     * ([readClipboardTextAsync]), which a graceful `shutdown()` would wait
     * out — exactly the UI-thread stall this whole split exists to avoid,
     * only now on a thread nothing is joining. `shutdownNow()` interrupts it
     * and returns immediately; the epoch bump is what actually stops a late
     * answer from being delivered even on a provider call that does not
     * honour the interrupt.
     */
    private fun shutdownClipboardExecutor() {
        clipboardResolutionEpoch++
        clipboardExecutor?.shutdownNow()
        clipboardExecutor = null
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
                contentType = obj.optString("contentType", CONTENT_TYPE_NORMAL),
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
        /**
         * The wire `"contentType"` string (`"normal"` / `"password"` /
         * `"noSuggestions"`) `nativeImeState` encodes from
         * `frust_core::event::ImeContentType`. Drives
         * [applyImeContentType]'s `EditorInfo`/`InputType` mapping and
         * [pollImeAfterDispatch]'s content-type-change restart.
         */
        val contentType: String,
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
            // Per-frame clipboard drain: the framework's copy/paste
            // intent is a one-shot edge, so a frame that skipped this would
            // drop a copy outright. Two cheap JNI reads on an idle frame (the
            // `pollSystemUiState` cost bar below); `ClipboardManager` is
            // touched only when a drain actually reports something. Placed
            // after the IME reconcile so a paste dispatched here still sees the
            // mirror the poll just settled.
            syncClipboard()
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
        // Same reasoning: hold no clipboard-coercion thread while backgrounded.
        // A fresh one is created lazily ([clipboardWorker]) the next time a
        // URI-backed paste is actually asked for after [onResume].
        shutdownClipboardExecutor()
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
        // Same belt-and-suspenders reasoning: no clipboard-coercion thread may
        // outlive this view.
        shutdownClipboardExecutor()
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

        /**
         * Answer the IME's own clipboard affordances — the paste/copy/cut/
         * select-all actions a soft keyboard raises (Gboard's clipboard chip,
         * a keyboard's own edit strip, an accessibility service) — by decoding
         * the `android.R.id.*` action into a framework edit command.
         *
         * This is the route that matters on Android: an IME sends
         * `performContextMenuAction` rather than typing the clipboard's
         * contents, and `BaseInputConnection`'s default implementation only
         * knows how to act on a real `TextView`, so without this override the
         * action would reach the mirror [Editable] and never the framework's
         * field.
         *
         * `pasteAsPlainText` is answered identically to `paste`: this side
         * coerces every clip to plain text on the way in
         * ([readClipboardText]), so the framework has no styled paste to strip.
         * Anything else (`startSelectingText`, `switchInputMethod`, a future
         * action) falls through to `super`, which keeps the mirror's own
         * behaviour rather than silently claiming an action we do not perform.
         */
        override fun performContextMenuAction(id: Int): Boolean {
            if (handle == 0L) {
                return super.performContextMenuAction(id)
            }
            val command = when (id) {
                android.R.id.paste, android.R.id.pasteAsPlainText -> EDIT_COMMAND_PASTE
                android.R.id.copy -> EDIT_COMMAND_COPY
                android.R.id.cut -> EDIT_COMMAND_CUT
                android.R.id.selectAll -> EDIT_COMMAND_SELECT_ALL
                else -> return super.performContextMenuAction(id)
            }
            dispatchEditCommand(command)
            return true
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

        /**
         * Forward an editor action (Gboard's Done key /
         * `EditorInfo.imeOptions = IME_ACTION_DONE`) to Rust as a submit, then
         * reconcile the mirror synchronously — the same two-step shape
         * [onKeyDown]'s hardware-Enter path already uses.
         *
         * The synchronous [pollImeAfterDispatch] call catches a focused widget
         * that republishes its IME surface *during this very event pass*
         * (`EventCtx::publish_ime_state`, "refreshed on every event" per
         * `RenderRoot::ime_state`'s doc) — the common case and the fix for the
         * gap this method used to leave open. Without it, the mirror [Editable]
         * kept the pre-submit text until some *other* IME callback happened to
         * fire and push it back to Rust (`sync()`) against an already-cleared
         * app-side baseline — a stale full-line resend of whatever was just
         * submitted, including a password.
         *
         * A *controlled* field's clear-on-submit is instead a signal write whose
         * new value only reaches the widget's own editable buffer (and therefore
         * its next `publish_ime_state`) on the *next rebuild* — the synchronous
         * poll above can still observe stale text in that case. [doFrame]'s own
         * **unconditional** per-frame [pollImeAfterDispatch] call is what
         * actually closes it, on whichever frame the rebuild lands.
         */
        override fun performEditorAction(actionCode: Int): Boolean {
            if (handle != 0L) {
                nativeImeAction(handle, actionCode)
                pollImeAfterDispatch()
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
            // `contentType` is irrelevant to this dedup snapshot — it never
            // compares against `lastKnownState`, only against a prior call's
            // own `snapshot` — so a fixed placeholder is correct here.
            val snapshot =
                ImeWireState(true, text, selStart, selEnd, compStart, compEnd, CONTENT_TYPE_NORMAL)
            if (snapshot == lastPushed) return
            lastPushed = snapshot

            nativeImeApply(handle, text, selStart, selEnd, compStart, compEnd)

            val state = parseImeState(nativeImeState(handle)) ?: return
            lastKnownState = state

            val wholesale = state.text != text
            if (wholesale) {
                // Rust replaced the text: rebuild the mirror to match.
                editable.replace(0, editable.length, state.text)
                // A later identical `sync()` must not short-circuit on a stale snapshot.
                lastPushed = null
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
