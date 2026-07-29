package dev.frust

import android.app.Activity
import android.content.Context
import android.content.pm.ApplicationInfo
import android.graphics.Color
import android.graphics.Rect
import android.util.Log
import android.view.Choreographer
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import android.widget.TextView
import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject

/**
 * Applies the native-sibling-compositor command backlog (platform-views
 * feature) that `FrustSurfaceView.doFrame` polls from
 * `nativePlatformViewCommands` each frame. Package `dev.frust` is fixed (same
 * reason as [FrustSurfaceView]) so the reflectively-resolved
 * `dev.frust.*Factory` contract is stable across every generated app.
 *
 * The host owns the mapping from Frust slot ids to live [View]s and their
 * [FrustPlatformViewFactory]. Hosted views are added to the same [FrameLayout]
 * root as the [FrustSurfaceView], **above** it in opaque (Mode A) apps and
 * **below** it in translucent (Mode B) apps — the index is chosen once from
 * [FrustSurfaceView.translucentSurface] (a per-slot z policy is future
 * work). Every method runs on the main thread: `doFrame` already does, and the
 * host is only ever driven from there, so no synchronization is needed.
 *
 * **Coordinates:** rects arrive as **physical px** straight from the Rust FFI
 * boundary (the shell already multiplied by the display scale factor — the
 * physical-at-FFI/logical-inside rule, applied outbound), and Android's
 * `FrameLayout.LayoutParams` / `translationX`/`translationY` are physical px
 * too, so the host does zero density math.
 *
 * **Insets:** hosted views sit in the window coordinate space; Frust's rects
 * are absolute paint coordinates that already account for the safe-area insets
 * (SafeArea handling on the Rust side), so the host never does inset math.
 *
 * **Allocation:** applying a batch reuses each slot's [Slot.layoutParams] and
 * [Slot.clipRect] — the only per-poll allocation is the `org.json` parse of a
 * non-empty batch. A no-change frame returns a null command string on the Rust
 * side and never reaches [applyCommands] at all (see `FrustSurfaceView.doFrame`).
 */
