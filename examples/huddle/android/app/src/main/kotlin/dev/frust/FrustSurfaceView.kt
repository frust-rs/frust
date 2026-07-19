package dev.frust

import android.app.Activity
import android.content.Context
import android.content.res.Configuration
import android.os.Build
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
 * The Frust Android render surface (spec Phase 2 §10.1). Package
 * `dev.frust` is fixed across every generated app — it's what keeps the
 * JNI export names (`Java_dev_frust_FrustSurfaceView_native*`)
 * stable, so do not rename or move this file. The app's own
 * `MainActivity` (package `it.f0x.huddle`) owns the activity
 * lifecycle and forwards `onResume`/`onPause`/`onDestroy` here.
 *
 * Soft-keyboard text input (spec §14 Phase 4) uses the Flutter-proven
 * **state-sync** contract, not op-forwarding: [FrustInputConnection] owns
 * composition mechanics against a mirror [Editable], pushes the whole editing
 * state into Rust via `nativeImeApply`, then pulls the reconciled state back via
 * `nativeImeState` to keep the `InputMethodManager` synchronised.
 *
 * `androidx.core` (device-parity task 08) backs the inset listener
 * ([dispatchInsets]) and status/nav-bar icon contrast
 * ([updateSystemBarsAppearance]) below — the only AndroidX dependency this
 * view uses.
 */
