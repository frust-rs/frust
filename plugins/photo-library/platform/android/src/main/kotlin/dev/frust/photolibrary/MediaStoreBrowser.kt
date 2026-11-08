package dev.frust.photolibrary

import android.content.ContentResolver
import android.content.ContentUris
import android.content.Context
import android.content.pm.PackageManager
import android.database.Cursor
import android.graphics.Bitmap
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.BaseColumns
import android.provider.MediaStore
import android.util.Log
import android.util.Size
import dev.frust.photolibrary.FrustPhotoLibraryHost.AlbumsResult
import dev.frust.photolibrary.FrustPhotoLibraryHost.AssetsResult
import dev.frust.photolibrary.FrustPhotoLibraryHost.ThumbnailResult
import java.io.FileNotFoundException
import java.nio.ByteBuffer
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/**
 * Read-only photo-library browsing over `MediaStore`, called from
 * [FrustPhotoLibraryHost]'s statics on the Rust caller's own thread. Never
 * touches the host's single in-flight slot.
 *
 * ## Platform facts this builds on
 *
 * Checked against AOSP and the API 36 SDK sources:
 * - `MediaProvider.query` runs `DatabaseUtils.resolveQueryArgs` from
 *   `android11-release` (API 30) on, which turns `QUERY_ARG_OFFSET` /
 *   `QUERY_ARG_LIMIT` into SQL. `android10-release` (API 29) has no such call,
 *   so [assets] pages with `Cursor.moveToPosition` there.
 * - `ContentResolver.loadThumbnail` (API 29) decodes with
 *   `ImageDecoder.ALLOCATOR_SOFTWARE`, applies the provider's orientation, and
 *   a missing item surfaces as `FileNotFoundException`
 *   (`MediaProvider.ensureThumbnail`).
 * - `MediaProvider` limits an app holding only
 *   `READ_MEDIA_VISUAL_USER_SELECTED` to the items the user selected
 *   (`hasUserSelectedAccess`, API 34), so partial access needs no special path
 *   here.
 * - `MediaStore.MediaColumns.WIDTH`/`HEIGHT` are the stored pixels, before
 *   `ORIENTATION` is applied ([displaySize]).
 *
 * ## Access
 *
 * Every call first checks [mediaPermissions] with `checkSelfPermission`. No
 * permission held answers [FrustPhotoLibraryHost.PERMISSION_DENIED]; a partial
 * grant proceeds. The sets mirror `frust-permissions`' photo table, pinned by
 * `tests/kotlin_contract.rs`.
 *
 * ## The asked flag
 *
 * [recordAsked] writes the same `SharedPreferences` keys `frust-permissions`
 * reads, so a selection re-prompt counts as a request there. The two literals
 * are pinned against that plugin's Kotlin source by
 * `tests/kotlin_contract.rs`.
 */
internal object MediaStoreBrowser {
    private const val TAG = "frust"

    /** `SharedPreferences` file of the asked flags (`FrustPermissionsHost.PREFS_FILE`). */
    const val PREFS_FILE = "frust.permissions"

    /** Asked-flag key prefix; the permission name follows (`FrustPermissionsHost.ASKED_KEY_PREFIX`). */
    const val ASKED_KEY_PREFIX = "frust.permissions.asked."

    private val MEDIA_PERMISSIONS_SDK29 = listOf("android.permission.READ_EXTERNAL_STORAGE")
    private val MEDIA_PERMISSIONS_SDK33 = listOf("android.permission.READ_MEDIA_IMAGES", "android.permission.READ_MEDIA_VIDEO")
    private val MEDIA_PERMISSIONS_SDK34 = listOf("android.permission.READ_MEDIA_IMAGES", "android.permission.READ_MEDIA_VIDEO", "android.permission.READ_MEDIA_VISUAL_USER_SELECTED")

    private const val READ_MEDIA_IMAGES = "android.permission.READ_MEDIA_IMAGES"
    private const val READ_MEDIA_VISUAL_USER_SELECTED = "android.permission.READ_MEDIA_VISUAL_USER_SELECTED"

    /** The collection every listing queries: images and videos in one table. */
    private val FILES_URI: Uri = MediaStore.Files.getContentUri("external")

    private const val MEDIA_TYPE_IMAGE = MediaStore.Files.FileColumns.MEDIA_TYPE_IMAGE
    private const val MEDIA_TYPE_VIDEO = MediaStore.Files.FileColumns.MEDIA_TYPE_VIDEO

