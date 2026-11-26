package dev.frust.authsession

import android.app.Activity
import android.app.Application
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.result.ActivityResultLauncher
import androidx.browser.auth.AuthTabIntent
import androidx.browser.customtabs.CustomTabsClient
import androidx.browser.customtabs.CustomTabsIntent

/**
 * Well-known Custom Tabs-capable browsers `FrustAuthSessionHost.customTabsProvider`
 * is willing to launch a session in, in preference order. See that method's KDoc for why
 * this list exists and how it is used. Kept top-level rather than a member of
 * [FrustAuthSessionHost] — alongside [chooseCustomTabsProvider], the pure
 * function that reads it — so that neither can be reached from a test without
 * also loading [FrustAuthSessionHost]'s own class initializer (which touches
 * `Looper.getMainLooper()`, unavailable outside a real Android runtime or
 * Robolectric).
 */
private val CUSTOM_TABS_PROVIDER_ALLOWLIST: List<String> = listOf(
    "com.android.chrome",
    "com.chrome.beta",
    "com.chrome.dev",
    "com.chrome.canary",
    "org.chromium.chrome",
    "org.mozilla.firefox",
    "org.mozilla.firefox_beta",
    "org.mozilla.fenix",
    "org.mozilla.fennec_fdroid",
    "com.brave.browser",
    "com.sec.android.app.sbrowser",
    "com.microsoft.emmx",
    "com.vivaldi.browser",
    "com.duckduckgo.mobile.android",
)

/**
 * The pure decision half of `FrustAuthSessionHost.customTabsProvider`'s
 * three-step policy (see that method's KDoc for the full policy and its
 * rationale) — extracted to a top-level function, not a member of
 * [FrustAuthSessionHost], so `ProviderSelectionTest` can drive every branch
 * on the JVM with no `Activity`/`PackageManager`/`CustomTabsClient` and no
 * Robolectric: calling a member of the `object` would force its class
 * initializer to run first (it eagerly constructs a `Handler` against
 * `Looper.getMainLooper()`), which throws under a plain JVM unit test.
 * [answersCustomTabs] stands in for
 * `CustomTabsClient.getPackageName(context, packages, ignoreDefault)`;
 * `customTabsProvider` injects the real call, a test injects a fake.
 *
 * `internal`, not `private`: Kotlin's `internal` is module-scoped (see
 * `docs/PLUGINS_CODE_STANDARDS.md`), and the test source set is part of
 * this same Gradle module, so it can call this function directly.
 */
internal fun chooseCustomTabsProvider(
    defaultPackage: String?,
    installedBrowsers: Set<String>,
    answersCustomTabs: (List<String>, Boolean) -> String?,
): String? {
    if (defaultPackage != null && answersCustomTabs(listOf(defaultPackage), false) == defaultPackage) {
        return defaultPackage
    }
    val candidates = CUSTOM_TABS_PROVIDER_ALLOWLIST.filter { it in installedBrowsers }
    val chosen = answersCustomTabs(candidates, true)
    // Never pin a package outside the allow-list, even if a future
    // `getPackageName` implementation were to return one.
    return if (chosen != null && chosen in CUSTOM_TABS_PROVIDER_ALLOWLIST) chosen else null
}

/** How [FrustAuthSessionHost] presents one session — see [chooseLaunchMode]. */
internal enum class LaunchMode {
    /** `androidx.browser.auth.AuthTabIntent`: the redirect returns as an Activity result. */
    AUTH_TAB,

    /** A Custom Tab: the redirect returns as a deep link read on the next resume. */
    CUSTOM_TABS,

    /** Neither can catch the requested redirect (an https one without Auth Tab). */
    UNSUPPORTED,
}

/**
 * Chooses between Auth Tab and a Custom Tab for one session — pure, and
 * top-level for the same JVM-test reason as [chooseCustomTabsProvider]
 * (`ProviderSelectionTest` drives it).
 *
 * Auth Tab is used whenever it can be: [provider] (the package
 * [chooseCustomTabsProvider] picked, or `null` for none) answers
 * [authTabSupported] (`CustomTabsClient.isAuthTabSupported`) and the
 * launching Activity is a `ComponentActivity` ([componentActivity]), whose
 * `activityResultRegistry` carries the result back. Otherwise a custom-scheme
 * session takes today's Custom Tabs path unchanged, and an https one
 * ([httpsCallback]) is [LaunchMode.UNSUPPORTED]: only Auth Tab can catch an
 * https redirect on Android.
 */
internal fun chooseLaunchMode(
    httpsCallback: Boolean,
    provider: String?,
    componentActivity: Boolean,
    authTabSupported: (String) -> Boolean,
): LaunchMode {
    if (provider != null && componentActivity && authTabSupported(provider)) return LaunchMode.AUTH_TAB
    return if (httpsCallback) LaunchMode.UNSUPPORTED else LaunchMode.CUSTOM_TABS
}

