package dev.frust.photolibrary

import android.content.ContentResolver
import android.content.ContentUris
import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.provider.MediaStore
import android.util.Log
import android.webkit.MimeTypeMap
import java.io.File
import java.io.FileInputStream
import java.io.IOException
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Saves a file into the photo library as a new `MediaStore` row, on its own
 * single-thread executor. Never touches the host's pick slot.
 *
 * ## Platform facts this builds on
 *
 * - A pending row: inserting into a typed collection with
 *   `RELATIVE_PATH`/`IS_PENDING` needs no permission for the app's own items
 *   from API 29 (scoped storage); `IS_PENDING` hides the row from other apps
 *   until `IS_PENDING=0` is written after the bytes land.
 *   `MediaStore.VOLUME_EXTERNAL_PRIMARY` is available from API 29 (minSdk).
 * - `MediaProvider` de-duplicates a clashing `DISPLAY_NAME` itself.
 * - The sweep needs `QUERY_ARG_MATCH_PENDING`, `MATCH_INCLUDE` and
 *   `ContentResolver.delete(Uri, Bundle)`, all API 30+, so API 29 does not
 *   sweep (`DATE_EXPIRES` still reclaims those rows).
 *
 * ## Answers
 *
 * [save] always answers exactly once through
 * [FrustPhotoLibraryHost.notifySaveDone]: [FrustPhotoLibraryHost.OK] with the
 * asset's typed `external` URI (the form browsing reports), or a failure code
 * after the row is deleted. Only exception class names are logged.
 */
internal object MediaStoreWriter {
    private const val TAG = "frust"

    /** A pending row of this app older than this is left over from a dead save. */
    const val STALE_PENDING_SECONDS = 60L * 60L

    /** Bytes per write of the copy into the row. */
    private const val SAVE_CHUNK_BYTES = 1024 * 1024

    private val saveExecutor: ExecutorService by lazy {
        Executors.newSingleThreadExecutor { runnable ->
            Thread(runnable, "frust-photo-library-save").apply { isDaemon = true }
        }
    }

    /** Set by the first save, so the stale-row sweep runs at most once per process. */
    private val swept = AtomicBoolean(false)

    /** Queues one save; the answer arrives through the host's save callback. */
    fun save(ctx: Context, requestId: Long, source: File, isVideo: Boolean, displayName: String, album: String?) {
        saveExecutor.execute {
            sweepStalePending(ctx)
            var uri: String? = null
            val code = try {
                uri = insertRow(ctx, source, isVideo, displayName, album)
                if (uri == null) FrustPhotoLibraryHost.SAVE_FAILED else FrustPhotoLibraryHost.OK
            } catch (e: SecurityException) {
                FrustPhotoLibraryHost.PERMISSION_DENIED
            } catch (e: Throwable) {
                Log.w(TAG, "frust-photo-library: saving to the library failed: ${e.javaClass.name}")
                FrustPhotoLibraryHost.SAVE_FAILED
            }
            FrustPhotoLibraryHost.notifySaveDone(requestId, code, uri)
        }
    }