    /** Newest first; the id keeps pages stable when two items share a timestamp. */
    private const val SORT_ORDER =
        "${MediaStore.MediaColumns.DATE_TAKEN} DESC, ${MediaStore.MediaColumns.DATE_ADDED} DESC, ${BaseColumns._ID} DESC"

    private val ASSET_PROJECTION = arrayOf(
        BaseColumns._ID,
        MediaStore.Files.FileColumns.MEDIA_TYPE,
        MediaStore.MediaColumns.WIDTH,
        MediaStore.MediaColumns.HEIGHT,
        MediaStore.MediaColumns.DATE_TAKEN,
        MediaStore.MediaColumns.DATE_ADDED,
        MediaStore.MediaColumns.DURATION,
        MediaStore.MediaColumns.ORIENTATION,
    )

    /** The photo permissions on [sdk] (`frust-permissions`' `Photos(ReadWrite)` set). */
    internal fun mediaPermissions(sdk: Int): List<String> = when {
        sdk >= 34 -> MEDIA_PERMISSIONS_SDK34
        sdk == 33 -> MEDIA_PERMISSIONS_SDK33
        else -> MEDIA_PERMISSIONS_SDK29
    }

    private fun isGranted(ctx: Context, perm: String): Boolean =
        ctx.checkSelfPermission(perm) == PackageManager.PERMISSION_GRANTED

    private fun hasAccess(ctx: Context): Boolean =
        mediaPermissions(Build.VERSION.SDK_INT).any { isGranted(ctx, it) }

    /** Whether the app holds only a user selection (API 34+ partial access). */
    internal fun isLimited(ctx: Context): Boolean =
        Build.VERSION.SDK_INT >= 34 &&
            !isGranted(ctx, READ_MEDIA_IMAGES) &&
            isGranted(ctx, READ_MEDIA_VISUAL_USER_SELECTED)

    /** Sets the asked flag of every permission in [permissions]. */
    internal fun recordAsked(ctx: Context, permissions: Collection<String>) {
        val editor = ctx.getSharedPreferences(PREFS_FILE, Context.MODE_PRIVATE).edit()
        permissions.forEach { editor.putBoolean(ASKED_KEY_PREFIX + it, true) }
        editor.apply()
    }

    /**
     * The shared front of every browse call: no context, no permission, a
     * revoked permission and any query failure all become a result code, and
     * only an exception's class name is logged.
     */
    private inline fun <T> browse(ctx: Context?, op: String, failed: (Int) -> T, body: (Context) -> T): T {
        if (ctx == null) return failed(FrustPhotoLibraryHost.NOT_INITIALIZED)
        if (!hasAccess(ctx)) return failed(FrustPhotoLibraryHost.PERMISSION_DENIED)
        return try {
            body(ctx)
        } catch (e: SecurityException) {
            failed(FrustPhotoLibraryHost.PERMISSION_DENIED)
        } catch (e: Exception) {
            Log.w(TAG, "frust-photo-library: $op failed: ${e.javaClass.name}")
            failed(FrustPhotoLibraryHost.QUERY_FAILED)
        }
    }

    private fun albumsFailed(code: Int) = AlbumsResult(code, emptyArray(), emptyArray(), LongArray(0))

    private fun assetsFailed(code: Int, total: Long = 0) =
        AssetsResult(code, total, emptyArray(), IntArray(0), IntArray(0), IntArray(0), LongArray(0), LongArray(0))

    private fun thumbnailFailed(code: Int) = ThumbnailResult(code, 0, 0, 0, null)

    /** The `media_type` selection of a filter code, or null for an unknown code. */
    private fun mediaSelection(filter: Int): String? {
        val column = MediaStore.Files.FileColumns.MEDIA_TYPE
        return when (filter) {
            FrustPhotoLibraryHost.FILTER_IMAGES -> "$column = $MEDIA_TYPE_IMAGE"
            FrustPhotoLibraryHost.FILTER_VIDEOS -> "$column = $MEDIA_TYPE_VIDEO"
            FrustPhotoLibraryHost.FILTER_IMAGES_AND_VIDEOS -> "$column IN ($MEDIA_TYPE_IMAGE, $MEDIA_TYPE_VIDEO)"
            else -> null
        }
    }

    private class Bucket(val id: String, val title: String?) {
        var count = 0L
    }

