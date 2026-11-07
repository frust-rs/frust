package dev.frust.permissions

import android.app.Activity
import android.app.Application
import android.content.Context
import android.content.Intent
import android.content.SharedPreferences
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat

/**
 * The `frust-permissions` plugin's Android host — the Kotlin half of the frozen
 * Rust↔Kotlin contract also implemented by `plugins/permissions/src/android.rs`.
 *
 * ## The contract (LAW — see `plugins/permissions/src/android.rs`)
 *
 * This object's fully-qualified name (`dev.frust.permissions.FrustPermissionsHost`)
 * and its `@JvmStatic` entry points are looked up by string from Rust through
 * the application classloader, and [nativeOnPermissionResult]'s mangled JNI
 * symbol (`Java_dev_frust_permissions_FrustPermissionsHost_nativeOnPermissionResult`)
 * is exported by that same module. **The class may never move and no signature
 * may change without a coordinated change on both sides.** The integer codes
 * below are pinned against their Rust twins (`plugins/permissions/src/codes.rs`)
 * by `plugins/permissions/tests/kotlin_contract.rs`.
 *
 * | Direction | Member | Returns |
 * |---|---|---|
 * | Rust → Kotlin | `status(kind: Int): Int` — `(I)I` | a status code, [NOT_DECLARED], or [NO_ACTIVITY] when no application context was installed |
 * | Rust → Kotlin | `request(requestId: Long, kind: Int): Int` — `(JI)I` | a final status code, [PENDING] after launching the prompt, [NO_ACTIVITY], or [NOT_DECLARED] |
 * | Rust → Kotlin | `undeclaredPermission(kind: Int): String?` — `(I)Ljava/lang/String;` | the first required permission the manifest lacks, or null |
 * | Rust → Kotlin | `openSettings(): Int` — `()I` | [SETTINGS_OPENED], [SETTINGS_FAILED], or [NO_ACTIVITY] when no application context was installed |
 * | Kotlin → Rust | `nativeOnPermissionResult(requestId: Long, code: Int)` | answers the [request] that returned [PENDING] with the same `requestId` |
 *
 * ## The answer is read, never remembered
 *
 * [GRANTED] and [LIMITED] are always derived from `checkSelfPermission` at the
 * moment of the call. Android warns that a stored permission state goes stale
 * after a permission auto-reset, app hibernation, a change in Settings, or the
 * app moving to the background, and must not be kept in `SharedPreferences`
 * (developer.android.com/about/versions/14/changes/partial-photo-video-access,
 * "Best practices"). The only thing this host stores is the asked flag below.
 *
 * ## The asked flag (a cross-plugin contract)
 *
 * Android has no "never asked" state: an unasked permission and one refused
 * for good both read as not granted with no rationale. To tell [NOT_DETERMINED]
 * from [BLOCKED] the host records, per Android permission name, whether a
 * request has ever been answered: `SharedPreferences` file [PREFS_FILE]
 * (`frust.permissions`), key [ASKED_KEY_PREFIX] + the permission name, e.g.
 * `frust.permissions.asked.android.permission.CAMERA`, value `true`. Any other
 * frust plugin that requests a runtime permission itself must set the same key
 * so this host keeps reporting its status truthfully. The flag is only an
 * "ever requested" hint: it never makes a permission read as granted.
 *
 * A prompt dismissed without an answer (tapping outside the dialog on Android
 * 11+) is not counted by the system, so it leaves the flag unset: a denied
 * permission that shows no rationale after a first-ever request, and showed
 * none before it, was dismissed rather than refused. The flag lives in app
 * data, so clearing that data or reinstalling resets it while the system's own
 * record may differ.
 *
 * ## Photos and partial access
 *
 * Photos read-write needs `READ_EXTERNAL_STORAGE` on API 29-32 and
 * `READ_MEDIA_IMAGES` + `READ_MEDIA_VIDEO` on 33+. On 34+ the user may grant
 * only a selection, reported through `READ_MEDIA_VISUAL_USER_SELECTED`
 * (added in API 34): full access is `READ_MEDIA_IMAGES` granted, [LIMITED] is
 * `READ_MEDIA_VISUAL_USER_SELECTED` granted without it. A request while
 * [LIMITED] launches the prompt again, and Android re-shows its selection UI.
 *
 * That permission applies only to an app that targets API 34+ and declares
 * it. Otherwise the system runs the app in a compatibility mode: a partial
 * selection grants `READ_MEDIA_IMAGES` / `READ_MEDIA_VIDEO` temporarily, for
 * the app session only (same page, "Compatibility mode"). This host then
 * reports [GRANTED] for that session, because that is what the system
 * reports, and the status drops back once the grant lapses. So
 * `READ_MEDIA_VISUAL_USER_SELECTED` is optional in the declaration check
 * ([OPTIONAL_DECLARATIONS]) and requested only when declared. The derivation
 * itself ([photosGrantCode]) reads grants only, never the manifest, so
 * declaring that permission (Android adds it automatically when an app
 * declares `READ_MEDIA_IMAGES`, `READ_MEDIA_VIDEO` or `ACCESS_MEDIA_LOCATION`,
 * per `Manifest.permission`) cannot change how [LIMITED] is derived.
 *
 * Photos add-only needs no permission on API 29+: an app does not need any
 * storage permission to add or modify media files it owns in `MediaStore`
 * (developer.android.com/training/data-storage/shared/media, "Access your own
 * media files"), so it always reads [GRANTED].
 *
 * ## Media location
 *
 * `ACCESS_MEDIA_LOCATION` (a dangerous runtime permission added in API 29)
 * unredacts the location stored inside photos. Android recommends asking for
 * it together with the media permissions in one operation; this API asks for
 * one kind per request, so ask for Photos first, then MediaLocation.
 *
 * ## Delivering the answer
 *
 * [request] registers a one-shot `RequestMultiplePermissions` launcher on the
 * resumed [ComponentActivity]'s `activityResultRegistry` under the key
 * `frust.permissions.<requestId>`, launches it on the main thread, and
 * unregisters it from its own callback. The consuming app never forwards
 * `onRequestPermissionsResult` (a plugin cannot subclass the app's Activity).
 * The embedding's `configChanges` keeps a rotation from recreating the
 * Activity; process death while the prompt is up loses the callback, and the
 * Rust side then times out and re-reads [status].
 */
