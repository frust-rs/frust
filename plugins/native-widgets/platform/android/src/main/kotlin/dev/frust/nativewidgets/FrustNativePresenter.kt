// The `frust-native-widgets` plugin's presentation host: the Kotlin half of
// the Rust <-> Kotlin contract also implemented by
// `plugins/native-widgets/src/present/android_host.rs` (host discovery, the
// `nativeOnOutcome` callback export) and
// `plugins/native-widgets/src/present/android_alert.rs` (the `showAlert`/
// `dismiss` calls below) and `plugins/native-widgets/src/present/android_sheet.rs`
// (`showSheet`/`dismissSheet`/`selectSheetDetent`) — see those modules'
// contract tables. Native presentations (alerts, sheets) are not
// platform-view slots: they are shown over the resumed Activity, which this
// object tracks.
//
// Package `dev.frust.nativewidgets` and the object name are baked into the
// mangled JNI symbol of `nativeOnOutcome`
// (`Java_dev_frust_nativewidgets_FrustNativePresenter_nativeOnOutcome`) and
// into the binary class name Rust loads it by (`PRESENTER_CLASS_BINARY`), so
// they are fixed once shipped. The `OUTCOME_*` and `ROLE_*` codes are
// mirrored verbatim by the Rust side's `present::wire` table and pinned
// against drift by its tests (`android_alert.rs` keeps its own copy of the
// `ROLE_*` codes, edited together).
package dev.frust.nativewidgets

import android.app.Activity
import android.app.AlertDialog
import android.app.Application
import android.app.Dialog
import android.content.Context
import android.content.DialogInterface
import android.content.pm.PackageManager
import android.content.res.ColorStateList
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.util.TypedValue
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.TextView
import androidx.core.widget.NestedScrollView
import com.google.android.material.bottomsheet.BottomSheetBehavior
import com.google.android.material.bottomsheet.BottomSheetDialog
import com.google.android.material.bottomsheet.BottomSheetDragHandleView
import com.google.android.material.button.MaterialButton
import com.google.android.material.color.MaterialColors
import com.google.android.material.shape.MaterialShapeDrawable
import org.json.JSONObject
import java.lang.ref.WeakReference

/**
 * Tracks the resumed [Activity] a native presentation is shown over, shows
 * the two presentation kinds this plugin ships (an [android.app.AlertDialog]
 * and a Material [BottomSheetDialog]), and carries the one callback every
 * presentation reports its outcome — and a sheet its detent changes —
 * through.
 *
 * ## Why this object tracks the Activity itself
 *
 * A dialog needs an Activity window, and an app may present an alert with no
 * native control mounted at all — so there is no view seam to discover one
 * through. [FrustNativePresenterInitProvider] installs the application
 * [Context] at process start, before any Activity exists, and the
 * process-wide [Application.ActivityLifecycleCallbacks] registered from it
 * record each resumed Activity.
 *
 * ## No strong Activity reference
 *
 * The Activity is held through a [WeakReference] and cleared in
 * [Application.ActivityLifecycleCallbacks.onActivityDestroyed] of that same
 * instance — never merely on pause, so a transient pause (a permission
 * prompt, multi-window focus) does not turn a presentation into "no host".
 * [resumedActivity] additionally refuses an Activity that is finishing or
 * already destroyed.
 *
 * ## The live dialog: exactly-once, own guard
 *
 * [liveDialog]/[liveActivity]/[liveGeneration] track the one dialog
 * currently shown by [showAlert] or [showSheet] (alerts and sheets share one
 * Busy slot on the Rust side, so at most one is live), independently of (and in
 * addition to) `android_alert.rs`'s own generation-keyed parking on the Rust
 * side. [resolveOnce] is this object's own exactly-once guard: it nulls
 * [liveDialog] the moment any path reports an outcome, so a framework
 * callback firing again for the same dialog (a button tap always dismisses
 * the dialog too, invoking both its own listener and the dismiss listener)
 * finds nothing left to report. Every field here is touched only from the
 * main thread: `showAlert`/`dismiss` are called directly, synchronously, by
 * Rust (`android_alert.rs`'s module doc, *Main-thread dispatch* — the whole
 * frust Android runtime is thread-confined to the main `Looper`), the sheet
 * members hop there themselves ([onMain]), and the dialogs' own listeners
 * plus `Application.ActivityLifecycleCallbacks` are always delivered there by
 * the platform.
 */
