package dev.frust.iap

import android.app.Activity
import android.app.Application
import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.database.Cursor
import android.net.Uri
import android.os.Bundle
import android.util.Log
import dev.hyo.openiap.DeepLinkOptions
import dev.hyo.openiap.ErrorCode
import dev.hyo.openiap.FetchProductsResult
import dev.hyo.openiap.FetchProductsResultAll
import dev.hyo.openiap.FetchProductsResultProducts
import dev.hyo.openiap.FetchProductsResultSubscriptions
import dev.hyo.openiap.InitConnectionConfig
import dev.hyo.openiap.OpenIapError
import dev.hyo.openiap.OpenIapModule
import dev.hyo.openiap.OpenIapProtocol
import dev.hyo.openiap.ProductRequest
import dev.hyo.openiap.PurchaseAndroid
import dev.hyo.openiap.PurchaseOptions
import dev.hyo.openiap.RequestPurchaseProps
import dev.hyo.openiap.listener.OpenIapPurchaseErrorListener
import dev.hyo.openiap.listener.OpenIapPurchaseUpdateListener
import dev.hyo.openiap.store.OpenIapStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

/**
 * The `frust-iap` plugin's Android billing host — the Kotlin half of the frozen
 * Rust↔Kotlin contract reproduced in `plugins/iap/src/android.rs`'s module doc.
 *
 * ## The contract (LAW — see `plugins/iap/src/android.rs`)
 *
 * Rust → Kotlin is a **single** static, resolved through the application
 * classloader exactly like `dev.frust.camera.FrustCameraHost`:
 *
 * | Method | Signature | Notes |
 * |---|---|---|
 * | [call] | `(long requestId, String method, String argsJson) -> void` | Never blocks; **exactly one** [nativeOnIapResult] per `requestId`, always |
 *
 * One static rather than one-per-operation because every operation here has the
 * same shape — a JSON request in, a JSON response or an OpenIAP error JSON out —
 * unlike camera's typed `int`/`float` statics. Adding an operation is a new
 * `method` string, never a new static or a changed signature.
 *
 * Kotlin → Rust ([nativeOnIapResult], [nativeOnIapEvent]). The package is baked
 * into those symbols' mangled names
 * (`Java_dev_frust_iap_FrustIapHost_native*`), so **this class may never move
 * once shipped** — the same rule `dev.frust.FrustSurfaceView`'s exports carry.
 * `dev.frust` itself belongs exclusively to the `frust-embedding` module; every
 * plugin takes a subpackage (`docs/CODE_STANDARDS.md`'s Plugin Conventions).
 *
 * ## Exactly one resolution per request id (LAW)
 *
 * A Rust caller parks on `requestId` until [nativeOnIapResult] carries its
 * answer, so **every** path out of [call] must resolve exactly once: the
 * success path, the OpenIAP-error path, an unexpected `Throwable`, an unknown
 * `method` string, and a failure raised before the coroutine is even launched
 * (a malformed `argsJson`). Nothing here may return without answering, and
 * nothing may answer twice — the Rust side drops a duplicate, but a *missing*
 * answer costs a blocked caller its whole timeout.
 *
 * ## Threading
 *
 * [call] is invoked over JNI from a Rust background thread and **must not
 * block**: it parses its arguments, hands the work to [scope] (a
 * `SupervisorJob` on [Dispatchers.Default]), and returns. Every OpenIAP entry
 * point is a `suspend` member, and the ones with a thread affinity impose it
 * themselves — `launchBillingFlow` is main-thread-only and `OpenIapModule`
 * re-posts it through `Activity.runOnUiThread` internally, so nothing here
 * hops threads on Play Billing's behalf.
 *
 * Purchase events arrive on Play Billing's own callback thread and are
 * forwarded straight to [nativeOnIapEvent] from it — no coroutine, no main
 * `Looper` — which is what makes an event's delivery independent of a busy UI
 * thread. The Rust registry documents the same contract to app listeners.
 *
 * ## `requestPurchase` resolves at dispatch, not at outcome (LAW)
 *
 * `OpenIapStore.requestPurchase` **suspends for the entire purchase flow** —
 * it resumes only once Play delivers the purchase or an error, which includes
 * however long the user spends in the Play sheet. The Rust API contract is the
 * opposite: `Iap::request_purchase` returns as soon as the store has accepted
 * the request, and the outcome arrives on the event stream (the two-phase
 * purchase contract in `plugins/iap/src/lib.rs`). So [dispatch]'s
 * `requestPurchase` branch validates its arguments, starts the flow in a
 * detached coroutine, and resolves immediately; a throw out of the detached
 * flow becomes a purchase-error **event**, which is where OpenIAP itself
 * already publishes every in-flow failure (`errorEventGate.publishOnce`).
 *
 * ## Why an `OpenIapStore` **and** an `OpenIapProtocol`
 *
 * [Bindings] holds both. `OpenIapStore` is the entry API for nine of the twelve
 * operations, but it exposes no member for `getStorefront`,
 * `acknowledgePurchaseAndroid`, `consumePurchaseAndroid` or `restorePurchases`
 * — those live on `OpenIapProtocol`/`OpenIapModule` (verified against
 * openiap-google 3.0.1; `flutter_inapp_purchase`'s Android plugin drives the
 * module directly for the same reason). Constructing the module here and
 * handing it to `OpenIapStore`'s `(OpenIapProtocol)` constructor is what makes
 * both reachable; `OpenIapStore(Context)` would build and hide its own.
 */