// `nativeOnAuthSessionResult` kinds — the table in `FrustAuthSessionHost.nativeOnAuthSessionResult`'s
// KDoc and `plugins/auth-session/src/android.rs`'s `RESULT_*` constants; change all three together.

/** The redirect arrived; the URL rides along. */
internal const val KIND_CALLBACK = 0

/** The user dismissed the session. */
internal const val KIND_CANCELLED = 1

/** Posted launch only: no browser could take the intent. */
internal const val KIND_NO_BROWSER = 2

/** Posted launch only: any other launch failure. */
internal const val KIND_LAUNCH_FAILED = 3

/** Posted launch only: no resumed Activity. */
internal const val KIND_NO_RESUMED_ACTIVITY = 4

/** Posted launch only: an https redirect, but the provider offers no Auth Tab. */
internal const val KIND_HTTPS_UNSUPPORTED = 5

/** Auth Tab: the browser could not verify the app's https redirect (Digital Asset Links). */
internal const val KIND_AUTH_TAB_VERIFICATION_FAILED = 6

/** Auth Tab: verifying the app's https redirect timed out. */
internal const val KIND_AUTH_TAB_VERIFICATION_TIMED_OUT = 7

/** Auth Tab: a result code this host does not know. */
internal const val KIND_AUTH_TAB_UNKNOWN_RESULT = 8

/**
 * Maps an `AuthTabIntent.AuthResult.resultCode` onto a
 * `nativeOnAuthSessionResult` kind — pure, top-level and JVM-tested
 * (`AuthTabResultTest`). `RESULT_OK` is a callback (the URI travels
 * separately; one missing is refused on the Rust side), `RESULT_CANCELED` a
 * cancel, the two verification codes and `RESULT_UNKNOWN_CODE` (or anything
 * newer) a distinct platform error each.
 */
internal fun authTabResultKind(resultCode: Int): Int = when (resultCode) {
    AuthTabIntent.RESULT_OK -> KIND_CALLBACK
    AuthTabIntent.RESULT_CANCELED -> KIND_CANCELLED
    AuthTabIntent.RESULT_VERIFICATION_FAILED -> KIND_AUTH_TAB_VERIFICATION_FAILED
    AuthTabIntent.RESULT_VERIFICATION_TIMED_OUT -> KIND_AUTH_TAB_VERIFICATION_TIMED_OUT
    else -> KIND_AUTH_TAB_UNKNOWN_RESULT
}