object FrustNativePresenter {
    /** The user chose the action at `actionIndex` (spec order). */
    const val OUTCOME_ACTION = 0

    /** The user cancelled (back key, outside tap). */
    const val OUTCOME_CANCELLED = 1

    /** Programmatic dismissal. */
    const val OUTCOME_DISMISSED = 2

    /** The hosting Activity was destroyed first. */
    const val OUTCOME_HOST_LOST = 3

    /** No resumed Activity when the show actually ran. */
    const val OUTCOME_NO_HOST = 4

    /** The show threw. */
    const val OUTCOME_FAILED = 5

    /** Not terminal: the user settled the live sheet at the detent at `actionIndex`. */
    const val OUTCOME_DETENT = 6

    /** An ordinary action — [showAlert]'s `roles` wire code for `Default`. */
    private const val ROLE_DEFAULT = 0

    /** The Cancel-role action — [showAlert]'s `roles` wire code for `Cancel`. */
    private const val ROLE_CANCEL = 1

    /**
     * The Destructive-role action — [showAlert]'s `roles` wire code for
     * `Destructive`.
     */
    private const val ROLE_DESTRUCTIVE = 2

    private const val TAG = "FrustNativePresenter"

    /** A sheet's spacing between rows and its content inset, in dp. */
    private const val SHEET_ROW_SPACING_DP = 12f
    private const val SHEET_INSET_DP = 24f

    /** The tallest a sheet's image row grows, in dp. */
    private const val SHEET_IMAGE_MAX_DP = 200f

    private val mainHandler by lazy { Handler(Looper.getMainLooper()) }

    /**
     * The most recently resumed Activity. `@Volatile`: written on the main
     * thread by the lifecycle callbacks, read from whatever thread Rust
     * probes [hasResumedActivity] on.
     */
    @Volatile
    private var resumed: WeakReference<Activity>? = null

    /**
     * Every created, not yet destroyed Activity, oldest first, so
     * [FrustNativeChrome] can find the frust host before its first resume or
     * behind a foreign resumed Activity. Main-thread only (the lifecycle
     * callbacks and [hostActivity]); each entry leaves on its destroy.
     */
    private val created = ArrayList<WeakReference<Activity>>()

    /**
     * The dialog [showAlert] currently has up, or `null` between
     * presentations — this object's own exactly-once guard (class doc's *The
     * live dialog*). Main-thread only, unlike [resumed]: nothing here is ever
     * read from a JNI worker thread.
     */
    private var liveDialog: Dialog? = null

    /** The sheet state beside [liveDialog] when it is a [showSheet] dialog. */
    private var liveSheet: LiveSheet? = null

    /** The Activity [liveDialog] is attached to, alongside [liveDialog]. */
    private var liveActivity: Activity? = null

    /** The generation [liveDialog] belongs to, alongside [liveDialog]. */
    private var liveGeneration: Long = 0L

    /**
     * Guards a single [Application.registerActivityLifecycleCallbacks] call.
     * Only touched from the init provider's `onCreate`, on the main thread.
     */
    private var lifecycleCallbacksRegistered: Boolean = false

    /**
     * Register the Activity lifecycle callbacks from the application
     * [Context] — called once at process start by
     * [FrustNativePresenterInitProvider]. Idempotent.
     *
     * [Context.getApplicationContext] is the [Application] on every supported
     * API level; the `as?` keeps a non-`Application` context (a test double,
     * an oddly-wrapped host) from throwing at process start.
     */
    internal fun installApplicationContext(context: Context) {
        if (lifecycleCallbacksRegistered) return
        val app = context.applicationContext as? Application ?: return
        lifecycleCallbacksRegistered = true
        app.registerActivityLifecycleCallbacks(activityCallbacks)
    }

