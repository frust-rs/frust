package dev.frust.photolibrary

import android.content.Context
import android.media.ExifInterface
import android.net.Uri
import android.util.Log
import dev.frust.photolibrary.FrustPhotoLibraryHost.DecodeResult
import java.io.FileNotFoundException

/**
 * The EXIF of one library asset for `PhotoLibrary::asset_metadata`, called
 * from [FrustPhotoLibraryHost]'s `assetMetadata` static on the Rust caller's
 * own thread (never the main one: Rust refuses the UI thread first). Rust
 * never looks this object up by name, so it needs no keep rule of its own; it
 * answers with [FrustPhotoLibraryHost]'s nested [DecodeResult], whose
 * `FrustPhotoLibraryHost$*` keep rule covers the fields Rust reads. Touches no
 * pick, export or decode state.
 *
 * ## Opening the asset
 *
 * [read] accepts only the `MediaStore` image and video ids browsing reports
 * ([FrustPhotoLibraryHost.isExportableId]) and opens them with
 * [FrustPhotoLibraryHost.openOriginalStream], the export's own open: the
 * unredacted bytes when `ACCESS_MEDIA_LOCATION` is granted, the plain URI
 * otherwise. `MediaStore` redacts the location of the plain URI, so its
 * result carries no GPS at all. The stream is closed on every path.
 *
 * ## One EXIF read
 *
 * The tags are read by [ImageDecodeHost.withExif], the decoder's own reader,
 * so both calls fill the same fields the same way (the capture time is
 * `DateTimeOriginal` only). The result's `colorSpace` is always
 * [FrustPhotoLibraryHost.COLOR_OTHER]. A video carries no EXIF this reads, so
 * it answers an empty [FrustPhotoLibraryHost.OK] result once it opens.
 * `MediaStore` makes no network distinction, so there is no network option.
 */
internal object AssetMetadataHost {
    private const val TAG = "frust"

    /** The EXIF of [id], or a failure code with every value absent. */
    fun read(ctx: Context?, id: String): DecodeResult {
        val context = ctx ?: return ImageDecodeHost.decodeFailed(FrustPhotoLibraryHost.NOT_INITIALIZED)
        val uri = Uri.parse(id)
        if (!FrustPhotoLibraryHost.isExportableId(uri)) {
            return ImageDecodeHost.decodeFailed(FrustPhotoLibraryHost.NOT_FOUND)
        }
        return try {
            val opened = FrustPhotoLibraryHost.openOriginalStream(context, uri)
            opened.stream.use { input ->
                val exif = if (isVideo(uri)) null else ExifInterface(input)
                ImageDecodeHost.withExif(FrustPhotoLibraryHost.COLOR_OTHER, exif, opened.unredacted)
            }
        } catch (e: FileNotFoundException) {
            ImageDecodeHost.decodeFailed(FrustPhotoLibraryHost.NOT_FOUND)
        } catch (e: SecurityException) {
            ImageDecodeHost.decodeFailed(FrustPhotoLibraryHost.PERMISSION_DENIED)
        } catch (e: Exception) {
            Log.w(TAG, "frust-photo-library: reading an asset's metadata failed: ${e.javaClass.name}")
            ImageDecodeHost.decodeFailed(FrustPhotoLibraryHost.DECODE_FAILED)
        } catch (e: OutOfMemoryError) {
            ImageDecodeHost.decodeFailed(FrustPhotoLibraryHost.DECODE_FAILED)
        }
    }

    /** Whether [uri] (an exportable id) is a `MediaStore` video item. */
    private fun isVideo(uri: Uri): Boolean = uri.pathSegments[1] == "video"
}