class FrustSurfaceView(context: Context) :
    SurfaceView(context),
    SurfaceHolder.Callback,
    Choreographer.FrameCallback {

    companion object {
        init {
            System.loadLibrary("huddle")
        }

        /**
         * How many frames to keep re-polling the IME surface after an editor
         * action, so [doFrame] catches the submit's next-frame rebuild (which
         * clears/replaces the field) and reseeds the IME mirror. A small budget
         * (a couple of frames) covers the one-frame rebuild latency with slack.
         */
        private const val IME_RESYNC_FRAMES = 3
    }

    // JNI exports implemented by `frust-shell-android` (spec Phase 2
    // task 24, Phase 4 IME) — these names and signatures are load-bearing,
    // matched exactly by `#[no_mangle] extern "system" fn Java_dev_frust_*`.
    // `cacheDir` is the app's `context.cacheDir.absolutePath` — the Rust side
    // persists the wgpu pipeline cache under it (`<cacheDir>/frust/`) so a
    // warm start skips Vulkan shader-pipeline compilation. The JNI symbol name is
    // unchanged (it doesn't encode params); this signature and the Rust
    // `native_init` gained the parameter together.
    private external fun nativeInit(surface: Surface, scaleFactor: Float, cacheDir: String): Long

    // `density` (device-parity task 06/08) is `resources.displayMetrics.density`
    // for the current configuration — a BREAKING signature change from the
    // pre-parity three-arg form; the Rust `native_on_surface_changed` gained
    // the same trailing parameter in the same phase (task 06).
    private external fun nativeOnSurfaceChanged(
        handle: Long,
        surface: Surface,
        width: Int,
        height: Int,
        density: Float,
    )

    private external fun nativeOnSurfaceDestroyed(handle: Long)

    private external fun nativeOnFrame(handle: Long, frameTimeNanos: Long)

    // Touch delivery (spec §9). `action` is a fixed numeric ABI shared with the
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

    // IME state-sync exports (spec §14 Phase 4). Indices are UTF-16 code units
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

    // Appearance (spec §17, task 08): flip the app's theme brightness between
    // light and dark. `dark` mirrors Configuration.UI_MODE_NIGHT_YES — see
    // [isDarkMode]. Called once right after `nativeInit` returns a handle and
    // again on every `onConfigurationChanged` (the manifest declares `uiMode`
    // in `android:configChanges` so a system dark-mode toggle reaches here
    // instead of recreating the activity).
    private external fun nativeSetAppearance(handle: Long, dark: Boolean)

    // Deep links (task 07). `url` is the raw `Intent.data` Uri's `toString()`,
    // forwarded to `frust_reactive::push_deep_link` on the Rust side.
    private external fun nativeOnDeepLink(handle: Long, url: String)

    // Accessibility (spec §9, phase 6d). Attaches the accesskit Android adapter
    // to this view. `view` is the accessibility host — always `this`
    // (`FrustSurfaceView` IS a `View`); the adapter installs a
    // `View.AccessibilityDelegate` + `OnHoverListener` on it (posted to the UI
    // thread) and depends on the bundled `dev.accesskit.android.Delegate` class.
    // Best-effort: the Rust side isolates any init failure so a11y never blocks
    // startup. Called once, right after `nativeInit` returns a live handle.
    private external fun nativeInitAccessibility(handle: Long, view: View)

    // Insets (device-parity task 06/08 — RESEARCH.md "Insets / SafeArea").
    // The eight floats are physical px: `viewPadding` (system-bar/cutout
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

    // Android back (device-parity task 06/08 — RESEARCH.md "Android back").
    // Returns whether the framework consumed the press (it will pop on the
    // next rebuild) — see [dispatchBackPress].
    private external fun nativeOnBackPress(handle: Long): Boolean

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

    private val imm: InputMethodManager
        get() = context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager

    private val scaleFactor: Float
        get() = resources.displayMetrics.density

    /** Whether the platform currently reports a dark appearance preference. */
    private val isDarkMode: Boolean
        get() = (resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) ==
            Configuration.UI_MODE_NIGHT_YES

    init {
        holder.addCallback(this)
        // Required for a custom editor view to receive IME focus + text input.
        isFocusable = true
        isFocusableInTouchMode = true
        // Insets (device-parity task 08 — RESEARCH.md "Insets / SafeArea").
        // Fires on attach and on every later system-bar/cutout/IME change;
        // this view is the activity's entire content view (no siblings to
        // propagate to), so the original `windowInsets` is returned unconsumed.
        ViewCompat.setOnApplyWindowInsetsListener(this) { _, windowInsets ->
            lastInsets = windowInsets
            dispatchInsets(windowInsets)
            windowInsets
        }
    }

    /**
     * Compute and forward the platform window insets to `nativeOnInsetsChanged`
     * (physical px, task 06 signature): `viewPadding` is the system-bar +
     * display-cutout occlusion, max-merged per edge (mirrors Flutter's
     * `FlutterView.onApplyWindowInsets`, `FlutterView.java:751-793`);
     * `viewInsets` is the IME area. A missing handle is a no-op — the insets
     * are still cached in [lastInsets] and re-dispatched once one exists (see
     * [surfaceCreated]).
     *
     * Pre-API-30 devices have no native `Type.ime()` insets; `WindowInsetsCompat`
     * falls back to its own best-effort IME detection there — an accepted
     * degradation, not a bug, per task 08's spec.
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
     * Android back (device-parity task 08 — RESEARCH.md "Android back"),
     * called by `MainActivity`'s `onBackPressedDispatcher` callback. Wraps
     * `nativeOnBackPress`: `true` means the framework consumed the press
     * (it will pop on the next rebuild); `false` means `MainActivity` should
     * fall through to its default (finish) behavior. A missing handle
     * (native side not up yet) never claims the press.
     */
    fun dispatchBackPress(): Boolean {
        if (handle == 0L) return false
        return nativeOnBackPress(handle)
    }

    /**
     * Status/nav-bar icon contrast (task 08 — RESEARCH.md "Insets / SafeArea
     * / SystemChrome"): light icons on a dark theme and vice versa, the one
     * `WindowInsetsControllerCompat` use that's real in Flutter's embedder
     * (`setSystemUIOverlayStyle` is not deprecated, but Frust uses the
     * AndroidX compat surface instead). `WindowCompat.getInsetsController`
     * (not the deprecated `ViewCompat.getWindowInsetsController(View)`)
     * needs the hosting `Activity`'s `Window` — always available here since
     * `MainActivity` is this view's sole constructor caller (see
     * `dev.frust.FrustSurfaceView`'s class doc). Called alongside
     * every `nativeSetAppearance` — see [surfaceCreated]/[onConfigurationChanged].
     */
    private fun updateSystemBarsAppearance(dark: Boolean) {
        val window = (context as? Activity)?.window ?: return
        WindowCompat.getInsetsController(window, this).apply {
            isAppearanceLightStatusBars = !dark
            isAppearanceLightNavigationBars = !dark
        }
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
            handle = nativeInit(holder.surface, scaleFactor, context.cacheDir.absolutePath)
            if (handle != 0L) {
                nativeSetAppearance(handle, isDarkMode)
                updateSystemBarsAppearance(isDarkMode)
                // Attach the accesskit accessibility adapter to this view (spec
                // §9, phase 6d). Best-effort: the native side isolates any
                // failure in its own guard, so a missing delegate class or JNI
                // hiccup degrades to "no a11y" rather than blocking startup.
                nativeInitAccessibility(handle, this)
                // Re-dispatch the last known insets (task 08): a recreated
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
     */
    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        if (handle != 0L) {
            nativeSetAppearance(handle, isDarkMode)
            updateSystemBarsAppearance(isDarkMode)
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
            nativeOnFrame(handle, frameTimeNanos)
            // Per-frame IME reconcile: pick up caret moves, field switches, and
            // submit-clears that no InputConnection callback originated.
            // `pollImeAfterDispatch` reconciles the live mirror to the focused
            // field's published state; a no-op when they already match, so normal
            // typing never triggers a spurious restart.
            pollImeAfterDispatch()
        }
        Choreographer.getInstance().postFrameCallback(this)
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
        Choreographer.getInstance().postFrameCallback(this)
    }

    /** Called by `MainActivity.onPause` — stops the Choreographer loop. */
    fun onPause() {
        running = false
        Choreographer.getInstance().removeFrameCallback(this)
        if (handle != 0L) {
            nativeOnPause(handle)
        }
    }

    /** Called by `MainActivity.onDestroy` — releases the native handle. */
    fun onDestroy() {
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