package dev.frust.iap

import android.app.Activity
import android.app.Application
import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.database.Cursor
import android.net.Uri
import android.os.Bundle
import dev.hyo.openiap.OpenIapError

/**
 * The `frust-iap` plugin's Android billing host — currently a process-start
 * bootstrap stub (task 02's Android module spike, `docs/PLUGINS_ARCHITECTURE.md`).
 * This proves the module's manifest wiring and its `openiap-google` dependency
 * compile and link; no OpenIAP billing calls are made yet and no Rust<->Kotlin
 * contract exists on this class yet — both are later work.
 *
 * Mirrors `plugins/camera/platform/android`'s `FrustCameraHost` process-start
 * shape: [installApplicationContext] is called once, at process start, by
 * [FrustIapInitProvider.onCreate] — before any Activity exists — and
 * [ensureLifecycleCallbacks] registers a process-wide
 * [Application.ActivityLifecycleCallbacks] that caches whichever Activity is
 * currently resumed. Camera's own doc comment calls that shape the fix for a
 * permission-dialog deadlock (no Activity observable before a view mounts);
 * this module adopts it pre-emptively because a billing purchase flow will
 * equally need a live Activity to launch `BillingClient`'s purchase UI from,
 * and that need cannot wait for a `frust::platform_view` slot the way camera's
 * preview does — billing has no view of its own.
 */
object FrustIapHost {
    /**
     * The application [Context], installed at process start by
     * [FrustIapInitProvider]. `@Volatile` since [installApplicationContext] and
     * any later Rust-facing static run on different threads (the plugin
     * never-on-the-UI-thread invariant `plugins/camera`'s host documents).
     */
    @Volatile
    private var appContext: Context? = null

    /**
     * The currently resumed Activity, learned from the process-wide lifecycle
     * callbacks registered in [ensureLifecycleCallbacks]. `@Volatile` for the
     * same cross-thread reason as [appContext].
     */
    @Volatile
    private var activity: Activity? = null

    /** Guards a single [Application.registerActivityLifecycleCallbacks] call. */
    private var lifecycleCallbacksRegistered: Boolean = false

    /**
     * Install the application [Context] — called once at process start by
     * [FrustIapInitProvider], before any Activity or view exists. Idempotent
     * and first-writer-wins, matching `FrustCameraHost.installApplicationContext`'s
     * contract in the sibling plugin (no direct reference — this module never
     * depends on `frust-camera`).
     */
    internal fun installApplicationContext(context: Context) {
        if (appContext == null) appContext = context
        ensureLifecycleCallbacks(context)
    }

    /**
     * Register the process-wide [Application.ActivityLifecycleCallbacks] once.
     *
     * [Context.getApplicationContext] is the `Application` instance on every
     * supported API level; the `as?` keeps a non-`Application` context (a test
     * double, an oddly-wrapped host) from throwing at process start.
     */
    private fun ensureLifecycleCallbacks(context: Context) {
        if (lifecycleCallbacksRegistered) return
        val app = context.applicationContext as? Application ?: return
        lifecycleCallbacksRegistered = true
        app.registerActivityLifecycleCallbacks(activityCallbacks)
    }

    /**
     * Caches whichever Activity is currently resumed/paused/destroyed. No
     * billing logic is driven off this yet (that is later work) — this spike
     * only proves the bootstrap plumbing.
     */
    private val activityCallbacks =
        object : Application.ActivityLifecycleCallbacks {
            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {}

            override fun onActivityStarted(activity: Activity) {}

            override fun onActivityResumed(activity: Activity) {
                this@FrustIapHost.activity = activity
            }

            override fun onActivityPaused(activity: Activity) {}

            override fun onActivityStopped(activity: Activity) {}

            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}

            override fun onActivityDestroyed(activity: Activity) {
                if (activity === this@FrustIapHost.activity) this@FrustIapHost.activity = null
            }
        }

    /**
     * References one `openiap-google` type so the dependency in
     * `build.gradle.kts` actually compiles and links against, rather than
     * merely resolving on the classpath. No production caller exists yet —
     * this spike's whole point is proving `api("io.github.hyochan.openiap:
     * openiap-google:3.0.1")` is usable; the real OpenIAP call surface and its
     * error mapping are later work (see the plugin's task list).
     */
    internal fun describeOpenIapDependency(): String = OpenIapError::class.java.name
}

/**
 * Installs the application [Context] into [FrustIapHost] at process start.
 *
 * A `ContentProvider` declared in this module's own `AndroidManifest.xml` is
 * created by the system before `Application.onCreate` — the standard
 * self-initialization mechanism for an Android library that needs a `Context`
 * with no app-side code (`androidx.startup`'s `InitializationProvider`,
 * Firebase's `FirebaseInitProvider`; the same shape `plugins/camera`'s
 * `FrustCameraInitProvider` uses).
 *
 * It provides no data: every `ContentProvider` operation returns null/0. It is
 * `exported="false"` and its authority is `${applicationId}`-scoped, so nothing
 * outside the app can reach it.
 */
class FrustIapInitProvider : ContentProvider() {
    override fun onCreate(): Boolean {
        context?.let { FrustIapHost.installApplicationContext(it.applicationContext) }
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