object FrustIapHost {
    // --- Contract constants -------------------------------------------------

    /** [call] `method`: open the store connection. Args: `{"config": obj?}`. */
    private const val METHOD_INIT_CONNECTION = "initConnection"

    /** [call] `method`: close the store connection. Args: `{}`. */
    private const val METHOD_END_CONNECTION = "endConnection"

    /** [call] `method`: product query. Args: an OpenIAP `ProductRequest`. */
    private const val METHOD_FETCH_PRODUCTS = "fetchProducts"

    /** [call] `method`: owned-purchase query. Args: `{"options": obj?}`. */
    private const val METHOD_GET_AVAILABLE_PURCHASES = "getAvailablePurchases"

    /** [call] `method`: subscription query. Args: `{"subscriptionIds": []?}`. */
    private const val METHOD_GET_ACTIVE_SUBSCRIPTIONS = "getActiveSubscriptions"

    /** [call] `method`: the account's billing country. Args: `{}`. */
    private const val METHOD_GET_STOREFRONT = "getStorefront"

    /** [call] `method`: start a purchase. Args: `RequestPurchaseProps`. */
    private const val METHOD_REQUEST_PURCHASE = "requestPurchase"

    /**
     * [call] `method`: settle a purchase. Args:
     * `{"purchase": obj, "isConsumable": bool?}`.
     */
    private const val METHOD_FINISH_TRANSACTION = "finishTransaction"

    /** [call] `method`: re-query Play for owned purchases. Args: `{}`. */
    private const val METHOD_RESTORE_PURCHASES = "restorePurchases"

    /** [call] `method`: open Play's subscription page. Args: `{"options": obj?}`. */
    private const val METHOD_DEEP_LINK_TO_SUBSCRIPTIONS = "deepLinkToSubscriptions"

    /** [call] `method`: acknowledge a token. Args: `{"purchaseToken": str}`. */
    private const val METHOD_ACKNOWLEDGE_PURCHASE = "acknowledgePurchase"

    /** [call] `method`: consume a token. Args: `{"purchaseToken": str}`. */
    private const val METHOD_CONSUME_PURCHASE = "consumePurchase"

    /** [nativeOnIapEvent] `kind`: the payload is an OpenIAP `Purchase`. */
    private const val EVENT_PURCHASE_UPDATED = "purchase-updated"

    /** [nativeOnIapEvent] `kind`: the payload is an OpenIAP `PurchaseError`. */
    private const val EVENT_PURCHASE_ERROR = "purchase-error"

    /** The `null` JSON document — every operation whose result is `void`. */
    private const val JSON_NULL = "null"

    private const val TAG = "frust"

    // --- Process state ------------------------------------------------------

    /**
     * The application [Context], installed at process start by
     * [FrustIapInitProvider]. `@Volatile` since [installApplicationContext] and
     * every Rust-facing static run on different threads.
     */
    @Volatile
    private var appContext: Context? = null

    /**
     * The currently resumed Activity, learned from the process-wide lifecycle
     * callbacks registered in [ensureLifecycleCallbacks] — a purchase flow needs
     * one to launch Play's sheet from, and billing has no view of its own to
     * discover one through. `@Volatile` for the same cross-thread reason as
     * [appContext].
     */
    @Volatile
    private var activity: Activity? = null

    /** Guards a single [Application.registerActivityLifecycleCallbacks] call. */
    private var lifecycleCallbacksRegistered: Boolean = false

