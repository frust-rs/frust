package dev.frust.filedialogs

import android.app.Activity
import android.app.Application
import android.content.Context
import android.content.Intent
import android.database.Cursor
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import android.util.Log
import android.webkit.MimeTypeMap
import androidx.activity.ComponentActivity
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContract
import androidx.activity.result.contract.ActivityResultContracts
import java.io.File
import java.io.FileInputStream
import java.io.FileNotFoundException
import java.io.FileOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/**
 * The `frust-file-dialogs` plugin's Android host: the Kotlin half of the frozen
 * Rust/Kotlin contract also implemented by `plugins/file-dialogs/src/android.rs`.
 *
 * ## The contract (see `plugins/file-dialogs/src/android.rs`)
 *
 * This object's fully-qualified name (`dev.frust.filedialogs.FrustFileDialogsHost`)
 * and its `@JvmStatic` entry points are looked up by string from Rust through
 * the application classloader, and the mangled JNI symbols of [nativeOnPaths],
 * [nativeOnTree], [nativeOnListing], [nativeOnCode] and [nativeOnProgress] are
 * exported by that same module. **The class may never move and no signature
 * may change without a coordinated change on both sides.** The integer codes
 * below are pinned against their Rust twins by
 * `plugins/file-dialogs/tests/kotlin_contract.rs`.
 *
 * | Direction | Member | Returns |
 * |---|---|---|
 * | Rust to Kotlin | `openDocuments(requestId: Long, mimes: Array<String>, multiple: Boolean, destDir: String): Int` | [PENDING] after posting the launch, [NO_ACTIVITY], [BUSY] or [NOT_INITIALIZED] |
 * | Rust to Kotlin | `exportFile(requestId: Long, sourcePath: String, suggestedName: String, mime: String): Int` | as above |
 * | Rust to Kotlin | `pickTree(requestId: Long): Int` | as above |
 * | Rust to Kotlin | `listTree(requestId: Long, uri: String, recursive: Boolean, maxDepth: Int, maxEntries: Int, extensions: Array<String>, includeHidden: Boolean): Int` | [PENDING] after queuing the walk, or [NOT_INITIALIZED] |
 * | Rust to Kotlin | `copyDocument(requestId: Long, uri: String, destPath: String, maxBytes: Long): Int` | [PENDING] after queuing the copy, or [NOT_INITIALIZED]; a negative `maxBytes` means no limit |
 * | Rust to Kotlin | `cancel(requestId: Long)` | nothing; an unknown or already-answered id is ignored |
 * | Rust to Kotlin | `isTreeGranted(uri: String): Boolean` | whether a persisted read grant covers `uri` |
 * | Rust to Kotlin | `releaseTree(uri: String)` | nothing; drops the persisted grant |
 * | Rust to Kotlin | `mimesForExtensions(extensions: Array<String>): Array<String>` | the MIME types to filter by |
 * | Rust to Kotlin | `mimeForName(name: String): String` | the MIME type for a new document of that name |
 * | Rust to Kotlin | `cacheRoot(): String?` | the app cache directory |
 * | Kotlin to Rust | `nativeOnPaths(requestId: Long, paths: Array<String>)` | the copies; an empty array means dismissed |
 * | Kotlin to Rust | `nativeOnTree(requestId: Long, uri: String?)` | the persisted tree URI; null means dismissed |
 * | Kotlin to Rust | `nativeOnListing(requestId: Long, names: Array<String>, relativePaths: Array<String>, sizes: LongArray, modified: LongArray, uris: Array<String>)` | the listed files, parallel arrays with [NO_VALUE] for unknown |
 * | Kotlin to Rust | `nativeOnCode(requestId: Long, code: Int)` | [SAVED], [DISMISSED], [CANCELLED], [ERROR], [ACCESS_LOST], [TOO_LARGE], [LAUNCH_FAILED], [NO_ACTIVITY] or [GRANT_FAILED] |
 * | Kotlin to Rust | `nativeOnProgress(requestId: Long, done: Int, items: Int, fraction: Double)` | copy progress, never an answer |
 *
 * Every request that returned [PENDING] gets exactly one answer.
 *
 * ## One dialog in flight
 *
 * [openDocuments], [exportFile] and [pickTree] share one in-flight slot. While
 * one holds it (its system dialog is up or its copy is running) a second
 * returns [BUSY]. The launch is posted to the main thread; the launcher is
 * registered on the resumed [ComponentActivity]'s `activityResultRegistry`
 * under the key `frust.filedialogs.<requestId>` and unregistered from its own
 * callback, so the consuming app never forwards `onActivityResult`. The slot is
 * released by the request's own answer, by a launch that fails, or when the
 * Activity it launched from is destroyed while the dialog is up (its callback
 * can no longer fire), which answers as a dismissal so Rust never waits forever.
 *
 * [listTree] and [copyDocument] open no dialog and take no slot: each runs on
 * [workExecutor] under its own entry in [jobs].
 *
 * ## Cancel and progress
 *
 * [cancel] sets the request's flag. A system dialog cannot be closed from the
 * app, so while one is up [CANCELLED] is answered at once, the slot stays held
 * until the dialog's result arrives, and that result is ignored (an export's
 * just-created empty document is removed; a tree gets no persisted grant).
 * Copies, exports and listings run on [workExecutor], check the flag between
 * chunks or entries, remove their partial output and answer [CANCELLED]. A user
 * backing out of a dialog is not a cancel: it answers an empty [nativeOnPaths],
 * a null [nativeOnTree] or [DISMISSED]. Progress is reported from the copy
 * thread when copying starts, at most every [PROGRESS_INTERVAL_MS] and after
 * each item, never while holding a monitor; the fraction counts bytes when
 * every item's size is known, items otherwise.
 *
 * ## Copies and grants
 *
 * [openDocuments] copies each chosen URI through `openInputStream` into the
 * Rust-chosen `destDir`, named after its display name with path separators
 * replaced and duplicates numbered ([safeCopyName]). [exportFile] writes
 * through `CreateDocument` and removes the created document on a cancel or
 * error once it has been opened for writing. [pickTree] takes
 * `takePersistableUriPermission(FLAG_GRANT_READ_URI_PERMISSION)`.
 * [copyDocument] writes a sibling temporary file and renames it over `destPath`
 * only once complete. No log line carries a URI, name or path.
 */
