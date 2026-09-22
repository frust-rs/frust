package dev.frust.authsession

import android.content.ContentProvider
import android.content.ContentValues
import android.database.Cursor
import android.net.Uri

/**
 * Installs the application [android.content.Context] into [FrustAuthSessionHost]
 * at process start.
 *
 * A `ContentProvider` declared in this module's own `AndroidManifest.xml` is
 * created by the system before `Application.onCreate` — the standard
 * self-initialization mechanism for an Android library that needs a `Context`
 * with no app-side code (`androidx.startup`'s `InitializationProvider`,
 * Firebase's `FirebaseInitProvider`; the same shape `plugins/iap`'s
 * `FrustIapInitProvider` and `plugins/camera`'s `FrustCameraInitProvider` use).
 *
 * It provides no data: every `ContentProvider` operation returns null/0. It is
 * `exported="false"` and its authority is `${applicationId}`-scoped, so nothing
 * outside the app can reach it.
 */
class FrustAuthSessionInitProvider : ContentProvider() {
    override fun onCreate(): Boolean {
        context?.let { FrustAuthSessionHost.installApplicationContext(it.applicationContext) }
        return true
    }

    override fun query(
        uri: Uri,
        projection: Array<out String>?,
        selection: String?,
        selectionArgs: Array<out String>?,
        sortOrder: String?,
    ): Cursor? = null

    override fun getType(uri: Uri): String? = null

    override fun insert(uri: Uri, values: ContentValues?): Uri? = null

    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int = 0

    override fun update(
        uri: Uri,
        values: ContentValues?,
        selection: String?,
        selectionArgs: Array<out String>?,
    ): Int = 0
}