    /**
     * The coroutine scope every [call] dispatch runs on.
     *
     * `SupervisorJob` so one failed operation cannot cancel the scope out from
     * under every later one, and [Dispatchers.Default] rather than
     * [Dispatchers.Main] so a dispatch never queues behind UI work — OpenIAP's
     * own members re-post to the main thread where Play Billing demands it.
     * Never cancelled: the host lives as long as the process.
     */
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    /** Guards [bindings]' one-time construction (see [bindings]). */
    private val bindingsLock = Any()

    /** The OpenIAP pair, built on first use — see the class doc. */
    @Volatile
    private var bindings: Bindings? = null

    /**
     * The `openiap-google` objects this host drives: the module (the
     * `OpenIapProtocol` implementation) and the store wrapping it. See the class
     * doc for why both are held.
     */
    private class Bindings(val module: OpenIapProtocol, val store: OpenIapStore)

    // --- Rust -> Kotlin: the one contract static ----------------------------

    /**
     * Run `method` with `argsJson` and answer [nativeOnIapResult] exactly once
     * with `requestId`.
     *
     * Never blocks and never throws into the JNI frame: the whole dispatch is
     * wrapped, so a malformed `argsJson`, an unknown `method`, an OpenIAP
     * refusal and an unexpected `Throwable` all leave through the same single
     * resolution (class doc's *Exactly one resolution per request id*).
     */
    @JvmStatic
    fun call(requestId: Long, method: String, argsJson: String) {
        val args =
            try {
                parseArgs(argsJson)
            } catch (t: Throwable) {
                resolve(requestId, null, errorJson(t))
                return
            }
        try {
            scope.launch {
                try {
                    resolve(requestId, dispatch(method, args), null)
                } catch (t: Throwable) {
                    // `Throwable`, not `Exception`, and deliberately including a
                    // `CancellationException`: the usual "rethrow cancellation"
                    // rule assumes someone upstream is waiting, and here the only
                    // party waiting is a blocked Rust caller that would learn
                    // nothing from it. [scope] is never cancelled anyway.
                    resolve(requestId, null, errorJson(t))
                }
            }
        } catch (t: Throwable) {
            // Launching itself failed (a cancelled or rejecting scope). The
            // coroutine body never ran, so this is still the first and only
            // resolution for `requestId`.
            resolve(requestId, null, errorJson(t))
        }
    }