object FrustPermissionsHost {
    private const val TAG = "frust"

    // --- Status codes: mirror `codes.rs`, pinned by `tests/kotlin_contract.rs`.

    /** Never asked. */
    const val NOT_DETERMINED = 0
    /** Granted. */
    const val GRANTED = 1
    /** Partial access (a user-selected subset of photos). */
    const val LIMITED = 2
    /** Denied; the system will still show a rationale. */
    const val DENIED = 3
    /** Denied after an ask with no rationale: only Settings can change it. */
    const val BLOCKED = 4
    /** Restricted by policy. Not produced on Android; kept for the shared table. */
    const val RESTRICTED = 5
    /** A request is outstanding; the answer arrives via [nativeOnPermissionResult]. */
    const val PENDING = 6
    /** No resumed Activity (or, for [status]/[openSettings], no application context). */
    const val NO_ACTIVITY = 7
    /** A required permission is missing from the app manifest. */
    const val NOT_DECLARED = 8

    // --- Kind codes: mirror `codes.rs`, pinned by `tests/kotlin_contract.rs`.

    /** Photo library, read and write. */
    const val PHOTOS_READ_WRITE = 0
    /** Photo library, add only. */
    const val PHOTOS_ADD_ONLY = 1
    /** Camera. */
    const val CAMERA = 2
    /** Microphone. */
    const val MICROPHONE = 3
    /** Media location. */
    const val MEDIA_LOCATION = 4

    // --- openSettings codes: mirror `src/android.rs`.

    /** [openSettings] started the Settings activity. */
    const val SETTINGS_OPENED = 0
    /** [openSettings] could not start the Settings activity. */
    const val SETTINGS_FAILED = -1

    /** Returned for a kind code this host does not know. */
    private const val UNKNOWN_KIND = -1

    // --- The asked flag (see the class doc). A cross-plugin contract.

    /** `SharedPreferences` file holding the asked flags. */
    const val PREFS_FILE = "frust.permissions"
    /** Asked-flag key prefix; the Android permission name follows it. */
    const val ASKED_KEY_PREFIX = "frust.permissions.asked."

    // --- Permission sets per kind and SDK: mirror `codes::android_permissions`,
    // pinned by `tests/kotlin_contract.rs`. One `listOf` per line.

