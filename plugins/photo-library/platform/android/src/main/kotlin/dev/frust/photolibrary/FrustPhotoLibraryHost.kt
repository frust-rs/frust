package dev.frust.photolibrary

import android.app.Activity
import android.app.Application
import android.content.Context
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.ext.SdkExtensions
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContract
import androidx.activity.result.contract.ActivityResultContracts
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/**
 * The `frust-photo-library` plugin's Android host — the Kotlin half of the
 * frozen Rust↔Kotlin contract also implemented by
 * `plugins/photo-library/src/android.rs`.
 *
 * ## The contract (LAW — see `plugins/photo-library/src/android.rs`)
 *
 * This object's fully-qualified name (`dev.frust.photolibrary.FrustPhotoLibraryHost`)
 * and its `@JvmStatic` entry point are looked up by string from Rust through
 * the application classloader, and the mangled JNI symbols of
 * [nativeOnPickResult] and [nativeOnPickError] are exported by that same
 * module. **The class may never move and no signature may change without a
 * coordinated change on both sides.** The integer codes below are pinned
 * against their Rust twins by `plugins/photo-library/tests/kotlin_contract.rs`.
 *
 * | Direction | Member | Returns |
 * |---|---|---|
 * | Rust → Kotlin | `pick(requestId: Long, filter: Int, limit: Int, destDir: String): Int` — `(JIILjava/lang/String;)I` | [PENDING] after posting the launch, [NO_ACTIVITY], [BUSY], [NOT_INITIALIZED], or [LAUNCH_FAILED] for an unknown filter |
 * | Kotlin → Rust | `nativeOnPickResult(requestId: Long, paths: Array<String>, names: Array<String?>, mimes: Array<String?>)` | the copies, in pick order; three empty arrays mean cancelled |
 * | Kotlin → Rust | `nativeOnPickError(requestId: Long, code: Int)` | [NO_ACTIVITY], [LAUNCH_FAILED] or [COPY_FAILED] |
 *
 * Every request that returned [PENDING] gets exactly one of the two answers.
 *
 * ## The picker
 *
 * [pick] launches `ActivityResultContracts.PickVisualMedia` for a limit of 1
 * and `PickMultipleVisualMedia` otherwise, with a `PickVisualMediaRequest` of
 * `ImageOnly`, `VideoOnly` or `ImageAndVideo`. The photo picker needs no
 * permission. `androidx.activity` 1.8.1 picks the first available of: the
 * system picker (API 33+, or API 30+ with R SDK extension 2), the Google Play
 * services backport, the GMS picker, and finally `ACTION_OPEN_DOCUMENT`
 * filtered to image and video MIME types.
 *
 * `PickMultipleVisualMedia(maxItems)` throws unless `maxItems > 1`, and on the
 * system picker its launch throws when `maxItems` exceeds
 * `MediaStore.getPickImagesMaxLimit()`. So a limit of 0 uses the contract's
 * default (that platform maximum), any other limit asks for
 * `maxOf(limit, 2)` capped at the platform maximum ([multipleMaxItems]), and
 * the result is truncated to the limit: the `ACTION_OPEN_DOCUMENT` fallback
 * enforces no maximum at all.
 *
 * ## Copies
 *
 * Each chosen content URI is copied on a background executor, through
 * `contentResolver.openInputStream`, into the Rust-chosen `destDir` (a fresh
 * directory under the plugin's pick cache) as [STAGING_PREFIX] + its index.
 * The host never builds a file name from platform text: it reports each
 * item's display name (`OpenableColumns.DISPLAY_NAME`) and MIME type
 * (`ContentResolver.getType`) next to its staging path, and the Rust side
 * sanitises the name and renames the copy, so one naming policy covers every
 * platform. A read failure on any item fails the pick with [COPY_FAILED].
 *
 * ## One pick in flight
 *
 * One pick is in flight at a time. While an earlier pick holds the slot (its
 * picker is on screen or its copies are running), [pick] launches nothing and
 * returns [BUSY]. The slot is released by the pick's own answer, by a launch
 * that fails, or when the Activity it launched from is destroyed while the
 * picker is still up: that sends a cancel, so the Rust side never waits
 * forever. A pick whose copies have started no longer depends on the Activity
 * and finishes on its own. The Rust side refuses a second concurrent pick too.
 *
 * The launch is posted to the main thread; the launcher is registered on the
 * resumed [ComponentActivity]'s `activityResultRegistry` under the key
 * `frust.photolibrary.<requestId>` and unregistered from its own callback, so
 * the consuming app never forwards `onActivityResult`.
 */
object FrustPhotoLibraryHost {
    private const val TAG = "frust"

    // --- `pick` return codes and `nativeOnPickError` codes: mirror
    // `src/android.rs`, pinned by `tests/kotlin_contract.rs`.