    /**
     * The Activity to present over, or `null` when none is resumed, or the
     * last one is finishing or destroyed.
     */
    @JvmStatic
    fun resumedActivity(): Activity? {
        val activity = resumed?.get() ?: return null
        return if (activity.isFinishing || activity.isDestroyed) null else activity
    }

    /**
     * The Activity app chrome installs into: [resumedActivity] when `hosts`
     * accepts it (its content holds the frust surface), else the most
     * recently created live one `hosts` accepts — so a foreign Activity
     * resumed over the frust one (a billing proxy) is passed over, not
     * chosen. `null` when no live Activity qualifies.
     */
    internal fun hostActivity(hosts: (Activity) -> Boolean): Activity? {
        resumedActivity()?.let { if (hosts(it)) return it }
        created.removeAll { it.get().let { activity -> activity == null || activity.isDestroyed } }
        return created.asReversed().firstNotNullOfOrNull { ref ->
            ref.get()?.takeIf { !it.isFinishing && hosts(it) }
        }
    }

    /** Whether [resumedActivity] has one — Rust's host-discovery probe. */
    @JvmStatic
    fun hasResumedActivity(): Boolean = resumedActivity() != null

    /**
     * Build and show an `android.app.AlertDialog` over [resumedActivity] —
     * `android_alert.rs`'s `Host::show_alert`, called directly on the main
     * thread (its module doc's *Main-thread dispatch*).
     *
     * `labels`/`roles` are parallel, spec-ordered arrays; `roles`' values are
     * [ROLE_DEFAULT]/[ROLE_CANCEL]/[ROLE_DESTRUCTIVE]. Each action claims its
     * preferred button slot in spec order ([assignSlots]) — at most one
     * `Cancel` role and at most three actions total (Rust's
     * `AlertSpec::validate`/`MAX_ALERT_ACTIONS`), so every action always
     * finds a free slot among `AlertDialog`'s three.
     *
     * A dialog already up from an earlier, abandoned presentation (its
     * caller dropped the Rust `Presentation` future without waiting —
     * `android_alert.rs`'s module doc, *A displaced presentation*) is taken
     * down first ([dismissStale]), so two never show stacked.
     *
     * Reports exactly once through [nativeOnOutcome] ([resolveOnce]): no
     * resumed Activity → [OUTCOME_NO_HOST]; an exception building or showing
     * the dialog → [OUTCOME_FAILED]; otherwise the dialog's own listeners
     * report later.
     */
    @JvmStatic
    fun showAlert(
        generation: Long,
        title: String,
        message: String,
        labels: Array<String>,
        roles: IntArray,
        cancelable: Boolean,
    ) {
        dismissStale()
        val activity = resumedActivity()
        if (activity == null) {
            nativeOnOutcome(generation, OUTCOME_NO_HOST, -1)
            return
        }
        try {
            val builder = AlertDialog.Builder(activity)
            // Always set, even empty — see `android_alert.rs`'s module doc's
            // *Mapping*.
            builder.setTitle(title)
            builder.setMessage(message)
            builder.setCancelable(cancelable)
            builder.setOnCancelListener { resolveOnce(generation, OUTCOME_CANCELLED, -1) }

            val slots = assignSlots(roles)
            for (index in labels.indices) {
                val listener =
                    DialogInterface.OnClickListener { _, _ ->
                        resolveOnce(generation, OUTCOME_ACTION, index)
                    }
                when (slots[index]) {
                    ButtonSlot.POSITIVE -> builder.setPositiveButton(labels[index], listener)
                    ButtonSlot.NEGATIVE -> builder.setNegativeButton(labels[index], listener)
                    ButtonSlot.NEUTRAL -> builder.setNeutralButton(labels[index], listener)
                    // Unreachable: `assignSlots` always finds a free slot for
                    // up to three actions (this function's doc).
                    null -> Unit
                }
            }

            val dialog = builder.create()
            dialog.setOnDismissListener { resolveOnce(generation, OUTCOME_DISMISSED, -1) }
            liveGeneration = generation
            liveDialog = dialog
            liveActivity = activity
            dialog.show()
        } catch (t: Throwable) {
            liveDialog = null
            liveActivity = null
            liveSheet = null
            nativeOnOutcome(generation, OUTCOME_FAILED, -1)
        }
    }