object FrustFileDialogsHost {
    private const val TAG = "frust"

    // --- Return and `nativeOnCode` codes: mirror `src/android.rs`, pinned by
    // `tests/kotlin_contract.rs`.

    /** The call was queued; the answer arrives through a native callback. */
    const val PENDING = 0
    /** No resumed Activity to launch from, or it was finishing by launch time. */
    const val NO_ACTIVITY = 1
    /** Another dialog is in flight. */
    const val BUSY = 2
    /** No application context was installed (the init provider never ran). */
    const val NOT_INITIALIZED = 3
    /** The system dialog could not be launched. */
    const val LAUNCH_FAILED = 4
    /** A file operation failed. */
    const val ERROR = 5
    /** The request was cancelled through [cancel]. */
    const val CANCELLED = 6
    /** An export or document copy finished. */
    const val SAVED = 7
    /** The user dismissed the export dialog. */
    const val DISMISSED = 8
    /** The folder or its persisted grant is gone. */
    const val ACCESS_LOST = 9
    /** A document exceeded the copy's byte limit. */
    const val TOO_LARGE = 10
    /** The system refused to persist the folder grant (for example the per-app cap). */
    const val GRANT_FAILED = 11

    /** [nativeOnListing]'s marker for an unknown size or time. */
    const val NO_VALUE = -1L

    /** Activity-result registry key prefix; the request id follows. */
    const val LAUNCHER_KEY_PREFIX = "frust.filedialogs."

    /** Bytes per read of every copy; the cancel flag is checked between chunks. */
    const val COPY_CHUNK_BYTES = 64 * 1024

    /** Minimum spacing of in-copy progress reports. */
    const val PROGRESS_INTERVAL_MS = 100L

    /** [copyDocument]'s temporary sibling; the request id follows. */
    const val PARTIAL_PREFIX = ".frust-partial-"

    /** The filter for "any file". */
    private const val ANY_MIME = "*/*"

    /** The type of a new document whose extension is unknown. */
    private const val BINARY_MIME = "application/octet-stream"

    /** Longest copy name, leaving room below the 255-byte filesystem limit. */
    private const val MAX_NAME_BYTES = 200

    /** Threads in [workExecutor]. */
    private const val WORK_THREADS = 4

    /** A listing visits at most this multiple of `maxEntries` folders and children. */
    private const val WALK_VISIT_MULTIPLE = 16

    /** Which dialog holds the slot, so a destroyed Activity can answer it. */
    private enum class Flow { OPEN, EXPORT, TREE }

    /** The application [Context], installed once by [FrustFileDialogsInitProvider]. */
    @Volatile
    private var appContext: Context? = null

    /**
     * The currently resumed Activity: set in `onActivityResumed`, cleared when
     * that same instance pauses or is destroyed. Dialogs launch from it.
     */
    @Volatile
    private var resumedActivity: Activity? = null

    /** The request holding the single in-flight slot, or null. Guarded by this object's monitor. */
    private var inFlight: InFlight? = null

    /**
     * One unanswered dialog. [activity] is the Activity its dialog launched
     * from, or null once its work started. [cancelled] is read by the copy
     * loop; [answered] records that [cancel] already answered Rust.
     */
    private class InFlight(val requestId: Long, val flow: Flow, var activity: Activity?) {
        @Volatile
        var cancelled: Boolean = false
        var answered: Boolean = false
    }

    /** One [listTree] or [copyDocument] request; [cancelled] is read between chunks or entries. */
    private class Job {
        @Volatile
        var cancelled: Boolean = false
    }

    /** The folder grant the latest tree answer newly took, for [releaseNewTree]. */
    private var newTreeGrant: TreeGrant? = null

    /** Running background requests by id, separate from the dialog slot. */
    private val jobs = ConcurrentHashMap<Long, Job>()

    /** Set once [nativeOnProgress] is found missing, so it is not retried. */
    @Volatile
    private var progressUnavailable: Boolean = false

    /** Guards a single [Application.registerActivityLifecycleCallbacks] call. */
    @Volatile
    private var lifecycleCallbacksRegistered: Boolean = false

    /** Runs every launch on the main thread. */
    private val mainHandler = Handler(Looper.getMainLooper())

    /** Runs every copy and listing off the main thread: [WORK_THREADS] threads, the rest queued. */
    private val workExecutor: ExecutorService by lazy {
        Executors.newFixedThreadPool(WORK_THREADS) { runnable ->
            Thread(runnable, "frust-file-dialogs-work").apply { isDaemon = true }
        }
    }