    /**
     * Invoke the OpenIAP member `method` names and return its result as a JSON
     * document (`null` for the `void` ones).
     *
     * Every branch either returns a JSON string or throws — an unknown method
     * throws rather than returning silently, so no `method` string can leave a
     * Rust caller parked (class doc's LAW).
     */
    private suspend fun dispatch(method: String, args: Map<String, Any?>): String {
        val bound = bindings()
        val store = bound.store
        return when (method) {
            METHOD_INIT_CONNECTION -> {
                val config = objectArg(args, "config")?.let { InitConnectionConfig.fromJson(it) }
                // `init_connection` is idempotent by this crate's contract: a
                // second call on an open connection is a success. The Play
                // module already answers `true` early when its billing client is
                // ready, so this only catches a store/flavor that reports the
                // spec's `already-prepared` refusal instead.
                val connected =
                    try {
                        store.initConnection(config)
                    } catch (e: OpenIapError) {
                        if (e.code == ErrorCode.AlreadyPrepared.rawValue) true else throw e
                    }
                connected.toString()
            }

            METHOD_END_CONNECTION -> store.endConnection().toString()

            METHOD_FETCH_PRODUCTS -> {
                val request =
                    ProductRequest.fromJson(args)
                        ?: throw OpenIapError.DeveloperError("fetchProducts requires `skus`")
                fetchProductsJson(store.fetchProducts(request))
            }

            METHOD_GET_AVAILABLE_PURCHASES -> {
                val options = objectArg(args, "options")?.let { PurchaseOptions.fromJson(it) }
                jsonArrayOf(store.getAvailablePurchases(options).map { it.toJson() })
            }

            METHOD_GET_ACTIVE_SUBSCRIPTIONS -> {
                val ids = stringListArg(args, "subscriptionIds")
                jsonArrayOf(store.getActiveSubscriptions(ids).map { it.toJson() })
            }

            METHOD_GET_STOREFRONT -> {
                val storefront =
                    bound.module.queryHandlers.getStorefront?.invoke()
                        ?: throw OpenIapError.FeatureNotSupported(
                            "this openiap module exposes no getStorefront handler",
                        )
                JSONObject.quote(storefront)
            }

            METHOD_REQUEST_PURCHASE -> {
                // Arguments are validated (and the Activity resolved) on this
                // coroutine so a bad request still reports synchronously; the
                // flow itself is detached — see the class doc's LAW.
                val props =
                    try {
                        RequestPurchaseProps.fromJson(args)
                    } catch (e: IllegalArgumentException) {
                        // `fromJson` enforces the spec's own invariant (exactly
                        // one of requestPurchase/requestSubscription, matching
                        // `type`); report it as the caller error it is.
                        throw OpenIapError.DeveloperError(e.message)
                    }
                this.activity?.let { store.setActivity(it) }
                    ?: throw OpenIapError.MissingCurrentActivity
                scope.launch {
                    try {
                        store.requestPurchase(props)
                    } catch (t: Throwable) {
                        emitEvent(EVENT_PURCHASE_ERROR, errorJson(t))
                    }
                }
                JSON_NULL
            }

            METHOD_FINISH_TRANSACTION -> {
                val purchase =
                    objectArg(args, "purchase")
                        ?: throw OpenIapError.DeveloperError(
                            "finishTransaction requires `purchase`",
                        )
                // `PurchaseAndroid.fromJson`, not the `Purchase` sealed
                // interface's own `fromJson`: that one dispatches on a
                // `__typename` the Rust `PurchaseInput` wire shape does not
                // carry, and every purchase this host settles is an Android one.
                store.finishTransaction(
                    PurchaseAndroid.fromJson(purchase),
                    args["isConsumable"] as? Boolean,
                )
                JSON_NULL
            }

            METHOD_RESTORE_PURCHASES -> {
                bound.module.restorePurchases()
                JSON_NULL
            }

            METHOD_DEEP_LINK_TO_SUBSCRIPTIONS -> {
                // Play's subscription page opens through the application
                // `Context` with `FLAG_ACTIVITY_NEW_TASK`, so unlike
                // `requestPurchase` this needs no Activity; one is still handed
                // over when cached, so a later flow starts from the right task.
                this.activity?.let { store.setActivity(it) }
                val options =
                    objectArg(args, "options")?.let { DeepLinkOptions.fromJson(it) }
                        ?: DeepLinkOptions()
                store.deepLinkToSubscriptions(options)
                JSON_NULL
            }

            // Play answers both of these with a boolean rather than throwing on
            // a non-OK response code, so the boolean is the wire result and the
            // Rust side turns a `false` into a typed failure.
            METHOD_ACKNOWLEDGE_PURCHASE ->
                bound.module.acknowledgePurchaseAndroid(purchaseTokenArg(args)).toString()

            METHOD_CONSUME_PURCHASE ->
                bound.module.consumePurchaseAndroid(purchaseTokenArg(args)).toString()

            else ->
                throw OpenIapError.DeveloperError(
                    "FrustIapHost.call: unknown method `$method`",
                )
        }
    }

    // --- OpenIAP bindings ---------------------------------------------------

    /**
     * The OpenIAP pair, constructed on first use and kept for the process.
     *
     * The purchase listeners are attached here, inside the same one-time
     * construction, so they are registered exactly once and cannot be lost:
     * `OpenIapStore.endConnection` removes only the store's *own* listeners
     * (and cancels only its *own* internal scope, which nothing here reads), so
     * these survive an end/init cycle and keep delivering.
     */
    private fun bindings(): Bindings {
        bindings?.let { return it }
        synchronized(bindingsLock) {
            bindings?.let { return it }
            val context = appContext
            if (context == null) {
                Log.e(
                    TAG,
                    "frust-iap: no application Context — is FrustIapInitProvider still in the " +
                        "app's merged manifest?",
                )
                throw OpenIapError.NotPrepared
            }
            val module = OpenIapModule(context)
            val store = OpenIapStore(module)
            store.addPurchaseUpdateListener(
                OpenIapPurchaseUpdateListener { purchase ->
                    emitEvent(EVENT_PURCHASE_UPDATED, JSONObject(purchase.toJson()).toString())
                },
            )
            store.addPurchaseErrorListener(
                OpenIapPurchaseErrorListener { error ->
                    emitEvent(EVENT_PURCHASE_ERROR, JSONObject(error.toJSON()).toString())
                },
            )
            val created = Bindings(module, store)
            bindings = created
            return created
        }
    }