/**
 * The `frust-auth-session` plugin's Android host — the Kotlin half of the
 * frozen Rust↔Kotlin contract also implemented by
 * `plugins/auth-session/src/android.rs`. It presents a session in Auth Tab
 * when the chosen browser offers it, and in a Custom Tab otherwise.
 *
 * ## The contract (LAW — see `plugins/auth-session/src/android.rs`)
 *
 * This class's fully-qualified name (`dev.frust.authsession.FrustAuthSessionHost`)
 * and the names of its `@JvmStatic` entry points ([start], [startHttps],
 * [httpsCallbackSupported], [cancel]) are looked up by string from the Rust
 * side through the application classloader, and
 * [nativeOnAuthSessionResult]'s mangled JNI symbol
 * (`Java_dev_frust_authsession_FrustAuthSessionHost_nativeOnAuthSessionResult`)
 * is exported by that same module — **this class may never move, and no
 * signature may change, without a coordinated change on both sides.**
 *
 * ## Why an Activity is needed
 *
 * Both tabs are launched from a live Activity: they need a task to animate
 * into and a back stack to return to. There is no `Application`-scoped way to
 * open one — unlike this plugin's application `Context` (installed by
 * [FrustAuthSessionInitProvider] before any Activity exists), a session
 * cannot start until one is resumed, hence [start]'s "no resumed Activity"
 * outcome.
 *
 * ## Auth Tab or Custom Tab
 *
 * Each launch picks a browser ([customTabsProvider]) and then asks
 * [chooseLaunchMode]: when that browser answers
 * `CustomTabsClient.isAuthTabSupported` and the resumed Activity is a
 * `ComponentActivity`, the session runs in Auth Tab, for either redirect
 * form; otherwise a custom-scheme session runs in a Custom Tab exactly as
 * before, and an https one is refused (code `5`).
 *
 * ## Auth Tab: the result comes from the launcher, never from a resume
 *
 * The launcher is registered at launch time on the launching Activity's
 * `activityResultRegistry` under the key `frust.authsession.<generation>`
 * (the plugin tier's Activity Result rule — no registration before
 * `onCreate`, no shell change). Its callback is the only thing that resolves
 * an Auth Tab session: [onActivityResumed] neither reads a redirect nor
 * reports Cancelled for one, so leaving the app and coming back while the tab
 * is open changes nothing. The callback checks that the session it was
 * registered for is still [pending] (cancel, or a newer session, makes it a
 * no-op) and maps the result with [authTabResultKind].
 *
 * The launching Activity can go away while the tab is open:
 *
 * - **Recreated** (a configuration change, or the system destroying it while
 *   it keeps its saved state): its registry saved the key with its request
 *   code, so [onActivityPostCreated] registers the same key on the new
 *   instance (same task, same class, restored state) and the result is
 *   delivered there.
 * - **Finishing**: the result can never arrive, so the session is resolved
 *   Cancelled at once.
 * - **Process death**: nothing pending survives; a result reaching the
 *   restored Activity finds no registration and is dropped by its registry.
 *   No callback, no deep link.
 *
 * Only the launching instance's registry ever holds the launcher, so a
 * second Activity instance never sees or resolves the session. [pending]
 * drops its references to an Activity when that Activity is destroyed.
 *
 * ## Custom Tab: why the outcome is read from `activity.intent`
 *
 * The OAuth/OIDC redirect back into the app arrives as a deep link: the
 * system delivers it to the embedding's `FrustActivity`, whose
 * `onNewIntent` calls `setIntent(intent)` (see `FrustActivity.onNewIntent`,
 * `platform/android/frust-embedding`) precisely so the Activity's own
 * `intent` field reflects the redirect rather than the original launch
 * intent. Combined with `singleTop` launch semantics, that means the redirect
 * never spawns a new Activity instance — it re-delivers into the same one via
 * `onNewIntent`, which resumes the Activity. [onActivityResumed] is therefore
 * the one place this host can observe the redirect, by comparing the
 * Activity's current `intent` against the intent that was live when the
 * Custom Tab was launched.
 *
 * ## The one-delivery rule
 *
 * [pending] is cleared *before* [nativeOnAuthSessionResult] is called, so a
 * second resume with the same (already-consumed) intent, a second launcher
 * result, or any of them once no session is pending, cannot re-deliver a
 * result. At most one native callback answers each start: exactly one,
 * unless [cancel] reaches that session first (clearing [pending], or
 * stopping its launch before it runs), in which case the native side has
 * already resolved that session itself and none is sent.
 *
 * ## Cancelling a session
 *
 * [cancel] resolves nothing itself (the native side already has) and closes
 * nothing: Android has no API to close either tab, so **the browser stays
 * in front until the user leaves it**. A Custom Tab's later redirect finds no
 * pending session; an Auth Tab's launcher is unregistered, so its later
 * result is dropped by the Activity's registry. What [cancel] guarantees,
 * whatever thread it and the start are called on:
 *
 * - **A launch that has not run yet never opens a tab.** [cancel] records the
 *   generation in [cancelledGenerations] at once, on the calling thread, and
 *   a [launch] for a recorded generation returns without launching.
 * - **The cancel always runs after that session's launch.** [cancelOnMain] is
 *   always posted to [mainHandler] — never run inline, even on the main
 *   thread — and the native side can only call [cancel] after the start has
 *   returned, by which time a posted launch is already queued ahead of it on
 *   the same handler. So [cancelOnMain] clears any [pending] its own launch
 *   set, and it is also what removes the generation from
 *   [cancelledGenerations] again: every recorded generation is removed by its
 *   own posted [cancelOnMain], so the set never holds more than the cancels
 *   still waiting in the queue.
 * - **A cancel never touches another session.** Both the skip and the clear
 *   match on the generation only.
 *
 * ## The double-delivery note
 *
 * A Custom Tab's redirect intent is the *same* `Intent` instance the
 * embedding's `FrustSurfaceView.onDeepLink` also observes (via
 * `FrustActivity.onNewIntent`) as the framework's own deep-link source — a
 * frust app's ordinary deep-link handling sees it too. That is by design:
 * this host answers the pending Rust `AuthSession` future, while the
 * framework-level deep-link path is free to react to the same URL for
 * navigation/routing. An Auth Tab redirect returns as an Activity result
 * instead and is not a deep link.
 *
 * ## No `<uses-permission>` needed
 *
 * Both launches are a plain `startActivity`/`startActivityForResult` under
 * the hood — launching another app's Activity needs no permission, and this
 * host never binds to a `CustomTabsService` directly (see
 * `AndroidManifest.xml`'s `<queries>` comment for the one manifest element
 * this module does need).
 */
object FrustAuthSessionHost {
    private const val TAG = "frust"

    /** Return code: the launch was accepted. */
    private const val START_ACCEPTED = 0

    /** Return code: no resumed Activity. */
    private const val START_NO_RESUMED_ACTIVITY = 1

    /** Return code: [ActivityNotFoundException]. */
    private const val START_NO_BROWSER = 2

    /** Return code: any other exception. */
    private const val START_LAUNCH_FAILED = 3

    /** Return code: an https redirect, but the provider offers no Auth Tab. */
    private const val START_HTTPS_UNSUPPORTED = 5

    /** Prefix of the `activityResultRegistry` key an Auth Tab launcher is registered under. */
    private const val AUTH_TAB_KEY_PREFIX = "frust.authsession."

    /**
     * The application [Context], installed once by
     * [FrustAuthSessionInitProvider] at process start. `@Volatile`: read from
     * whatever thread Rust calls [start] on, written from the main thread.
     */
    @Volatile
    private var appContext: Context? = null