    /** The picker launch was posted; the answer arrives through a native callback. */
    const val PENDING = 0
    /** No resumed Activity to launch from, or it was finishing by launch time. */
    const val NO_ACTIVITY = 1
    /** Another pick is in flight. */
    const val BUSY = 2
    /** No application context was installed (the init provider never ran). */
    const val NOT_INITIALIZED = 3
    /** The picker could not be launched (or the filter code is unknown). */
    const val LAUNCH_FAILED = 4
    /** A chosen item could not be copied. */
    const val COPY_FAILED = 5

    // --- Filter codes: mirror `src/android.rs`.

    /** Still images only. */
    const val FILTER_IMAGES = 0
    /** Videos only. */
    const val FILTER_VIDEOS = 1
    /** Images and videos. */
    const val FILTER_IMAGES_AND_VIDEOS = 2

    /**
     * File-name prefix of a staging copy inside `destDir`; the item's index
     * follows. Starts with a dot, which the Rust sanitiser never produces, so
     * a renamed copy can never land on a staging file.
     */
    const val STAGING_PREFIX = ".staging-"

    /** Activity-result registry key prefix; the request id follows. */
    const val LAUNCHER_KEY_PREFIX = "frust.photolibrary."

    /** The application [Context], installed once by [FrustPhotoLibraryInitProvider]. */
    @Volatile
    private var appContext: Context? = null

    /**
     * The currently resumed Activity: set in `onActivityResumed`, cleared when
     * that same instance pauses or is destroyed. The picker launches from it.
     */
    @Volatile
    private var resumedActivity: Activity? = null

    /**
     * The pick holding the single in-flight slot, or null. Read and written
     * only through [claimInFlight], [releaseInFlight], [detachInFlight] and
     * [releaseInFlightFor], which hold this object's monitor.
     */
    private var inFlight: InFlight? = null

    /**
     * One unanswered pick. [activity] is the Activity its picker launched
     * from, or null once its copies started (it no longer depends on one).
     */
    private class InFlight(val requestId: Long, var activity: Activity?)

    /** Guards a single [Application.registerActivityLifecycleCallbacks] call. */
    @Volatile
    private var lifecycleCallbacksRegistered: Boolean = false

    /** Runs every launch on the main thread. */
    private val mainHandler = Handler(Looper.getMainLooper())

    /** Copies picked items off the main thread, one pick at a time. */
    private val copyExecutor: ExecutorService by lazy {
        Executors.newSingleThreadExecutor { runnable ->
            Thread(runnable, "frust-photo-library-copy").apply { isDaemon = true }
        }
    }

    /**
     * Install the application [Context] — called once at process start by
     * [FrustPhotoLibraryInitProvider]. Idempotent and first-writer-wins.
     */
    internal fun installApplicationContext(context: Context) {
        if (appContext == null) appContext = context
        ensureLifecycleCallbacks(context)
    }

    /**
     * Register the process-wide [Application.ActivityLifecycleCallbacks] once.
     * The `as?` keeps a non-`Application` context (a test double, an
     * oddly-wrapped host) from throwing at process start.
     */
    @Synchronized
    private fun ensureLifecycleCallbacks(context: Context) {
        if (lifecycleCallbacksRegistered) return
        val app = context.applicationContext as? Application ?: return
        lifecycleCallbacksRegistered = true
        app.registerActivityLifecycleCallbacks(activityCallbacks)
    }

    // --- Rust -> Kotlin -----------------------------------------------------

    /**
     * Open the photo picker for [filter], allowing up to [limit] items (0 =
     * the platform maximum), and copy the chosen items into [destDir]. Any
     * thread. Returns [PENDING] after posting the launch to the main thread;
     * the answer then arrives through [nativeOnPickResult] or
     * [nativeOnPickError] carrying [requestId]. Returns [BUSY] while another
     * pick holds the in-flight slot (class doc, "One pick in flight").
     */
    @JvmStatic
    fun pick(requestId: Long, filter: Int, limit: Int, destDir: String): Int {
        val ctx = appContext ?: return NOT_INITIALIZED
        val mediaType = mediaTypeFor(filter) ?: return LAUNCH_FAILED
        val activity = resumedActivity as? ComponentActivity ?: return NO_ACTIVITY
        if (!claimInFlight(requestId, activity)) {
            Log.i(TAG, "frust-photo-library: a pick is already in flight")
            return BUSY
        }
        val request = PickVisualMediaRequest.Builder().setMediaType(mediaType).build()
        mainHandler.post { launch(ctx, requestId, request, limit, File(destDir), activity) }
        return PENDING
    }

    // --- Kotlin -> Rust -----------------------------------------------------

