package dev.frust.photolibrary

import android.Manifest
import android.app.Activity
import android.app.Application
import android.content.ContentResolver
import android.content.Context
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
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
import java.io.FileNotFoundException
import java.io.FileOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The `frust-photo-library` plugin's Android host — the Kotlin half of the
 * frozen Rust↔Kotlin contract also implemented by
 * `plugins/photo-library/src/android.rs`.
 *
 * ## The contract (LAW — see `plugins/photo-library/src/android.rs`)
 *
 * This object's fully-qualified name (`dev.frust.photolibrary.FrustPhotoLibraryHost`)
 * and its `@JvmStatic` entry points are looked up by string from Rust through
 * the application classloader, and the mangled JNI symbols of
 * [nativeOnPickResult], [nativeOnPickError], [nativeOnPickProgress],
 * [nativeOnExportProgress], [nativeOnExportDone] and [nativeOnSaveDone] are
 * exported by that same module. **The class may never move and no signature may change without a
 * coordinated change on both sides.** The integer codes below are pinned
 * against their Rust twins by `plugins/photo-library/tests/kotlin_contract.rs`.
 *
 * | Direction | Member | Returns |
 * |---|---|---|
 * | Rust → Kotlin | `pick(requestId: Long, filter: Int, limit: Int, destDir: String): Int` — `(JIILjava/lang/String;)I` | [PENDING] after posting the launch, [NO_ACTIVITY], [BUSY], [NOT_INITIALIZED], or [LAUNCH_FAILED] for an unknown filter |
 * | Rust → Kotlin | `cancel(requestId: Long)` — `(J)V` | nothing; an id that does not hold the slot is ignored |
 * | Kotlin → Rust | `nativeOnPickResult(requestId: Long, paths: Array<String>, names: Array<String?>, mimes: Array<String?>)` | the copies, in pick order; three empty arrays mean cancelled |
 * | Kotlin → Rust | `nativeOnPickError(requestId: Long, code: Int)` | [NO_ACTIVITY], [LAUNCH_FAILED], [COPY_FAILED] or [CANCELLED] |
 * | Kotlin → Rust | `nativeOnPickProgress(requestId: Long, done: Int, items: Int, fraction: Double)` | copy progress, never an answer |
 * | Rust → Kotlin | `albums(): AlbumsResult` | an [AlbumsResult]; `code` is [OK], [NOT_INITIALIZED], [PERMISSION_DENIED] or [QUERY_FAILED] |
 * | Rust → Kotlin | `assets(albumId: String?, filter: Int, offset: Long, limit: Int): AssetsResult` | an [AssetsResult], same codes as `albums` |
 * | Rust → Kotlin | `thumbnail(id: String, width: Int, height: Int, fill: Boolean): ThumbnailResult` | a [ThumbnailResult], same codes plus [NOT_FOUND] |
 * | Rust → Kotlin | `manageLimitedSelection(requestId: Long): Int` — `(J)I` | [PENDING] after posting the prompt, [NOT_LIMITED], [NO_ACTIVITY], [BUSY] or [NOT_INITIALIZED]; answered through [nativeOnPickResult] (three empty arrays) or [nativeOnPickError] |
 * | Rust → Kotlin | `exportOriginal(requestId: Long, uri: String, destPath: String): Int` — `(JLjava/lang/String;Ljava/lang/String;)I` | [PENDING] after queuing the copy, [NOT_INITIALIZED], or [NOT_FOUND] for an id that is not a `MediaStore` image or video item on an external volume |
 * | Rust → Kotlin | `cancelExport(requestId: Long)` — `(J)V` | nothing; an unknown or already-answered id is ignored |
 * | Rust → Kotlin | `saveToLibrary(requestId: Long, sourcePath: String, isVideo: Boolean, displayName: String, album: String?): Int` — `(JLjava/lang/String;ZLjava/lang/String;Ljava/lang/String;)I` | [PENDING] after queuing the save, or [NOT_INITIALIZED] |
 * | Kotlin → Rust | `nativeOnExportProgress(requestId: Long, done: Long, total: Long)` | bytes copied, once per chunk; absent when the size is unknown |
 * | Kotlin → Rust | `nativeOnExportDone(requestId: Long, code: Int, mime: String?, name: String?)` | [OK] with the provider's MIME type and display name, or [CANCELLED], [NOT_FOUND], [PERMISSION_DENIED], [EXPORT_FAILED] |
 * | Kotlin → Rust | `nativeOnSaveDone(requestId: Long, code: Int, uri: String?)` | [OK] with the new asset's URI, or [PERMISSION_DENIED], [SAVE_FAILED] |
 *
 * Every request that returned [PENDING] gets exactly one answer: a pick gets
 * [nativeOnPickResult] or [nativeOnPickError], an export [nativeOnExportDone],
 * a save [nativeOnSaveDone].
 *
 * ## Exports and saves
 *
 * [exportOriginal] copies one asset on [exportExecutor], in chunks of
 * [EXPORT_CHUNK_BYTES] written to the Rust-chosen `destPath`. It is keyed by
 * its own request map ([exports]), so it never takes the pick slot and an
 * export runs while a pick does; several exports queue on that executor.
 * [cancelExport] flips the export's flag, checked before every chunk; on a
 * cancel or error the partial file is deleted before the answer. A still
 * queued export is answered from [cancelExport] and never opens anything.
 * The copy uses `MediaStore.setRequireOriginal` only when
 * `ACCESS_MEDIA_LOCATION` is granted, so location EXIF is not redacted; its
 * `UnsupportedOperationException` falls back to the plain URI, because an
 * export never fails over location. `MediaStore` has no rendition split and
 * its files are local, so the rendition and network options are ignored here.
 * [saveToLibrary] runs in [MediaStoreWriter].
 *
 * ## Browsing
 *
 * [albums], [assets] and [thumbnail] run in [MediaStoreBrowser] on the calling
 * thread, return their result object synchronously, and never touch the
 * in-flight slot. [manageLimitedSelection] presents the Android 14 selection
 * UI: `RequestMultiplePermissions` for the SDK media set, through the same
 * slot and launcher key as a pick.
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
 * ## Cancel and progress
 *
 * [cancel] marks the request cancelled. The system picker has no dismissal
 * API, so while it is still up the slot is freed at once, [CANCELLED] is
 * answered, and the picker's eventual result is discarded. Once copies run,
 * the copy loop checks the flag between items and between [COPY_CHUNK_BYTES]
 * chunks, deletes this call's staging files and answers [CANCELLED]. The copy
 * thread reports [nativeOnPickProgress] when copying starts (0 done, 0.0), at
 * most every [PROGRESS_INTERVAL_MS] during a copy, and after each item; the
 * fraction counts bytes when `OpenableColumns.SIZE` is known for every item,
 * items otherwise. It is never called while holding a monitor.
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
    /** The pick was cancelled through [cancel]. */
    const val CANCELLED = 6
    /** A browse call succeeded; the result object carries its data. */
    const val OK = 7
    /** No photo permission of the SDK set is granted. */
    const val PERMISSION_DENIED = 8
    /** The asset no longer exists (a stale identifier). */
    const val NOT_FOUND = 9
    /** A `MediaStore` query or decode failed. */
    const val QUERY_FAILED = 10
    /** [manageLimitedSelection] had nothing to do: access is not limited. */
    const val NOT_LIMITED = 11
    /** An export could not be copied. */
    const val EXPORT_FAILED = 12
    /** A save could not be written. */
    const val SAVE_FAILED = 13

    // --- Filter codes: mirror `src/android.rs`.

    /** Still images only. */
    const val FILTER_IMAGES = 0
    /** Videos only. */
    const val FILTER_VIDEOS = 1
    /** Images and videos. */
    const val FILTER_IMAGES_AND_VIDEOS = 2

    // --- Asset kinds in [AssetsResult.kinds]: mirror `src/android_library.rs`.

    /** A still image. */
    const val KIND_IMAGE = 0
    /** A video. */
    const val KIND_VIDEO = 1

    /** The synthetic all-media album's id, listed first by [albums]. */
    const val ALL_MEDIA_ALBUM_ID = "frust.all_media"

    /**
     * File-name prefix of a staging copy inside `destDir`; the item's index
     * follows. Starts with a dot, which the Rust sanitiser never produces, so
     * a renamed copy can never land on a staging file.
     */
    const val STAGING_PREFIX = ".staging-"

    /** Activity-result registry key prefix; the request id follows. */
    const val LAUNCHER_KEY_PREFIX = "frust.photolibrary."

    /** Bytes per read of the copy loop; the cancel flag is checked between chunks. */
    const val COPY_CHUNK_BYTES = 64 * 1024

    /** Minimum spacing of in-copy progress reports. */
    const val PROGRESS_INTERVAL_MS = 100L

    /** Bytes per read of an export; the cancel flag is checked between chunks. */
    const val EXPORT_CHUNK_BYTES = 1024 * 1024

    // --- Browse results: read field by field from Rust, nested here so the
    // module's `FrustPhotoLibraryHost$*` keep rule covers them.

    /** [albums]: parallel arrays, one entry per album. */
    class AlbumsResult(
        @JvmField val code: Int,
        @JvmField val ids: Array<String>,
        @JvmField val titles: Array<String?>,
        @JvmField val counts: LongArray,
    )

    /** [assets]: parallel arrays for one page; [total] counts the whole query. */
    class AssetsResult(
        @JvmField val code: Int,
        @JvmField val total: Long,
        @JvmField val ids: Array<String>,
        @JvmField val kinds: IntArray,
        @JvmField val widths: IntArray,
        @JvmField val heights: IntArray,
        @JvmField val dates: LongArray,
        @JvmField val durations: LongArray,
    )

    /** [thumbnail]: premultiplied RGBA8 with [rowBytes] per row. */
    class ThumbnailResult(
        @JvmField val code: Int,
        @JvmField val width: Int,
        @JvmField val height: Int,
        @JvmField val rowBytes: Int,
        @JvmField val pixels: ByteArray?,
    )

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
     * only through the `*InFlight*` helpers and [releaseCopied], which hold
     * this object's monitor.
     */
    private var inFlight: InFlight? = null

    /**
     * One unanswered pick. [activity] is the Activity its picker launched
     * from, or null once its copies started (it no longer depends on one).
     * [cancelled] is set by [cancel] and read by the copy loop.
     */
    private class InFlight(val requestId: Long, var activity: Activity?) {
        @Volatile
        var cancelled: Boolean = false
    }

    /** Set once [nativeOnPickProgress] is found missing, so it is not retried. */
    @Volatile
    private var progressUnavailable: Boolean = false

    /** Exports not yet answered, by request id. Separate from the pick slot. */
    private val exports = ConcurrentHashMap<Long, ExportFlight>()

    /**
     * One queued or running export. [cancelled] is checked before every
     * chunk; [started] is claimed by whichever of the copy and a cancel gets
     * there first, so a queued cancel and the copy answer exactly once.
     */
    private class ExportFlight {
        val cancelled = AtomicBoolean(false)
        val started = AtomicBoolean(false)
    }

    /** Set once [nativeOnExportProgress] is found missing, so it is not retried. */
    @Volatile
    private var exportProgressUnavailable: Boolean = false

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

    /** Runs exports, in order, off [copyExecutor] and the main thread. */
    private val exportExecutor: ExecutorService by lazy {
        Executors.newSingleThreadExecutor { runnable ->
            Thread(runnable, "frust-photo-library-export").apply { isDaemon = true }
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

    /**
     * Cancel the pick [requestId] (class doc, "Cancel and progress"). Any
     * thread; an id that does not hold the in-flight slot is ignored.
     */
    @JvmStatic
    fun cancel(requestId: Long) {
        if (cancelInFlight(requestId)) notifyError(requestId, CANCELLED)
    }

    /** The albums the app can see. Any thread but the main one (the query blocks). */
    @JvmStatic
    fun albums(): AlbumsResult = MediaStoreBrowser.albums(appContext)

    /** One page of assets, newest first. Any thread but the main one (the query blocks). */
    @JvmStatic
    fun assets(albumId: String?, filter: Int, offset: Long, limit: Int): AssetsResult =
        MediaStoreBrowser.assets(appContext, albumId, filter, offset, limit)

    /** One thumbnail. Any thread but the main one (the decode blocks). */
    @JvmStatic
    fun thumbnail(id: String, width: Int, height: Int, fill: Boolean): ThumbnailResult =
        MediaStoreBrowser.thumbnail(appContext, id, width, height, fill)

    /**
     * Re-show the Android 14 photo selection UI by requesting the SDK media
     * set (class doc, "Browsing"). Returns [PENDING] after posting the prompt,
     * answered through [nativeOnPickResult] with three empty arrays, or
     * [NOT_LIMITED] when the app does not hold a partial grant.
     */
    @JvmStatic
    fun manageLimitedSelection(requestId: Long): Int {
        val ctx = appContext ?: return NOT_INITIALIZED
        if (!MediaStoreBrowser.isLimited(ctx)) return NOT_LIMITED
        val activity = resumedActivity as? ComponentActivity ?: return NO_ACTIVITY
        if (!claimInFlight(requestId, activity)) {
            Log.i(TAG, "frust-photo-library: a pick is already in flight")
            return BUSY
        }
        mainHandler.post { launchSelectionPrompt(ctx, requestId, activity) }
        return PENDING
    }

    /**
     * Queue a copy of the original of [uri] (a `MediaStore` content URI, else
     * [NOT_FOUND]) to [destPath] on [exportExecutor]. Any thread. Returns
     * [PENDING]; the answer then arrives through [nativeOnExportDone]. Takes
     * no pick slot (class doc, "Exports and saves").
     */
    @JvmStatic
    fun exportOriginal(requestId: Long, uri: String, destPath: String): Int {
        val ctx = appContext ?: return NOT_INITIALIZED
        val source = Uri.parse(uri)
        if (!isExportableId(source)) return NOT_FOUND
        val flight = ExportFlight()
        exports[requestId] = flight
        exportExecutor.execute { runExport(ctx, requestId, source, File(destPath), flight) }
        return PENDING
    }

    /**
     * True for `content://media/<external volume>/images|video/media/<id>`, the
     * shape [MediaStoreBrowser] reports; every other URI is not exportable.
     */
    internal fun isExportableId(uri: Uri): Boolean {
        if (uri.scheme != ContentResolver.SCHEME_CONTENT || uri.authority != MediaStore.AUTHORITY) return false
        val segments = uri.pathSegments
        if (segments.size != 4) return false
        val volume = segments[0]
        return (volume == MediaStore.VOLUME_EXTERNAL || volume == MediaStore.VOLUME_EXTERNAL_PRIMARY) &&
            (segments[1] == "images" || segments[1] == "video") &&
            segments[2] == "media" &&
            segments[3].isNotEmpty() && segments[3].all { it in '0'..'9' }
    }

    /** Cancels export [requestId] (class doc). Any thread; an unknown id is ignored. */
    @JvmStatic
    fun cancelExport(requestId: Long) {
        val flight = exports[requestId] ?: return
        flight.cancelled.set(true)
        if (flight.started.compareAndSet(false, true)) {
            exports.remove(requestId)
            notifyExportDone(requestId, CANCELLED, null, null)
        }
    }

    /**
     * Save [sourcePath] into the library as a new row (class doc). Any
     * thread. Returns [PENDING]; the answer then arrives through
     * [nativeOnSaveDone]. Takes no pick slot.
     */
    @JvmStatic
    fun saveToLibrary(requestId: Long, sourcePath: String, isVideo: Boolean, displayName: String, album: String?): Int {
        val ctx = appContext ?: return NOT_INITIALIZED
        MediaStoreWriter.save(ctx, requestId, File(sourcePath), isVideo, displayName, album)
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

    /** Copy progress of the [pick] with the same [requestId]; [fraction] is 0.0..1.0. */
    @JvmStatic
    external fun nativeOnPickProgress(requestId: Long, done: Int, items: Int, fraction: Double)

    /** Answers the [exportOriginal] that returned [PENDING] with the same [requestId]. */
    @JvmStatic
    external fun nativeOnExportDone(requestId: Long, code: Int, mime: String?, name: String?)

    /** Bytes copied of the [exportOriginal] with the same [requestId]. */
    @JvmStatic
    external fun nativeOnExportProgress(requestId: Long, done: Long, total: Long)

    /** Answers the [saveToLibrary] that returned [PENDING] with the same [requestId]. */
    @JvmStatic
    external fun nativeOnSaveDone(requestId: Long, code: Int, uri: String?)

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

    private fun notifyProgress(requestId: Long, done: Int, items: Int, fraction: Double) {
        if (progressUnavailable) return
        try {
            nativeOnPickProgress(requestId, done, items, fraction)
        } catch (e: Throwable) {
            progressUnavailable = true
            Log.w(TAG, "frust-photo-library: nativeOnPickProgress unavailable", e)
        }
    }

    private fun notifyExportDone(requestId: Long, code: Int, mime: String?, name: String?) {
        try {
            nativeOnExportDone(requestId, code, mime, name)
        } catch (e: Throwable) {
            Log.w(TAG, "frust-photo-library: nativeOnExportDone unavailable", e)
        }
    }

    private fun notifyExportProgress(requestId: Long, done: Long, total: Long) {
        if (exportProgressUnavailable) return
        try {
            nativeOnExportProgress(requestId, done, total)
        } catch (e: Throwable) {
            exportProgressUnavailable = true
            Log.w(TAG, "frust-photo-library: nativeOnExportProgress unavailable", e)
        }
    }

    internal fun notifySaveDone(requestId: Long, code: Int, uri: String?) {
        try {
            nativeOnSaveDone(requestId, code, uri)
        } catch (e: Throwable) {
            Log.w(TAG, "frust-photo-library: nativeOnSaveDone unavailable", e)
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
        // Cancelled before the launch ran: open nothing.
        if (!holdsInFlight(requestId)) return
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

    /**
     * Register, launch and answer the selection prompt. Main thread. An empty
     * result (the request was cancelled) records no asked flag.
     */
    private fun launchSelectionPrompt(ctx: Context, requestId: Long, activity: ComponentActivity) {
        if (!holdsInFlight(requestId)) return
        if (activity.isFinishing || activity.isDestroyed) {
            if (releaseInFlight(requestId)) notifyError(requestId, NO_ACTIVITY)
            return
        }
        val key = LAUNCHER_KEY_PREFIX + requestId
        var launcher: ActivityResultLauncher<Array<String>>? = null
        try {
            launcher = register(activity, key, ActivityResultContracts.RequestMultiplePermissions()) { results ->
                if (releaseInFlight(requestId)) {
                    MediaStoreBrowser.recordAsked(ctx, results.keys)
                    notifyCancelled(requestId)
                }
            }
            launcher.launch(MediaStoreBrowser.mediaPermissions(Build.VERSION.SDK_INT).toTypedArray())
        } catch (e: Exception) {
            launcher?.unregister()
            Log.w(TAG, "frust-photo-library: launching the selection prompt failed: ${e.javaClass.name}")
            if (releaseInFlight(requestId)) notifyError(requestId, LAUNCH_FAILED)
        }
    }

    /** A one-shot launcher under [key] that unregisters itself before [onResult]. */
    private fun <I, O> register(
        activity: ComponentActivity,
        key: String,
        contract: ActivityResultContract<I, O>,
        onResult: (O) -> Unit,
    ): ActivityResultLauncher<I> {
        var launcher: ActivityResultLauncher<I>? = null
        val registered = activity.activityResultRegistry.register(key, contract) { result ->
            launcher?.unregister()
            onResult(result)
        }
        launcher = registered
        return registered
    }

    /**
     * The picker answered with [uris] (empty when cancelled). Truncates to
     * [limit], then copies on [copyExecutor] and answers Rust exactly once:
     * the copies, [COPY_FAILED], or [CANCELLED] after deleting the staging
     * files. A result for a request that no longer holds the slot (it was
     * cancelled while the picker was up) is discarded. Main thread.
     */
    private fun onPicked(ctx: Context, requestId: Long, uris: List<Uri>, limit: Int, destDir: File) {
        val chosen = if (limit > 0) uris.take(limit) else uris
        if (chosen.isEmpty()) {
            if (releaseInFlight(requestId)) notifyCancelled(requestId)
            return
        }
        // Detached from its Activity: a destroy from here on must not cancel it.
        val flight = detachInFlight(requestId) ?: return
        copyExecutor.execute {
            val staging = chosen.indices.map { index -> File(destDir, STAGING_PREFIX + index) }
            val copies = try {
                copyAll(ctx, requestId, chosen, staging, flight)
            } catch (e: Exception) {
                if (!flight.cancelled) {
                    Log.w(TAG, "frust-photo-library: copying a picked item failed: ${e.javaClass.name}")
                }
                null
            }
            when (releaseCopied(flight)) {
                null -> Unit
                true -> {
                    staging.forEach { it.delete() }
                    notifyError(requestId, CANCELLED)
                }
                false -> if (copies == null) {
                    staging.forEach { it.delete() }
                    notifyError(requestId, COPY_FAILED)
                } else {
                    notifyResult(
                        requestId,
                        copies.map { it.path }.toTypedArray(),
                        copies.map { it.name }.toTypedArray(),
                        copies.map { it.mime }.toTypedArray(),
                    )
                }
            }
        }
    }

    /** One copied item: the staging path plus what the provider reported. */
    private class Copy(val path: String, val name: String?, val mime: String?)

    /** What the provider reports about one item before it is copied. */
    private class Meta(val name: String?, val mime: String?, val size: Long?)

    /**
     * Copies [uris] to [staging] in order, reporting progress (class doc,
     * "Cancel and progress"). Null once [flight] is cancelled. Copy thread,
     * no monitor held.
     */
    private fun copyAll(
        ctx: Context,
        requestId: Long,
        uris: List<Uri>,
        staging: List<File>,
        flight: InFlight,
    ): List<Copy>? {
        val progress = Progress(requestId, uris.size)
        progress.send()
        val resolver = ctx.contentResolver
        val metas = uris.map { describe(ctx, it) }
        val sizes = metas.map { it.size }
        if (sizes.all { it != null && it > 0 }) progress.totalBytes = sizes.sumOf { it ?: 0L }
        val copies = ArrayList<Copy>(uris.size)
        for ((index, uri) in uris.withIndex()) {
            if (flight.cancelled) return null
            val input = resolver.openInputStream(uri) ?: throw IOException("no input stream")
            val dest = staging[index]
            val finished = input.use { source ->
                FileOutputStream(dest).use { sink -> copyChunks(source, sink, flight, progress) }
            }
            if (!finished) return null
            copies.add(Copy(dest.absolutePath, metas[index].name, metas[index].mime))
            progress.itemDone()
        }
        return copies
    }

    /**
     * Copies [source] to [sink] in [COPY_CHUNK_BYTES] chunks, checking the
     * cancel flag before each. False when [flight] was cancelled.
     */
    private fun copyChunks(source: InputStream, sink: OutputStream, flight: InFlight, progress: Progress): Boolean {
        val buffer = ByteArray(COPY_CHUNK_BYTES)
        while (true) {
            if (flight.cancelled) return false
            val read = source.read(buffer)
            if (read < 0) return true
            sink.write(buffer, 0, read)
            progress.copied(read.toLong())
        }
    }

    /** The display name, MIME type and size (`OpenableColumns.SIZE`, null when unknown) of [uri]. */
    private fun describe(ctx: Context, uri: Uri): Meta {
        val resolver = ctx.contentResolver
        var name: String? = null
        var size: Long? = null
        try {
            val columns = arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
            resolver.query(uri, columns, null, null, null)?.use { cursor ->
                if (cursor.moveToFirst()) {
                    if (!cursor.isNull(0)) name = cursor.getString(0)
                    if (!cursor.isNull(1)) size = cursor.getLong(1)
                }
            }
        } catch (e: Exception) {
            // Name and size stay unknown.
        }
        val mime = try {
            resolver.getType(uri)
        } catch (e: Exception) {
            null
        }
        return Meta(name, mime, size)
    }

    /**
     * One export, on [exportExecutor]: claims the flight, copies, and answers
     * Rust exactly once, deleting the partial file before a non-[OK] answer.
     */
    private fun runExport(ctx: Context, requestId: Long, uri: Uri, dest: File, flight: ExportFlight) {
        if (!flight.started.compareAndSet(false, true)) return
        var meta: Meta? = null
        val code = try {
            meta = describe(ctx, uri)
            copyOriginal(ctx, requestId, uri, dest, meta.size?.takeIf { it > 0 }, flight)
        } catch (e: SecurityException) {
            PERMISSION_DENIED
        } catch (e: Throwable) {
            Log.w(TAG, "frust-photo-library: exporting an original failed: ${e.javaClass.name}")
            EXPORT_FAILED
        }
        if (code != OK) dest.delete()
        exports.remove(requestId)
        val done = meta.takeIf { code == OK }
        notifyExportDone(requestId, code, done?.mime, done?.name)
    }

    /**
     * Copies [uri] to [dest] in [EXPORT_CHUNK_BYTES] chunks, checking the
     * cancel flag before each. Reports [nativeOnExportProgress] when [total]
     * is known. Export thread, no monitor held. [OK], [CANCELLED] or
     * [NOT_FOUND].
     */
    private fun copyOriginal(ctx: Context, requestId: Long, uri: Uri, dest: File, total: Long?, flight: ExportFlight): Int {
        if (flight.cancelled.get()) return CANCELLED
        val input = try {
            openOriginal(ctx, uri)
        } catch (e: FileNotFoundException) {
            return NOT_FOUND
        }
        input.use { source ->
            FileOutputStream(dest).use { sink ->
                if (total != null) notifyExportProgress(requestId, 0L, total)
                val buffer = ByteArray(EXPORT_CHUNK_BYTES)
                var done = 0L
                while (true) {
                    if (flight.cancelled.get()) return CANCELLED
                    val read = source.read(buffer)
                    if (read < 0) return OK
                    sink.write(buffer, 0, read)
                    done += read
                    if (total != null) notifyExportProgress(requestId, done, total)
                }
            }
        }
    }

    /**
     * Opens [uri], asking for the unredacted bytes when `ACCESS_MEDIA_LOCATION`
     * is held (class doc). A refused original falls back to the plain URI.
     */
    private fun openOriginal(ctx: Context, uri: Uri): InputStream {
        val resolver = ctx.contentResolver
        if (ctx.checkSelfPermission(Manifest.permission.ACCESS_MEDIA_LOCATION) == PackageManager.PERMISSION_GRANTED) {
            try {
                resolver.openInputStream(MediaStore.setRequireOriginal(uri))?.let { return it }
            } catch (e: UnsupportedOperationException) {
                Log.i(TAG, "frust-photo-library: original unavailable, using the plain uri: ${e.javaClass.name}")
            } catch (e: SecurityException) {
                Log.i(TAG, "frust-photo-library: original unavailable, using the plain uri: ${e.javaClass.name}")
            }
        }
        return resolver.openInputStream(uri) ?: throw IOException("no input stream")
    }

    /**
     * Copy progress of one pick, owned by the copy thread. [totalBytes] is 0
     * unless every item's size is known; the fraction then counts items.
     */
    private class Progress(val requestId: Long, val items: Int) {
        var totalBytes: Long = 0L
        private var done = 0
        private var bytes = 0L
        private var lastSent = 0L

        fun copied(n: Long) {
            bytes += n
            if (SystemClock.elapsedRealtime() - lastSent >= PROGRESS_INTERVAL_MS) send()
        }

        fun itemDone() {
            done += 1
            send()
        }

        fun send() {
            lastSent = SystemClock.elapsedRealtime()
            val fraction = when {
                done >= items -> 1.0
                totalBytes > 0 -> (bytes.toDouble() / totalBytes).coerceIn(0.0, 1.0)
                else -> done.toDouble() / items
            }
            notifyProgress(requestId, done, items, fraction)
        }
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

    /** Whether [requestId] still holds the slot. */
    @Synchronized
    private fun holdsInFlight(requestId: Long): Boolean = inFlight?.requestId == requestId

    /** Detach [requestId]'s slot from its Activity; null when it no longer holds the slot. */
    @Synchronized
    private fun detachInFlight(requestId: Long): InFlight? {
        val current = inFlight ?: return null
        if (current.requestId != requestId) return null
        current.activity = null
        return current
    }

    /**
     * Mark [requestId] cancelled. While its picker is still up the slot is
     * freed and this returns true (the caller answers); once its copies run,
     * the copy thread answers.
     */
    @Synchronized
    private fun cancelInFlight(requestId: Long): Boolean {
        val current = inFlight ?: return false
        if (current.requestId != requestId) return false
        current.cancelled = true
        if (current.activity == null) return false
        inFlight = null
        return true
    }

    /**
     * Free [flight]'s slot after its copies: null when it no longer holds the
     * slot, otherwise whether it was cancelled (read under the same monitor
     * [cancelInFlight] writes it under, so exactly one answer follows).
     */
    @Synchronized
    private fun releaseCopied(flight: InFlight): Boolean? {
        if (inFlight !== flight) return null
        inFlight = null
        return flight.cancelled
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
