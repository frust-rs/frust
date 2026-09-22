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
import androidx.browser.customtabs.CustomTabsClient
import androidx.browser.customtabs.CustomTabsIntent

/**
 * The `frust-auth-session` plugin's Android Custom Tabs host — the Kotlin half
 * of the frozen Rust↔Kotlin contract also implemented by
 * `plugins/auth-session/src/android.rs`.
 *
 * ## The contract (LAW — see `plugins/auth-session/src/android.rs`)
 *
 * This class's fully-qualified name (`dev.frust.authsession.FrustAuthSessionHost`)
 * and the names of its `@JvmStatic` entry points ([start]) are looked up by
 * string from the Rust side through the application classloader, and
 * [nativeOnAuthSessionResult]'s mangled JNI symbol
 * (`Java_dev_frust_authsession_FrustAuthSessionHost_nativeOnAuthSessionResult`)
 * is exported by that same module — **this class may never move, and neither
 * signature may change, without a coordinated change on both sides.**
 *
 * ## Why an Activity is needed
 *
 * A Chrome Custom Tab is launched with `Context.startActivity` from a live
 * Activity: it needs a task to animate into and a back stack to return to when
 * the user dismisses it or completes the flow. There is no
 * `Application`-scoped way to open one — unlike this plugin's application
 * `Context` (installed by [FrustAuthSessionInitProvider] before any Activity
 * exists), a session cannot start until one is resumed, hence [start]'s
 * "no resumed Activity" outcome.
 *
 * ## Why the outcome is read from `activity.intent`, not a fresh `Intent` extra
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
 * [pending] is cleared *before* [nativeOnAuthSessionResult] is called in
 * [onActivityResumed], so a second resume with the same (already-consumed)
 * intent — or any resume once no session is pending — cannot re-deliver a
 * result. Exactly one native callback answers each [start].
 *
 * ## The double-delivery note
 *
 * The redirect intent this host consumes is the *same* `Intent` instance the
 * embedding's `FrustSurfaceView.onDeepLink` also observes (via
 * `FrustActivity.onNewIntent`) as the framework's own deep-link source — a
 * frust app's ordinary deep-link handling sees it too. That is by design:
 * this host answers the pending Rust `AuthSession` future, while the
 * framework-level deep-link path is free to react to the same URL for
 * navigation/routing. Neither path consumes the `Intent` in a way that hides
 * it from the other.
 *
 * ## No `<uses-permission>` needed
 *
 * `CustomTabsIntent.launchUrl` is a plain `startActivity` under the hood —
 * launching another app's Activity needs no permission, and this host never
 * binds to a `CustomTabsService` directly (see `AndroidManifest.xml`'s
 * `<queries>` comment for the one manifest element this module does need).
 */
object FrustAuthSessionHost {
    private const val TAG = "frust"

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
     * to launch a Custom Tab from, and this plugin mounts no view of its own
     * to discover one through. Cleared only in [onActivityDestroyed] of that
     * same instance, never merely between resumes: clearing it while the app
     * is backgrounded (e.g. while the Custom Tab itself is in front) would
     * turn a pending session's return trip into a spurious "no resumed
     * Activity" state. `@Volatile` for the same cross-thread reason as
     * [appContext].
     */
    @Volatile
    private var activity: Activity? = null

    /** Guards a single [Application.registerActivityLifecycleCallbacks] call. */
    private var lifecycleCallbacksRegistered: Boolean = false

    /** Posts a queued [start] onto the main thread; see [start]'s own doc. */
    private val mainHandler = Handler(Looper.getMainLooper())

    /**
     * The one session this host is waiting on a redirect for, or `null` when
     * none is in flight. Set by a successful [launch], cleared the instant
     * [onActivityResumed] answers it (or decides it cannot) — see the class
     * doc's one-delivery rule.
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
     * One in-flight session: its [generation] (echoed back on every
     * [nativeOnAuthSessionResult] call this session can produce), the
     * callback URL scheme this launch is waiting for, and the Activity's
     * `intent` at the moment the Custom Tab was launched (never itself
     * mistaken for the redirect).
     */
    private data class Pending(val generation: Long, val callbackScheme: String, val launchIntent: Intent?)