class FrustViewHost(
    private val activity: Activity,
    private val root: FrameLayout,
    private val surfaceView: FrustSurfaceView,
) {
    /**
     * The last command-backlog generation this host has fully applied. Read by
     * `FrustSurfaceView.doFrame` and round-tripped back into
     * `nativePlatformViewCommands(handle, ackedGeneration)` so the Rust differ
     * can compact acknowledged commands and take its null-on-no-change fast
     * path. Starts at `0`, matching the differ's initial generation.
     */
    var ackedGeneration: Long = 0L
        private set

    /** Live slots by Frust slot id. */
    private val slots = HashMap<Long, Slot>()

    /**
     * Whether any native sibling is currently hosted. Read once per frame by
     * `FrustSurfaceView.sampleFrameTimeline` (camera task 12) to scope the
     * scroll-sync tail's Choreographer frame-timeline sampling to the frames
     * where a hosted view's geometry can actually be out of step — an app with
     * no platform view posts no vsync callback at all.
     */
    val hasHostedViews: Boolean
        get() = slots.isNotEmpty()

    /** Successfully-resolved factories, cached per `viewType`. */
    private val factories = HashMap<String, FrustPlatformViewFactory>()

    /** `viewType`s whose resolution already failed — never retried. */
    private val failedViewTypes = HashSet<String>()

    /** Slot ids whose creation failed (dead): every later command is ignored. */
    private val deadSlots = HashSet<Long>()

    private val translucent: Boolean
        get() = surfaceView.translucentSurface

    /** One live hosted view, its factory, and the reusable per-frame scratch. */
    private class Slot(
        val view: View,
        val factory: FrustPlatformViewFactory,
        val layoutParams: FrameLayout.LayoutParams,
        val clipRect: Rect,
        /** Mode B input forwarding (native-widgets spike 3): whether a
         * touch-DOWN inside this slot's rect hands the gesture to [view]. */
        val interactive: Boolean,
        /** The z-shield list (physical px, absolute window space): regions
         * where frust content over the slot keeps winning input. */
        val shields: MutableList<android.graphics.RectF>,
    )

    /**
     * Mode B input forwarding (native-widgets spike 3): the view that should
     * own a gesture starting at physical-px window point `(x, y)`, or null
     * when the frust surface keeps it. A slot wins when it is interactive,
     * visible, contains the point, and no z-shield rect covers it.
     */
    fun interactiveTargetAt(x: Float, y: Float): View? {
        for (slot in slots.values) {
            if (!slot.interactive) continue
            val v = slot.view
            if (v.visibility != View.VISIBLE) continue
            val left = v.translationX
            val top = v.translationY
            val right = left + slot.layoutParams.width
            val bottom = top + slot.layoutParams.height
            if (x < left || x >= right || y < top || y >= bottom) continue
            if (slot.shields.any { it.contains(x, y) }) continue
            return v
        }
        return null
    }

    /**
     * Apply one non-null command batch (`{"generation":N,"commands":[...]}`).
     * Malformed JSON is logged and dropped without advancing [ackedGeneration]
     * (a re-poll re-delivers the same backlog — commands are idempotent). Each
     * op is applied in array order; an unknown op is ignored.
     */
    fun applyCommands(json: String) {
        val batch = try {
            JSONObject(json)
        } catch (e: JSONException) {
            Log.w(TAG, "malformed platform-view command batch dropped", e)
            return
        }
        val generation = batch.optLong("generation", ackedGeneration)
        val commands = batch.optJSONArray("commands")
        if (commands != null) {
            for (i in 0 until commands.length()) {
                val cmd = commands.optJSONObject(i) ?: continue
                when (cmd.optString("op")) {
                    "create" -> applyCreate(cmd)
                    "update" -> applyUpdate(cmd)
                    "updateParams" -> applyUpdateParams(cmd)
                    "dispose" -> applyDispose(cmd)
                    else -> Log.w(TAG, "unknown platform-view op: ${cmd.optString("op")}")
                }
            }
        }
        ackedGeneration = generation
    }

    private fun applyCreate(cmd: JSONObject) {
        val slotId = cmd.optLong("slot")
        if (slots.containsKey(slotId) || deadSlots.contains(slotId)) return
        val viewType = cmd.optString("viewType")
        val params = cmd.optString("params")
        val factory = resolveFactory(viewType)
        if (factory == null) {
            deadSlots.add(slotId)
            return
        }
        val lp = FrameLayout.LayoutParams(0, 0, Gravity.TOP or Gravity.START)
        // Mode A (opaque): host views ABOVE the render surface (index just after
        // it). Mode B (translucent): BELOW, so they show through the alpha
        // surface (insert at the surface's index, pushing it up one).
        val surfaceIndex = root.indexOfChild(surfaceView)
        val index = when {
            surfaceIndex < 0 -> root.childCount
            translucent -> surfaceIndex
            else -> surfaceIndex + 1
        }
        // `view.visibility`/`root.addView` live INSIDE this try, not just
        // `createView` itself: a null return with no thrown exception (a
        // future/misbehaving factory, or today's re-entrant-runtime edge
        // case) would otherwise NPE on `view.visibility` unguarded — the
        // defect this whole block fixes (f2-05-null-create-npe.md). Catching
        // here means ANY factory's null-or-throw failure, present or future,
        // lands in the same guarded path.
        val view = try {
            val created = factory.createView(activity, activity, params)
            created.visibility = View.INVISIBLE
            root.addView(created, index, lp)
            created
        } catch (e: Throwable) {
            Log.w(TAG, "factory '$viewType' failed to create slot $slotId — marking dead", e)
            deadSlots.add(slotId)
            return
        }
        slots[slotId] =
            Slot(view, factory, lp, Rect(), cmd.optBoolean("interactive", false), mutableListOf())
    }

    private fun applyUpdate(cmd: JSONObject) {
        val slot = slots[cmd.optLong("slot")] ?: return
        val rect = cmd.optJSONArray("rect") ?: return
        if (rect.length() < 4) return
        val x = rect.optDouble(0, 0.0).toFloat()
        val y = rect.optDouble(1, 0.0).toFloat()
        val w = rect.optDouble(2, 0.0).toInt()
        val h = rect.optDouble(3, 0.0).toInt()

        if (slot.layoutParams.width != w || slot.layoutParams.height != h) {
            slot.layoutParams.width = w
            slot.layoutParams.height = h
            slot.view.layoutParams = slot.layoutParams
        }
        slot.view.translationX = x
        slot.view.translationY = y

        val clip = cmd.optJSONArray("clip")
        if (clip != null && clip.length() >= 4) {
            // Clip arrives in the same absolute paint space as `rect`; convert
            // to the view's local space (its own top-left) for `clipBounds`.
            val cx = clip.optDouble(0, 0.0).toFloat()
            val cy = clip.optDouble(1, 0.0).toFloat()
            val cw = clip.optDouble(2, 0.0).toInt()
            val ch = clip.optDouble(3, 0.0).toInt()
            val left = (cx - x).toInt()
            val top = (cy - y).toInt()
            slot.clipRect.set(left, top, left + cw, top + ch)
            slot.view.clipBounds = slot.clipRect
        } else {
            slot.view.clipBounds = null
        }

        slot.view.visibility = if (cmd.optBoolean("visible", true)) View.VISIBLE else View.INVISIBLE

        // The z-shield list rides every update (same absolute physical-px
        // space as `rect`) — replaced wholesale.
        slot.shields.clear()
        val shields = cmd.optJSONArray("shields")
        if (shields != null) {
            for (i in 0 until shields.length()) {
                val s = shields.optJSONArray(i) ?: continue
                if (s.length() < 4) continue
                val sx = s.optDouble(0, 0.0).toFloat()
                val sy = s.optDouble(1, 0.0).toFloat()
                val sw = s.optDouble(2, 0.0).toFloat()
                val sh = s.optDouble(3, 0.0).toFloat()
                slot.shields.add(android.graphics.RectF(sx, sy, sx + sw, sy + sh))
            }
        }
    }

    private fun applyUpdateParams(cmd: JSONObject) {
        val slot = slots[cmd.optLong("slot")] ?: return
        try {
            slot.factory.updateParams(slot.view, cmd.optString("params"))
        } catch (e: Throwable) {
            Log.w(TAG, "factory failed to update params", e)
        }
    }

    private fun applyDispose(cmd: JSONObject) {
        val slotId = cmd.optLong("slot")
        deadSlots.remove(slotId)
        val slot = slots.remove(slotId) ?: return
        root.removeView(slot.view)
        try {
            slot.factory.disposeView(slot.view)
        } catch (e: Throwable) {
            Log.w(TAG, "factory failed to dispose slot $slotId", e)
        }
    }

    /**
     * Resolve `viewType` (a fully-qualified `dev.frust.*Factory` class name) to
     * a [FrustPlatformViewFactory] via the application classloader and a public
     * no-arg constructor. Successes and failures are both cached, so a bad
     * `viewType` costs one reflection attempt, not one per frame. A failure
     * never throws — it logs and returns null (the caller marks the slot dead).
     *
     * **Check order is part of the contract, not an implementation detail:**
     * (1) the `dev.frust.` package prefix, (2) [FrustPlatformViewFactory]
     * assignability via [Class.isAssignableFrom] on the *class object* — BOTH
     * checked BEFORE (3) the no-arg constructor ever runs. Reflectively
     * instantiating an arbitrary attacker-influenced class name before
     * checking what it is lets a public no-arg constructor's side effects run
     * unconditionally; verifying the type first closes that hole.
     */
    private fun resolveFactory(viewType: String): FrustPlatformViewFactory? {
        factories[viewType]?.let { return it }
        if (viewType in failedViewTypes) return null
        if (!viewType.startsWith(FACTORY_PACKAGE_PREFIX)) {
            Log.w(
                TAG,
                "platform-view factory '$viewType' rejected: " +
                    "viewType must start with '$FACTORY_PACKAGE_PREFIX'",
            )
            failedViewTypes.add(viewType)
            return null
        }
        return try {
            val cls = activity.classLoader.loadClass(viewType)
            if (!FrustPlatformViewFactory::class.java.isAssignableFrom(cls)) {
                Log.w(
                    TAG,
                    "platform-view factory '$viewType' rejected: " +
                        "does not implement FrustPlatformViewFactory",
                )
                failedViewTypes.add(viewType)
                return null
            }
            val factory = cls.getDeclaredConstructor().newInstance() as FrustPlatformViewFactory
            factories[viewType] = factory
            factory
        } catch (e: Throwable) {
            Log.w(TAG, "platform-view factory '$viewType' failed to resolve", e)
            failedViewTypes.add(viewType)
            null
        }
    }

    companion object {
        private const val TAG = "frust"

        /** The [dev.frust] package prefix every `viewType` must carry (CODE_STANDARDS LAW). */
        private const val FACTORY_PACKAGE_PREFIX = "dev.frust."
    }
}