    /**
     * Install the application [Context]: called once at process start by
     * [FrustFileDialogsInitProvider]. Idempotent and first-writer-wins.
     */
    internal fun installApplicationContext(context: Context) {
        if (appContext == null) appContext = context
        ensureLifecycleCallbacks(context)
    }

    /**
     * Register the process-wide [Application.ActivityLifecycleCallbacks] once.
     * The `as?` keeps a non-`Application` context from throwing at process start.
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
     * Open the system document picker for [mimes] and copy the chosen files
     * into [destDir]. Any thread. Answered through [nativeOnPaths] or [nativeOnCode].
     */
    @JvmStatic
    fun openDocuments(requestId: Long, mimes: Array<String>, multiple: Boolean, destDir: String): Int {
        val ctx = appContext ?: return NOT_INITIALIZED
        val activity = resumedActivity as? ComponentActivity ?: return NO_ACTIVITY
        if (!claimInFlight(requestId, Flow.OPEN, activity)) return busy()
        val dest = File(destDir)
        mainHandler.post {
            if (multiple) {
                launchDialog(requestId, activity, ActivityResultContracts.OpenMultipleDocuments(), mimes) { uris ->
                    onOpened(ctx, requestId, uris, dest)
                }
            } else {
                launchDialog(requestId, activity, ActivityResultContracts.OpenDocument(), mimes) { uri ->
                    onOpened(ctx, requestId, listOfNotNull(uri), dest)
                }
            }
        }
        return PENDING
    }

    /**
     * Let the user create a document named [suggestedName] of type [mime] and
     * copy [sourcePath] into it. Any thread. Answered through [nativeOnCode].
     */
    @JvmStatic
    fun exportFile(requestId: Long, sourcePath: String, suggestedName: String, mime: String): Int {
        val ctx = appContext ?: return NOT_INITIALIZED
        val activity = resumedActivity as? ComponentActivity ?: return NO_ACTIVITY
        if (!claimInFlight(requestId, Flow.EXPORT, activity)) return busy()
        val source = File(sourcePath)
        mainHandler.post {
            launchDialog(requestId, activity, ActivityResultContracts.CreateDocument(mime), suggestedName) { uri ->
                onCreated(ctx, requestId, uri, source)
            }
        }
        return PENDING
    }

    /**
     * Let the user pick a folder and persist read access to it. Any thread.
     * Answered through [nativeOnTree] or [nativeOnCode].
     */
    @JvmStatic
    fun pickTree(requestId: Long): Int {
        val ctx = appContext ?: return NOT_INITIALIZED
        val activity = resumedActivity as? ComponentActivity ?: return NO_ACTIVITY
        if (!claimInFlight(requestId, Flow.TREE, activity)) return busy()
        mainHandler.post {
            launchDialog(requestId, activity, ActivityResultContracts.OpenDocumentTree(), null) { uri ->
                onTree(ctx, requestId, uri)
            }
        }
        return PENDING
    }

    /** Whether a persisted read grant still covers [uri]. */
    @JvmStatic
    fun isTreeGranted(uri: String): Boolean {
        val ctx = appContext ?: return false
        val tree = Uri.parse(uri)
        return ctx.contentResolver.persistedUriPermissions.any { it.uri == tree && it.isReadPermission }
    }

    /** List the files under [uri] on [workExecutor]. Answered through [nativeOnListing] or [nativeOnCode]. */
    @JvmStatic
    fun listTree(
        requestId: Long,
        uri: String,
        recursive: Boolean,
        maxDepth: Int,
        maxEntries: Int,
        extensions: Array<String>,
        includeHidden: Boolean,
    ): Int {
        val ctx = appContext ?: return NOT_INITIALIZED
        val job = Job()
        jobs[requestId] = job
        val query = TreeQuery(Uri.parse(uri), recursive, maxDepth, maxEntries, extensions.toSet(), includeHidden)
        workExecutor.execute { runListing(ctx, requestId, query, job) }
        return PENDING
    }

    /**
     * Copy the document at [uri] to [destPath] on [workExecutor], failing with
     * [TOO_LARGE] past [maxBytes] when it is not negative. Answered through
     * [nativeOnCode].
     */
    @JvmStatic
    fun copyDocument(requestId: Long, uri: String, destPath: String, maxBytes: Long): Int {
        val ctx = appContext ?: return NOT_INITIALIZED
        val job = Job()
        jobs[requestId] = job
        val document = Uri.parse(uri)
        val dest = File(destPath)
        workExecutor.execute { runCopy(ctx, requestId, document, dest, maxBytes, job) }
        return PENDING
    }

    /** Drop the persisted grant on [uri]. Any thread. */
    @JvmStatic
    fun releaseTree(uri: String) {
        val ctx = appContext ?: return
        try {
            ctx.contentResolver.releasePersistableUriPermission(Uri.parse(uri), Intent.FLAG_GRANT_READ_URI_PERMISSION)
        } catch (e: Exception) {
            Log.w(TAG, "frust-file-dialogs: releasing a folder grant failed: ${e.javaClass.name}")
        }
    }

    /**
     * Drop the persisted grant on [uri] only if the tree answer to [requestId]
     * newly took it: a folder that was already granted keeps the permission an
     * older grant holds. Any thread.
     */
    @JvmStatic
    fun releaseNewTree(requestId: Long, uri: String) {
        if (takeNewTreeGrant(requestId, uri)) releaseTree(uri)
    }