    // --- JSON marshalling ---------------------------------------------------

    /** `argsJson` as a plain Kotlin map, ready for OpenIAP's `fromJson`s. */
    private fun parseArgs(argsJson: String): Map<String, Any?> {
        @Suppress("UNCHECKED_CAST")
        return unwrap(JSONObject(argsJson)) as Map<String, Any?>
    }

    /**
     * `org.json` values as plain Kotlin ones — `JSONObject` → [Map],
     * `JSONArray` → [List], `JSONObject.NULL` → `null`.
     *
     * OpenIAP's generated `fromJson` companions read `Map<String, Any?>` with
     * plain nested maps/lists, which `JSONObject` itself is not.
     */
    private fun unwrap(value: Any?): Any? =
        when (value) {
            null, JSONObject.NULL -> null
            is JSONObject -> value.keys().asSequence().associateWith { unwrap(value.get(it)) }
            is JSONArray -> (0 until value.length()).map { unwrap(value.get(it)) }
            else -> value
        }

    /** The `object`-valued argument `key`, or null when absent/`null`. */
    private fun objectArg(args: Map<String, Any?>, key: String): Map<String, Any?>? {
        @Suppress("UNCHECKED_CAST")
        return args[key] as? Map<String, Any?>
    }

    /** The `string[]`-valued argument `key`, or null when absent/`null`. */
    private fun stringListArg(args: Map<String, Any?>, key: String): List<String>? =
        (args[key] as? List<*>)?.mapNotNull { it as? String }

    /** The required, non-blank `purchaseToken` argument. */
    private fun purchaseTokenArg(args: Map<String, Any?>): String =
        (args["purchaseToken"] as? String)?.takeIf { it.isNotBlank() }
            ?: throw OpenIapError.DeveloperError("a purchase token is required")

    /**
     * This crate's own `FetchProductsResult` envelope —
     * `{"type": "products"|"subscriptions"|"all", "items": [...]}`.
     *
     * OpenIAP models the result as a sealed interface with no `toJson` of its
     * own, so the shape is designed Rust-side (`plugins/iap/src/types.rs`) and
     * built here by matching the three variants, exactly as `OpenIapStore` does
     * for its own state flows.
     */
    private fun fetchProductsJson(result: FetchProductsResult): String {
        val (tag, items) =
            when (result) {
                is FetchProductsResultProducts ->
                    "products" to result.value.orEmpty().map { it.toJson() }
                is FetchProductsResultSubscriptions ->
                    "subscriptions" to result.value.orEmpty().map { it.toJson() }
                is FetchProductsResultAll ->
                    "all" to result.value.orEmpty().map { it.toJson() }
            }
        val envelope = JSONObject()
        envelope.put("type", tag)
        envelope.put("items", jsonArray(items))
        return envelope.toString()
    }

    /** `maps` as a JSON array document. */
    private fun jsonArrayOf(maps: List<Map<String, Any?>>): String = jsonArray(maps).toString()

    /** `maps` as a [JSONArray] of objects. */
    private fun jsonArray(maps: List<Map<String, Any?>>): JSONArray {
        val array = JSONArray()
        maps.forEach { array.put(JSONObject(it)) }
        return array
    }

    /**
     * `t` as OpenIAP error JSON — the wire shape the Rust side maps back onto
     * its own `IapErrorCode`/`IapError::Store` pair.
     *
     * An [OpenIapError] carries its own `toJSON()`; anything else is synthesized
     * with the spec's `unknown` code rather than dropped, so no failure ever
     * reaches Rust as an empty or absent payload.
     */
    private fun errorJson(t: Throwable): String {
        val payload =
            when (t) {
                is OpenIapError -> t.toJSON()
                else ->
                    mapOf(
                        "code" to ErrorCode.Unknown.rawValue,
                        "message" to (t.message ?: t.javaClass.name),
                        "platform" to "android",
                    )
            }
        return JSONObject(payload).toString()
    }

    // --- Kotlin -> Rust delivery -------------------------------------------

    /**
     * Deliver one request's answer. Exactly one of `okJson`/`errJson` is
     * non-null.
     *
     * A failure of the native call itself (an unloaded library) is logged rather
     * than thrown: throwing would unwind out of a coroutine with nothing left to
     * catch it, and the Rust caller's timeout already covers a lost answer.
     */
    private fun resolve(requestId: Long, okJson: String?, errJson: String?) {
        try {
            nativeOnIapResult(requestId, okJson, errJson)
        } catch (t: Throwable) {
            Log.e(TAG, "frust-iap: failed to deliver the answer for request $requestId", t)
        }
    }