    /**
     * Take presentation [generation] down programmatically —
     * `android_alert.rs`'s `Host::dismiss`. Ignores a `generation` that is
     * not [liveGeneration]'s (a stale handle, or a race with the dialog
     * resolving on its own): [resolveOnce]'s own guard already covers it, so
     * this amounts to a no-op either way.
     */
    @JvmStatic
    fun dismiss(generation: Long) {
        val dialog = if (liveGeneration == generation) liveDialog else null
        resolveOnce(generation, OUTCOME_DISMISSED, -1)
        closeQuietly(dialog)
    }

    /**
     * Build and show a Material [BottomSheetDialog] over [resumedActivity] —
     * `android_sheet.rs`'s `Host::show_sheet`. `specJson` is the Rust side's
     * flat `present::wire::sheet_json` object; `image` the encoded image row
     * (`null` for none). Hops to the main thread ([onMain]).
     *
     * Takes an abandoned presentation's dialog down first ([dismissStale]),
     * like [showAlert], and reports exactly once through [nativeOnOutcome]
     * ([resolveOnce]): no resumed Activity → [OUTCOME_NO_HOST]; an exception
     * building or showing the sheet → [OUTCOME_FAILED]; otherwise the
     * sheet's own listeners report later. `android_sheet.rs`'s module doc
     * maps every spec field.
     */
    @JvmStatic
    fun showSheet(generation: Long, specJson: String, image: ByteArray?) {
        onMain { showSheetNow(generation, specJson, image) }
    }

    /** [dismiss] for a sheet — `android_sheet.rs`'s `Host::dismiss`, on the main thread. */
    @JvmStatic
    fun dismissSheet(generation: Long) {
        onMain { dismiss(generation) }
    }

    /**
     * Move the live sheet to the detent at spec `index` —
     * `android_sheet.rs`'s `Host::select_detent`, on the main thread. Ignored
     * unless `generation` is the live sheet's; never echoed back as an
     * [OUTCOME_DETENT] report.
     */
    @JvmStatic
    fun selectSheetDetent(generation: Long, index: Int) {
        onMain {
            val live = liveSheet
            if (live != null && live.generation == generation && index in live.detents.indices) {
                moveSheet(live, index)
            }
        }
    }