    /**
     * Answers the [pick] that returned [PENDING] with the same [requestId]:
     * the staging copies in pick order with each item's display name and MIME
     * type (either may be null). Three empty arrays mean the user cancelled.
     * Implemented in `plugins/photo-library/src/android.rs`. The native
     * library is already loaded by the embedding's `FrustActivity`, so no
     * `System.loadLibrary` happens here.
     */
    @JvmStatic
    external fun nativeOnPickResult(
        requestId: Long,
        paths: Array<String>,
        names: Array<String?>,
        mimes: Array<String?>,
    )

    /** Fails the [pick] that returned [PENDING] with the same [requestId]. */
    @JvmStatic
    external fun nativeOnPickError(requestId: Long, code: Int)

    // Every native call is guarded: a missing symbol (an app that depends on
    // this Gradle module without the `frust-photo-library` Rust crate) must
    // degrade to a log line, never an UnsatisfiedLinkError thrown into an
    // Activity result callback or the copy thread.
    private fun notifyResult(
        requestId: Long,
        paths: Array<String>,
        names: Array<String?>,
        mimes: Array<String?>,
    ) {
        try {
            nativeOnPickResult(requestId, paths, names, mimes)
        } catch (e: Throwable) {
            Log.w(TAG, "frust-photo-library: nativeOnPickResult unavailable", e)
        }
    }

    private fun notifyCancelled(requestId: Long) {
        notifyResult(requestId, emptyArray(), emptyArray(), emptyArray())
    }

    private fun notifyError(requestId: Long, code: Int) {
        try {
            nativeOnPickError(requestId, code)
        } catch (e: Throwable) {
            Log.w(TAG, "frust-photo-library: nativeOnPickError unavailable", e)
        }
    }

    // --- Internals ----------------------------------------------------------

    /** The picker media type for a filter code, or null for an unknown code. */
    private fun mediaTypeFor(filter: Int): ActivityResultContracts.PickVisualMedia.VisualMediaType? =
        when (filter) {
            FILTER_IMAGES -> ActivityResultContracts.PickVisualMedia.ImageOnly
            FILTER_VIDEOS -> ActivityResultContracts.PickVisualMedia.VideoOnly
            FILTER_IMAGES_AND_VIDEOS -> ActivityResultContracts.PickVisualMedia.ImageAndVideo
            else -> null
        }

    /**
     * The `maxItems` for `PickMultipleVisualMedia` given a positive [limit]:
     * at least 2 (the contract's own lower bound) and at most [platformMax]
     * (the system picker's `MediaStore.getPickImagesMaxLimit()`).
     */
    internal fun multipleMaxItems(limit: Int, platformMax: Int): Int =
        maxOf(limit, 2).coerceAtMost(maxOf(platformMax, 2))

    /**
     * Whether `androidx.activity` will launch the system picker: the same
     * test as its internal `isSystemPickerAvailable` (API 33+, or API 30+ with
     * R SDK extension 2). Only that picker enforces a maximum.
     */
    private fun systemPickerAvailable(): Boolean = when {
        Build.VERSION.SDK_INT >= 33 -> true
        Build.VERSION.SDK_INT >= 30 -> SdkExtensions.getExtensionVersion(Build.VERSION_CODES.R) >= 2
        else -> false
    }

    /** The most items the launched picker accepts. */
    private fun platformMaxItems(): Int =
        if (systemPickerAvailable()) MediaStore.getPickImagesMaxLimit() else Int.MAX_VALUE

    /** Register, launch and answer one picker. Main thread. */
    private fun launch(
        ctx: Context,
        requestId: Long,
        request: PickVisualMediaRequest,
        limit: Int,
        destDir: File,
        activity: ComponentActivity,
    ) {
        if (activity.isFinishing || activity.isDestroyed) {
            if (releaseInFlight(requestId)) notifyError(requestId, NO_ACTIVITY)
            return
        }
        val key = LAUNCHER_KEY_PREFIX + requestId
        var launcher: ActivityResultLauncher<PickVisualMediaRequest>? = null
        try {
            launcher = if (limit == 1) {
                register(activity, key, ActivityResultContracts.PickVisualMedia()) { uri ->
                    onPicked(ctx, requestId, listOfNotNull(uri), limit, destDir)
                }
            } else {
                val contract = if (limit <= 0) {
                    ActivityResultContracts.PickMultipleVisualMedia()
                } else {
                    ActivityResultContracts.PickMultipleVisualMedia(multipleMaxItems(limit, platformMaxItems()))
                }
                register(activity, key, contract) { uris ->
                    onPicked(ctx, requestId, uris, limit, destDir)
                }
            }
            launcher.launch(request)
        } catch (e: Exception) {
            launcher?.unregister()
            Log.w(TAG, "frust-photo-library: launching the picker failed: ${e.javaClass.name}")
            if (releaseInFlight(requestId)) notifyError(requestId, LAUNCH_FAILED)
        }
    }