    private val PHOTOS_READ_WRITE_SDK29 = listOf("android.permission.READ_EXTERNAL_STORAGE")
    private val PHOTOS_READ_WRITE_SDK33 = listOf("android.permission.READ_MEDIA_IMAGES", "android.permission.READ_MEDIA_VIDEO")
    private val PHOTOS_READ_WRITE_SDK34 = listOf("android.permission.READ_MEDIA_IMAGES", "android.permission.READ_MEDIA_VIDEO", "android.permission.READ_MEDIA_VISUAL_USER_SELECTED")
    private val PHOTOS_ADD_ONLY_ALL = emptyList<String>()
    private val CAMERA_ALL = listOf("android.permission.CAMERA")
    private val MICROPHONE_ALL = listOf("android.permission.RECORD_AUDIO")
    private val MEDIA_LOCATION_ALL = listOf("android.permission.ACCESS_MEDIA_LOCATION")

    private const val READ_EXTERNAL_STORAGE = "android.permission.READ_EXTERNAL_STORAGE"
    private const val READ_MEDIA_IMAGES = "android.permission.READ_MEDIA_IMAGES"
    private const val READ_MEDIA_VISUAL_USER_SELECTED = "android.permission.READ_MEDIA_VISUAL_USER_SELECTED"

    /**
     * Permissions a set may contain that the app need not declare: without
     * `READ_MEDIA_VISUAL_USER_SELECTED` the system runs the app in its photo
     * compatibility mode instead (class doc, "Photos and partial access").
     */
    private val OPTIONAL_DECLARATIONS = setOf(READ_MEDIA_VISUAL_USER_SELECTED)

    /** The application [Context], installed once by [FrustPermissionsInitProvider]. */
    @Volatile
    private var appContext: Context? = null

    /**
     * The currently resumed Activity: set in `onActivityResumed`, cleared when
     * that same instance pauses or is destroyed. A prompt needs a resumed
     * Activity to launch from, and the rationale is read against it.
     */
    @Volatile
    private var resumedActivity: Activity? = null

    /** Guards a single [Application.registerActivityLifecycleCallbacks] call. */
    @Volatile
    private var lifecycleCallbacksRegistered: Boolean = false

    /** Posts a launch onto the main thread when [request] is called off it. */
    private val mainHandler = Handler(Looper.getMainLooper())

    /** The manifest's requested permissions, read once. */
    @Volatile
    private var declared: Set<String>? = null

    /**
     * Install the application [Context] — called once at process start by
     * [FrustPermissionsInitProvider]. Idempotent and first-writer-wins.
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

    /** The current status code for [kind]. Any thread; never prompts. */
    @JvmStatic
    fun status(kind: Int): Int {
        val ctx = appContext ?: return NO_ACTIVITY
        val perms = permissionsFor(kind, Build.VERSION.SDK_INT) ?: return UNKNOWN_KIND
        if (firstUndeclared(ctx, perms) != null) return NOT_DECLARED
        return currentCode(ctx, kind, perms, resumedActivity)
    }

    /**
     * Ask for [kind]. Any thread. Returns a final code when no prompt is
     * needed ([GRANTED], [NO_ACTIVITY], [NOT_DECLARED]), else launches the
     * prompt on the main thread and returns [PENDING]; the answer then
     * arrives through [nativeOnPermissionResult] carrying [requestId].
     */
    @JvmStatic
    fun request(requestId: Long, kind: Int): Int {
        val ctx = appContext ?: return NO_ACTIVITY
        val perms = permissionsFor(kind, Build.VERSION.SDK_INT) ?: return UNKNOWN_KIND
        if (firstUndeclared(ctx, perms) != null) return NOT_DECLARED
        if (perms.isEmpty()) return GRANTED
        if (currentCode(ctx, kind, perms, resumedActivity) == GRANTED) return GRANTED
        val activity = resumedActivity as? ComponentActivity ?: return NO_ACTIVITY
        val toRequest = perms.filter { isDeclared(ctx, it) }
        if (Looper.myLooper() == Looper.getMainLooper()) {
            launch(ctx, requestId, kind, perms, toRequest, activity)
        } else {
            mainHandler.post { launch(ctx, requestId, kind, perms, toRequest, activity) }
        }
        return PENDING
    }