    /**
     * The synthetic all-media album ([FrustPhotoLibraryHost.ALL_MEDIA_ALBUM_ID]),
     * then one album per `BUCKET_ID` sorted by title, each with its item count.
     */
    fun albums(ctx: Context?): AlbumsResult = browse(ctx, "albums", ::albumsFailed) { context ->
        val selection = mediaSelection(FrustPhotoLibraryHost.FILTER_IMAGES_AND_VIDEOS)
        val projection = arrayOf(MediaStore.MediaColumns.BUCKET_ID, MediaStore.MediaColumns.BUCKET_DISPLAY_NAME)
        val buckets = LinkedHashMap<String, Bucket>()
        var total = 0L
        val cursor = context.contentResolver.query(FILES_URI, projection, selection, null, null)
            ?: return@browse albumsFailed(FrustPhotoLibraryHost.QUERY_FAILED)
        cursor.use { c ->
            while (c.moveToNext()) {
                total += 1
                val id = c.getString(0) ?: continue
                buckets.getOrPut(id) { Bucket(id, c.getString(1)) }.count += 1
            }
        }
        val sorted = buckets.values.sortedBy { it.title?.lowercase().orEmpty() }
        val ids = arrayOf(FrustPhotoLibraryHost.ALL_MEDIA_ALBUM_ID) + sorted.map { it.id }
        val titles = arrayOf<String?>(ALL_MEDIA_TITLE) + sorted.map { it.title }
        val counts = longArrayOf(total) + sorted.map { it.count }
        AlbumsResult(FrustPhotoLibraryHost.OK, ids, titles, counts)
    }

    private const val ALL_MEDIA_TITLE = "All Media"

    /** One row of a listing, read before it is split into parallel arrays. */
    private class Row(
        val id: String,
        val kind: Int,
        val width: Int,
        val height: Int,
        val date: Long,
        val duration: Long,
    )

    /**
     * One page, newest first, with the total row count of the query. Pages with
     * `QUERY_ARG_OFFSET`/`QUERY_ARG_LIMIT` on API 30+ and
     * `Cursor.moveToPosition` on API 29 (class doc).
     */
    fun assets(ctx: Context?, albumId: String?, filter: Int, offset: Long, limit: Int): AssetsResult =
        browse(ctx, "assets", { assetsFailed(it) }) { context ->
            val mediaType = mediaSelection(filter)
                ?: return@browse assetsFailed(FrustPhotoLibraryHost.QUERY_FAILED)
            val bucket = albumId?.takeIf { it != FrustPhotoLibraryHost.ALL_MEDIA_ALBUM_ID }
            val selection = if (bucket == null) mediaType else "$mediaType AND ${MediaStore.MediaColumns.BUCKET_ID} = ?"
            val args = bucket?.let { arrayOf(it) }
            val resolver = context.contentResolver
            val total = countRows(resolver, selection, args)
                ?: return@browse assetsFailed(FrustPhotoLibraryHost.QUERY_FAILED)
            if (offset >= total) return@browse assetsPage(emptyList(), total)
            // `offset < total`, and a row count fits in an Int.
            val cursor = queryPage(resolver, selection, args, offset.toInt(), limit)
                ?: return@browse assetsFailed(FrustPhotoLibraryHost.QUERY_FAILED, total.toLong())
            val rows = cursor.use { c -> readRows(c, offset.toInt(), limit) }
            assetsPage(rows, total)
        }

    private fun countRows(resolver: ContentResolver, selection: String, args: Array<String>?): Int? =
        resolver.query(FILES_URI, arrayOf(BaseColumns._ID), selection, args, null)?.use { it.count }

    private fun queryPage(
        resolver: ContentResolver,
        selection: String,
        args: Array<String>?,
        offset: Int,
        limit: Int,
    ): Cursor? {
        if (Build.VERSION.SDK_INT < 30) {
            return resolver.query(FILES_URI, ASSET_PROJECTION, selection, args, SORT_ORDER)
        }
        val queryArgs = Bundle().apply {
            putString(ContentResolver.QUERY_ARG_SQL_SELECTION, selection)
            args?.let { putStringArray(ContentResolver.QUERY_ARG_SQL_SELECTION_ARGS, it) }
            putString(ContentResolver.QUERY_ARG_SQL_SORT_ORDER, SORT_ORDER)
            putInt(ContentResolver.QUERY_ARG_OFFSET, offset)
            putInt(ContentResolver.QUERY_ARG_LIMIT, limit)
        }
        return resolver.query(FILES_URI, ASSET_PROJECTION, queryArgs, null)
    }

