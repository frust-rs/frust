package dev.frust.backgroundtask

import android.content.Context
import android.content.Intent
import android.os.Handler
import android.os.Looper
import android.util.Log

/**
 * The live-task table behind one [FrustBackgroundTaskService], reached from Rust over JNI.
 *
 * Every field is touched on the main thread only; [end] may be called from any thread and
 * posts there. [begin] is deliberately just a map insert and one binder call, because it
 * runs inside `onPause`: the channel and notification are built by the service, a later
 * main-looper turn.
 *
 * The service is never stopped before it has called `startForeground`, even when a short
 * task ends first. Stopping a service started with `startForegroundService` earlier than
 * that crashes the app, so [end] only forgets the id and the service decides.
 */
object FrustBackgroundTaskHost {
    internal const val CHANNEL_ID = "frust_background_work"
    internal const val NOTIFICATION_ID = 0x46525354

    /** Below the 3-minute `shortService` limit, and the only limit before API 34. */
    private const val WATCHDOG_MS = 150_000L
    private const val TAG = "frust"

    private val mainHandler = Handler(Looper.getMainLooper())
    private val tasks = LinkedHashMap<Long, String>()
    private val watchdog = Runnable { timeout() }
    private var service: FrustBackgroundTaskService? = null
    private var refusalLogged = false

    /**
     * Track task [id] and start the service. Returns whether the service covers it.
     *
     * A start that fails while other tasks are live is ignored: the service already
     * running (or on its way) covers this task too. A null `ComponentName` means the app
     * removed the service from its manifest.
     */
    @JvmStatic
    fun begin(context: Context, id: Long, name: String): Boolean {
        val covered = tasks.isNotEmpty()
        tasks[id] = name
        var failure: Exception? = null
        val started = try {
            context.startForegroundService(
                Intent(context, FrustBackgroundTaskService::class.java),
            ) != null
        } catch (e: Exception) {
            failure = e
            false
        }
        if (started || covered) return true
        tasks.remove(id)
        logRefusal("startForegroundService", failure)
        return false
    }

    /** Forget task [id]; idempotent. [context] is part of the JNI contract, and unused. */
    @Suppress("UNUSED_PARAMETER")
    @JvmStatic
    fun end(context: Context, id: Long) {
        mainHandler.post {
            tasks.remove(id)
            if (tasks.isEmpty()) {
                mainHandler.removeCallbacks(watchdog)
                service?.stopIfForeground()
            }
        }
    }

    @JvmStatic
    external fun nativeOnTimeout(ids: LongArray)

    internal fun attach(instance: FrustBackgroundTaskService) {
        service = instance
    }

    internal fun detach(instance: FrustBackgroundTaskService) {
        if (service === instance) service = null
    }

    internal fun isIdle(): Boolean = tasks.isEmpty()

    internal fun firstTaskName(): String = tasks.values.firstOrNull().orEmpty()

    /** (Re-)arm the watchdog after a successful `startForeground`. */
    internal fun armWatchdog() {
        mainHandler.removeCallbacks(watchdog)
        if (tasks.isNotEmpty()) mainHandler.postDelayed(watchdog, WATCHDOG_MS)
    }

    /** The watchdog or `Service.onTimeout`: expire every live task, then stop. */
    internal fun timeout() {
        expireLive()
        service?.stopNow()
    }

    /** Tell Rust every live task lost its protection. A no-op when none are live. */
    internal fun expireLive() {
        mainHandler.removeCallbacks(watchdog)
        if (tasks.isEmpty()) return
        val ids = tasks.keys.toLongArray()
        tasks.clear()
        nativeOnTimeout(ids)
    }

    internal fun logRefusal(what: String, cause: Exception?) {
        if (refusalLogged) return
        refusalLogged = true
        val reason = cause?.javaClass?.name ?: "no component"
        Log.w(TAG, "frust-background-task: $what was refused ($reason); work runs unprotected")
    }
}
