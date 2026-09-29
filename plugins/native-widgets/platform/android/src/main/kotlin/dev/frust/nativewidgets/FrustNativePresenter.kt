// The `frust-native-widgets` plugin's presentation host: the Kotlin half of
// the Rust <-> Kotlin contract also implemented by
// `plugins/native-widgets/src/present/android_host.rs` (see that module's
// contract table). Native presentations (alerts) are not platform-view slots:
// they are shown over the resumed Activity, which this object tracks.
//
// Package `dev.frust.nativewidgets` and the object name are baked into the
// mangled JNI symbol of `nativeOnOutcome`
// (`Java_dev_frust_nativewidgets_FrustNativePresenter_nativeOnOutcome`) and
// into the binary class name Rust loads it by (`PRESENTER_CLASS_BINARY`), so
// they are fixed once shipped. The `OUTCOME_*` codes are mirrored verbatim by
// the Rust side's `present::wire` table and pinned against drift by its tests.
package dev.frust.nativewidgets

import android.app.Activity
import android.app.Application
import android.content.Context
import android.os.Bundle
import java.lang.ref.WeakReference

/**
 * Tracks the resumed [Activity] a native presentation is shown over, and
 * carries the one callback every presentation reports its outcome through.
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

    /**
     * The most recently resumed Activity. `@Volatile`: written on the main
     * thread by the lifecycle callbacks, read from whatever thread Rust
     * probes [hasResumedActivity] on.
     */
    @Volatile
    private var resumed: WeakReference<Activity>? = null

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
            }
        }
}