    /**
     * The currently resumed Activity, learned from the process-wide lifecycle
     * callbacks registered in [ensureLifecycleCallbacks] — a session needs one
     * to launch from, and this plugin mounts no view of its own to discover
     * one through. Cleared only in [onActivityDestroyed] of that same
     * instance, never merely between resumes: clearing it while the app is
     * backgrounded (e.g. while the tab itself is in front) would turn a
     * pending session's return trip into a spurious "no resumed Activity"
     * state. `@Volatile` for the same cross-thread reason as [appContext].
     */
    @Volatile
    private var activity: Activity? = null

    /** Guards a single [Application.registerActivityLifecycleCallbacks] call. */
    private var lifecycleCallbacksRegistered: Boolean = false

    /**
     * Posts a queued start and every [cancel] onto the main thread; see
     * [start]'s own doc and the class doc's "Cancelling a session".
     */
    private val mainHandler = Handler(Looper.getMainLooper())

    /**
     * The one session this host is waiting on, or `null` when none is in
     * flight. Set by a successful [launch], cleared the instant its answer is
     * delivered (or found impossible), or by [cancelOnMain] for its own
     * generation — see the class doc's one-delivery rule. Main-thread only.
     */
    private var pending: Pending? = null

    /**
     * The most recently consumed redirect [Intent], kept so a later resume
     * with the very same (already-answered) intent is never mistaken for a
     * second redirect — the Activity's `intent` field stays set to it until
     * the next `onNewIntent`/relaunch.
     */
    private var consumedIntent: Intent? = null

    /**
     * Generations [cancel] was called for whose posted [cancelOnMain] has not
     * run yet — read by [launch] so a launch queued before the cancel opens no
     * tab (the class doc's "Cancelling a session"). Written from any thread,
     * so every access holds the set's own monitor.
     */
    private val cancelledGenerations = HashSet<Long>()

    /** The redirect a session waits for. */
    private sealed class Redirect {
        data class Scheme(val scheme: String) : Redirect()

        data class Https(val host: String, val path: String) : Redirect()
    }

    /**
     * One in-flight session and its [generation] (echoed back on every
     * [nativeOnAuthSessionResult] call this session can produce).
     */
    private sealed class Pending {
        abstract val generation: Long
    }

    /**
     * A Custom Tab session: the callback URL scheme it is waiting for, and the
     * Activity's `intent` at the moment the tab was launched (never itself
     * mistaken for the redirect).
     */
    private class CustomTabPending(
        override val generation: Long,
        val callbackScheme: String,
        val launchIntent: Intent?,
    ) : Pending()

    /**
     * An Auth Tab session: its registry [key], and the task and class of the
     * launching Activity so a recreated instance can be recognised. [owner]
     * and [launcher] are the Activity whose registry currently holds the
     * registration and that registration; both are dropped when that
     * Activity is destroyed, so nothing here outlives it.
     */
    private class AuthTabPending(
        override val generation: Long,
        val key: String,
        val taskId: Int,
        val activityClass: String,
    ) : Pending() {
        var owner: ComponentActivity? = null
        var launcher: ActivityResultLauncher<Intent>? = null
    }

    /**
     * Install the application [Context] — called once at process start by
     * [FrustAuthSessionInitProvider], before any Activity or view exists.
     * Idempotent and first-writer-wins, matching `FrustIapHost`'s and
     * `FrustCameraHost`'s `installApplicationContext` contract in the sibling
     * plugins (no direct reference — this module never depends on them).
     */
    internal fun installApplicationContext(context: Context) {
        if (appContext == null) appContext = context
        ensureLifecycleCallbacks(context)
    }

    /**
     * Register the process-wide [Application.ActivityLifecycleCallbacks] once.
     *
     * Registering them from the init provider rather than from a view seam is
     * what lets a session find an Activity at all: this plugin mounts no view
     * of its own, so there is no other moment at which one becomes
     * observable.
     *
     * [Context.getApplicationContext] is the `Application` instance on every
     * supported API level; the `as?` keeps a non-`Application` context (a test
     * double, an oddly-wrapped host) from throwing at process start.
     */
    private fun ensureLifecycleCallbacks(context: Context) {
        if (lifecycleCallbacksRegistered) return
        val app = context.applicationContext as? Application ?: return
        lifecycleCallbacksRegistered = true
        app.registerActivityLifecycleCallbacks(activityCallbacks)
    }