    @Synchronized
    private fun takeNewTreeGrant(requestId: Long, uri: String): Boolean {
        val grant = newTreeGrant ?: return false
        if (grant.requestId != requestId || grant.uri != uri) return false
        newTreeGrant = null
        return true
    }

    @Synchronized
    private fun recordNewTreeGrant(grant: TreeGrant?) {
        newTreeGrant = grant
    }

    /**
     * Cancel [requestId] (class doc, "Cancel and progress"). Any thread; an id
     * that holds neither the slot nor a running job is ignored.
     */
    @JvmStatic
    fun cancel(requestId: Long) {
        if (cancelInFlight(requestId)) notifyCode(requestId, CANCELLED)
        jobs[requestId]?.cancelled = true
    }

    /** The app cache directory, or null before initialisation. */
    @JvmStatic
    fun cacheRoot(): String? = appContext?.cacheDir?.absolutePath

    /**
     * The MIME types to filter by for [extensions], without duplicates. An
     * unknown extension widens the filter to any file rather than hiding it,
     * as does an empty list.
     */
    @JvmStatic
    fun mimesForExtensions(extensions: Array<String>): Array<String> {
        val map = MimeTypeMap.getSingleton()
        val mimes = LinkedHashSet<String>()
        for (extension in extensions) {
            mimes += map.getMimeTypeFromExtension(extension) ?: return arrayOf(ANY_MIME)
        }
        return if (mimes.isEmpty()) arrayOf(ANY_MIME) else mimes.toTypedArray()
    }

