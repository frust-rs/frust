package dev.frust.photolibrary

import android.content.Context
import android.content.pm.PackageManager
import android.database.ContentObserver
import android.net.Uri
import android.os.Build
import android.provider.BaseColumns
import android.provider.MediaStore
import android.util.Log
import dev.frust.photolibrary.FrustPhotoLibraryHost.ChangesResult

/**
 * The change feed's host side, called from [FrustPhotoLibraryHost]'s statics:
 * a snapshot of the library's change position, and the `ContentObserver` that
 * wakes Rust. The token, the generation filter and the id-set diff live in
 * `plugins/photo-library/src/android_changes.rs`; this side only reads.
 *
 * ## The snapshot
 *
 * [snapshot] reads the volume version (`MediaStore.getVersion`) and, from API
 * 30, the generation (`MediaStore.getGeneration`) first, then every visible
 * image and video `_ID` with its kind, and with [snapshot]'s `withGenerations`
 * also each row's `GENERATION_ADDED` and `GENERATION_MODIFIED`. Reading the
 * position before the rows means a row written during the query is reported
 * again next time, never missed. The query is [MediaStoreBrowser]'s own:
 * `MediaStore.Files` on the `external` volume, images and videos only, with
 * the provider's default visibility, so every id matches an `assets` id. The
 * version and generation are the primary external volume's: `MediaProvider`
 * keeps every external volume in one database with one generation counter,
 * so they cover the rows of all of them. Below API 30 there are no generations: the snapshot
 * reports [NO_GENERATION] and no rows, and Rust answers a full rescan.
 *
 * ## Access
 *
 * The same check as browsing: no context is
 * [FrustPhotoLibraryHost.NOT_INITIALIZED], no permission of the SDK set
 * ([MediaStoreBrowser.mediaPermissions]) or a `SecurityException` is
 * [FrustPhotoLibraryHost.PERMISSION_DENIED], any other failure is
 * [FrustPhotoLibraryHost.QUERY_FAILED] with only the exception's class logged.
 *
 * ## The observer
 *
 * [startObserver] registers one [WakeObserver] on the image and video external
 * collections with `notifyForDescendants`. It runs `onChange` on the binder
 * thread that delivers the notification (no `Handler`), and every overload
 * calls [FrustPhotoLibraryHost.notifyLibraryChanged] with no data and no lock
 * held, so a subscriber may stop the observer from inside the wake. The
 * observer is published before it is registered (and unpublished if
 * registering fails), so a change delivered while it registers reaches the
 * native wake. The Rust registry adds its first subscriber only after start
 * returns, so that early wake reaches no callback: the app calls
 * `changes_since` once after subscribing. [stopObserver] unregisters it; a
 * wake already in flight from an unregistered observer is dropped.
 */
internal object MediaStoreChanges {
    private const val TAG = "frust"

    /** [ChangesResult.generation] below API 30, where `MediaStore` keeps no generations. */
    const val NO_GENERATION = -1L

    /** The browser's collection: images and videos of every external volume in one table. */
    private val FILES_URI: Uri = MediaStore.Files.getContentUri("external")

    private const val MEDIA_TYPE_IMAGE = MediaStore.Files.FileColumns.MEDIA_TYPE_IMAGE
    private const val MEDIA_TYPE_VIDEO = MediaStore.Files.FileColumns.MEDIA_TYPE_VIDEO

    /** The browser's all-media selection. */
    private const val SELECTION =
        "${MediaStore.Files.FileColumns.MEDIA_TYPE} IN ($MEDIA_TYPE_IMAGE, $MEDIA_TYPE_VIDEO)"

    private val ID_COLUMNS = arrayOf(BaseColumns._ID, MediaStore.Files.FileColumns.MEDIA_TYPE)

    private val GENERATION_COLUMNS = arrayOf(
        MediaStore.MediaColumns.GENERATION_ADDED,
        MediaStore.MediaColumns.GENERATION_MODIFIED,
    )

    /** The registered (or registering) observer, or null. Written under [observerLock]; read by [WakeObserver]. */
    @Volatile
    private var observer: WakeObserver? = null

    private val observerLock = Any()

    private fun hasAccess(ctx: Context): Boolean =
        MediaStoreBrowser.mediaPermissions(Build.VERSION.SDK_INT).any {
            ctx.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED
        }

    private fun failed(code: Int) =
        ChangesResult(code, null, NO_GENERATION, LongArray(0), IntArray(0), LongArray(0), LongArray(0), null, null)

    /** The change position and the visible ids (class doc, "The snapshot"). Any thread but the main one. */
    fun snapshot(ctx: Context?, withGenerations: Boolean): ChangesResult {
        if (ctx == null) return failed(FrustPhotoLibraryHost.NOT_INITIALIZED)
        if (!hasAccess(ctx)) return failed(FrustPhotoLibraryHost.PERMISSION_DENIED)
        return try {
            read(ctx, withGenerations)
        } catch (e: SecurityException) {
            failed(FrustPhotoLibraryHost.PERMISSION_DENIED)
        } catch (e: Exception) {
            Log.w(TAG, "frust-photo-library: change snapshot failed: ${e.javaClass.name}")
            failed(FrustPhotoLibraryHost.QUERY_FAILED)
        }
    }