    /** At most [limit] rows of [cursor]; on API 29 the cursor is unpaged, so it seeks [offset]. */
    private fun readRows(cursor: Cursor, offset: Int, limit: Int): List<Row> {
        val first = if (Build.VERSION.SDK_INT < 30) offset else 0
        val rows = ArrayList<Row>()
        if (!cursor.moveToPosition(first)) return rows
        do {
            val video = cursor.getInt(1) == MEDIA_TYPE_VIDEO
            val base = if (video) MediaStore.Video.Media.EXTERNAL_CONTENT_URI else MediaStore.Images.Media.EXTERNAL_CONTENT_URI
            val (width, height) = displaySize(cursor.getInt(2), cursor.getInt(3), cursor.getInt(7))
            val taken = cursor.getLong(4)
            rows.add(
                Row(
                    // The typed collection's URI: `loadThumbnail` resolves it on every API level.
                    id = ContentUris.withAppendedId(base, cursor.getLong(0)).toString(),
                    kind = if (video) FrustPhotoLibraryHost.KIND_VIDEO else FrustPhotoLibraryHost.KIND_IMAGE,
                    width = width,
                    height = height,
                    date = if (taken > 0) taken else cursor.getLong(5) * 1000,
                    duration = if (video) cursor.getLong(6) else 0,
                ),
            )
        } while (rows.size < limit && cursor.moveToNext())
        return rows
    }

    /** The display size: width and height swapped when the item is rotated a quarter turn. */
    private fun displaySize(width: Int, height: Int, orientation: Int): Pair<Int, Int> =
        if (orientation == 90 || orientation == 270) Pair(height, width) else Pair(width, height)

    private fun assetsPage(rows: List<Row>, total: Int) = AssetsResult(
        FrustPhotoLibraryHost.OK,
        total.toLong(),
        rows.map { it.id }.toTypedArray(),
        IntArray(rows.size) { rows[it].kind },
        IntArray(rows.size) { rows[it].width },
        IntArray(rows.size) { rows[it].height },
        LongArray(rows.size) { rows[it].date },
        LongArray(rows.size) { rows[it].duration },
    )

    /**
     * The thumbnail of [id] as RGBA8 bytes (premultiplied; the Rust side
     * unpremultiplies). [fill] centre-crops to [width] x
     * [height]; otherwise the image is scaled to fit inside that box. An id
     * that is not a `MediaStore` content URI, or a stale one, is
     * [FrustPhotoLibraryHost.NOT_FOUND].
     */
    fun thumbnail(ctx: Context?, id: String, width: Int, height: Int, fill: Boolean): ThumbnailResult =
        browse(ctx, "thumbnail", ::thumbnailFailed) { context ->
            val uri = Uri.parse(id)
            if (uri.scheme != ContentResolver.SCHEME_CONTENT || uri.authority != MediaStore.AUTHORITY) {
                return@browse thumbnailFailed(FrustPhotoLibraryHost.NOT_FOUND)
            }
            val loaded = try {
                context.contentResolver.loadThumbnail(uri, Size(width, height), null)
            } catch (e: FileNotFoundException) {
                return@browse thumbnailFailed(FrustPhotoLibraryHost.NOT_FOUND)
            }
            val source = if (loaded.config == Bitmap.Config.ARGB_8888) {
                loaded
            } else {
                loaded.copy(Bitmap.Config.ARGB_8888, false)
                    ?: return@browse thumbnailFailed(FrustPhotoLibraryHost.QUERY_FAILED)
            }
            val sized = if (fill) cropped(source, width, height) else fitted(source, width, height)
            // ARGB_8888 is RGBA in memory, premultiplied; `rowBytes` may exceed `width * 4`.
            val pixels = ByteArray(sized.byteCount)
            sized.copyPixelsToBuffer(ByteBuffer.wrap(pixels))
            ThumbnailResult(FrustPhotoLibraryHost.OK, sized.width, sized.height, sized.rowBytes, pixels)
        }

    private fun cropped(source: Bitmap, width: Int, height: Int): Bitmap {
        val scale = max(width.toFloat() / source.width, height.toFloat() / source.height)
        val cropWidth = (width / scale).roundToInt().coerceIn(1, source.width)
        val cropHeight = (height / scale).roundToInt().coerceIn(1, source.height)
        val crop = Bitmap.createBitmap(
            source,
            (source.width - cropWidth) / 2,
            (source.height - cropHeight) / 2,
            cropWidth,
            cropHeight,
        )
        return Bitmap.createScaledBitmap(crop, width, height, true)
    }

    private fun fitted(source: Bitmap, width: Int, height: Int): Bitmap {
        val scale = min(width.toFloat() / source.width, height.toFloat() / source.height)
        val fitWidth = (source.width * scale).roundToInt().coerceIn(1, width)
        val fitHeight = (source.height * scale).roundToInt().coerceIn(1, height)
        return Bitmap.createScaledBitmap(source, fitWidth, fitHeight, true)
    }
}