    /** The first permission [kind] requires that the manifest lacks, or null. */
    @JvmStatic
    fun undeclaredPermission(kind: Int): String? {
        val ctx = appContext ?: return null
        val perms = permissionsFor(kind, Build.VERSION.SDK_INT) ?: return null
        return firstUndeclared(ctx, perms)
    }

    /**
     * Open this app's page in the system Settings, from the application
     * context with `FLAG_ACTIVITY_NEW_TASK`. Any thread; fire-and-forget.
     * Android may refuse to start an activity while the app is in the
     * background; call it from a foreground interaction.
     */
    @JvmStatic
    fun openSettings(): Int {
        val ctx = appContext ?: return NO_ACTIVITY
        return try {
            val intent = Intent(
                Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                Uri.fromParts("package", ctx.packageName, null),
            ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            ctx.startActivity(intent)
            SETTINGS_OPENED
        } catch (e: Exception) {
            Log.w(TAG, "frust-permissions: openSettings failed: ${e.javaClass.name}")
            SETTINGS_FAILED
        }
    }

    // --- Kotlin -> Rust -----------------------------------------------------

    /**
     * Answers the [request] that returned [PENDING] with the same [requestId];
     * [code] is a status code, or [NO_ACTIVITY] when the prompt could not be
     * launched after all. Implemented in `plugins/permissions/src/android.rs`.
     * The native library is already loaded by the embedding's `FrustActivity`,
     * so no `System.loadLibrary` happens here.
     */
    @JvmStatic
    external fun nativeOnPermissionResult(requestId: Long, code: Int)

    // Every native call is guarded: a missing symbol (an app that depends on
    // this Gradle module without the `frust-permissions` Rust crate) must
    // degrade to a log line, never an UnsatisfiedLinkError thrown into an
    // Activity result callback.
    private fun notifyResult(requestId: Long, code: Int) {
        try {
            nativeOnPermissionResult(requestId, code)
        } catch (e: Throwable) {
            Log.w(TAG, "frust-permissions: nativeOnPermissionResult unavailable", e)
        }
    }

    // --- Internals ----------------------------------------------------------

    /** The Android permissions behind [kind] on [sdk], or null for an unknown kind. */
    internal fun permissionsFor(kind: Int, sdk: Int): List<String>? = when (kind) {
        PHOTOS_READ_WRITE -> when {
            sdk >= 34 -> PHOTOS_READ_WRITE_SDK34
            sdk == 33 -> PHOTOS_READ_WRITE_SDK33
            else -> PHOTOS_READ_WRITE_SDK29
        }
        PHOTOS_ADD_ONLY -> PHOTOS_ADD_ONLY_ALL
        CAMERA -> CAMERA_ALL
        MICROPHONE -> MICROPHONE_ALL
        MEDIA_LOCATION -> MEDIA_LOCATION_ALL
        else -> null
    }

    /**
     * The photo library's grant code from live grant state alone: [GRANTED]
     * for full read access, [LIMITED] for a user selection on 34+, null when
     * neither is held. Reads no declaration, so what the manifest declares
     * cannot change it (class doc, "Photos and partial access").
     */
    internal fun photosGrantCode(sdk: Int, fullRead: Boolean, userSelected: Boolean): Int? = when {
        fullRead -> GRANTED
        sdk >= 34 && userSelected -> LIMITED
        else -> null
    }

    /** The status code for [kind] right now, derived from live grants. */
    private fun currentCode(ctx: Context, kind: Int, perms: List<String>, activity: Activity?): Int {
        if (perms.isEmpty()) return GRANTED
        val sdk = Build.VERSION.SDK_INT
        if (kind == PHOTOS_READ_WRITE) {
            val full = if (sdk >= 33) READ_MEDIA_IMAGES else READ_EXTERNAL_STORAGE
            val selected = sdk >= 34 && isGranted(ctx, READ_MEDIA_VISUAL_USER_SELECTED)
            photosGrantCode(sdk, isGranted(ctx, full), selected)?.let { return it }
        } else if (perms.all { isGranted(ctx, it) }) {
            return GRANTED
        }
        if (perms.none { wasAsked(ctx, it) }) return NOT_DETERMINED
        if (activity == null) return DENIED
        return if (perms.any { rationale(activity, it) }) DENIED else BLOCKED
    }

    /** Register, launch and answer one prompt. Main thread. */
    private fun launch(
        ctx: Context,
        requestId: Long,
        kind: Int,
        perms: List<String>,
        toRequest: List<String>,
        activity: ComponentActivity,
    ) {
        if (activity.isFinishing || activity.isDestroyed) {
            notifyResult(requestId, NO_ACTIVITY)
            return
        }
        val before = toRequest.associateWith { RequestState(wasAsked(ctx, it), rationale(activity, it)) }
        var launcher: ActivityResultLauncher<Array<String>>? = null
        try {
            val registered = activity.activityResultRegistry.register(
                "frust.permissions.$requestId",
                ActivityResultContracts.RequestMultiplePermissions(),
            ) { results ->
                launcher?.unregister()
                recordAnswers(ctx, activity, results, before)
                notifyResult(requestId, currentCode(ctx, kind, perms, activity))
            }
            launcher = registered
            registered.launch(toRequest.toTypedArray())
        } catch (e: Exception) {
            launcher?.unregister()
            Log.w(TAG, "frust-permissions: launching the prompt failed: ${e.javaClass.name}")
            notifyResult(requestId, currentCode(ctx, kind, perms, activity))
        }
    }

    /** One permission's asked flag and rationale just before its prompt. */
    private data class RequestState(val asked: Boolean, val rationale: Boolean)

    /**
     * Set the asked flag for every permission the system answered. An empty
     * result (the request was cancelled, e.g. another prompt was showing)
     * records nothing. A first-ever denial with no rationale before or after
     * is a dismissed prompt and is not recorded (class doc).
     */
    private fun recordAnswers(
        ctx: Context,
        activity: Activity,
        results: Map<String, Boolean>,
        before: Map<String, RequestState>,
    ) {
        for ((perm, granted) in results) {
            val prior = before[perm] ?: continue
            val dismissed = !granted && !prior.asked && !prior.rationale && !rationale(activity, perm)
            if (!dismissed) markAsked(ctx, perm)
        }
    }

    private fun isGranted(ctx: Context, perm: String): Boolean =
        ContextCompat.checkSelfPermission(ctx, perm) == PackageManager.PERMISSION_GRANTED

    private fun rationale(activity: Activity, perm: String): Boolean =
        ActivityCompat.shouldShowRequestPermissionRationale(activity, perm)

    private fun prefs(ctx: Context): SharedPreferences =
        ctx.getSharedPreferences(PREFS_FILE, Context.MODE_PRIVATE)

    private fun wasAsked(ctx: Context, perm: String): Boolean =
        prefs(ctx).getBoolean(ASKED_KEY_PREFIX + perm, false)

    private fun markAsked(ctx: Context, perm: String) {
        prefs(ctx).edit().putBoolean(ASKED_KEY_PREFIX + perm, true).apply()
    }

    /** The first non-optional entry of [perms] missing from the manifest, or null. */
    private fun firstUndeclared(ctx: Context, perms: List<String>): String? =
        perms.firstOrNull { it !in OPTIONAL_DECLARATIONS && !isDeclared(ctx, it) }

    private fun isDeclared(ctx: Context, perm: String): Boolean = perm in declaredPermissions(ctx)

    /**
     * The manifest's `<uses-permission>` names, from
     * `PackageManager.getPackageInfo(packageName, GET_PERMISSIONS)`. Read once:
     * the manifest cannot change while the process lives.
     */
    private fun declaredPermissions(ctx: Context): Set<String> {
        declared?.let { return it }
        val pm = ctx.packageManager
        val info = try {
            if (Build.VERSION.SDK_INT >= 33) {
                pm.getPackageInfo(
                    ctx.packageName,
                    PackageManager.PackageInfoFlags.of(PackageManager.GET_PERMISSIONS.toLong()),
                )
            } else {
                @Suppress("DEPRECATION")
                pm.getPackageInfo(ctx.packageName, PackageManager.GET_PERMISSIONS)
            }
        } catch (e: PackageManager.NameNotFoundException) {
            Log.w(TAG, "frust-permissions: own package info unavailable")
            null
        }
        val names = info?.requestedPermissions?.toSet() ?: emptySet()
        declared = names
        return names
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
            }
        }
}
