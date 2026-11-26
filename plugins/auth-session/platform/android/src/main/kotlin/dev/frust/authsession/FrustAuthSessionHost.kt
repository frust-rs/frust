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

/**
 * How [FrustAuthSessionHost] presents one session — [chooseLaunchMode]'s
 * answer. Each case carries what its launch needs, so the launch branches on
 * the case alone and never re-checks a condition [chooseLaunchMode] already
 * decided. [A] is the launching Activity's type: `ComponentActivity` in the
 * host, any stand-in in a JVM test.
 */
internal sealed class LaunchMode<out A : Any> {
    /**
     * `androidx.browser.auth.AuthTabIntent`, pinned to [provider], with its
     * result launcher registered on [activity]'s `activityResultRegistry`:
     * the redirect returns as an Activity result.
     */
    data class AuthTab<out A : Any>(val provider: String, val activity: A) : LaunchMode<A>()

    /**
     * A Custom Tab waiting for a redirect to [scheme], pinned to [provider]
     * when there is one: the redirect returns as a deep link read on the next
     * resume.
     */
    data class CustomTabs(val provider: String?, val scheme: String) : LaunchMode<Nothing>()

    /** Neither can catch the requested redirect (an https one without Auth Tab). */
    data object Unsupported : LaunchMode<Nothing>()
}

/**
 * Chooses between Auth Tab and a Custom Tab for one session — pure, and
 * top-level for the same JVM-test reason as [chooseCustomTabsProvider]
 * (`ProviderSelectionTest` drives it).
 *
 * Auth Tab is used whenever it can be: [provider] (the package
 * [chooseCustomTabsProvider] picked, or `null` for none) answers
 * [authTabSupported] (`CustomTabsClient.isAuthTabSupported`) and the
 * launching Activity is a `ComponentActivity` ([componentActivity] non-null),
 * whose `activityResultRegistry` carries the result back. Otherwise a
 * custom-scheme session ([customScheme] non-null) takes the Custom Tabs
 * path, and an https one ([customScheme] `null`) is [LaunchMode.Unsupported]:
 * only Auth Tab can catch an https redirect on Android.
 */
internal fun <A : Any> chooseLaunchMode(
    customScheme: String?,
    provider: String?,
    componentActivity: A?,
    authTabSupported: (String) -> Boolean,
): LaunchMode<A> {
    if (provider != null && componentActivity != null && authTabSupported(provider)) {
        return LaunchMode.AuthTab(provider, componentActivity)
    }
    return if (customScheme != null) LaunchMode.CustomTabs(provider, customScheme) else LaunchMode.Unsupported
}

// `FrustAuthSessionHost.start`/`startHttps` return codes — the table in `start`'s KDoc and
// `plugins/auth-session/src/android.rs`'s `START_*` constants; change all three together. A
// different scale from the `KIND_*` constants below: the two meet only in [postedLaunchKind].

/** Return code: the launch was accepted. */
internal const val START_ACCEPTED = 0

/** Return code: no resumed Activity. */
internal const val START_NO_RESUMED_ACTIVITY = 1

/** Return code: `ActivityNotFoundException` (no browser). */
internal const val START_NO_BROWSER = 2

/** Return code: any other exception. */
internal const val START_LAUNCH_FAILED = 3