    private fun showSheetNow(generation: Long, specJson: String, image: ByteArray?) {
        dismissStale()
        val activity = resumedActivity()
        if (activity == null) {
            nativeOnOutcome(generation, OUTCOME_NO_HOST, -1)
            return
        }
        try {
            val spec = JSONObject(specJson)
            val detents =
                (0 until spec.optInt("detentCount")).map {
                    SheetDetent(spec.optBoolean("detent${it}Full"), spec.optDouble("detent${it}Ratio", 0.5).toFloat())
                }
            val selected = spec.optInt("selected").coerceIn(detents.indices)
            val dismissible = spec.optBoolean("dismissible", true)
            val dialog = BottomSheetDialog(sheetContext(activity, spec))
            dialog.setContentView(sheetContent(dialog, spec, image, generation))
            val sheet =
                dialog.findViewById<FrameLayout>(com.google.android.material.R.id.design_bottom_sheet)
                    ?: error("BottomSheetDialog has no design_bottom_sheet")
            // Full height, so a half-expanded detent shows the content's head.
            sheet.layoutParams = sheet.layoutParams.apply { height = ViewGroup.LayoutParams.MATCH_PARENT }
            if (spec.has("cornerRadius")) applyCornerRadius(sheet, spec.getDouble("cornerRadius").toFloat())

            val behavior = dialog.behavior
            val live = LiveSheet(generation, behavior, sheet, detents)
            dialog.setCancelable(dismissible)
            behavior.isFitToContents = false
            behavior.skipCollapsed = true
            behavior.isDraggable = dismissible || detents.size > 1
            live.halfIndex =
                if (!detents[selected].full) selected else detents.indices.filter { !detents[it].full }.minByOrNull { detents[it].ratio } ?: -1
            live.fullIndex = if (detents[selected].full) selected else detents.indexOfFirst { it.full }
            if (live.halfIndex >= 0) behavior.halfExpandedRatio = detents[live.halfIndex].ratio
            live.lastIndex = selected
            behavior.state = stateOf(detents[selected])
            behavior.addBottomSheetCallback(
                object : BottomSheetBehavior.BottomSheetCallback() {
                    override fun onStateChanged(bottomSheet: View, newState: Int) = onSheetState(live, newState)

                    override fun onSlide(bottomSheet: View, slideOffset: Float) {}
                },
            )
            dialog.setOnCancelListener { resolveOnce(generation, OUTCOME_CANCELLED, -1) }
            dialog.setOnDismissListener {
                val action = live.pendingAction
                if (action >= 0) {
                    resolveOnce(generation, OUTCOME_ACTION, action)
                } else {
                    resolveOnce(generation, OUTCOME_CANCELLED, -1)
                }
            }
            liveGeneration = generation
            liveDialog = dialog
            liveActivity = activity
            liveSheet = live
            dialog.show()
        } catch (t: Throwable) {
            Log.w(TAG, "Frust: native sheet failed to show (the app theme must derive from Theme.Material3.*): $t")
            liveDialog = null
            liveActivity = null
            liveSheet = null
            nativeOnOutcome(generation, OUTCOME_FAILED, -1)
        }
    }

    /**
     * The dialog's context: the Activity's manifest theme, with the night bit
     * forced when the spec pins `dark`, the way [FrustNativeChrome] themes its bars.
     * A theme that cannot be resolved (no manifest entry, or a zero theme
     * resource) is logged once and the Activity itself is used instead.
     */
    private fun sheetContext(activity: Activity, spec: JSONObject): Context =
        try {
            FrustMaterialContext.wrap(activity, null, if (spec.has("dark")) spec.getBoolean("dark") else null)
        } catch (e: PackageManager.NameNotFoundException) {
            sheetThemeFallback(e)
            activity
        } catch (e: IllegalStateException) {
            sheetThemeFallback(e)
            activity
        }

    private var sheetThemeWarned = false

    private fun sheetThemeFallback(cause: Exception) {
        if (sheetThemeWarned) return
        sheetThemeWarned = true
        Log.w(TAG, "Frust: native sheet theme lookup failed; using the Activity's own theme", cause)
    }