    /**
     * Start a session that completes on a redirect to the custom scheme
     * [callbackScheme]. Called from any thread by Rust.
     *
     * [generation] identifies this session to the native side: every
     * [nativeOnAuthSessionResult] call this session can produce echoes it
     * back unchanged, so Rust can match a result to the start that produced
     * it even if a later session starts before this one resolves.
     *
     * When already running on the main thread, the launch happens
     * synchronously and this call's own outcome is the return value. When
     * called from any other thread, the launch is posted to the main thread
     * via [mainHandler] and this call returns `0` immediately — the eventual
     * outcome then arrives through [nativeOnAuthSessionResult]: the session's
     * answer later, or a failed deferred launch at once from the posted
     * runnable as kind `2`, `3`, `4` or `5` (`4` stands in for return code
     * `1`, because kind `1` means "cancelled" on the native side).
     *
     * Return codes:
     * - `0` — accepted (the tab was launched; the answer arrives later via
     *   [nativeOnAuthSessionResult])
     * - `1` — no resumed Activity
     * - `2` — [ActivityNotFoundException] (no browser installed)
     * - `3` — any other exception launching the tab (only the exception's
     *   class name is ever recorded — never [Throwable.message], which would
     *   contain the URL)
     * - `5` — [startHttps] only: the browser offers no Auth Tab, which an
     *   https redirect needs
     */
    @JvmStatic
    fun start(url: String, callbackScheme: String, ephemeral: Boolean, generation: Long): Int =
        startSession(url, Redirect.Scheme(callbackScheme), ephemeral, generation)

    /**
     * Start a session that completes on a redirect to
     * `https://<host><path>` — Auth Tab only, so the browser checks the app's
     * Digital Asset Links for [host]. Called from any thread by Rust; same
     * threading, [generation] and return codes as [start], including `5`
     * when the chosen browser offers no Auth Tab.
     */
    @JvmStatic
    fun startHttps(url: String, host: String, path: String, ephemeral: Boolean, generation: Long): Int =
        startSession(url, Redirect.Https(host, path), ephemeral, generation)

    /**
     * Whether a [startHttps] made now would run: the browser [launch] would
     * pick offers Auth Tab and the resumed Activity (if one is known yet) is
     * a `ComponentActivity`. Called from any thread by Rust before it claims
     * the session slot; a `false` makes Rust refuse the request without
     * starting anything. Any failure answers `false`.
     */
    @JvmStatic
    fun httpsCallbackSupported(): Boolean {
        val act = activity
        val ctx: Context = act ?: appContext ?: return false
        return try {
            val provider = customTabsProvider(ctx)
            chooseLaunchMode(true, provider, act?.let { it is ComponentActivity } ?: true) {
                CustomTabsClient.isAuthTabSupported(ctx, it)
            } == LaunchMode.AUTH_TAB
        } catch (e: Exception) {
            Log.w(TAG, "frust-auth-session: Auth Tab support check failed (${e.javaClass.name})")
            false
        }
    }