    private fun read(ctx: Context, withGenerations: Boolean): ChangesResult {
        // The typed collections `ContentUris.withAppendedId` builds the browser's ids under.
        val imageBase = MediaStore.Images.Media.EXTERNAL_CONTENT_URI.toString()
        val videoBase = MediaStore.Video.Media.EXTERNAL_CONTENT_URI.toString()
        if (Build.VERSION.SDK_INT < 30) {
            return ChangesResult(
                FrustPhotoLibraryHost.OK, MediaStore.getVersion(ctx), NO_GENERATION,
                LongArray(0), IntArray(0), LongArray(0), LongArray(0), imageBase, videoBase,
            )
        }
        // The position first, then the rows (class doc).
        val version = MediaStore.getVersion(ctx, MediaStore.VOLUME_EXTERNAL_PRIMARY)
        val generation = MediaStore.getGeneration(ctx, MediaStore.VOLUME_EXTERNAL_PRIMARY)
        val projection = if (withGenerations) ID_COLUMNS + GENERATION_COLUMNS else ID_COLUMNS
        val cursor = ctx.contentResolver.query(FILES_URI, projection, SELECTION, null, "${BaseColumns._ID} ASC")
            ?: return failed(FrustPhotoLibraryHost.QUERY_FAILED)
        return cursor.use { c ->
            val capacity = c.count
            val ids = LongArray(capacity)
            val kinds = IntArray(capacity)
            val added = LongArray(if (withGenerations) capacity else 0)
            val modified = LongArray(if (withGenerations) capacity else 0)
            var n = 0
            while (n < capacity && c.moveToNext()) {
                ids[n] = c.getLong(0)
                kinds[n] = if (c.getInt(1) == MEDIA_TYPE_VIDEO) FrustPhotoLibraryHost.KIND_VIDEO else FrustPhotoLibraryHost.KIND_IMAGE
                if (withGenerations) {
                    added[n] = c.getLong(2)
                    modified[n] = c.getLong(3)
                }
                n += 1
            }
            ChangesResult(
                FrustPhotoLibraryHost.OK, version, generation,
                ids.copyOf(n), kinds.copyOf(n),
                added.copyOf(if (withGenerations) n else 0), modified.copyOf(if (withGenerations) n else 0),
                imageBase, videoBase,
            )
        }
    }

    /**
     * Registers the wake observer (class doc, "The observer"). [FrustPhotoLibraryHost.OK]
     * once it is registered, also when it already was. Any thread.
     */
    fun startObserver(ctx: Context?): Int {
        if (ctx == null) return FrustPhotoLibraryHost.NOT_INITIALIZED
        if (!hasAccess(ctx)) return FrustPhotoLibraryHost.PERMISSION_DENIED
        synchronized(observerLock) {
            if (observer != null) return FrustPhotoLibraryHost.OK
            val created = WakeObserver()
            val resolver = ctx.contentResolver
            // Published before registering: a change delivered while the second collection is
            // still registering reaches the native wake. No subscriber is registered yet on the
            // Rust side, so the app covers that window by calling changes_since after subscribing.
            observer = created
            val code = try {
                resolver.registerContentObserver(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, true, created)
                resolver.registerContentObserver(MediaStore.Video.Media.EXTERNAL_CONTENT_URI, true, created)
                FrustPhotoLibraryHost.OK
            } catch (e: SecurityException) {
                FrustPhotoLibraryHost.PERMISSION_DENIED
            } catch (e: Exception) {
                Log.w(TAG, "frust-photo-library: registering the change observer failed: ${e.javaClass.name}")
                FrustPhotoLibraryHost.QUERY_FAILED
            }
            if (code != FrustPhotoLibraryHost.OK) {
                observer = null
                unregister(ctx, created)
                return code
            }
            return FrustPhotoLibraryHost.OK
        }
    }

    /** Unregisters the wake observer. Any thread, including from inside its own wake; a stopped observer is ignored. */
    fun stopObserver(ctx: Context?) {
        val current = synchronized(observerLock) {
            val registered = observer
            observer = null
            registered
        } ?: return
        if (ctx != null) unregister(ctx, current)
    }

    private fun unregister(ctx: Context, target: ContentObserver) {
        try {
            ctx.contentResolver.unregisterContentObserver(target)
        } catch (e: Exception) {
            Log.w(TAG, "frust-photo-library: unregistering the change observer failed: ${e.javaClass.name}")
        }
    }

    /**
     * Calls [FrustPhotoLibraryHost.notifyLibraryChanged] from every `onChange`
     * overload, without calling `super` (each overload's default forwards to
     * the next, so one notification wakes once). No `Handler`: it runs on the
     * binder thread that delivers the notification.
     */
    private class WakeObserver : ContentObserver(null) {
        override fun onChange(selfChange: Boolean) = wake()

        override fun onChange(selfChange: Boolean, uri: Uri?) = wake()

        override fun onChange(selfChange: Boolean, uri: Uri?, flags: Int) = wake()

        override fun onChange(selfChange: Boolean, uris: Collection<Uri>, flags: Int) = wake()

        private fun wake() {
            // Unregistered already: drop the late wake.
            if (observer !== this) return
            FrustPhotoLibraryHost.notifyLibraryChanged()
        }
    }
}