/**
 * A template-shipped, debug-only factory that hosts a [TextView] showing its
 * own `paramsJson` plus a self-incrementing counter. It exists to prove the
 * zero-Frust-frame self-update property (task 11's trace check): the counter
 * advances on its own `Choreographer` loop — driven by the display's vsync,
 * independent of Frust's render loop — so a device trace can confirm a hosted
 * native view animates without waking the Frust frame path.
 *
 * BuildConfig is not generated for this template (the `buildConfig` Gradle
 * feature is off), so the "guard on `BuildConfig.DEBUG`" is implemented with
 * the equivalent `ApplicationInfo.FLAG_DEBUGGABLE` check: outside a debuggable
 * build the label renders statically (no self-update loop).
 */
class TestLabelFactory : FrustPlatformViewFactory {
    override fun createView(activity: Activity, context: Context, paramsJson: String): View {
        val debuggable =
            (context.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE) != 0
        return SelfUpdatingLabel(context, paramsJson, debuggable)
    }

    /** A [TextView] that ticks its own counter on the Choreographer while attached. */
    private class SelfUpdatingLabel(
        context: Context,
        private val paramsJson: String,
        private val selfUpdating: Boolean,
    ) : TextView(context), Choreographer.FrameCallback {
        private var counter = 0L

        init {
            setBackgroundColor(Color.argb(160, 0, 0, 0))
            setTextColor(Color.WHITE)
            textSize = 14f
            render()
        }

        private fun render() {
            text = "TestLabel[$counter] $paramsJson"
        }

        override fun onAttachedToWindow() {
            super.onAttachedToWindow()
            if (selfUpdating) Choreographer.getInstance().postFrameCallback(this)
        }

        override fun onDetachedFromWindow() {
            Choreographer.getInstance().removeFrameCallback(this)
            super.onDetachedFromWindow()
        }

        override fun doFrame(frameTimeNanos: Long) {
            if (!isAttachedToWindow) return
            counter++
            render()
            Choreographer.getInstance().postFrameCallback(this)
        }
    }
}