    /**
     * Well-known Custom Tabs-capable browsers this host is willing to pin an
     * `Intent` to, in preference order. See [pinCustomTabsProvider] for why
     * this list exists and how it is used.
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
     * Start a Custom Tabs auth session. Called from any thread by Rust.
     *
     * [generation] identifies this session to the native side: every
     * [nativeOnAuthSessionResult] call this session can produce — whether
     * from [onActivityResumed]'s redirect/cancel outcome or from this call's
     * own posted-failure path — echoes it back unchanged, so Rust can match
     * a result to the [start] call that produced it even if a later session
     * starts before this one resolves.
     *
     * When already running on the main thread, the Custom Tab launch happens
     * synchronously and this call's own outcome is the return value. When
     * called from any other thread, the launch is posted to the main thread
     * via [mainHandler] and this call returns `0` immediately — the eventual
     * outcome then arrives either through [nativeOnAuthSessionResult] (a
     * successful launch's redirect, delivered later by [onActivityResumed]),
     * or — for the failure cases a deferred launch can still hit — through
     * an immediate call to [nativeOnAuthSessionResult] with kind `2`, `3` or
     * `4` from the posted runnable itself (`4` stands in for return code `1`,
     * because kind `1` means "cancelled" on the native side).
     *
     * Return / result codes:
     * - `0` — accepted (Custom Tab launch started; the redirect, if any,
     *   arrives later via [nativeOnAuthSessionResult])
     * - `1` — no resumed Activity
     * - `2` — [ActivityNotFoundException] (no browser installed)
     * - `3` — any other exception launching the tab (only the exception's
     *   class name is ever recorded — never [Throwable.message], which would
     *   contain the URL)
     * - `4` — posted-path only: the deferred launch found no resumed Activity
     *   (return code `1` re-coded so it cannot be mistaken for kind `1`)
     */
    @JvmStatic
    fun start(url: String, callbackScheme: String, ephemeral: Boolean, generation: Long): Int {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            return launch(url, callbackScheme, ephemeral, generation)
        }
        mainHandler.post {
            val result = launch(url, callbackScheme, ephemeral, generation)
            // A deferred launch that fails must still resolve the Rust future:
            // `1` (no resumed Activity) is a synchronous return code, so on the
            // posted path it is reported as kind `4` — `1` as a *kind* means
            // "cancelled" and must never be sent here.
            when (result) {
                0 -> Unit
                1 -> nativeOnAuthSessionResult(generation, 4, null)
                else -> nativeOnAuthSessionResult(generation, result, null)
            }
        }
        return 0
    }

    /**
     * Launches the Custom Tab. Must run on the main thread — both [start]
     * call sites guarantee that. Returns the same code [start] documents.
     *
     * [pending] is assigned only immediately before [CustomTabsIntent.launchUrl]
     * (never earlier — a provider-selection failure above must not leave a
     * stale session behind) and is rolled back to `null` by both `catch`
     * arms below, so a launch that fails never leaves a pending session that
     * will never resolve.
     */
    private fun launch(url: String, callbackScheme: String, ephemeral: Boolean, generation: Long): Int {
        val act = activity ?: return 1
        return try {
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
            pinCustomTabsProvider(act, intent)
            pending = Pending(generation, callbackScheme, act.intent)
            intent.launchUrl(act, Uri.parse(url))
            0
        } catch (e: ActivityNotFoundException) {
            // Never log the exception itself: its message carries the launch
            // Intent, i.e. the authorization URL.
            pending = null
            Log.e(TAG, "frust-auth-session: no browser available to launch the Custom Tab (${e.javaClass.name})")
            2
        } catch (e: Exception) {
            pending = null
            Log.e(TAG, "frust-auth-session: Custom Tab launch failed (${e.javaClass.name})")
            3
        }
    }

    /**
     * Pins [intent] to a Custom Tabs provider per this host's
     * provider-selection policy, or leaves it unpinned when none qualifies.
     * Called from [launch], before [pending] is assigned.
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
     * 3. **Otherwise, leave [intent] unpinned.** `launchUrl` then resolves
     *    through the OS like any other `VIEW` intent — a plain
     *    (non-Custom-Tabs) browser window is an acceptable degrade, but
     *    pinning an unrecognized package is not.
     *
     * The `ephemeral` flag passed to [launch] (see its
     * `setEphemeralBrowsingEnabled` call) is advisory to whichever provider
     * is ultimately used, pinned or not — a provider that doesn't implement
     * it simply falls back to a normal (non-ephemeral) tab.
     */
    private fun pinCustomTabsProvider(act: Activity, intent: CustomTabsIntent) {
        val probe = Intent(Intent.ACTION_VIEW, Uri.parse("http://"))
        val defaultPackage = act.packageManager
            .resolveActivity(probe, PackageManager.MATCH_DEFAULT_ONLY)
            ?.activityInfo?.packageName
        if (defaultPackage != null &&
            CustomTabsClient.getPackageName(act, listOf(defaultPackage)) == defaultPackage
        ) {
            intent.intent.setPackage(defaultPackage)
            return
        }

        val browserProbe = Intent(Intent.ACTION_VIEW, Uri.parse("http://"))
            .addCategory(Intent.CATEGORY_BROWSABLE)
        // `MATCH_ALL` is deliberate: without it the platform applies its
        // default-app filtering and, once the user has chosen a default
        // browser, the query returns ONLY that browser — so the allow-list
        // intersection is empty whenever the default has no Custom Tabs
        // support (the exact case this step exists for; observed on the gate
        // device). The allow-list, not the query flag, is the trust boundary.
        val installed = act.packageManager
            .queryIntentActivities(browserProbe, PackageManager.MATCH_ALL)
            .map { it.activityInfo.packageName }
            .toSet()
        val candidates = CUSTOM_TABS_PROVIDER_ALLOWLIST.filter { it in installed }
        val chosen = CustomTabsClient.getPackageName(act, candidates, /* ignoreDefault = */ true)
        // Never pin a package outside the allow-list, even if a future
        // `getPackageName` implementation were to return one.
        if (chosen != null && chosen in CUSTOM_TABS_PROVIDER_ALLOWLIST) {
            intent.intent.setPackage(chosen)
            return
        }

        Log.d(TAG, "frust-auth-session: no allow-listed Custom Tabs provider found; launching unpinned")
    }

    /**
     * Answers the [start] (identified by [generation]) that a redirect
     * completes or that a deferred launch failed to start. [generation] is
     * always the value the corresponding [start] call received.
     *
     * Kinds:
     * - `0` — Callback: the OAuth/OIDC redirect arrived; [url] carries it
     * - `1` — Cancelled: the Activity resumed without the expected callback
     *   intent (the user backed out of the tab without completing the flow)
     * - `2` — posted-path only: the deferred launch found no browser able to
     *   handle the Custom Tab ([ActivityNotFoundException])
     * - `3` — posted-path only: the deferred launch failed for any other
     *   reason
     * - `4` — posted-path only: the deferred launch found no resumed
     *   Activity (kept out of kind `1`'s slot, which means "cancelled")
     *
     * Kinds `2`-`4` are reported from [start]'s posted runnable, on whatever
     * thread issued the deferred launch; kinds `0`/`1` are reported from
     * [onActivityResumed] on the main thread.
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
     * Caches whichever Activity is currently resumed and, when a session is
     * pending, decides whether this resume is the OAuth/OIDC redirect
     * returning — see the class doc's "why the outcome is read from
     * `activity.intent`" and one-delivery-rule sections.
     */
    private val activityCallbacks =
        object : Application.ActivityLifecycleCallbacks {
            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {}

            override fun onActivityStarted(activity: Activity) {}

            override fun onActivityResumed(activity: Activity) {
                this@FrustAuthSessionHost.activity = activity
                val p = pending ?: return
                val i = activity.intent
                if (i != null && i !== p.launchIntent && i !== consumedIntent &&
                    i.data?.scheme == p.callbackScheme
                ) {
                    pending = null
                    consumedIntent = i
                    nativeOnAuthSessionResult(p.generation, 0, i.data.toString())
                } else {
                    pending = null
                    nativeOnAuthSessionResult(p.generation, 1, null)
                }
            }

            override fun onActivityPaused(activity: Activity) {}

            override fun onActivityStopped(activity: Activity) {}

            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}

            override fun onActivityDestroyed(activity: Activity) {
                if (activity === this@FrustAuthSessionHost.activity) {
                    this@FrustAuthSessionHost.activity = null
                }
            }
        }
}
