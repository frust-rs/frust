package dev.frust.photolibrary

import android.content.ContentResolver
import android.content.Context
import android.graphics.Bitmap
import android.graphics.ColorSpace
import android.graphics.ImageDecoder
import android.media.ExifInterface
import android.media.MediaCodecList
import android.net.Uri
import android.util.Log
import android.util.Size
import android.webkit.MimeTypeMap
import dev.frust.photolibrary.FrustPhotoLibraryHost.DecodeHeader
import dev.frust.photolibrary.FrustPhotoLibraryHost.DecodeResult
import java.io.File
import java.io.FileNotFoundException
import java.io.IOException
import java.io.InputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.Locale
import java.util.concurrent.ConcurrentHashMap
import kotlin.math.max
import kotlin.math.roundToInt

/**
 * Full-resolution decode for `PhotoLibrary::decode_file`, called from
 * [FrustPhotoLibraryHost]'s `decodeHeader`, `decodeInto` and `cancelDecode`
 * statics on the Rust caller's own thread (never the main one: Rust refuses
 * the UI thread first). Rust never looks this object up by name, so it needs
 * no keep rule of its own; its two result types are nested in
 * [FrustPhotoLibraryHost], whose `FrustPhotoLibraryHost$*` keep rule covers
 * the fields Rust reads. Touches no pick or export state.
 *
 * ## Two calls, one buffer
 *
 * A 48 MP RGBA8 image is about 192 MB, so the pixels never cross JNI as a Java
 * array. [header] reads only the header (an `OnHeaderDecodedListener` that
 * stops the decode by throwing [HeaderRead]) and reports the source size and
 * the capped size ([targetSize]). Rust allocates exactly `width * height * 4`
 * bytes and passes them as a direct [ByteBuffer] to [decodeInto], which
 * decodes with `ALLOCATOR_SOFTWARE` at that target size and copies the
 * premultiplied `ARGB_8888` pixels (bytes R, G, B, A) into the buffer with
 * `copyPixelsToBuffer`. [decodeInto] writes through the buffer only before it
 * returns and never keeps it: the memory is a Rust `Vec` that is only
 * guaranteed alive for the length of that call. Rust converts to straight
 * alpha.
 *
 * ## Missing decoders
 *
 * HEIF decoding is optional on a device. A MIME type known before decoding
 * (sniffed from an ISO-BMFF `ftyp` brand, then the content provider's type or
 * the file extension), and the header's own type afterwards, is checked with
 * `ImageDecoder.isMimeTypeSupported`; HEIF additionally needs an HEVC decoder
 * in `MediaCodecList`. A type with no decoder answers [FrustPhotoLibraryHost.NO_DECODER];
 * a `DecodeException` (and any other read failure) answers
 * [FrustPhotoLibraryHost.DECODE_FAILED].
 *
 * ## Orientation, colour and EXIF
 *
 * `ImageDecoder` applies the EXIF orientation itself and reports the oriented
 * size, so nothing here rotates again. The target colour space is sRGB when
 * the caller asked for it or the source is not wide gamut, Display P3
 * otherwise; the reported space is the decoded bitmap's own. EXIF is read with
 * `android.media.ExifInterface` from the same path or content URI; the capture
 * time is `DateTimeOriginal` only, never the last-modified `DateTime`.
 * [withExif] is the one place the tags are read: [AssetMetadataHost] answers
 * `assetMetadata` through it too, so the decode and the metadata call fill the
 * same fields the same way.
 *
 * ## Cancel
 *
 * [header] registers the request in [live]; [cancel] removes it. [decodeInto]
 * answers [FrustPhotoLibraryHost.CANCELLED] when the request is gone before
 * the decode starts or after it finishes (before the copy), and always drops
 * the request when it returns. Rust checks its token between the calls too.
 */
internal object ImageDecodeHost {
    private const val TAG = "frust"

    /**
     * The EXIF 2.3 lens-model tag. The platform `ExifInterface` has no public
     * constant for it; [ExifInterface.getAttribute] answers null when the tag
     * is absent or unknown to it.
     */
    private const val TAG_LENS_MODEL = "LensModel"

    /** Seconds per degree, minute and second of an EXIF GPS coordinate. */
    private val GPS_UNITS = doubleArrayOf(1.0, 60.0, 3600.0)

    /** Bytes of the ISO-BMFF box head [sniffMime] reads: size, `ftyp`, major brand. */
    private const val SNIFF_BYTES = 12

    /** Requests between their header and the end of their [decodeInto]. */
    private val live: MutableSet<Long> = ConcurrentHashMap.newKeySet()

