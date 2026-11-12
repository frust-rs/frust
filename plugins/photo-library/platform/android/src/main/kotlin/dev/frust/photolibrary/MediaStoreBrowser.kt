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
import android.os.CancellationSignal
import android.os.OperationCanceledException
import android.provider.BaseColumns
import android.provider.MediaStore
import android.util.Log
import android.util.Size
import dev.frust.photolibrary.FrustPhotoLibraryHost.AlbumsResult
import dev.frust.photolibrary.FrustPhotoLibraryHost.AssetsAfterResult
import dev.frust.photolibrary.FrustPhotoLibraryHost.AssetsByIdResult
import dev.frust.photolibrary.FrustPhotoLibraryHost.AssetsResult
import dev.frust.photolibrary.FrustPhotoLibraryHost.ThumbnailResult
import java.io.FileNotFoundException
import java.io.IOException
import java.nio.ByteBuffer
import java.util.concurrent.ConcurrentHashMap
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
 *   so [assets] pages with `Cursor.moveToPosition` there, and [assetsAfter]
 *   reads only its first rows.
 * - `ContentResolver.loadThumbnail` (API 29) decodes with
 *   `ImageDecoder.ALLOCATOR_SOFTWARE`, applies the provider's orientation, and
 *   a missing item surfaces as `FileNotFoundException`
 *   (`MediaProvider.ensureThumbnail`). Its `CancellationSignal` reaches the
 *   provider's `openTypedAssetFile`; a cancelled load throws
 *   `OperationCanceledException`, or an `IOException` from the stream it
 *   abandoned, so any failure after a cancel is reported as a cancel.
 * - `MediaProvider` limits an app holding only
 *   `READ_MEDIA_VISUAL_USER_SELECTED` to the items the user selected
 *   (`hasUserSelectedAccess`, API 34), so partial access needs no special path
 *   here.
 * - A computed `ORDER BY` ([SORT_ORDER]) is accepted. On `android11-release`
 *   `MediaProvider.queryInternal` sets `SQLiteQueryBuilder.setStrictGrammar(true)`
 *   for third-party callers, and `SQLiteQueryBuilder.enforceStrictGrammar`
 *   tokenizes the sort order through `enforceStrictToken`, which admits column
 *   names (`isTableOrColumn`) and the SQL keywords `CASE`, `WHEN`, `THEN`,
 *   `ELSE`, `END` (`SQLiteTokenizer.isKeyword`) while rejecting only
 *   `SELECT`/`FROM`/`WHERE`/`GROUP`/`HAVING`/`WINDOW`/`VALUES`/`ORDER`/`LIMIT`;
 *   numeric literals and operators are not tokens it checks. On
 *   `android10-release` the query builder sets no strict grammar at all.
 * - `MediaStore.MediaColumns.WIDTH`/`HEIGHT` are the stored pixels, before
 *   `ORIENTATION` is applied ([displaySize]).
 *
 * ## Access
 *
 * Every call first checks [mediaPermissions] with `checkSelfPermission`. No
 * permission held answers [FrustPhotoLibraryHost.PERMISSION_DENIED]; a partial
 * grant proceeds. The candidate sets mirror `frust-permissions`' photo table,
 * pinned by `tests/kotlin_contract.rs`; on API 33+ only the subset the manifest
 * declares is checked and requested ([narrowPermissions]). The by-id and
 * listing paths share the [browse] guard, so both follow that subset. A listing
 * for a media type whose permission is not declared (for example videos with
 * only images declared) returns the platform's result, which is empty; it is
 * not an error.
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

    /**
     * Newest first by the timestamp [readRows] reports: `DATE_TAKEN` when it is
     * positive, otherwise `DATE_ADDED` in milliseconds. A plain
     * `DATE_TAKEN DESC` would sort NULL and 0 last, away from the reported time.
     * The id keeps pages stable when two items share a timestamp.
     */
    private const val SORT_ORDER =
        "CASE WHEN ${MediaStore.MediaColumns.DATE_TAKEN} > 0 THEN ${MediaStore.MediaColumns.DATE_TAKEN} " +
            "ELSE ${MediaStore.MediaColumns.DATE_ADDED} * 1000 END DESC, ${BaseColumns._ID} DESC"

    /**
     * The first key of [SORT_ORDER] on its own, for [keysetSelection]. The
     * same text as in [SORT_ORDER] (pinned by `tests/kotlin_contract.rs`), so
     * the keyset compares exactly what the page is ordered by.
     */
    private const val SORT_KEY =
        "CASE WHEN ${MediaStore.MediaColumns.DATE_TAKEN} > 0 THEN ${MediaStore.MediaColumns.DATE_TAKEN} " +
            "ELSE ${MediaStore.MediaColumns.DATE_ADDED} * 1000 END"

    private val ASSET_PROJECTION = arrayOf(
        BaseColumns._ID,
        MediaStore.Files.FileColumns.MEDIA_TYPE,
        MediaStore.MediaColumns.WIDTH,
        MediaStore.MediaColumns.HEIGHT,
        MediaStore.MediaColumns.DATE_TAKEN,
        MediaStore.MediaColumns.DATE_ADDED,
        MediaStore.MediaColumns.DURATION,
        MediaStore.MediaColumns.ORIENTATION,
        MediaStore.MediaColumns.DATE_MODIFIED,
    )

    /** Projected only when the query asks for details: the file name and the MIME type. */
    private val DETAIL_COLUMNS = arrayOf(
        MediaStore.MediaColumns.DISPLAY_NAME,
        MediaStore.MediaColumns.MIME_TYPE,
    )

    /**
     * [ASSET_PROJECTION], plus `IS_FAVORITE` on API 30 and later (the column does
     * not exist on 29), plus [DETAIL_COLUMNS] only when [details] is set. Columns
     * are read back by name, so the positions here carry no meaning.
     */
    private fun projection(details: Boolean): Array<String> {
        var columns = ASSET_PROJECTION
        if (Build.VERSION.SDK_INT >= 30) columns += MediaStore.MediaColumns.IS_FAVORITE
        if (details) columns += DETAIL_COLUMNS
        return columns
    }

    /** The photo permissions on [sdk] (`frust-permissions`' `Photos(ReadWrite)` candidate set). */
    private fun candidatePermissions(sdk: Int): List<String> = when {
        sdk >= 34 -> MEDIA_PERMISSIONS_SDK34
        sdk == 33 -> MEDIA_PERMISSIONS_SDK33
        else -> MEDIA_PERMISSIONS_SDK29
    }

    /**
     * The photo permissions to check and request on [sdk]: on API 33+ the
     * subset of the candidates that [declared] holds (`READ_MEDIA_IMAGES`,
     * `READ_MEDIA_VIDEO`, and `READ_MEDIA_VISUAL_USER_SELECTED` on 34+), so an
     * undeclared permission is never requested. With no media type declared
     * the subset is empty and nothing is requested or accepted. Below 33 the
     * candidates are returned unchanged. Same rule as `frust-permissions`.
     */
    internal fun narrowPermissions(sdk: Int, declared: Set<String>): List<String> {
        val candidates = candidatePermissions(sdk)
        if (sdk < 33) return candidates
        val subset = candidates.filter { it in declared }
        return if (subset.any { it != READ_MEDIA_VISUAL_USER_SELECTED }) subset else emptyList()
    }

    /** [narrowPermissions] against this app's manifest on the running device. */
    internal fun mediaPermissions(ctx: Context, sdk: Int): List<String> =
        narrowPermissions(sdk, manifestPermissions(ctx))

    /**
     * The manifest's `<uses-permission>` names, read once: the manifest cannot
     * change while the process lives.
     */
    private fun manifestPermissions(ctx: Context): Set<String> {
        declaredManifest?.let { return it }
        val pm = ctx.packageManager
        val info = try {
            if (Build.VERSION.SDK_INT >= 33) {
                pm.getPackageInfo(ctx.packageName, PackageManager.PackageInfoFlags.of(PackageManager.GET_PERMISSIONS.toLong()))
            } else {
                @Suppress("DEPRECATION")
                pm.getPackageInfo(ctx.packageName, PackageManager.GET_PERMISSIONS)
            }
        } catch (e: Exception) {
            Log.w(TAG, "frust-photo-library: reading the manifest failed: ${e.javaClass.name}")
            return emptySet()
        }
        val names = info.requestedPermissions?.toSet() ?: emptySet()
        declaredManifest = names
        return names
    }

    @Volatile
    private var declaredManifest: Set<String>? = null

    private fun isGranted(ctx: Context, perm: String): Boolean =
        ctx.checkSelfPermission(perm) == PackageManager.PERMISSION_GRANTED

    private fun hasAccess(ctx: Context): Boolean =
        mediaPermissions(ctx, Build.VERSION.SDK_INT).any { isGranted(ctx, it) }

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
        AssetsResult(
            code, total, emptyArray(), IntArray(0), IntArray(0), IntArray(0), LongArray(0), LongArray(0),
            LongArray(0), IntArray(0), emptyArray(), emptyArray(),
        )

    private fun assetsAfterFailed(code: Int) = AssetsAfterResult(code, assetsFailed(code), 0, 0, false)

    private fun assetsByIdFailed(code: Int) = AssetsByIdResult(code, IntArray(0), assetsFailed(code))

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

    /** `AssetsResult.favorites`: the platform does not report it (below API 30). */
    private const val FAVORITE_UNKNOWN = -1

    private const val ALL_MEDIA_TITLE = "All Media"

    /** One row of a listing, read before it is split into parallel arrays. */
    private class Row(
        val id: String,
        /** `_ID`, the [SORT_ORDER] tie-break. */
        val rowId: Long,
        val kind: Int,
        val width: Int,
        val height: Int,
        val date: Long,
        val duration: Long,
        val modified: Long,
        val favorite: Int,
        val name: String?,
        val type: String?,
    )

    /**
     * One page, newest first, with the total row count of the query. Pages with
     * `QUERY_ARG_OFFSET`/`QUERY_ARG_LIMIT` on API 30+ and
     * `Cursor.moveToPosition` on API 29 (class doc). [details] adds the file
     * name and MIME type to the same query; there is no extra query per row.
     */
    fun assets(ctx: Context?, albumId: String?, filter: Int, offset: Long, limit: Int, details: Boolean): AssetsResult =
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
            val cursor = queryPage(resolver, selection, args, offset.toInt(), limit, details)
                ?: return@browse assetsFailed(FrustPhotoLibraryHost.QUERY_FAILED, total.toLong())
            val rows = cursor.use { c -> readRows(c, offset.toInt(), limit, details) }
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
        details: Boolean,
    ): Cursor? {
        val projection = projection(details)
        if (Build.VERSION.SDK_INT < 30) {
            return resolver.query(FILES_URI, projection, selection, args, SORT_ORDER)
        }
        val queryArgs = Bundle().apply {
            putString(ContentResolver.QUERY_ARG_SQL_SELECTION, selection)
            args?.let { putStringArray(ContentResolver.QUERY_ARG_SQL_SELECTION_ARGS, it) }
            putString(ContentResolver.QUERY_ARG_SQL_SORT_ORDER, SORT_ORDER)
            putInt(ContentResolver.QUERY_ARG_OFFSET, offset)
            putInt(ContentResolver.QUERY_ARG_LIMIT, limit)
        }
        return resolver.query(FILES_URI, projection, queryArgs, null)
    }

    /**
     * One keyset page, newest first: the first [limit] rows after the cursor
     * at ([afterKey], [afterId]) in [SORT_ORDER], or from the newest when
     * [first] is set. No row is counted: the query reads one row past the page,
     * and [AssetsAfterResult.hasMore] says whether it found one.
     * [AssetsAfterResult.lastKey] and [AssetsAfterResult.lastId] are the
     * page's last row's key (the reported timestamp, [readRows]) and `_ID`.
     */
    fun assetsAfter(
        ctx: Context?,
        albumId: String?,
        filter: Int,
        first: Boolean,
        afterKey: Long,
        afterId: Long,
        limit: Int,
        details: Boolean,
    ): AssetsAfterResult =
        browse(ctx, "assetsAfter", ::assetsAfterFailed) { context ->
            val mediaType = mediaSelection(filter)
                ?: return@browse assetsAfterFailed(FrustPhotoLibraryHost.QUERY_FAILED)
            val bucket = albumId?.takeIf { it != FrustPhotoLibraryHost.ALL_MEDIA_ALBUM_ID }
            val scoped = if (bucket == null) mediaType else "$mediaType AND ${MediaStore.MediaColumns.BUCKET_ID} = ?"
            val selection = if (first) scoped else "$scoped AND ${keysetSelection(afterKey, afterId)}"
            val args = bucket?.let { arrayOf(it) }
            val pageSize = limit.coerceIn(1, Int.MAX_VALUE - 1)
            val window = pageSize + 1
            val cursor = queryAfter(context.contentResolver, selection, args, window, details)
                ?: return@browse assetsAfterFailed(FrustPhotoLibraryHost.QUERY_FAILED)
            val rows = cursor.use { c -> readRows(c, 0, window, details) }
            val more = rows.size > pageSize
            val page = if (more) rows.subList(0, pageSize) else rows
            val last = page.lastOrNull()
            AssetsAfterResult(FrustPhotoLibraryHost.OK, assetsPage(page, 0), last?.date ?: 0, last?.rowId ?: 0, more)
        }

    /**
     * The rows after ([afterKey], [afterId]) in [SORT_ORDER]: an older key, or
     * the same key and a smaller `_ID`. Both values are inlined as integer
     * literals rather than bound as `?` arguments: a bound argument is text,
     * and SQLite compares the affinity-less `CASE` key with text as greater
     * than every number, which would match every row. A Kotlin `Long` prints
     * as digits and a sign only.
     */
    internal fun keysetSelection(afterKey: Long, afterId: Long): String =
        "(($SORT_KEY) < $afterKey OR (($SORT_KEY) = $afterKey AND ${BaseColumns._ID} < $afterId))"

    /**
     * At most [window] rows of [selection] in [SORT_ORDER], from the first:
     * `QUERY_ARG_LIMIT` with no offset on API 30+. API 29 cannot limit a query
     * (class doc), so there the cursor is unlimited and [readRows] reads only
     * its first [window] rows, with no seek.
     */
    private fun queryAfter(
        resolver: ContentResolver,
        selection: String,
        args: Array<String>?,
        window: Int,
        details: Boolean,
    ): Cursor? {
        val projection = projection(details)
        if (Build.VERSION.SDK_INT < 30) {
            return resolver.query(FILES_URI, projection, selection, args, SORT_ORDER)
        }
        val queryArgs = Bundle().apply {
            putString(ContentResolver.QUERY_ARG_SQL_SELECTION, selection)
            args?.let { putStringArray(ContentResolver.QUERY_ARG_SQL_SELECTION_ARGS, it) }
            putString(ContentResolver.QUERY_ARG_SQL_SORT_ORDER, SORT_ORDER)
            putInt(ContentResolver.QUERY_ARG_LIMIT, window)
        }
        return resolver.query(FILES_URI, projection, queryArgs, null)
    }

    /**
     * Row ids bound by one query of [assetsById]: well under SQLite's limit on
     * bound arguments (999 before SQLite 3.32, which API 29 and 30 ship).
     */
    internal const val BY_ID_CHUNK = 500

    /**
     * The `MediaStore` row id an asset id names, or null when [id] is not a
     * `content://media` URI (the check [loadThumbnail] makes) or does not end
     * in a row id.
     */
    internal fun rowIdOf(id: String): Long? {
        val uri = Uri.parse(id)
        if (uri.scheme != ContentResolver.SCHEME_CONTENT || uri.authority != MediaStore.AUTHORITY) return null
        return uri.lastPathSegment?.toLongOrNull()?.takeIf { it > 0 }
    }

    /**
     * The rows of [ids], answered by position: [AssetsByIdResult.page] holds
     * one row per input id that resolved, in input order (a duplicate id gives
     * a duplicate row), and [AssetsByIdResult.positions] the input index of
     * each. An id that is not a `content://media` URI ([rowIdOf]), a deleted
     * row, a row that is not an image or a video, and a row outside a partial
     * grant (class doc, "Access") have no row. The distinct row ids are
     * queried from [FILES_URI] in chunks of at most [BY_ID_CHUNK] bound
     * arguments, with the listing's projection and [readRows], so a row reads
     * exactly as it does in [assets]; [details] adds the same columns.
     */
    fun assetsById(ctx: Context?, ids: Array<String>, details: Boolean): AssetsByIdResult =
        browse(ctx, "assetsById", ::assetsByIdFailed) { context ->
            val mediaType = mediaSelection(FrustPhotoLibraryHost.FILTER_IMAGES_AND_VIDEOS)
                ?: return@browse assetsByIdFailed(FrustPhotoLibraryHost.QUERY_FAILED)
            val rowIds = Array(ids.size) { rowIdOf(ids[it]) }
            val wanted = LinkedHashSet<Long>()
            rowIds.forEach { rowId -> if (rowId != null) wanted.add(rowId) }
            val projection = projection(details)
            val resolver = context.contentResolver
            val found = HashMap<Long, Row>()
            for (chunk in wanted.chunked(BY_ID_CHUNK)) {
                val placeholders = chunk.joinToString(",") { "?" }
                val selection = "$mediaType AND ${BaseColumns._ID} IN ($placeholders)"
                val args = Array(chunk.size) { chunk[it].toString() }
                val cursor = resolver.query(FILES_URI, projection, selection, args, null)
                    ?: return@browse assetsByIdFailed(FrustPhotoLibraryHost.QUERY_FAILED)
                // The ids are distinct, so a chunk has at most one row per id.
                cursor.use { c -> readRows(c, 0, chunk.size, details) }.forEach { found[it.rowId] = it }
            }
            val positions = ArrayList<Int>()
            val rows = ArrayList<Row>()
            rowIds.forEachIndexed { index, rowId ->
                val row = rowId?.let { found[it] }
                if (row != null) {
                    positions.add(index)
                    rows.add(row)
                }
            }
            AssetsByIdResult(FrustPhotoLibraryHost.OK, positions.toIntArray(), assetsPage(rows, 0))
        }

    /** At most [limit] rows of [cursor]; on API 29 the cursor is unpaged, so it seeks [offset]. */
    private fun readRows(cursor: Cursor, offset: Int, limit: Int, details: Boolean): List<Row> {
        val first = if (Build.VERSION.SDK_INT < 30) offset else 0
        val rows = ArrayList<Row>()
        if (!cursor.moveToPosition(first)) return rows
        val modifiedAt = cursor.getColumnIndexOrThrow(MediaStore.MediaColumns.DATE_MODIFIED)
        // Absent below API 30, where the favourite is unknown rather than false.
        val favoriteAt = cursor.getColumnIndex(MediaStore.MediaColumns.IS_FAVORITE)
        val nameAt = if (details) cursor.getColumnIndexOrThrow(MediaStore.MediaColumns.DISPLAY_NAME) else -1
        val typeAt = if (details) cursor.getColumnIndexOrThrow(MediaStore.MediaColumns.MIME_TYPE) else -1
        do {
            val video = cursor.getInt(1) == MEDIA_TYPE_VIDEO
            val base = if (video) MediaStore.Video.Media.EXTERNAL_CONTENT_URI else MediaStore.Images.Media.EXTERNAL_CONTENT_URI
            val (width, height) = displaySize(cursor.getInt(2), cursor.getInt(3), cursor.getInt(7))
            val taken = cursor.getLong(4)
            val rowId = cursor.getLong(0)
            rows.add(
                Row(
                    // The typed collection's URI: `loadThumbnail` resolves it on every API level.
                    id = ContentUris.withAppendedId(base, rowId).toString(),
                    rowId = rowId,
                    kind = if (video) FrustPhotoLibraryHost.KIND_VIDEO else FrustPhotoLibraryHost.KIND_IMAGE,
                    width = width,
                    height = height,
                    date = if (taken > 0) taken else cursor.getLong(5) * 1000,
                    duration = if (video) cursor.getLong(6) else 0,
                    // Seconds; the Rust side converts to milliseconds.
                    modified = cursor.getLong(modifiedAt),
                    favorite = if (favoriteAt < 0) FAVORITE_UNKNOWN else cursor.getInt(favoriteAt).coerceIn(0, 1),
                    name = if (nameAt < 0) null else cursor.getString(nameAt),
                    type = if (typeAt < 0) null else cursor.getString(typeAt),
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
        LongArray(rows.size) { rows[it].modified },
        IntArray(rows.size) { rows[it].favorite },
        Array(rows.size) { rows[it].name },
        Array(rows.size) { rows[it].type },
    )

    /** The signal of each running [thumbnail], by request id, until it returns. */
    private val thumbnailSignals = ConcurrentHashMap<Long, CancellationSignal>()

    /**
     * The thumbnail of [id] as RGBA8 bytes (premultiplied; the Rust side
     * unpremultiplies). [fill] centre-crops to [width] x
     * [height]; otherwise the image is scaled to fit inside that box. An id
     * that is not a `MediaStore` content URI, or a stale one, is
     * [FrustPhotoLibraryHost.NOT_FOUND]. The load runs with a
     * `CancellationSignal` registered under [requestId] for [cancelThumbnail];
     * once it fires the answer is [FrustPhotoLibraryHost.CANCELLED].
     */
    fun thumbnail(ctx: Context?, requestId: Long, id: String, width: Int, height: Int, fill: Boolean): ThumbnailResult {
        val signal = CancellationSignal()
        thumbnailSignals[requestId] = signal
        try {
            return loadThumbnail(ctx, signal, id, width, height, fill)
        } finally {
            thumbnailSignals.remove(requestId)
        }
    }

    /** Fires the signal of thumbnail [requestId]. Any thread; an unknown or finished id is ignored. */
    fun cancelThumbnail(requestId: Long) {
        thumbnailSignals[requestId]?.cancel()
    }

    /** [code], or [FrustPhotoLibraryHost.CANCELLED] once [signal] has fired. */
    internal fun cancelledOr(signal: CancellationSignal, code: Int): Int =
        if (signal.isCanceled) FrustPhotoLibraryHost.CANCELLED else code

    private fun loadThumbnail(ctx: Context?, signal: CancellationSignal, id: String, width: Int, height: Int, fill: Boolean): ThumbnailResult =
        browse(ctx, "thumbnail", ::thumbnailFailed) { context ->
            val uri = Uri.parse(id)
            if (uri.scheme != ContentResolver.SCHEME_CONTENT || uri.authority != MediaStore.AUTHORITY) {
                return@browse thumbnailFailed(FrustPhotoLibraryHost.NOT_FOUND)
            }
            val loaded = try {
                context.contentResolver.loadThumbnail(uri, Size(width, height), signal)
            } catch (e: OperationCanceledException) {
                return@browse thumbnailFailed(FrustPhotoLibraryHost.CANCELLED)
            } catch (e: FileNotFoundException) {
                return@browse thumbnailFailed(cancelledOr(signal, FrustPhotoLibraryHost.NOT_FOUND))
            } catch (e: IOException) {
                if (signal.isCanceled) return@browse thumbnailFailed(FrustPhotoLibraryHost.CANCELLED)
                throw e
            }
            // Cancelled after the load: skip the crop and the copy.
            if (signal.isCanceled) return@browse thumbnailFailed(FrustPhotoLibraryHost.CANCELLED)
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