/** Return code: an https redirect, but the provider offers no Auth Tab. */
internal const val START_HTTPS_UNSUPPORTED = 5

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
 * The `nativeOnAuthSessionResult` kind a posted launch reports for the
 * return code [code] its synchronous twin would have returned, or `null` for
 * [START_ACCEPTED] (nothing to report: the session's answer comes later) —
 * pure, top-level and JVM-tested (`AuthTabResultTest`). The one place the two
 * scales meet, so either table can change without the other: in particular
 * [START_NO_RESUMED_ACTIVITY] is `1`, which as a kind means Cancelled, so it
 * becomes [KIND_NO_RESUMED_ACTIVITY]. A code this function does not know is
 * [KIND_LAUNCH_FAILED], never passed through as a kind.
 */
internal fun postedLaunchKind(code: Int): Int? = when (code) {
    START_ACCEPTED -> null
    START_NO_RESUMED_ACTIVITY -> KIND_NO_RESUMED_ACTIVITY
    START_NO_BROWSER -> KIND_NO_BROWSER
    START_LAUNCH_FAILED -> KIND_LAUNCH_FAILED
    START_HTTPS_UNSUPPORTED -> KIND_HTTPS_UNSUPPORTED
    else -> KIND_LAUNCH_FAILED
}

/** The Activity lifecycle callback [authTabLifecycleAction] is asked about. */
internal enum class AuthTabEvent {
    /** `onActivityPostCreated`: an Activity of the app was created (or recreated). */
    CREATED,

    /** `onActivityResumed`: an Activity of the app resumed. */
    RESUMED,

    /** `onActivityDestroyed`: an Activity of the app was destroyed. */
    DESTROYED,
}

/**
 * What [FrustAuthSessionHost] does with its pending Auth Tab session on one
 * [AuthTabEvent] — [authTabLifecycleAction]'s answer. Every `CANCEL_*` case
 * resolves the session Cancelled once and logs its [cancelReason].
 */
internal enum class AuthTabAction(val cancelReason: String?) {
    /** Nothing changes: the launcher (if any) stays as it is. */
    KEEP(null),

    /**
     * The launching Activity is destroyed to be recreated: drop it and its
     * launcher, and wait for the new instance.
     */
    DETACH(null),

    /** A recreated launching Activity came up: register the session's key on it. */
    REATTACH(null),

    /** The launching Activity finished: the result can never arrive. */
    CANCEL_FINISHED("the launching Activity finished"),

    /** The launching Activity came back without the state its launcher needs. */
    CANCEL_NOT_RESTORED("the launching Activity was recreated without its saved launcher"),

    /** Another Activity came up while the launcher was gone. */
    CANCEL_OTHER_ACTIVITY("an Activity other than the launching one was created while the launcher was gone"),

    /** An Activity resumed while the launcher was gone, with nothing left to re-attach to. */
    CANCEL_NO_LAUNCHER("an Activity resumed while the launcher was gone"),
}

/**
 * Decides what an Activity lifecycle [event] does to the pending Auth Tab
 * session — pure, top-level and JVM-tested (`AuthTabResultTest`); the
 * lifecycle callbacks gather the inputs and apply the answer. The rule is
 * that every way the session can lose its launcher ends in exactly one
 * resolution, and that a session whose launcher is attached is never
 * resolved by a lifecycle event (leaving the app and coming back must not
 * cancel it).
 *
 * - [launched]: the session's tab has been launched. Before that, the launch
 *   itself owns the session, and every event is [AuthTabAction.KEEP].
 * - [hasOwner]: an Activity currently holds the session's launcher.
 * - [isOwner]: the event's Activity is that holder ([AuthTabEvent.DESTROYED]
 *   only; a destroyed bystander is [AuthTabAction.KEEP]).
 * - [finishing]: the event's Activity is finishing ([AuthTabEvent.DESTROYED]
 *   only): [AuthTabAction.CANCEL_FINISHED], or [AuthTabAction.DETACH] when
 *   it is being destroyed to be recreated.
 * - [savedState]: the created Activity has a saved state
 *   ([AuthTabEvent.CREATED] only).
 * - [sameTaskAndClass]: the created Activity is a `ComponentActivity` of the
 *   launching task and class ([AuthTabEvent.CREATED] only).
 * - [keyRestored]: its result registry restored the session's key as a
 *   launched one ([AuthTabEvent.CREATED] only), so the result still in
 *   flight can be delivered to a new registration.
 *
 * With no owner, a created Activity that passes all three [AuthTabEvent.CREATED]
 * gates is [AuthTabAction.REATTACH]; one of the launching task and class that
 * misses either of the last two is [AuthTabAction.CANCEL_NOT_RESTORED]; any
 * other is [AuthTabAction.CANCEL_OTHER_ACTIVITY]. A resume with no owner is
 * [AuthTabAction.CANCEL_NO_LAUNCHER]: a recreated launcher is re-attached on
 * creation, before it resumes, so by then nothing can attach it any more.
 */
internal fun authTabLifecycleAction(
    event: AuthTabEvent,
    launched: Boolean,
    hasOwner: Boolean,
    isOwner: Boolean = false,
    finishing: Boolean = false,
    savedState: Boolean = false,
    sameTaskAndClass: Boolean = false,
    keyRestored: Boolean = false,
): AuthTabAction {
    if (!launched) return AuthTabAction.KEEP
    return when (event) {
        AuthTabEvent.DESTROYED -> when {
            !isOwner -> AuthTabAction.KEEP
            finishing -> AuthTabAction.CANCEL_FINISHED
            else -> AuthTabAction.DETACH
        }
        AuthTabEvent.CREATED -> when {
            hasOwner -> AuthTabAction.KEEP
            !sameTaskAndClass -> AuthTabAction.CANCEL_OTHER_ACTIVITY
            savedState && keyRestored -> AuthTabAction.REATTACH
            else -> AuthTabAction.CANCEL_NOT_RESTORED
        }
        AuthTabEvent.RESUMED -> if (hasOwner) AuthTabAction.KEEP else AuthTabAction.CANCEL_NO_LAUNCHER
    }
}

/**
 * The name `androidx.activity`'s `ActivityResultRegistry.onSaveInstanceState`
 * stores its launched keys under (1.9.0, the pinned version — read from the
 * artifact's class file). Read by `FrustAuthSessionHost.registryRestoredKey`
 * only; a registry that writes no such entry is treated as having restored
 * the key, the behaviour before that check existed.
 */
private const val REGISTRY_LAUNCHED_KEYS = "KEY_COMPONENT_ACTIVITY_LAUNCHED_KEYS"

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
 * before, and an https one is refused (code `5`). The answer carries the
 * provider and the `ComponentActivity` the launch needs ([LaunchMode]).
 *
 * ## Auth Tab: the result comes from the launcher, never from a resume
 *
 * The launcher is registered at launch time on the launching Activity's
 * `activityResultRegistry` under the key `frust.authsession.<generation>`
 * (the plugin tier's Activity Result rule — no registration before
 * `onCreate`, no shell change). While that launcher is attached, its
 * callback is the only thing that resolves an Auth Tab session:
 * [onActivityResumed] neither reads a redirect nor reports Cancelled for
 * one, so leaving the app and coming back while the tab is open changes
 * nothing. The callback checks that the session it was registered for is
 * still [pending] (cancel, or a newer session, makes it a no-op) and maps the
 * result with [authTabResultKind].
 *
 * The launching Activity can go away while the tab is open. What each
 * lifecycle callback does about it is decided by [authTabLifecycleAction]:
 *
 * - **Recreated** (a configuration change, or the system destroying it while
 *   it keeps its saved state): its registry saved the key with its request
 *   code, so [onActivityPostCreated] registers the same key on the new
 *   instance (same task, same class, saved state present, and the key
 *   restored as launched — [registryRestoredKey]) and the result is
 *   delivered there.
 * - **Recreated, but not re-attachable**: an Activity of the launching task
 *   and class comes back without saved state or without the key, another
 *   Activity is created while the launcher is gone, or an Activity resumes
 *   with no launcher attached. Nothing can deliver the result any more, so
 *   the session is resolved Cancelled once, with the reason logged.
 * - **Finishing**: the result can never arrive, so the session is resolved
 *   Cancelled at once.
 * - **Re-registration throws**: the session is resolved with kind `3`.
 * - **Process death**: nothing pending survives, and the restored Activity
 *   is not resolved by anything: no callback, no deep link. Its registry
 *   restored the key and parks the result under it, and generations restart
 *   in the new process, so a later session with the same generation is
 *   handed that parked result synchronously inside [bind], before it is
 *   [pending]: [onAuthTabResult] drops it there, and the session's own
 *   result arrives afterwards (observed on a phone).
 *
 * Only the launching instance's registry ever holds the launcher, so a
 * second Activity instance never receives the session's result. [pending]
 * drops its references to an Activity when that Activity is destroyed.
 *
 * Every way a session ends — its result, a lifecycle resolution, a cancel,
 * a failed launch or re-registration — goes through [clearPending], which
 * unregisters an Auth Tab launcher and forgets the session; [resolvePending]
 * pairs it with the one native answer.
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
 * pending session. An Auth Tab's launcher is unregistered, which removes its
 * callback but not the launch: `androidx.activity`'s registry still maps the
 * key's request code, so a later result is parked under the key for a future
 * registration of that same key. None comes in this process — keys carry the
 * generation, which is never reused within a process — so the result is
 * never delivered; in a new process, where generations restart, a parked
 * result is dropped as the process-death case above describes. What [cancel] guarantees,
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
     * Activity is destroyed, so nothing here outlives it. [launched] turns
     * true just before this session's tab is launched: a result handed over
     * before that belongs to an earlier process (see [onAuthTabResult]).
     */
    private class AuthTabPending(
        override val generation: Long,
        val key: String,
        val taskId: Int,
        val activityClass: String,
    ) : Pending() {
        var owner: ComponentActivity? = null
        var launcher: ActivityResultLauncher<Intent>? = null
        var launched: Boolean = false
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
     * runnable as kind `2`, `3`, `4` or `5`, mapped from the return code by
     * [postedLaunchKind] (`4` stands in for return code `1`, because kind `1`
     * means "cancelled" on the native side).
     *
     * Return codes (the top-level `START_*` constants):
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
        // No Activity known yet: answer for the `ComponentActivity` a frust
        // app launches from (the launch itself checks the real one).
        val launcherHolder: Any? = if (act == null) ctx else act as? ComponentActivity
        return try {
            val provider = customTabsProvider(ctx)
            chooseLaunchMode(null, provider, launcherHolder) {
                CustomTabsClient.isAuthTabSupported(ctx, it)
            } is LaunchMode.AuthTab
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
            // A deferred launch that fails must still resolve the Rust future,
            // with the kind [postedLaunchKind] maps its return code to.
            postedLaunchKind(launch(url, redirect, ephemeral, generation))?.let { kind ->
                nativeOnAuthSessionResult(generation, kind, null)
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

    /**
     * Forget [pending], unregistering an Auth Tab launcher — the one cleanup
     * every exit of a session goes through (the class doc's Auth Tab
     * section). Main thread.
     */
    private fun clearPending() {
        (pending as? AuthTabPending)?.let(::unbind)
        pending = null
    }

    /**
     * Answers [session] with [kind] and [url] if it is still [pending],
     * clearing it first ([clearPending]) so nothing can answer it twice (the
     * class doc's one-delivery rule). A session no longer pending is left
     * alone. Main thread.
     */
    private fun resolvePending(session: Pending, kind: Int, url: String?) {
        if (pending !== session) return
        clearPending()
        nativeOnAuthSessionResult(session.generation, kind, url)
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
            val customScheme = (redirect as? Redirect.Scheme)?.scheme
            val mode = chooseLaunchMode(customScheme, provider, act as? ComponentActivity) {
                CustomTabsClient.isAuthTabSupported(act, it)
            }
            when (mode) {
                is LaunchMode.AuthTab ->
                    launchAuthTab(mode.activity, mode.provider, url, redirect, ephemeral, generation)
                is LaunchMode.CustomTabs ->
                    launchCustomTab(act, mode.provider, url, mode.scheme, ephemeral, generation)
                LaunchMode.Unsupported -> {
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
        Log.i(TAG, "frust-auth-session: session $generation launching in a Custom Tab (${provider ?: "unpinned"})")
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
        Log.i(TAG, "frust-auth-session: session $generation launching in Auth Tab ($provider)")
        flight.launched = true
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
     * flight is delivered to this registration — synchronously inside
     * `register` when it arrived while no instance was registered, in which
     * case the session is already resolved here and the registration is
     * dropped at once.
     */
    private fun bind(flight: AuthTabPending, act: ComponentActivity): ActivityResultLauncher<Intent> {
        val launcher = act.activityResultRegistry.register(
            flight.key,
            AuthTabIntent.AuthenticateUserResultContract(),
        ) { result -> onAuthTabResult(flight, result) }
        if (flight.launched && pending !== flight) {
            launcher.unregister()
            return launcher
        }
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
     * Whether [act]'s result registry restored [key] as a launched
     * registration, so a registration made now receives the result still in
     * flight. Asked through the registry's public `onSaveInstanceState`,
     * which only copies its state into the bundle it is given. A registry
     * that writes no launched-keys entry (a layout other than the pinned
     * `androidx.activity`'s), or a probe that throws, answers `true`: the
     * key is registered as it was before this check existed.
     */
    private fun registryRestoredKey(act: ComponentActivity, key: String): Boolean = try {
        val probe = Bundle()
        act.activityResultRegistry.onSaveInstanceState(probe)
        probe.getStringArrayList(REGISTRY_LAUNCHED_KEYS)?.contains(key) ?: true
    } catch (e: Exception) {
        Log.w(TAG, "frust-auth-session: reading the result registry's state failed (${e.javaClass.name})")
        true
    }

    /**
     * The Auth Tab launcher's callback — the only thing that resolves an Auth
     * Tab session. Main thread. A result for a session that is no longer
     * [pending] (cancelled, superseded) is dropped.
     *
     * A result delivered before [flight] was launched was parked by the
     * registry for an earlier process's session under the same key (the
     * class doc's "Process death"): it is dropped without touching [flight],
     * whose own launch follows.
     */
    private fun onAuthTabResult(flight: AuthTabPending, result: AuthTabIntent.AuthResult) {
        if (!flight.launched) {
            Log.i(TAG, "frust-auth-session: session ${flight.generation} dropped a result left by an earlier process")
            return
        }
        // The result code only: the URI carries the authorization response.
        Log.i(TAG, "frust-auth-session: session ${flight.generation} Auth Tab result code ${result.resultCode}")
        if (pending !== flight) {
            unbind(flight)
            return
        }
        val kind = authTabResultKind(result.resultCode)
        val url = if (kind == KIND_CALLBACK) result.resultUri?.toString() else null
        resolvePending(flight, kind, url)
    }

    /**
     * Resolves the pending Auth Tab session [p] Cancelled for the lifecycle
     * reason [action] names, logging that reason (never the URL).
     */
    private fun cancelForLifecycle(p: AuthTabPending, action: AuthTabAction) {
        Log.i(TAG, "frust-auth-session: session ${p.generation} resolved Cancelled: ${action.cancelReason}")
        resolvePending(p, KIND_CANCELLED, null)
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
     *   `RESULT_CANCELED`), or an Auth Tab session lost its launcher for good
     *   (one of [authTabLifecycleAction]'s `CANCEL_*` answers)
     * - `2` — posted-path only: no browser could take the intent
     *   ([ActivityNotFoundException])
     * - `3` — the deferred launch failed for any other reason, or
     *   re-registering an Auth Tab launcher on a recreated Activity threw
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
     * Kinds `2`-`5` are reported from [startSession]'s posted runnable
     * ([postedLaunchKind]), `3` also from a failed re-registration; every
     * kind is reported on the main thread.
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
     * recreation, or resolves the session once it cannot be — see the class
     * doc's sections on each. The Auth Tab decisions are
     * [authTabLifecycleAction]'s; these callbacks only gather its inputs and
     * apply its answer.
     */
    private val activityCallbacks =
        object : Application.ActivityLifecycleCallbacks {
            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {}

            /**
             * Re-registers a pending Auth Tab session's key on a recreated
             * launching Activity, or resolves the session Cancelled when the
             * Activity that came up cannot take the launcher
             * ([authTabLifecycleAction]). Runs after `onCreate`, by which time
             * the registry has restored the key's request code, and before
             * any result is dispatched to the new instance.
             */
            override fun onActivityPostCreated(activity: Activity, savedInstanceState: Bundle?) {
                val p = pending as? AuthTabPending ?: return
                val component = activity as? ComponentActivity
                val sameTaskAndClass = component != null &&
                    activity.taskId == p.taskId && activity.javaClass.name == p.activityClass
                val action = authTabLifecycleAction(
                    AuthTabEvent.CREATED,
                    launched = p.launched,
                    hasOwner = p.owner != null,
                    savedState = savedInstanceState != null,
                    sameTaskAndClass = sameTaskAndClass,
                    keyRestored = p.owner == null && component != null && savedInstanceState != null &&
                        registryRestoredKey(component, p.key),
                )
                when {
                    action == AuthTabAction.REATTACH && component != null -> try {
                        bind(p, component)
                        Log.i(TAG, "frust-auth-session: session ${p.generation} Auth Tab launcher re-registered")
                    } catch (e: Exception) {
                        Log.e(TAG, "frust-auth-session: re-registering the Auth Tab launcher failed (${e.javaClass.name})")
                        resolvePending(p, KIND_LAUNCH_FAILED, null)
                    }
                    action.cancelReason != null -> cancelForLifecycle(p, action)
                    else -> Unit
                }
            }

            override fun onActivityStarted(activity: Activity) {}

            override fun onActivityResumed(activity: Activity) {
                this@FrustAuthSessionHost.activity = activity
                when (val p = pending) {
                    // An Auth Tab session with its launcher attached is resolved
                    // only by that launcher; one without is resolved here.
                    is AuthTabPending -> {
                        val action = authTabLifecycleAction(
                            AuthTabEvent.RESUMED,
                            launched = p.launched,
                            hasOwner = p.owner != null,
                        )
                        if (action.cancelReason != null) cancelForLifecycle(p, action)
                    }
                    is CustomTabPending -> {
                        val i = activity.intent
                        if (i != null && i !== p.launchIntent && i !== consumedIntent &&
                            i.data?.scheme == p.callbackScheme
                        ) {
                            consumedIntent = i
                            resolvePending(p, KIND_CALLBACK, i.data.toString())
                        } else {
                            resolvePending(p, KIND_CANCELLED, null)
                        }
                    }
                    null -> Unit
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
             * session is resolved Cancelled ([authTabLifecycleAction]).
             */
            override fun onActivityDestroyed(activity: Activity) {
                if (activity === this@FrustAuthSessionHost.activity) {
                    this@FrustAuthSessionHost.activity = null
                }
                val p = pending as? AuthTabPending ?: return
                val action = authTabLifecycleAction(
                    AuthTabEvent.DESTROYED,
                    launched = p.launched,
                    hasOwner = p.owner != null,
                    isOwner = p.owner === activity,
                    finishing = activity.isFinishing,
                )
                if (action == AuthTabAction.KEEP) return
                p.owner = null
                p.launcher = null
                if (action.cancelReason != null) {
                    cancelForLifecycle(p, action)
                } else {
                    Log.i(TAG, "frust-auth-session: session ${p.generation} waiting for its launching Activity to be recreated")
                }
            }
        }
}