    /** Whether any HEVC decoder is installed; HEIF decoding goes through one. */
    private val hevcDecoderPresent: Boolean by lazy {
        try {
            MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.any { info ->
                !info.isEncoder && info.supportedTypes.any { it.equals("video/hevc", ignoreCase = true) }
            }
        } catch (e: Exception) {
            // Unknown: let the decode itself decide.
            true
        }
    }

    /** Thrown from the header listener so [header] decodes no pixels. */
    private class HeaderRead : RuntimeException() {
        override fun fillInStackTrace(): Throwable = this
    }

    /** A result code decided before or instead of a decode. */
    private class Refused(val code: Int) : Exception() {
        override fun fillInStackTrace(): Throwable = this
    }

    /**
     * The header of [path] (a file path or a content URI): the oriented source
     * size and the size capped so the long edge is at most [maxDimension].
     * Registers [requestId] until [decodeInto] or [cancel].
     */
    fun header(ctx: Context?, requestId: Long, path: String, maxDimension: Int): DecodeHeader {
        live.add(requestId)
        val header = try {
            readHeader(ctx, path, maxDimension)
        } catch (e: Exception) {
            headerFailed(codeFor(e))
        } catch (e: OutOfMemoryError) {
            headerFailed(FrustPhotoLibraryHost.DECODE_FAILED)
        }
        if (header.code != FrustPhotoLibraryHost.OK) live.remove(requestId)
        return header
    }

    /**
     * Decodes [path] at exactly [width] x [height] into [pixels], which must
     * hold `width * height * 4` bytes. Writes through [pixels] only before it
     * returns and never keeps it (class doc, "Two calls, one buffer").
     */
    fun decodeInto(
        ctx: Context?,
        requestId: Long,
        path: String,
        width: Int,
        height: Int,
        srgb: Boolean,
        pixels: ByteBuffer,
    ): DecodeResult {
        try {
            if (!live.contains(requestId)) return decodeFailed(FrustPhotoLibraryHost.CANCELLED)
            val colorSpace = try {
                decodeAndCopy(ctx, requestId, path, width, height, srgb, pixels)
            } catch (e: Exception) {
                return decodeFailed(codeFor(e))
            } catch (e: OutOfMemoryError) {
                return decodeFailed(FrustPhotoLibraryHost.DECODE_FAILED)
            }
            if (colorSpace == null) return decodeFailed(FrustPhotoLibraryHost.CANCELLED)
            return withExif(colorSpace, readExif(ctx, path))
        } finally {
            live.remove(requestId)
        }
    }

    /** Cancels [requestId]: [decodeInto] then copies nothing. An unknown id is ignored. */
    fun cancel(requestId: Long) {
        live.remove(requestId)
    }

    /**
     * The size that fits [width] x [height] in a [maxDimension] long edge,
     * keeping the aspect ratio; unchanged when it already fits. Neither side
     * drops below 1.
     */
    internal fun targetSize(width: Int, height: Int, maxDimension: Int): Size {
        val longEdge = max(width, height)
        if (longEdge <= maxDimension) return Size(width, height)
        val scale = maxDimension.toDouble() / longEdge
        return Size(
            if (width == longEdge) maxDimension else max(1, (width * scale).roundToInt()),
            if (height == longEdge) maxDimension else max(1, (height * scale).roundToInt()),
        )
    }

    /** sRGB when asked for or when the source is not wide gamut; Display P3 otherwise. */
    internal fun targetColorSpace(source: ColorSpace?, srgb: Boolean): ColorSpace =
        if (!srgb && source != null && source.isWideGamut) {
            ColorSpace.get(ColorSpace.Named.DISPLAY_P3)
        } else {
            ColorSpace.get(ColorSpace.Named.SRGB)
        }

    // --- Internals ----------------------------------------------------------

    private fun readHeader(ctx: Context?, path: String, maxDimension: Int): DecodeHeader {
        val source = openSource(ctx, path)
        var size: Size? = null
        var mime: String? = null
        try {
            ImageDecoder.decodeBitmap(source) { _, info, _ ->
                size = info.size
                mime = info.mimeType
                throw HeaderRead()
            }
        } catch (e: HeaderRead) {
            // Expected: the header is all this call reads.
        }
        requireDecoder(mime)
        val read = size ?: throw IOException("no image header")
        if (read.width <= 0 || read.height <= 0) throw IOException("empty image")
        val target = targetSize(read.width, read.height, maxDimension)
        return DecodeHeader(FrustPhotoLibraryHost.OK, read.width, read.height, target.width, target.height)
    }

