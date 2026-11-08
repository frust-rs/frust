package dev.frust.share

import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import java.io.File
import java.util.UUID

/**
 * Shares files through the system chooser using content URIs.
 *
 * The Rust side (`plugins/share/src/android.rs`) loads this class through the
 * application classloader and calls [shareFiles]. Each file is first copied
 * into `cacheDir/frust-share/<unique>/<name>`; only that directory is exposed
 * by [FrustShareFileProvider], and the chooser receives read grants on the
 * copies, never on the originals.
 */
object FrustShareHost {
    const val OK = 0
    const val ERR_INVALID_ARGUMENT = 1
    const val ERR_COPY_FAILED = 2
    const val ERR_LAUNCH_FAILED = 3

    private const val STAGING_DIR = "frust-share"
    private const val MAX_AGE_MS = 24L * 60 * 60 * 1000

    @JvmStatic
    fun shareFiles(context: Context, paths: Array<String>, subject: String?): Int {
        if (paths.isEmpty()) return ERR_INVALID_ARGUMENT
        return try {
            val root = File(context.cacheDir, STAGING_DIR)
            prune(root)

            val authority = context.packageName + ".frustshare.fileprovider"
            val uris = ArrayList<Uri>(paths.size)
            val mimes = ArrayList<String>(paths.size)
            for (path in paths) {
                val source = File(path)
                if (!source.isFile) return ERR_COPY_FAILED
                val dir = File(root, UUID.randomUUID().toString())
                if (!dir.mkdirs()) return ERR_COPY_FAILED
                val copy = File(dir, source.name)
                source.copyTo(copy)
                uris.add(FileProvider.getUriForFile(context, authority, copy))
                mimes.add(mimeOf(copy.name))
            }

            val send = Intent(
                if (uris.size == 1) Intent.ACTION_SEND else Intent.ACTION_SEND_MULTIPLE
            )
            send.type = commonMime(mimes)
            if (uris.size == 1) {
                send.putExtra(Intent.EXTRA_STREAM, uris[0])
            } else {
                send.putParcelableArrayListExtra(Intent.EXTRA_STREAM, uris)
            }
            val clip = ClipData.newRawUri(null, uris[0])
            for (i in 1 until uris.size) clip.addItem(ClipData.Item(uris[i]))
            send.clipData = clip
            send.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            if (subject != null) send.putExtra(Intent.EXTRA_SUBJECT, subject)

            val chooser = Intent.createChooser(send, null)
            chooser.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            context.startActivity(chooser)
            OK
        } catch (e: java.io.IOException) {
            ERR_COPY_FAILED
        } catch (e: IllegalArgumentException) {
            ERR_INVALID_ARGUMENT
        } catch (e: RuntimeException) {
            ERR_LAUNCH_FAILED
        }
    }

    private fun mimeOf(name: String): String {
        val ext = name.substringAfterLast('.', "").lowercase()
        return MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "application/octet-stream"
    }

    // The shared MIME type; a wildcard subtype when only the top-level type agrees, else all.
    private fun commonMime(mimes: List<String>): String {
        val first = mimes[0]
        if (mimes.all { it == first }) return first
        val type = first.substringBefore('/')
        return if (mimes.all { it.substringBefore('/') == type }) "$type/*" else "*/*"
    }

    /** Delete staging subdirectories older than [MAX_AGE_MS]. */
    private fun prune(root: File) {
        val cutoff = System.currentTimeMillis() - MAX_AGE_MS
        root.listFiles()?.forEach { dir ->
            if (dir.lastModified() < cutoff) dir.deleteRecursively()
        }
    }
}