    /**
     * Inserts a pending row, streams [source] into it, and publishes it; any
     * failure deletes the row and rethrows. Null when no row could be inserted.
     */
    private fun insertRow(ctx: Context, source: File, isVideo: Boolean, displayName: String, album: String?): String? {
        val resolver = ctx.contentResolver
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, displayName)
            put(MediaStore.MediaColumns.MIME_TYPE, mimeFor(displayName, isVideo))
            put(MediaStore.MediaColumns.RELATIVE_PATH, relativePath(ctx, isVideo, album))
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val row = resolver.insert(collection(isVideo), values) ?: return null
        try {
            writeRow(resolver, row, source)
            val published = ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }
            if (resolver.update(row, published, null, null) < 1) throw IOException("the row was not published")
        } catch (e: Throwable) {
            deleteRow(resolver, row)
            throw e
        }
        return ContentUris.withAppendedId(typedExternalUri(isVideo), ContentUris.parseId(row)).toString()
    }

    /** Copies [source] into [row]; the stream is closed before the row is published. */
    private fun writeRow(resolver: ContentResolver, row: Uri, source: File) {
        val output = resolver.openOutputStream(row) ?: throw IOException("no output stream")
        output.use { sink ->
            FileInputStream(source).use { from ->
                val buffer = ByteArray(SAVE_CHUNK_BYTES)
                while (true) {
                    val read = from.read(buffer)
                    if (read < 0) break
                    sink.write(buffer, 0, read)
                }
            }
        }
    }

    private fun deleteRow(resolver: ContentResolver, row: Uri) {
        try {
            resolver.delete(row, null, null)
        } catch (e: Exception) {
            Log.w(TAG, "frust-photo-library: removing a failed row failed: ${e.javaClass.name}")
        }
    }

    private fun collection(isVideo: Boolean): Uri =
        if (isVideo) {
            MediaStore.Video.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)
        } else {
            MediaStore.Images.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)
        }

    private fun typedExternalUri(isVideo: Boolean): Uri =
        if (isVideo) MediaStore.Video.Media.EXTERNAL_CONTENT_URI else MediaStore.Images.Media.EXTERNAL_CONTENT_URI

    /** The extension's MIME type when it matches the kind, else the kind's common type. */
    private fun mimeFor(displayName: String, isVideo: Boolean): String {
        val prefix = if (isVideo) "video/" else "image/"
        val extension = displayName.substringAfterLast('.', "").lowercase()
        val known = MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension)
        if (known != null && known.startsWith(prefix)) return known
        return if (isVideo) "video/mp4" else "image/jpeg"
    }

    /** `Pictures/<album or app label>/` or `Movies/<...>/`; a name with nothing usable is dropped. */
    private fun relativePath(ctx: Context, isVideo: Boolean, album: String?): String {
        val root = if (isVideo) Environment.DIRECTORY_MOVIES else Environment.DIRECTORY_PICTURES
        val raw = album ?: ctx.applicationInfo.loadLabel(ctx.packageManager).toString()
        return folderName(raw)?.let { "$root/$it/" } ?: "$root/"
    }

    /** One safe path segment: separators and control characters become `_`, and a name of only dots has no segment. */
    internal fun folderName(raw: String): String? {
        val cleaned = raw
            .map { c -> if (c < ' ' || c in "/\\:*?\"<>|") '_' else c }
            .joinToString("")
            .trim()
        return cleaned.takeIf { it.isNotEmpty() && it.any { c -> c != '.' } }
    }

    /**
     * Deletes this app's [MediaStore.MediaColumns.IS_PENDING] rows older than
     * [STALE_PENDING_SECONDS]: a save that died before publishing. Best effort,
     * once per process, on the first save.
     */
    private fun sweepStalePending(ctx: Context) {
        if (!swept.compareAndSet(false, true)) return
        if (Build.VERSION.SDK_INT < 30) return
        try {
            val selection = "${MediaStore.MediaColumns.IS_PENDING} = 1 AND " +
                "${MediaStore.MediaColumns.OWNER_PACKAGE_NAME} = ? AND ${MediaStore.MediaColumns.DATE_ADDED} < ?"
            val cutoff = System.currentTimeMillis() / 1000 - STALE_PENDING_SECONDS
            val queryArgs = Bundle().apply {
                putString(ContentResolver.QUERY_ARG_SQL_SELECTION, selection)
                putStringArray(ContentResolver.QUERY_ARG_SQL_SELECTION_ARGS, arrayOf(ctx.packageName, cutoff.toString()))
                putInt(MediaStore.QUERY_ARG_MATCH_PENDING, MediaStore.MATCH_INCLUDE)
            }
            val resolver = ctx.contentResolver
            listOf(false, true).forEach { resolver.delete(collection(it), queryArgs) }
        } catch (e: Exception) {
            Log.i(TAG, "frust-photo-library: the stale-row sweep failed: ${e.javaClass.name}")
        }
    }
}