    /**
     * Decodes and copies into [pixels]; the decoded bitmap's colour-space
     * code, or null when [requestId] was cancelled during the decode.
     */
    private fun decodeAndCopy(
        ctx: Context?,
        requestId: Long,
        path: String,
        width: Int,
        height: Int,
        srgb: Boolean,
        pixels: ByteBuffer,
    ): Int? {
        val source = openSource(ctx, path)
        var bitmap = ImageDecoder.decodeBitmap(source) { decoder, info, _ ->
            decoder.setAllocator(ImageDecoder.ALLOCATOR_SOFTWARE)
            decoder.setTargetSize(width, height)
            decoder.setTargetColorSpace(targetColorSpace(info.colorSpace, srgb))
        }
        try {
            if (bitmap.config != Bitmap.Config.ARGB_8888) {
                // A high-bit-depth source decodes to a wider config; the contract is RGBA8.
                val converted = bitmap.copy(Bitmap.Config.ARGB_8888, false) ?: throw IOException("no ARGB_8888 copy")
                bitmap.recycle()
                bitmap = converted
            }
            // Cancelled while decoding: copy nothing.
            if (!live.contains(requestId)) return null
            if (bitmap.width != width || bitmap.height != height || bitmap.rowBytes != width * 4 ||
                pixels.capacity() != bitmap.byteCount
            ) {
                throw IOException("decoded bitmap does not match the requested size")
            }
            pixels.order(ByteOrder.nativeOrder())
            pixels.rewind()
            bitmap.copyPixelsToBuffer(pixels)
            return colorCode(bitmap.colorSpace)
        } finally {
            bitmap.recycle()
        }
    }

    /**
     * The decoder source for [path], refusing a missing file, a content URI
     * without a context, and a known MIME type no installed decoder handles.
     */
    private fun openSource(ctx: Context?, path: String): ImageDecoder.Source {
        if (isContentUri(path)) {
            val resolver = ctx?.contentResolver ?: throw Refused(FrustPhotoLibraryHost.NOT_INITIALIZED)
            val uri = Uri.parse(path)
            requireDecoder(sniffMime { resolver.openInputStream(uri) } ?: resolver.getType(uri))
            return ImageDecoder.createSource(resolver, uri)
        }
        val file = File(path)
        if (!file.isFile) throw Refused(FrustPhotoLibraryHost.NOT_FOUND)
        requireDecoder(sniffMime { file.inputStream() } ?: extensionMime(file))
        return ImageDecoder.createSource(file)
    }

    private fun isContentUri(path: String): Boolean =
        path.startsWith(ContentResolver.SCHEME_CONTENT + "://")

    /** Refuses an image or video MIME type with no installed decoder (class doc). */
    private fun requireDecoder(mime: String?) {
        val type = mime?.lowercase(Locale.US) ?: return
        if (!type.startsWith("image/") && !type.startsWith("video/")) return
        if (!ImageDecoder.isMimeTypeSupported(type)) throw Refused(FrustPhotoLibraryHost.NO_DECODER)
        if ((type == "image/heic" || type == "image/heif") && !hevcDecoderPresent) {
            throw Refused(FrustPhotoLibraryHost.NO_DECODER)
        }
    }

    /** `image/heic`, `image/heif` or `image/avif` from an ISO-BMFF `ftyp` brand; null otherwise. */
    private fun sniffMime(open: () -> InputStream?): String? {
        val head: ByteArray? = try {
            open()?.use { input ->
                val bytes = ByteArray(SNIFF_BYTES)
                var read = 0
                while (read < SNIFF_BYTES) {
                    val n = input.read(bytes, read, SNIFF_BYTES - read)
                    if (n < 0) break
                    read += n
                }
                if (read == SNIFF_BYTES) bytes else null
            }
        } catch (e: IOException) {
            null
        } catch (e: SecurityException) {
            null
        }
        if (head == null) return null
        if (String(head, 4, 4, Charsets.US_ASCII) != "ftyp") return null
        return when (String(head, 8, 4, Charsets.US_ASCII)) {
            "heic", "heix", "hevc", "hevx", "heim", "heis" -> "image/heic"
            "mif1", "msf1" -> "image/heif"
            "avif", "avis" -> "image/avif"
            else -> null
        }
    }