    /** The MIME type for a new document called [name]. */
    @JvmStatic
    fun mimeForName(name: String): String {
        val extension = name.substringAfterLast('.', "").lowercase()
        return MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension) ?: BINARY_MIME
    }

    // --- Kotlin -> Rust -----------------------------------------------------

    /**
     * Answers [openDocuments] with the copies; an empty array means dismissed.
     * Implemented in `plugins/file-dialogs/src/android.rs`. The native library
     * is already loaded by the embedding's `FrustActivity`, so no
     * `System.loadLibrary` happens here.
     */
    @JvmStatic
    external fun nativeOnPaths(requestId: Long, paths: Array<String>)

    /** Answers [pickTree] with the persisted tree URI; null means dismissed. */
    @JvmStatic
    external fun nativeOnTree(requestId: Long, uri: String?)

    /** Answers [listTree] with parallel arrays, one element per file. */
    @JvmStatic
    external fun nativeOnListing(
        requestId: Long,
        names: Array<String>,
        relativePaths: Array<String>,
        sizes: LongArray,
        modified: LongArray,
        uris: Array<String>,
    )

    /** Answers a request with every other outcome. */
    @JvmStatic
    external fun nativeOnCode(requestId: Long, code: Int)

    /** Copy progress of the request with the same [requestId]; [fraction] is 0.0..1.0. */
    @JvmStatic
    external fun nativeOnProgress(requestId: Long, done: Int, items: Int, fraction: Double)

    // Every native call is guarded: a missing symbol (an app that depends on
    // this Gradle module without the `frust-file-dialogs` Rust crate) must
    // degrade to a log line, never an UnsatisfiedLinkError thrown into an
    // Activity result callback or the work thread.
    private fun notifyPaths(requestId: Long, paths: Array<String>) {
        try {
            nativeOnPaths(requestId, paths)
        } catch (e: Throwable) {
            Log.w(TAG, "frust-file-dialogs: nativeOnPaths unavailable", e)
        }
    }

    private fun notifyTree(requestId: Long, uri: String?) {
        try {
            nativeOnTree(requestId, uri)
        } catch (e: Throwable) {
            Log.w(TAG, "frust-file-dialogs: nativeOnTree unavailable", e)
        }
    }

    private fun notifyListing(requestId: Long, listing: Listing) {
        try {
            nativeOnListing(
                requestId,
                listing.names.toTypedArray(),
                listing.relativePaths.toTypedArray(),
                listing.sizes.toLongArray(),
                listing.modified.toLongArray(),
                listing.uris.toTypedArray(),
            )
        } catch (e: Throwable) {
            Log.w(TAG, "frust-file-dialogs: nativeOnListing unavailable", e)
        }
    }

    private fun notifyCode(requestId: Long, code: Int) {
        try {
            nativeOnCode(requestId, code)
        } catch (e: Throwable) {
            Log.w(TAG, "frust-file-dialogs: nativeOnCode unavailable", e)
        }
    }

    private fun notifyProgress(requestId: Long, done: Int, items: Int, fraction: Double) {
        if (progressUnavailable) return
        try {
            nativeOnProgress(requestId, done, items, fraction)
        } catch (e: Throwable) {
            progressUnavailable = true
            Log.w(TAG, "frust-file-dialogs: nativeOnProgress unavailable", e)
        }
    }

    // --- Dialogs ------------------------------------------------------------

    private fun busy(): Int {
        Log.i(TAG, "frust-file-dialogs: a dialog is already in flight")
        return BUSY
    }

    /** Register and launch one system dialog. Main thread. */
    private fun <I, O> launchDialog(
        requestId: Long,
        activity: ComponentActivity,
        contract: ActivityResultContract<I, O>,
        input: I,
        onResult: (O) -> Unit,
    ) {
        val flight = holderOf(requestId) ?: return
        // Cancelled before the launch ran: open nothing. `cancel` already answered.
        if (flight.cancelled) {
            releaseFlight(flight)
            return
        }
        if (activity.isFinishing || activity.isDestroyed) {
            answer(flight) { notifyCode(requestId, NO_ACTIVITY) }
            return
        }
        val key = LAUNCHER_KEY_PREFIX + requestId
        var launcher: ActivityResultLauncher<I>? = null
        try {
            launcher = register(activity, key, contract, onResult)
            launcher.launch(input)
        } catch (e: Exception) {
            launcher?.unregister()
            Log.w(TAG, "frust-file-dialogs: launching the dialog failed: ${e.javaClass.name}")
            answer(flight) { notifyCode(requestId, LAUNCH_FAILED) }
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

    /** The picker answered with [uris] (empty when dismissed). Main thread. */
    private fun onOpened(ctx: Context, requestId: Long, uris: List<Uri>, destDir: File) {
        val flight = claimResult(requestId) ?: return
        if (uris.isEmpty()) {
            answer(flight) { notifyPaths(requestId, emptyArray()) }
            return
        }
        workExecutor.execute {
            val written = ArrayList<File>()
            val paths = try {
                copyAll(ctx, requestId, uris, destDir, flight, written)
            } catch (e: Exception) {
                Log.w(TAG, "frust-file-dialogs: copying a document failed: ${e.javaClass.name}")
                null
            }
            answer(flight, cleanup = { written.forEach { it.delete() } }) {
                if (paths == null) {
                    written.forEach { it.delete() }
                    notifyCode(requestId, ERROR)
                } else {
                    notifyPaths(requestId, paths.toTypedArray())
                }
            }
        }
    }

    /**
     * Copies [uris] into [destDir], recording each destination in [written]
     * before it is opened. Null once [flight] is cancelled. Work thread, no
     * monitor held.
     */
    private fun copyAll(
        ctx: Context,
        requestId: Long,
        uris: List<Uri>,
        destDir: File,
        flight: InFlight,
        written: MutableList<File>,
    ): List<String>? {
        val progress = Progress(requestId, uris.size)
        progress.send()
        val metas = uris.map { describe(ctx, it) }
        if (metas.all { it.size != null && it.size > 0 }) progress.totalBytes = metas.sumOf { it.size ?: 0L }
        val used = HashSet<String>()
        val paths = ArrayList<String>(uris.size)
        for ((index, uri) in uris.withIndex()) {
            if (flight.cancelled) return null
            val dest = File(destDir, safeCopyName(metas[index].name, index, used))
            written.add(dest)
            val input = ctx.contentResolver.openInputStream(uri) ?: throw IOException("no input stream")
            val code = input.use { source ->
                FileOutputStream(dest).use { sink ->
                    streamChunks(source, sink, { flight.cancelled }, -1L, progress::copied)
                }
            }
            if (code != SAVED) return null
            paths.add(dest.absolutePath)
            progress.itemDone()
        }
        return paths
    }

    /** The create-document dialog answered with [uri] (null when dismissed). Main thread. */
    private fun onCreated(ctx: Context, requestId: Long, uri: Uri?, source: File) {
        val flight = claimResult(requestId)
        if (flight == null) {
            // Cancelled while the dialog was up: drop the document it just created.
            if (uri != null) workExecutor.execute { removeCreated(ctx, uri, false) }
            return
        }
        if (uri == null) {
            answer(flight) { notifyCode(requestId, DISMISSED) }
            return
        }
        workExecutor.execute { runExport(ctx, requestId, uri, source, flight) }
    }

    /** One export: copies [source] into the created document and answers once. Work thread. */
    private fun runExport(ctx: Context, requestId: Long, uri: Uri, source: File, flight: InFlight) {
        var opened = false
        val code = try {
            if (flight.cancelled) {
                CANCELLED
            } else {
                val progress = Progress(requestId, 1)
                progress.totalBytes = source.length()
                progress.send()
                val sink = openForWrite(ctx, uri)
                opened = true
                val copied = sink.use { out ->
                    FileInputStream(source).use { input ->
                        streamChunks(input, out, { flight.cancelled }, -1L, progress::copied)
                    }
                }
                if (copied == SAVED) progress.itemDone()
                copied
            }
        } catch (e: Exception) {
            Log.w(TAG, "frust-file-dialogs: exporting a file failed: ${e.javaClass.name}")
            ERROR
        }
        if (code != SAVED) removeCreated(ctx, uri, opened)
        answer(flight, cleanup = { if (code == SAVED) removeCreated(ctx, uri, true) }) {
            notifyCode(requestId, code)
        }
    }

    /**
     * Opens [uri] truncating: a plain `"w"` leaves old bytes behind on some
     * providers. A provider that refuses `"wt"` gets `"w"`.
     */
    private fun openForWrite(ctx: Context, uri: Uri): OutputStream {
        val resolver = ctx.contentResolver
        val truncating = try {
            resolver.openOutputStream(uri, "wt")
        } catch (e: FileNotFoundException) {
            null
        } catch (e: IllegalArgumentException) {
            null
        }
        return truncating ?: resolver.openOutputStream(uri) ?: throw IOException("no output stream")
    }

    /**
     * Removes a document this host created. One never opened for writing may be
     * an existing file the user chose to replace, so it goes only when empty.
     */
    private fun removeCreated(ctx: Context, uri: Uri, opened: Boolean) {
        try {
            if (opened || describe(ctx, uri).size == 0L) {
                DocumentsContract.deleteDocument(ctx.contentResolver, uri)
            }
        } catch (e: Exception) {
            Log.w(TAG, "frust-file-dialogs: removing a partial document failed: ${e.javaClass.name}")
        }
    }

    /** The folder dialog answered with [uri] (null when dismissed). Main thread. */
    private fun onTree(ctx: Context, requestId: Long, uri: Uri?) {
        val flight = claimResult(requestId) ?: return
        if (uri == null) {
            answer(flight) { notifyTree(requestId, null) }
            return
        }
        val text = uri.toString()
        val alreadyGranted = isTreeGranted(text)
        val persisted = try {
            ctx.contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
            true
        } catch (e: Exception) {
            // Typically the per-app cap on persisted grants; the reason is the class only.
            Log.w(TAG, "frust-file-dialogs: persisting the folder grant failed: ${e.javaClass.name}")
            false
        }
        val newlyTaken = persisted && !alreadyGranted
        answer(flight, cleanup = { if (newlyTaken) releaseTree(uri.toString()) }) {
            if (persisted) {
                recordNewTreeGrant(if (newlyTaken) TreeGrant(requestId, text) else null)
                notifyTree(requestId, text)
            } else {
                notifyCode(requestId, GRANT_FAILED)
            }
        }
    }

    // --- Copies -------------------------------------------------------------

    /**
     * Copies [source] to [sink] in [COPY_CHUNK_BYTES] chunks, checking
     * [isCancelled] before each. [SAVED], [CANCELLED], or [TOO_LARGE] past a
     * non-negative [maxBytes].
     */
    private fun streamChunks(
        source: InputStream,
        sink: OutputStream,
        isCancelled: () -> Boolean,
        maxBytes: Long,
        onBytes: (Long) -> Unit,
    ): Int {
        val buffer = ByteArray(COPY_CHUNK_BYTES)
        var total = 0L
        while (true) {
            if (isCancelled()) return CANCELLED
            val read = source.read(buffer)
            if (read < 0) return SAVED
            total += read
            if (maxBytes >= 0 && total > maxBytes) return TOO_LARGE
            sink.write(buffer, 0, read)
            onBytes(read.toLong())
        }
    }

    /** One [copyDocument]: writes a temporary sibling and renames it into place. Work thread. */
    private fun runCopy(ctx: Context, requestId: Long, uri: Uri, dest: File, maxBytes: Long, job: Job) {
        val partial = File(dest.parentFile, PARTIAL_PREFIX + requestId)
        var code = try {
            val progress = Progress(requestId, 1)
            progress.totalBytes = describe(ctx, uri).size ?: 0L
            progress.send()
            val input = ctx.contentResolver.openInputStream(uri) ?: throw IOException("no input stream")
            val copied = input.use { source ->
                FileOutputStream(partial).use { sink ->
                    streamChunks(source, sink, { job.cancelled }, maxBytes, progress::copied)
                }
            }
            if (copied == SAVED) progress.itemDone()
            copied
        } catch (e: SecurityException) {
            ACCESS_LOST
        } catch (e: Exception) {
            Log.w(TAG, "frust-file-dialogs: copying a document failed: ${e.javaClass.name}")
            ERROR
        }
        if (code == SAVED && !partial.renameTo(dest)) code = ERROR
        if (code != SAVED) partial.delete()
        jobs.remove(requestId)
        notifyCode(requestId, code)
    }

    /** What the provider reports about one document before it is copied. */
    private class Meta(val name: String?, val size: Long?)

    /** The display name and size of [uri], each null when unknown. */
    private fun describe(ctx: Context, uri: Uri): Meta {
        var name: String? = null
        var size: Long? = null
        try {
            val columns = arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
            ctx.contentResolver.query(uri, columns, null, null, null)?.use { cursor ->
                if (cursor.moveToFirst()) {
                    if (!cursor.isNull(0)) name = cursor.getString(0)
                    if (!cursor.isNull(1)) size = cursor.getLong(1)
                }
            }
        } catch (e: Exception) {
            // Name and size stay unknown.
        }
        return Meta(name, size)
    }

    /**
     * A file name for the copy of item [index]: [display] with path separators
     * and NUL replaced, shortened to [MAX_NAME_BYTES], unique within [used]
     * (case-folded) by a numeric suffix. Provider text cannot leave the
     * destination directory.
     */
    internal fun safeCopyName(display: String?, index: Int, used: MutableSet<String>): String {
        val cleaned = (display ?: "")
            .map { if (it == '/' || it == '\\' || it == '\u0000') '_' else it }
            .joinToString("")
            .trim()
        val base = if (cleaned.isEmpty() || cleaned == "." || cleaned == "..") "document-$index" else fitName(cleaned)
        var name = base
        var attempt = 0
        while (!used.add(name.lowercase())) {
            attempt += 1
            val dot = base.lastIndexOf('.')
            name = if (dot > 0) "${base.substring(0, dot)}-$attempt${base.substring(dot)}" else "$base-$attempt"
        }
        return name
    }

    private fun fitName(name: String): String {
        if (name.toByteArray().size <= MAX_NAME_BYTES) return name
        val dot = name.lastIndexOf('.').takeIf { it > 0 && name.length - it <= 16 } ?: name.length
        var stem = name.substring(0, dot)
        val extension = name.substring(dot)
        while ((stem + extension).toByteArray().size > MAX_NAME_BYTES && stem.isNotEmpty()) {
            stem = stem.dropLast(if (stem.last().isLowSurrogate()) 2 else 1)
        }
        return stem + extension
    }

    /**
     * Copy progress of one request, owned by its work thread. [totalBytes] is
     * 0 unless every item's size is known; the fraction then counts items.
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

    // --- Listing ------------------------------------------------------------

    /** One [listTree] request's parameters. */
    private class TreeQuery(
        val tree: Uri,
        val recursive: Boolean,
        val maxDepth: Int,
        val maxEntries: Int,
        val extensions: Set<String>,
        val includeHidden: Boolean,
    )

    /** The listed files, as the parallel arrays [nativeOnListing] takes. */
    private class Listing {
        val names = ArrayList<String>()
        val relativePaths = ArrayList<String>()
        val sizes = ArrayList<Long>()
        val modified = ArrayList<Long>()
        val uris = ArrayList<String>()
    }

    private class TreeGrant(val requestId: Long, val uri: String)

    private class PendingDir(val documentId: String, val relative: String, val depth: Int)

    private class Child(
        val documentId: String,
        val name: String,
        val mime: String?,
        val size: Long,
        val modified: Long,
        val flags: Int,
    )

    /** One listing, on [workExecutor]: answers Rust exactly once. */
    private fun runListing(ctx: Context, requestId: Long, query: TreeQuery, job: Job) {
        val listing = Listing()
        val failure = try {
            walkTree(ctx, query, job, listing)
        } catch (e: SecurityException) {
            ACCESS_LOST
        } catch (e: IllegalArgumentException) {
            // Not a tree URI, or the provider refused the document.
            ACCESS_LOST
        } catch (e: Exception) {
            Log.w(TAG, "frust-file-dialogs: listing a folder failed: ${e.javaClass.name}")
            ERROR
        }
        jobs.remove(requestId)
        if (failure == null) notifyListing(requestId, listing) else notifyCode(requestId, failure)
    }

    /**
     * Fills [listing] with the regular files under the tree: depth first,
     * children in name order so the entry cap is deterministic, stopping at
     * `maxEntries`. Returns null when finished, otherwise the failure code. A
     * subfolder that cannot be read is skipped; an unreadable root is
     * [ACCESS_LOST]. Virtual documents are not listed: they have no byte stream.
     */
    private fun walkTree(ctx: Context, query: TreeQuery, job: Job, listing: Listing): Int? {
        val tree = query.tree
        val visited = HashSet<String>()
        val budget = maxOf(query.maxEntries, 1).toLong() * WALK_VISIT_MULTIPLE
        var seen = 0L
        var skippedNames = 0
        val pending = ArrayDeque<PendingDir>()
        pending.addLast(PendingDir(DocumentsContract.getTreeDocumentId(tree), "", 0))
        while (true) {
            val dir = pending.removeLastOrNull() ?: return null
            if (job.cancelled) return CANCELLED
            if (!visited.add(dir.documentId)) continue
            if (seen >= budget) return truncatedVisit(skippedNames)
            seen += 1
            val room = (budget - seen).coerceAtMost(Int.MAX_VALUE.toLong()).toInt()
            val children = readChildren(ctx, tree, dir.documentId, job, room) ?: if (dir.depth == 0) {
                return ACCESS_LOST
            } else {
                Log.w(TAG, "frust-file-dialogs: skipping an unreadable subfolder")
                continue
            }
            seen += children.size
            val subdirs = ArrayList<PendingDir>()
            for (child in children) {
                if (job.cancelled) return CANCELLED
                if (!isSafeEntryName(child.name)) {
                    skippedNames += 1
                    continue
                }
                if (!query.includeHidden && child.name.startsWith(".")) continue
                val relative = if (dir.relative.isEmpty()) child.name else "${dir.relative}/${child.name}"
                if (child.mime == DocumentsContract.Document.MIME_TYPE_DIR) {
                    if (query.recursive && dir.depth < query.maxDepth) {
                        subdirs.add(PendingDir(child.documentId, relative, dir.depth + 1))
                    }
                } else if (
                    child.flags and DocumentsContract.Document.FLAG_VIRTUAL_DOCUMENT == 0 &&
                    matchesExtension(child.name, query.extensions)
                ) {
                    if (listing.names.size >= query.maxEntries) {
                        Log.w(TAG, "frust-file-dialogs: listing truncated at ${query.maxEntries} entries")
                        return null
                    }
                    listing.names.add(child.name)
                    listing.relativePaths.add(relative)
                    listing.sizes.add(child.size)
                    listing.modified.add(child.modified)
                    listing.uris.add(DocumentsContract.buildDocumentUriUsingTree(tree, child.documentId).toString())
                }
            }
            if (children.size >= room) return truncatedVisit(skippedNames)
            for (subdir in subdirs.asReversed()) pending.addLast(subdir)
        }
    }

    /** The walk visited as many folders and children as it may: a truncated listing. Logs counts only. */
    private fun truncatedVisit(skippedNames: Int): Int? {
        Log.w(TAG, "frust-file-dialogs: listing truncated at the visit limit, $skippedNames unsafe names skipped")
        return null
    }

    /**
     * Whether a provider's display [name] is one path component: not empty, not
     * `.` or `..`, and free of `/`, `\` and NUL (the rules [safeCopyName] applies).
     */
    internal fun isSafeEntryName(name: String): Boolean {
        val trimmed = name.trim()
        if (trimmed.isEmpty() || trimmed == "." || trimmed == "..") return false
        return name.none { it == '/' || it == '\\' || it == '\u0000' }
    }

    /** True when [extensions] is empty or holds [name]'s extension, in lower case. */
    private fun matchesExtension(name: String, extensions: Set<String>): Boolean {
        if (extensions.isEmpty()) return true
        val dot = name.lastIndexOf('.')
        return dot > 0 && name.substring(dot + 1).lowercase() in extensions
    }

    /**
     * The children of [documentId], in name order, or null when the provider
     * has no such folder (a `DocumentsProvider` answers a missing document
     * with a null cursor). Empty once [job] is cancelled; at most [limit] children.
     */
    private fun readChildren(ctx: Context, tree: Uri, documentId: String, job: Job, limit: Int): List<Child>? {
        val uri = DocumentsContract.buildChildDocumentsUriUsingTree(tree, documentId)
        val columns = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
            DocumentsContract.Document.COLUMN_SIZE,
            DocumentsContract.Document.COLUMN_LAST_MODIFIED,
            DocumentsContract.Document.COLUMN_FLAGS,
        )
        val cursor: Cursor = ctx.contentResolver.query(uri, columns, null, null, null) ?: return null
        val children = ArrayList<Child>()
        cursor.use {
            while (!job.cancelled && children.size < limit && it.moveToNext()) {
                val id = it.getString(0) ?: continue
                val name = it.getString(1) ?: continue
                children.add(
                    Child(
                        documentId = id,
                        name = name,
                        mime = it.getString(2),
                        size = if (it.isNull(3)) NO_VALUE else it.getLong(3),
                        modified = if (it.isNull(4)) NO_VALUE else it.getLong(4),
                        flags = it.getInt(5),
                    ),
                )
            }
        }
        children.sortBy { it.name }
        return children
    }

    // --- The slot -----------------------------------------------------------

    /**
     * Frees [flight]'s slot, then answers it once: `cleanup` and [CANCELLED]
     * when it was cancelled, [deliver] otherwise, nothing when it no longer
     * holds the slot. No monitor is held while answering.
     */
    private inline fun answer(flight: InFlight, cleanup: () -> Unit = {}, deliver: () -> Unit) {
        when (releaseFlight(flight)) {
            null -> Unit
            true -> {
                cleanup()
                notifyCode(flight.requestId, CANCELLED)
            }
            false -> deliver()
        }
    }

    /**
     * Take the single in-flight slot for [requestId], or return false while
     * another request holds it. No takeover: every holder is answered exactly
     * once, so a waiting Rust caller is never orphaned.
     */
    @Synchronized
    private fun claimInFlight(requestId: Long, flow: Flow, activity: Activity): Boolean {
        if (inFlight != null) return false
        inFlight = InFlight(requestId, flow, activity)
        return true
    }

    /** The slot's holder when it is [requestId], or null. */
    @Synchronized
    private fun holderOf(requestId: Long): InFlight? = inFlight?.takeIf { it.requestId == requestId }

    /**
     * A dialog's result arrived: null, discarding the result, when [requestId]
     * no longer holds the slot or was cancelled while the dialog was up (the
     * slot is then freed); otherwise the flight, detached from its Activity so
     * a destroy from here on cannot answer it.
     */
    @Synchronized
    private fun claimResult(requestId: Long): InFlight? {
        val current = inFlight ?: return null
        if (current.requestId != requestId) return null
        if (current.cancelled) {
            inFlight = null
            return null
        }
        current.activity = null
        return current
    }

    /**
     * Mark [requestId] cancelled. While its dialog is up this returns true
     * once (the caller answers) and the slot stays held until the result
     * arrives; once its copy runs, the work thread answers.
     */
    @Synchronized
    private fun cancelInFlight(requestId: Long): Boolean {
        val current = inFlight ?: return false
        if (current.requestId != requestId) return false
        current.cancelled = true
        if (current.activity == null || current.answered) return false
        current.answered = true
        return true
    }

    /**
     * Free [flight]'s slot: null when it no longer holds the slot or [cancel]
     * already answered it, otherwise whether it was cancelled (read under the
     * same monitor [cancelInFlight] writes it under).
     */
    @Synchronized
    private fun releaseFlight(flight: InFlight): Boolean? {
        if (inFlight !== flight) return null
        inFlight = null
        if (flight.answered) return null
        return flight.cancelled
    }

    /** Free the slot if its dialog launched from [activity]; the flight still owing an answer, or null. */
    @Synchronized
    private fun releaseInFlightFor(activity: Activity): InFlight? {
        val current = inFlight ?: return null
        if (current.activity !== activity) return null
        inFlight = null
        return current.takeIf { !it.answered }
    }

    /** The Activity's dialog callback can no longer fire: answer as a dismissal. */
    private fun answerDismissed(flight: InFlight) {
        when (flight.flow) {
            Flow.OPEN -> notifyPaths(flight.requestId, emptyArray())
            Flow.EXPORT -> notifyCode(flight.requestId, DISMISSED)
            Flow.TREE -> notifyTree(flight.requestId, null)
        }
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
                // Its dialog's callback can no longer fire: answer, so Rust stops waiting.
                releaseInFlightFor(activity)?.let { answerDismissed(it) }
            }
        }
}
