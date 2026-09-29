// The `frust-native-widgets` plugin's presentation host: the Kotlin half of
// the Rust <-> Kotlin contract also implemented by
// `plugins/native-widgets/src/present/android_host.rs` (host discovery, the
// `nativeOnOutcome` callback export) and
// `plugins/native-widgets/src/present/android_alert.rs` (the `showAlert`/
// `dismiss` calls below — see that module's contract table). Native
// presentations (alerts) are not platform-view slots: they are shown over the
// resumed Activity, which this object tracks.
//
// Package `dev.frust.nativewidgets` and the object name are baked into the
// mangled JNI symbol of `nativeOnOutcome`
// (`Java_dev_frust_nativewidgets_FrustNativePresenter_nativeOnOutcome`) and
// into the binary class name Rust loads it by (`PRESENTER_CLASS_BINARY`), so
// they are fixed once shipped. The `OUTCOME_*` codes are mirrored verbatim by
// the Rust side's `present::wire` table and pinned against drift by its
// tests; the `ROLE_*` codes below are mirrored by `android_alert.rs`'s own
// constants of the same names (edited together — no drift test pins this
// one, since `android_alert.rs` is Android-only and never builds under the
// host-targeted `cargo test` the `OUTCOME_*` check runs under).
package dev.frust.nativewidgets

import android.app.Activity
import android.app.AlertDialog
import android.app.Application
import android.content.Context
import android.content.DialogInterface
import android.os.Bundle
import java.lang.ref.WeakReference

/**
 * Tracks the resumed [Activity] a native presentation is shown over, shows
 * the one presentation kind this plugin ships (an
 * [android.app.AlertDialog]), and carries the one callback every
 * presentation reports its outcome through.
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
 * [liveDialog]/[liveActivity]/[liveGeneration] track the one
 * [AlertDialog] currently shown by [showAlert], independently of (and in
 * addition to) `android_alert.rs`'s own generation-keyed parking on the Rust
 * side. [resolveOnce] is this object's own exactly-once guard: it nulls
 * [liveDialog] the moment any path reports an outcome, so a framework
 * callback firing again for the same dialog (a button tap always dismisses
 * the dialog too, invoking both its own listener and the dismiss listener)
 * finds nothing left to report. Every field here is touched only from the
 * main thread: `showAlert`/`dismiss` are called directly, synchronously, by
 * Rust (`android_alert.rs`'s module doc, *Main-thread dispatch* — the whole
 * frust Android runtime is thread-confined to the main `Looper`), and
 * `AlertDialog`'s own listeners plus `Application.ActivityLifecycleCallbacks`
 * are always delivered there by the platform.
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

    /** An ordinary action — [showAlert]'s `roles` wire code for `Default`. */
    private const val ROLE_DEFAULT = 0

    /** The Cancel-role action — [showAlert]'s `roles` wire code for `Cancel`. */
    private const val ROLE_CANCEL = 1

    /**
     * The Destructive-role action — [showAlert]'s `roles` wire code for
     * `Destructive`.
     */
    private const val ROLE_DESTRUCTIVE = 2

    /**
     * The most recently resumed Activity. `@Volatile`: written on the main
     * thread by the lifecycle callbacks, read from whatever thread Rust
     * probes [hasResumedActivity] on.
     */
    @Volatile
    private var resumed: WeakReference<Activity>? = null

    /**
     * The dialog [showAlert] currently has up, or `null` between
     * presentations — this object's own exactly-once guard (class doc's *The
     * live dialog*). Main-thread only, unlike [resumed]: nothing here is ever
     * read from a JNI worker thread.
     */
    private var liveDialog: AlertDialog? = null

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
        closeQuietly(dialog)
    }

    /**
     * `dialog.dismiss()`, detaching its dismiss listener first (so it cannot
     * re-enter [resolveOnce]) and swallowing whatever it throws — its window
     * may already be gone by the time a caller here needs it closed.
     */
    private fun closeQuietly(dialog: AlertDialog?) {
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

    /** Records the resumed Activity; see the class doc. */
    private val activityCallbacks =
        object : Application.ActivityLifecycleCallbacks {
            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {}

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