    private fun extensionMime(file: File): String? {
        val extension = file.extension.lowercase(Locale.US)
        if (extension.isEmpty()) return null
        return MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension)
    }

    /** The result code for a failed header or decode. */
    private fun codeFor(e: Exception): Int = when (e) {
        is Refused -> e.code
        is FileNotFoundException -> FrustPhotoLibraryHost.NOT_FOUND
        is SecurityException -> FrustPhotoLibraryHost.PERMISSION_DENIED
        is ImageDecoder.DecodeException -> {
            Log.i(TAG, "frust-photo-library: decoding an image failed: ${e.javaClass.name}")
            FrustPhotoLibraryHost.DECODE_FAILED
        }
        else -> {
            Log.w(TAG, "frust-photo-library: reading an image failed: ${e.javaClass.name}")
            FrustPhotoLibraryHost.DECODE_FAILED
        }
    }

    private fun colorCode(space: ColorSpace?): Int = when (space) {
        ColorSpace.get(ColorSpace.Named.SRGB) -> FrustPhotoLibraryHost.COLOR_SRGB
        ColorSpace.get(ColorSpace.Named.DISPLAY_P3) -> FrustPhotoLibraryHost.COLOR_DISPLAY_P3
        else -> FrustPhotoLibraryHost.COLOR_OTHER
    }

    /** EXIF of [path], or null when it has none or cannot be read; never fails the decode. */
    private fun readExif(ctx: Context?, path: String): ExifInterface? {
        return try {
            if (isContentUri(path)) {
                val resolver = ctx?.contentResolver ?: return null
                resolver.openInputStream(Uri.parse(path))?.use { ExifInterface(it) }
            } else {
                ExifInterface(path)
            }
        } catch (e: Exception) {
            null
        }
    }

    private fun headerFailed(code: Int): DecodeHeader = DecodeHeader(code, 0, 0, 0, 0)

    /** A failed [DecodeResult]: [code] with every EXIF value absent. Shared with [AssetMetadataHost]. */
    internal fun decodeFailed(code: Int): DecodeResult = DecodeResult(
        code, FrustPhotoLibraryHost.COLOR_OTHER, null, null, null, null, 0,
        Double.NaN, Double.NaN, Double.NaN, Double.NaN, Double.NaN,
    )

    /**
     * An [FrustPhotoLibraryHost.OK] result; absent EXIF values are null, 0 or
     * NaN. With [withLocation] false the GPS values are NaN whatever [exif]
     * holds (a redacted read). Shared with [AssetMetadataHost].
     */
    internal fun withExif(colorSpace: Int, exif: ExifInterface?, withLocation: Boolean = true): DecodeResult {
        val located = exif?.takeIf { withLocation }
        return DecodeResult(
            FrustPhotoLibraryHost.OK,
            colorSpace,
            // The original capture only: `TAG_DATETIME` is when the file was last changed.
            exif?.getAttribute(ExifInterface.TAG_DATETIME_ORIGINAL),
            exif?.getAttribute(ExifInterface.TAG_MAKE),
            exif?.getAttribute(ExifInterface.TAG_MODEL),
            exif?.getAttribute(TAG_LENS_MODEL),
            // `ISOSpeedRatings` is tag 0x8827, renamed `PhotographicSensitivity` by EXIF 2.3.
            exif?.getAttributeInt(ExifInterface.TAG_ISO_SPEED_RATINGS, 0) ?: 0,
            exif?.getAttributeDouble(ExifInterface.TAG_EXPOSURE_TIME, Double.NaN) ?: Double.NaN,
            exif?.getAttributeDouble(ExifInterface.TAG_F_NUMBER, Double.NaN) ?: Double.NaN,
            exif?.getAttributeDouble(ExifInterface.TAG_FOCAL_LENGTH, Double.NaN) ?: Double.NaN,
            located?.let { coordinate(it, ExifInterface.TAG_GPS_LATITUDE, ExifInterface.TAG_GPS_LATITUDE_REF, "S") }
                ?: Double.NaN,
            located?.let { coordinate(it, ExifInterface.TAG_GPS_LONGITUDE, ExifInterface.TAG_GPS_LONGITUDE_REF, "W") }
                ?: Double.NaN,
        )
    }

    /**
     * A GPS coordinate in degrees from its `d/n,m/n,s/n` rationals, negative
     * for [negativeRef]; NaN when absent or malformed. Parsed here rather than
     * through `getLatLong(FloatArray)`, which rounds to a float.
     */
    private fun coordinate(exif: ExifInterface, valueTag: String, refTag: String, negativeRef: String): Double {
        val parts = exif.getAttribute(valueTag)?.split(',') ?: return Double.NaN
        if (parts.size != GPS_UNITS.size) return Double.NaN
        var degrees = 0.0
        for ((index, part) in parts.withIndex()) {
            val fraction = part.trim().split('/')
            if (fraction.size != 2) return Double.NaN
            val numerator = fraction[0].toDoubleOrNull() ?: return Double.NaN
            val denominator = fraction[1].toDoubleOrNull()?.takeIf { it != 0.0 } ?: return Double.NaN
            degrees += numerator / denominator / GPS_UNITS[index]
        }
        val negative = exif.getAttribute(refTag)?.trim().equals(negativeRef, ignoreCase = true)
        return if (negative) -degrees else degrees
    }
}