    /** A one-shot launcher under [key] that unregisters itself before [onResult]. */
    private fun <O> register(
        activity: ComponentActivity,
        key: String,
        contract: ActivityResultContract<PickVisualMediaRequest, O>,
        onResult: (O) -> Unit,
    ): ActivityResultLauncher<PickVisualMediaRequest> {
        var launcher: ActivityResultLauncher<PickVisualMediaRequest>? = null
        val registered = activity.activityResultRegistry.register(key, contract) { result ->
            launcher?.unregister()
            onResult(result)
        }
        launcher = registered
        return registered
    }

    /**
     * The picker answered with [uris] (empty when cancelled). Truncates to
     * [limit], then copies on [copyExecutor] and answers Rust. Main thread.
     */
    private fun onPicked(ctx: Context, requestId: Long, uris: List<Uri>, limit: Int, destDir: File) {
        val chosen = if (limit > 0) uris.take(limit) else uris
        if (chosen.isEmpty()) {
            if (releaseInFlight(requestId)) notifyCancelled(requestId)
            return
        }
        // Detached from its Activity: a destroy from here on must not cancel it.
        if (!detachInFlight(requestId)) return
        copyExecutor.execute {
            val copies = try {
                chosen.mapIndexed { index, uri -> copyOne(ctx, uri, File(destDir, STAGING_PREFIX + index)) }
            } catch (e: Exception) {
                Log.w(TAG, "frust-photo-library: copying a picked item failed: ${e.javaClass.name}")
                if (releaseInFlight(requestId)) notifyError(requestId, COPY_FAILED)
                return@execute
            }
            if (releaseInFlight(requestId)) {
                notifyResult(
                    requestId,
                    copies.map { it.path }.toTypedArray(),
                    copies.map { it.name }.toTypedArray(),
                    copies.map { it.mime }.toTypedArray(),
                )
            }
        }
    }

    /** One copied item: the staging path plus what the provider reported. */
    private class Copy(val path: String, val name: String?, val mime: String?)

    /** Copy [uri] to [dest], reading its display name and MIME type. Copy thread. */
    private fun copyOne(ctx: Context, uri: Uri, dest: File): Copy {
        val resolver = ctx.contentResolver
        val name = try {
            resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
                if (cursor.moveToFirst() && !cursor.isNull(0)) cursor.getString(0) else null
            }
        } catch (e: Exception) {
            null
        }
        val mime = try {
            resolver.getType(uri)
        } catch (e: Exception) {
            null
        }
        val input = resolver.openInputStream(uri) ?: throw IOException("no input stream")
        input.use { source -> FileOutputStream(dest).use { sink -> source.copyTo(sink) } }
        return Copy(dest.absolutePath, name, mime)
    }

    /**
     * Take the single in-flight slot for [requestId], or return false while
     * another pick holds it. No takeover: every holder is answered exactly
     * once, so a waiting Rust caller is never orphaned.
     */
    @Synchronized
    private fun claimInFlight(requestId: Long, activity: Activity): Boolean {
        if (inFlight != null) return false
        inFlight = InFlight(requestId, activity)
        return true
    }

    /** Free the slot if [requestId] holds it; true when this call freed it (and must answer). */
    @Synchronized
    private fun releaseInFlight(requestId: Long): Boolean {
        if (inFlight?.requestId != requestId) return false
        inFlight = null
        return true
    }

    /** Detach [requestId]'s slot from its Activity; false when it no longer holds the slot. */
    @Synchronized
    private fun detachInFlight(requestId: Long): Boolean {
        val current = inFlight ?: return false
        if (current.requestId != requestId) return false
        current.activity = null
        return true
    }

    /** Free the slot if its picker launched from [activity]; the freed request id, or null. */
    @Synchronized
    private fun releaseInFlightFor(activity: Activity): Long? {
        val current = inFlight ?: return null
        if (current.activity !== activity) return null
        inFlight = null
        return current.requestId
    }

    /** Tracks the resumed Activity; see [resumedActivity]. */
    private val activityCallbacks =
        object : Application.ActivityLifecycleCallbacks {
            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {}

            override fun onActivityStarted(activity: Activity) {}

            override fun onActivityResumed(activity: Activity) {
                resumedActivity = activity
            }

            override fun onActivityPaused(activity: Activity) {
                if (activity === resumedActivity) resumedActivity = null
            }

            override fun onActivityStopped(activity: Activity) {}

            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}

            override fun onActivityDestroyed(activity: Activity) {
                if (activity === resumedActivity) resumedActivity = null
                // Its picker's callback can no longer fire: cancel, so Rust stops waiting.
                releaseInFlightFor(activity)?.let { notifyCancelled(it) }
            }
        }
}