    /**
     * Forward one purchase event, on the caller's thread (class doc's
     * *Threading*). Failures are logged for the same reason as [resolve]'s.
     */
    private fun emitEvent(kind: String, payloadJson: String) {
        try {
            nativeOnIapEvent(kind, payloadJson)
        } catch (t: Throwable) {
            Log.e(TAG, "frust-iap: failed to deliver a `$kind` event", t)
        }
    }

    // --- Kotlin -> Rust: this plugin's own JNI exports ---------------------
    //
    // Implemented in `plugins/iap/src/android.rs` as
    // `Java_dev_frust_iap_FrustIapHost_native*`. They resolve against the app's
    // already-loaded native library (the embedding's `FrustActivity` loads it
    // before any frust view exists, and every call below is downstream of a
    // Rust-initiated one), so no `System.loadLibrary` happens here.

    /**
     * Answers the [call] that was issued with `requestId` — exactly one of
     * `okJson` (a JSON document carrying the result) and `errJson` (OpenIAP
     * error JSON) is non-null.
     */
    @JvmStatic
    external fun nativeOnIapResult(
        requestId: Long,
        okJson: String?,
        errJson: String?,
    )

    /**
     * Delivers one purchase event: `kind` is [EVENT_PURCHASE_UPDATED] (an
     * OpenIAP `Purchase` payload) or [EVENT_PURCHASE_ERROR] (a `PurchaseError`
     * payload). Called on Play Billing's own callback thread.
     */
    @JvmStatic
    external fun nativeOnIapEvent(
        kind: String,
        payloadJson: String,
    )

    // --- Module-internal seam (NOT part of the JNI contract) ---------------

    /**
     * Install the application [Context] — called once at process start by
     * [FrustIapInitProvider], before any Activity or view exists. Idempotent
     * and first-writer-wins, matching `FrustCameraHost.installApplicationContext`'s
     * contract in the sibling plugin (no direct reference — this module never
     * depends on `frust-camera`).
     */
    internal fun installApplicationContext(context: Context) {
        if (appContext == null) appContext = context
        ensureLifecycleCallbacks(context)
    }

    /**
     * Register the process-wide [Application.ActivityLifecycleCallbacks] once.
     *
     * Registering them from the init provider rather than from a view seam is
     * what lets a purchase flow find an Activity at all: billing mounts no view
     * of its own, so there is no other moment at which one becomes observable.
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
     * Caches whichever Activity is currently resumed — the one
     * [METHOD_REQUEST_PURCHASE] hands to `OpenIapStore.setActivity` so Play's
     * purchase sheet has a task to launch into.
     *
     * The cache is never pushed into the store as `null`: `setActivity(null)`
     * clears OpenIAP's own weak reference, and clearing it while the app is
     * merely between resumes would turn a live flow's Activity into a
     * `MissingCurrentActivity` refusal.
     */
    private val activityCallbacks =
        object : Application.ActivityLifecycleCallbacks {
            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {}

            override fun onActivityStarted(activity: Activity) {}

            override fun onActivityResumed(activity: Activity) {
                this@FrustIapHost.activity = activity
            }

            override fun onActivityPaused(activity: Activity) {}

            override fun onActivityStopped(activity: Activity) {}

            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}

            override fun onActivityDestroyed(activity: Activity) {
                if (activity === this@FrustIapHost.activity) this@FrustIapHost.activity = null
            }
        }
}

/**
 * Installs the application [Context] into [FrustIapHost] at process start.
 *
 * A `ContentProvider` declared in this module's own `AndroidManifest.xml` is
 * created by the system before `Application.onCreate` — the standard
 * self-initialization mechanism for an Android library that needs a `Context`
 * with no app-side code (`androidx.startup`'s `InitializationProvider`,
 * Firebase's `FirebaseInitProvider`; the same shape `plugins/camera`'s
 * `FrustCameraInitProvider` uses).
 *
 * It provides no data: every `ContentProvider` operation returns null/0. It is
 * `exported="false"` and its authority is `${applicationId}`-scoped, so nothing
 * outside the app can reach it.
 */
class FrustIapInitProvider : ContentProvider() {
    override fun onCreate(): Boolean {
        context?.let { FrustIapHost.installApplicationContext(it.applicationContext) }
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