    /** [start]/[startHttps]'s shared main-thread or posted launch. */
    private fun startSession(url: String, redirect: Redirect, ephemeral: Boolean, generation: Long): Int {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            return launch(url, redirect, ephemeral, generation)
        }
        mainHandler.post {
            val result = launch(url, redirect, ephemeral, generation)
            // A deferred launch that fails must still resolve the Rust future:
            // `1` (no resumed Activity) is a synchronous return code, so on the
            // posted path it is reported as kind `4` — `1` as a *kind* means
            // "cancelled" and must never be sent here. Every other failure
            // code is the same number as its kind.
            when (result) {
                START_ACCEPTED -> Unit
                START_NO_RESUMED_ACTIVITY -> nativeOnAuthSessionResult(generation, KIND_NO_RESUMED_ACTIVITY, null)
                else -> nativeOnAuthSessionResult(generation, result, null)
            }
        }
        return START_ACCEPTED
    }

    /**
     * Cancel the session [generation] names. Called from any thread by Rust
     * (`plugins/auth-session/src/android.rs`'s `cancel`), after the native
     * side has already resolved that session as cancelled — so this host
     * never calls [nativeOnAuthSessionResult] for it.
     *
     * Records [generation] in [cancelledGenerations] at once, so a [launch]
     * still queued for it opens no tab, then posts [cancelOnMain] through
     * [mainHandler] — always posted, even from the main thread, so it runs
     * after any launch already queued there (the class doc's "Cancelling a
     * session"). Opens and closes nothing: the browser stays in front until
     * the user leaves it.
     */
    @JvmStatic
    fun cancel(generation: Long) {
        synchronized(cancelledGenerations) { cancelledGenerations.add(generation) }
        if (!mainHandler.post { cancelOnMain(generation) }) {
            // Only a quitting main looper refuses a post, and nothing launches
            // after that either: drop the record rather than keep it.
            synchronized(cancelledGenerations) { cancelledGenerations.remove(generation) }
        }
    }

    /**
     * Clears [pending] only when it is still [generation]'s, so a later
     * resume, a late redirect or a late Auth Tab result can neither resolve
     * nor cancel anything for the cancelled session, and a cancel for an old
     * generation can never clear a newer session's [pending]. An Auth Tab
     * launcher is unregistered on the way. Also forgets [generation] in
     * [cancelledGenerations]: any [launch] for it was queued ahead of this
     * call and has already run. Must run on the main thread.
     */
    private fun cancelOnMain(generation: Long) {
        synchronized(cancelledGenerations) { cancelledGenerations.remove(generation) }
        if (pending?.generation == generation) clearPending()
    }

    /** Forget [pending], unregistering an Auth Tab launcher. Main thread. */
    private fun clearPending() {
        (pending as? AuthTabPending)?.let(::unbind)
        pending = null
    }

    /**
     * Makes [next] the pending session. A previous Auth Tab session still
     * recorded here was already resolved on the native side (a cancel whose
     * [cancelOnMain] has not run yet), so its launcher is unregistered.
     */
    private fun setPending(next: Pending) {
        (pending as? AuthTabPending)?.let(::unbind)
        pending = next
    }

    /**
     * Launches the session in Auth Tab or a Custom Tab ([chooseLaunchMode]).
     * Must run on the main thread — both [startSession] call sites guarantee
     * that. Returns the code [start] documents.
     *
     * [pending] is assigned only immediately before the tab is launched
     * (never earlier — a provider-selection failure above must not leave a
     * stale session behind) and is rolled back by both `catch` arms below,
     * so a launch that fails never leaves a pending session that will never
     * resolve.
     *
     * A [generation] in [cancelledGenerations] returns `0` without launching
     * anything: the native side has already resolved that session as
     * cancelled, so there is no tab to open and no result to report.
     */
    private fun launch(url: String, redirect: Redirect, ephemeral: Boolean, generation: Long): Int {
        if (synchronized(cancelledGenerations) { generation in cancelledGenerations }) return START_ACCEPTED
        val act = activity ?: return START_NO_RESUMED_ACTIVITY
        return try {
            val provider = customTabsProvider(act)
            val mode = chooseLaunchMode(redirect is Redirect.Https, provider, act is ComponentActivity) {
                CustomTabsClient.isAuthTabSupported(act, it)
            }
            when {
                mode == LaunchMode.AUTH_TAB && provider != null && act is ComponentActivity ->
                    launchAuthTab(act, provider, url, redirect, ephemeral, generation)
                redirect is Redirect.Scheme ->
                    launchCustomTab(act, provider, url, redirect.scheme, ephemeral, generation)
                else -> {
                    Log.i(TAG, "frust-auth-session: an https redirect needs Auth Tab, which no browser offers")
                    START_HTTPS_UNSUPPORTED
                }
            }
        } catch (e: ActivityNotFoundException) {
            // Never log the exception itself: its message carries the launch
            // Intent, i.e. the authorization URL.
            if (pending?.generation == generation) clearPending()
            Log.e(TAG, "frust-auth-session: no browser available to launch the session (${e.javaClass.name})")
            START_NO_BROWSER
        } catch (e: Exception) {
            if (pending?.generation == generation) clearPending()
            Log.e(TAG, "frust-auth-session: launching the session failed (${e.javaClass.name})")
            START_LAUNCH_FAILED
        }
    }

    /** Today's Custom Tabs launch, pinned to [provider] when there is one. */
    private fun launchCustomTab(
        act: Activity,
        provider: String?,
        url: String,
        callbackScheme: String,
        ephemeral: Boolean,
        generation: Long,
    ): Int {
        val builder = CustomTabsIntent.Builder()
        if (ephemeral) {
            // androidx.browser 1.10.0 (the pinned version — see
            // `build.gradle.kts`) exposes
            // `CustomTabsIntent.Builder#setEphemeralBrowsingEnabled`
            // (confirmed against the artifact's own class file with
            // `javap`), so ephemeral mode is honoured for real rather than
            // being a best-effort no-op: a browser that supports it opens
            // the tab with no persisted cookies/history; a browser that
            // does not falls back to a normal tab (the flag is advisory).
            builder.setEphemeralBrowsingEnabled(true)
        }
        val intent = builder.build()
        if (provider != null) {
            intent.intent.setPackage(provider)
        } else {
            Log.d(TAG, "frust-auth-session: no allow-listed Custom Tabs provider found; launching unpinned")
        }
        setPending(CustomTabPending(generation, callbackScheme, act.intent))
        intent.launchUrl(act, Uri.parse(url))
        return START_ACCEPTED
    }

    /**
     * The Auth Tab launch: registers the launcher on [act]'s registry
     * ([bind]), records the session, then launches pinned to [provider]
     * with the redirect form [redirect] names. `ephemeral` is advisory here
     * too (`AuthTabIntent.Builder#setEphemeralBrowsingEnabled`).
     */
    private fun launchAuthTab(
        act: ComponentActivity,
        provider: String,
        url: String,
        redirect: Redirect,
        ephemeral: Boolean,
        generation: Long,
    ): Int {
        val flight = AuthTabPending(generation, AUTH_TAB_KEY_PREFIX + generation, act.taskId, act.javaClass.name)
        val launcher = bind(flight, act)
        val builder = AuthTabIntent.Builder()
        if (ephemeral) builder.setEphemeralBrowsingEnabled(true)
        val intent = builder.build()
        intent.intent.setPackage(provider)
        setPending(flight)
        when (redirect) {
            is Redirect.Scheme -> intent.launch(launcher, Uri.parse(url), redirect.scheme)
            is Redirect.Https -> intent.launch(launcher, Uri.parse(url), redirect.host, redirect.path)
        }
        return START_ACCEPTED
    }

    /**
     * Registers [flight]'s key on [act]'s `activityResultRegistry` (the
     * lifecycle-free overload, so it works at any point of [act]'s life) and
     * records [act] and the launcher on [flight]. On a recreated instance the
     * registry restored the key's request code, so the result still in
     * flight is delivered to this registration.
     */
    private fun bind(flight: AuthTabPending, act: ComponentActivity): ActivityResultLauncher<Intent> {
        val launcher = act.activityResultRegistry.register(
            flight.key,
            AuthTabIntent.AuthenticateUserResultContract(),
        ) { result -> onAuthTabResult(flight, result) }
        flight.owner = act
        flight.launcher = launcher
        return launcher
    }

    /** Unregisters [flight]'s launcher and drops its Activity. */
    private fun unbind(flight: AuthTabPending) {
        flight.launcher?.unregister()
        flight.launcher = null
        flight.owner = null
    }

    /**
     * The Auth Tab launcher's callback — the only thing that resolves an Auth
     * Tab session. Main thread. A result for a session that is no longer
     * [pending] (cancelled, superseded) is dropped.
     */
    private fun onAuthTabResult(flight: AuthTabPending, result: AuthTabIntent.AuthResult) {
        unbind(flight)
        if (pending !== flight) return
        pending = null
        val kind = authTabResultKind(result.resultCode)
        val url = if (kind == KIND_CALLBACK) result.resultUri?.toString() else null
        nativeOnAuthSessionResult(flight.generation, kind, url)
    }

    /**
     * Picks the browser a session launches in, or `null` when none qualifies,
     * per this host's provider-selection policy. Called from [launch] before
     * [pending] is assigned, and from [httpsCallbackSupported] on any thread.
     *
     * ## The policy
     *
     * 1. **The user's default browser, if it answers Custom Tabs.** Resolve
     *    the default `VIEW` handler for a bare, no-host `http://` URI
     *    (`PackageManager.MATCH_DEFAULT_ONLY`) and ask
     *    [CustomTabsClient.getPackageName] whether *that one package* alone
     *    qualifies. Respecting the platform's own default-app choice first
     *    is the least surprising outcome for a user who already picked a
     *    browser, and needs no allow-list check — the OS, not this host,
     *    made that choice.
     * 2. **Otherwise, the highest-preference allow-listed provider that is
     *    both installed and Custom-Tabs-capable.** The full-browser
     *    candidates (`queryIntentActivities` against the same no-host
     *    `http://` probe, `CATEGORY_BROWSABLE`, `MATCH_ALL` so a chosen default browser does not hide the rest) are
     *    intersected with [CUSTOM_TABS_PROVIDER_ALLOWLIST] — a fixed,
     *    documented set of mainstream browsers in preference order — and the
     *    ordered intersection is handed to
     *    `getPackageName(context, packages, ignoreDefault = true)`. The
     *    allow-list exists because `queryIntentActivities` enumerates *every*
     *    package that claims to handle a `VIEW`/`BROWSABLE` http(s) link,
     *    which on an unusual or compromised device could include something
     *    other than a trustworthy browser; pinning an `Intent` hands that
     *    package a `startActivity` call carrying the session's authorization
     *    URL, so this host pins only a package it recognizes by name.
     * 3. **Otherwise, `null`.** A Custom Tab then launches unpinned and
     *    resolves through the OS like any other `VIEW` intent — a plain
     *    (non-Custom-Tabs) browser window is an acceptable degrade, but
     *    pinning an unrecognized package is not. Auth Tab is never used
     *    unpinned.
     *
     * The three-step decision itself is [chooseCustomTabsProvider], a pure
     * function this method calls after doing both `PackageManager` lookups
     * below; see that function's own KDoc for why it is split out.
     */
    private fun customTabsProvider(ctx: Context): String? {
        val probe = Intent(Intent.ACTION_VIEW, Uri.parse("http://"))
        val defaultPackage = ctx.packageManager
            .resolveActivity(probe, PackageManager.MATCH_DEFAULT_ONLY)
            ?.activityInfo?.packageName

        val browserProbe = Intent(Intent.ACTION_VIEW, Uri.parse("http://"))
            .addCategory(Intent.CATEGORY_BROWSABLE)
        // `MATCH_ALL` is deliberate: without it the platform applies its
        // default-app filtering and, once the user has chosen a default
        // browser, the query returns ONLY that browser — so the allow-list
        // intersection is empty whenever the default has no Custom Tabs
        // support (the exact case this step exists for; observed on the gate
        // device). The allow-list, not the query flag, is the trust boundary.
        val installed = ctx.packageManager
            .queryIntentActivities(browserProbe, PackageManager.MATCH_ALL)
            .map { it.activityInfo.packageName }
            .toSet()

        return chooseCustomTabsProvider(defaultPackage, installed) { packages, ignoreDefault ->
            CustomTabsClient.getPackageName(ctx, packages, ignoreDefault)
        }
    }

    /**
     * Answers the start (identified by [generation]) that a redirect
     * completes, that the user dismissed, or that a deferred launch failed
     * to start. [generation] is always the value the start call received.
     *
     * Kinds (the `KIND_*` constants above; `plugins/auth-session/src/android.rs`'s
     * `RESULT_*` table is the same):
     * - `0` — Callback: the redirect arrived; [url] carries it
     * - `1` — Cancelled: the user dismissed the tab (a Custom Tab: the
     *   Activity resumed without the callback intent; Auth Tab:
     *   `RESULT_CANCELED`, or the launching Activity finished)
     * - `2` — posted-path only: no browser could take the intent
     *   ([ActivityNotFoundException])
     * - `3` — posted-path only: the deferred launch failed for any other
     *   reason
     * - `4` — posted-path only: the deferred launch found no resumed
     *   Activity (kept out of kind `1`'s slot, which means "cancelled")
     * - `5` — posted-path only: an https redirect, but the browser offers no
     *   Auth Tab
     * - `6` — Auth Tab `RESULT_VERIFICATION_FAILED`: the browser could not
     *   verify the app's https redirect
     * - `7` — Auth Tab `RESULT_VERIFICATION_TIMED_OUT`
     * - `8` — Auth Tab `RESULT_UNKNOWN_CODE`, or any code this host does not
     *   know
     *
     * Kinds `2`-`5` are reported from [startSession]'s posted runnable;
     * every other kind from the main thread.
     *
     * Implemented in `plugins/auth-session/src/android.rs` as
     * `Java_dev_frust_authsession_FrustAuthSessionHost_nativeOnAuthSessionResult`.
     * The native library is already loaded by the embedding's `FrustActivity`
     * (see `plugins/iap`'s `FrustIapHost` for the same reasoning), so no
     * `System.loadLibrary` happens here.
     */
    @JvmStatic
    external fun nativeOnAuthSessionResult(generation: Long, kind: Int, url: String?)

    /**
     * Caches whichever Activity is currently resumed, decides on resume
     * whether a pending Custom Tab session's redirect returned, and keeps an
     * Auth Tab session's launcher attached to the launching Activity across
     * recreation — see the class doc's sections on each.
     */
    private val activityCallbacks =
        object : Application.ActivityLifecycleCallbacks {
            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {}

            /**
             * Re-registers a pending Auth Tab session's key on a recreated
             * launching Activity: same task and class, restored state, and
             * no live registration left. Runs after `onCreate`, by which time
             * the registry has restored the key's request code.
             */
            override fun onActivityPostCreated(activity: Activity, savedInstanceState: Bundle?) {
                val p = pending as? AuthTabPending ?: return
                if (p.owner != null || savedInstanceState == null) return
                if (activity !is ComponentActivity) return
                if (activity.taskId != p.taskId || activity.javaClass.name != p.activityClass) return
                try {
                    bind(p, activity)
                } catch (e: Exception) {
                    Log.e(TAG, "frust-auth-session: re-registering the Auth Tab launcher failed (${e.javaClass.name})")
                    pending = null
                    nativeOnAuthSessionResult(p.generation, KIND_LAUNCH_FAILED, null)
                }
            }

            override fun onActivityStarted(activity: Activity) {}

            override fun onActivityResumed(activity: Activity) {
                this@FrustAuthSessionHost.activity = activity
                // An Auth Tab session is resolved only by its launcher.
                val p = pending as? CustomTabPending ?: return
                val i = activity.intent
                if (i != null && i !== p.launchIntent && i !== consumedIntent &&
                    i.data?.scheme == p.callbackScheme
                ) {
                    pending = null
                    consumedIntent = i
                    nativeOnAuthSessionResult(p.generation, KIND_CALLBACK, i.data.toString())
                } else {
                    pending = null
                    nativeOnAuthSessionResult(p.generation, KIND_CANCELLED, null)
                }
            }

            override fun onActivityPaused(activity: Activity) {}

            override fun onActivityStopped(activity: Activity) {}

            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}

            /**
             * Drops the destroyed Activity from [activity] and from a pending
             * Auth Tab session it launched. The registration dies with its
             * registry and is not unregistered here: its saved state is what
             * lets [onActivityPostCreated] reattach on a recreated instance.
             * A finishing launcher can never receive the result, so its
             * session is resolved Cancelled.
             */
            override fun onActivityDestroyed(activity: Activity) {
                if (activity === this@FrustAuthSessionHost.activity) {
                    this@FrustAuthSessionHost.activity = null
                }
                val p = pending as? AuthTabPending ?: return
                if (p.owner !== activity) return
                p.owner = null
                p.launcher = null
                if (activity.isFinishing) {
                    pending = null
                    nativeOnAuthSessionResult(p.generation, KIND_CANCELLED, null)
                }
            }
        }
}