    /**
     * Decode [bytes] with the longer side kept at or under the larger of four
     * times the image row's max height and the display width: bounds first,
     * then a power-of-two sample. `null` (logged) when it does not decode.
     */
    private fun decodeSheetImage(context: Context, bytes: ByteArray): Bitmap? = try {
        val metrics = context.resources.displayMetrics
        val cap = maxOf(4 * dp(context, SHEET_IMAGE_MAX_DP), metrics.widthPixels)
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) {
            null
        } else {
            val longer = maxOf(bounds.outWidth, bounds.outHeight)
            var sample = 1
            while ((longer + sample - 1) / sample > cap) sample *= 2
            BitmapFactory.decodeByteArray(bytes, 0, bytes.size, BitmapFactory.Options().apply { inSampleSize = sample })
        }
    } catch (t: Throwable) {
        Log.w(TAG, "Frust: native sheet image decode threw", t)
        null
    }

    private fun sheetContent(dialog: BottomSheetDialog, spec: JSONObject, image: ByteArray?, generation: Long): View {
        val context = dialog.context
        val grabber = spec.optBoolean("grabber", true)
        val inset = dp(context, SHEET_INSET_DP)
        val rows = LinearLayout(context)
        rows.orientation = LinearLayout.VERTICAL
        rows.setPadding(inset, if (grabber) 0 else inset, inset, inset)
        val addRow = { view: View ->
            val params = LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT)
            if (rows.childCount > 0) params.topMargin = dp(context, SHEET_ROW_SPACING_DP)
            rows.addView(view, params)
        }
        spec.optString("title").takeIf { it.isNotEmpty() }?.let { addRow(textRow(context, it, com.google.android.material.R.attr.textAppearanceTitleLarge)) }
        spec.optString("message").takeIf { it.isNotEmpty() }?.let {
            val message = textRow(context, it, com.google.android.material.R.attr.textAppearanceBodyMedium)
            message.setTextColor(MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnSurfaceVariant, message.currentTextColor))
            addRow(message)
        }
        if (image != null) {
            val bitmap = decodeSheetImage(context, image)
            if (bitmap == null) {
                Log.w(TAG, "Frust: native sheet image (${image.size} bytes) did not decode — row omitted")
            } else {
                val view = ImageView(context)
                view.adjustViewBounds = true
                view.maxHeight = dp(context, SHEET_IMAGE_MAX_DP)
                view.scaleType = ImageView.ScaleType.FIT_CENTER
                view.setImageBitmap(bitmap)
                addRow(view)
            }
        }
        val tint = if (spec.has("tint")) spec.getLong("tint").toInt() else null
        for (index in 0 until spec.optInt("actionCount")) {
            val button = actionButton(context, spec.optInt("action${index}Role"), tint)
            button.text = spec.optString("action${index}Label")
            button.setOnClickListener {
                val live = liveSheet
                if (live != null && live.generation == generation && live.pendingAction < 0) {
                    // Resolved by the dismiss listener, once the sheet is gone.
                    live.pendingAction = index
                    dialog.dismiss()
                }
            }
            addRow(button)
        }

        val scroll = NestedScrollView(context)
        scroll.addView(rows, ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT))
        val column = LinearLayout(context)
        column.orientation = LinearLayout.VERTICAL
        if (grabber) column.addView(BottomSheetDragHandleView(context))
        column.addView(scroll, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f))
        return column
    }

    private fun textRow(context: Context, text: String, appearanceAttr: Int): TextView {
        val view = TextView(context)
        val value = TypedValue()
        if (context.theme.resolveAttribute(appearanceAttr, value, true)) view.setTextAppearance(value.resourceId)
        view.text = text
        return view
    }

    /** `Default` filled (in `tint` when given), `Destructive` filled in `colorError`, `Cancel` outlined. */
    private fun actionButton(context: Context, role: Int, tint: Int?): MaterialButton =
        when (role) {
            ROLE_CANCEL -> MaterialButton(context, null, com.google.android.material.R.attr.materialButtonOutlinedStyle)
            ROLE_DESTRUCTIVE ->
                MaterialButton(context).apply {
                    backgroundTintList = ColorStateList.valueOf(MaterialColors.getColor(context, androidx.appcompat.R.attr.colorError, 0xFFB3261E.toInt()))
                    setTextColor(MaterialColors.getColor(context, com.google.android.material.R.attr.colorOnError, 0xFFFFFFFF.toInt()))
                }
            else -> MaterialButton(context).apply { if (tint != null) backgroundTintList = ColorStateList.valueOf(tint) }
        }

    /**
     * Round the sheet's top corners to `radiusDp` once the behavior has
     * installed its shape background (on the first layout).
     */
    private fun applyCornerRadius(sheet: FrameLayout, radiusDp: Float) {
        val radius = dp(sheet.context, radiusDp).toFloat()
        sheet.addOnLayoutChangeListener(
            object : View.OnLayoutChangeListener {
                override fun onLayoutChange(
                    v: View,
                    left: Int,
                    top: Int,
                    right: Int,
                    bottom: Int,
                    oldLeft: Int,
                    oldTop: Int,
                    oldRight: Int,
                    oldBottom: Int,
                ) {
                    v.removeOnLayoutChangeListener(this)
                    val shape = v.background as? MaterialShapeDrawable ?: return
                    shape.shapeAppearanceModel = shape.shapeAppearanceModel.toBuilder().setTopLeftCornerSize(radius).setTopRightCornerSize(radius).build()
                }
            },
        )
    }

    private fun dp(context: Context, value: Float): Int =
        TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, value, context.resources.displayMetrics).toInt()

    private fun stateOf(detent: SheetDetent): Int =
        if (detent.full) BottomSheetBehavior.STATE_EXPANDED else BottomSheetBehavior.STATE_HALF_EXPANDED

    /**
     * A settled state → the detent stream. A state no detent rests at
     * (expanded with no full detent, half expanded with no half one,
     * collapsed) returns to the detent the user left; the same detent again
     * (the one applied at show, a [selectSheetDetent] move) is not reported.
     */
    private fun onSheetState(live: LiveSheet, state: Int) {
        if (liveSheet !== live) return
        val settled =
            when (state) {
                BottomSheetBehavior.STATE_EXPANDED -> live.fullIndex
                BottomSheetBehavior.STATE_HALF_EXPANDED -> live.halfIndex
                BottomSheetBehavior.STATE_COLLAPSED -> -1
                else -> return
            }
        if (settled < 0) {
            live.sheet.post { if (liveSheet === live) live.behavior.state = stateOf(live.detents[live.lastIndex]) }
            return
        }
        if (settled == live.lastIndex) return
        live.lastIndex = settled
        nativeOnOutcome(live.generation, OUTCOME_DETENT, settled)
    }

    /** Animate [live] to the detent at `index`, recorded first so it is not echoed. */
    private fun moveSheet(live: LiveSheet, index: Int) {
        val detent = live.detents[index]
        live.lastIndex = index
        if (detent.full) {
            live.fullIndex = index
            live.behavior.state = BottomSheetBehavior.STATE_EXPANDED
            return
        }
        val ratioChanged = live.halfIndex != index
        live.halfIndex = index
        live.behavior.halfExpandedRatio = detent.ratio
        if (live.behavior.state != BottomSheetBehavior.STATE_HALF_EXPANDED) {
            live.behavior.state = BottomSheetBehavior.STATE_HALF_EXPANDED
        } else if (ratioChanged) {
            // Already half expanded: a layout pass re-offsets it to the new ratio.
            live.sheet.requestLayout()
        }
    }

    /** Run [block] on the main thread: inline when already there, else posted. */
    private fun onMain(block: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) block() else mainHandler.post { block() }
    }

    /** One spec detent: the full-height state, or half expanded at [ratio]. */
    private class SheetDetent(val full: Boolean, val ratio: Float)

    /**
     * The live sheet beside [liveDialog]: its behavior and detents, the
     * detent indices the half/full states currently stand for, the last
     * detent reported (or applied), and the tapped action awaiting the
     * dismissal (`-1` for none).
     */
    private class LiveSheet(
        val generation: Long,
        val behavior: BottomSheetBehavior<FrameLayout>,
        val sheet: FrameLayout,
        val detents: List<SheetDetent>,
    ) {
        var halfIndex = -1
        var fullIndex = -1
        var lastIndex = 0
        var pendingAction = -1
    }

    /**
     * Resolve `generation`'s presentation exactly once: a no-op once
     * [liveDialog] is `null` or belongs to a different generation — this
     * object's own guard (class doc's *The live dialog*), independent of
     * `android_alert.rs`'s own generation-keyed parking on the Rust side.
     * Never closes the dialog itself: every caller of [resolveOnce] other
     * than [dismiss]/[onActivityDestroyed] is itself one of the dialog's own
     * listeners, which the framework is already in the middle of tearing the
     * dialog down for.
     */
    private fun resolveOnce(generation: Long, code: Int, actionIndex: Int) {
        if (liveGeneration != generation || liveDialog == null) return
        liveDialog = null
        liveActivity = null
        liveSheet = null
        nativeOnOutcome(generation, code, actionIndex)
    }

    /**
     * Take an abandoned presentation's dialog down without reporting an
     * outcome for it — `android_alert.rs`'s `Host::show_alert` has already
     * resolved its Rust-side resolver directly (discarded on arrival, its
     * receiver already gone) by the time this runs, so nothing is listening
     * for whatever this dialog might still report.
     */
    private fun dismissStale() {
        val dialog = liveDialog ?: return
        liveDialog = null
        liveActivity = null
        liveSheet = null
        closeQuietly(dialog)
    }

    /**
     * `dialog.dismiss()`, detaching its dismiss listener first (so it cannot
     * re-enter [resolveOnce]) and swallowing whatever it throws — its window
     * may already be gone by the time a caller here needs it closed.
     */
    private fun closeQuietly(dialog: Dialog?) {
        if (dialog == null) return
        dialog.setOnDismissListener(null)
        try {
            dialog.dismiss()
        } catch (t: Throwable) {
            // Best-effort cleanup only; there is nothing more useful to do
            // with a window that is already gone.
        }
    }

    /** One of `AlertDialog`'s three button slots. */
    private enum class ButtonSlot { POSITIVE, NEGATIVE, NEUTRAL }

    /**
     * Assign each of `roles` (parallel to `showAlert`'s `labels`) its
     * preferred slot, in spec order, falling back down its preference list
     * when that slot is already taken by an earlier action — see
     * `android_alert.rs`'s module doc's *Button mapping* for the preference
     * lists and why every action always finds a free one.
     */
    private fun assignSlots(roles: IntArray): Array<ButtonSlot?> {
        val defaultPreference = arrayOf(ButtonSlot.POSITIVE, ButtonSlot.NEUTRAL, ButtonSlot.NEGATIVE)
        val preferenceByRole =
            mapOf(
                ROLE_DEFAULT to defaultPreference,
                ROLE_CANCEL to arrayOf(ButtonSlot.NEGATIVE, ButtonSlot.NEUTRAL, ButtonSlot.POSITIVE),
                ROLE_DESTRUCTIVE to defaultPreference,
            )
        val taken = mutableSetOf<ButtonSlot>()
        return Array(roles.size) { index ->
            val preference = preferenceByRole[roles[index]] ?: defaultPreference
            val slot = preference.firstOrNull { it !in taken }
            if (slot != null) taken.add(slot)
            slot
        }
    }

    /**
     * The one callback every presentation resolves through, on the main
     * thread: `generation` is the value Rust handed the show call (echoed
     * unchanged), `code` one of the `OUTCOME_*` constants above, and
     * `actionIndex` the chosen action's index for [OUTCOME_ACTION] (unused,
     * `-1`, otherwise). Rust drops a report whose generation is not the live
     * presentation's, so a duplicate or late report is harmless.
     *
     * Implemented in `plugins/native-widgets/src/present/android_host.rs`.
     * The native library is already loaded by the embedding's
     * `FrustActivity`, so no `System.loadLibrary` happens here.
     */
    @JvmStatic
    external fun nativeOnOutcome(generation: Long, code: Int, actionIndex: Int)

    /** Records the created and resumed Activity; see the class doc. */
    private val activityCallbacks =
        object : Application.ActivityLifecycleCallbacks {
            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {
                created.add(WeakReference(activity))
            }

            override fun onActivityStarted(activity: Activity) {}

            override fun onActivityResumed(activity: Activity) {
                resumed = WeakReference(activity)
            }

            override fun onActivityPaused(activity: Activity) {}

            override fun onActivityStopped(activity: Activity) {}

            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}

            override fun onActivityDestroyed(activity: Activity) {
                if (resumed?.get() === activity) {
                    resumed = null
                }
                created.removeAll { it.get().let { live -> live == null || live === activity } }
                FrustNativeChrome.onActivityDestroyed(activity)
                // The dialog [liveDialog] is attached to is going away with
                // its Activity (a configuration change, e.g. rotation, or
                // the user leaving the app) — report HostLost and take it
                // down, rather than let it leak a destroyed window.
                if (liveActivity === activity) {
                    val generation = liveGeneration
                    val dialog = liveDialog
                    resolveOnce(generation, OUTCOME_HOST_LOST, -1)
                    closeQuietly(dialog)
                }
            }
        }
}
