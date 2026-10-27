package dev.frust.connectivity

import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.database.Cursor
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.Uri
import android.os.Handler
import android.os.HandlerThread
import android.util.Log

/**
 * The `frust-connectivity` plugin's Android host — the Kotlin half of the
 * frozen Rust↔Kotlin contract in `plugins/connectivity/src/android.rs`'s
 * module doc. Changing a name, signature or transport code here means
 * changing that file in the same commit.
 *
 * ## The contract
 *
 * - Rust → Kotlin: [start] `()Z`, called once per process through the
 *   application classloader. Registers the default-network callback and
 *   answers whether it is registered.
 * - Kotlin → Rust: [nativeOnConnectivityEvent]
 *   `(usable: Boolean, handle: Long, transport: Int, expensive: Boolean)`.
 *   `usable == false` means "no usable default network"; the other three
 *   arguments are then ignored. `transport` is one of the `TRANSPORT_CODE_*`
 *   constants below.
 *
 * ## What "usable" means
 *
 * A default network whose capabilities carry both `NET_CAPABILITY_INTERNET`
 * and `NET_CAPABILITY_VALIDATED`. `expensive` is
 * `!NET_CAPABILITY_NOT_METERED`. The Rust side sets `constrained` to `false`
 * itself: it is Apple's Low Data Mode concept, and Android's Data Saver is a
 * per-app background restriction, not a property of the network.
 *
 * ## Transport
 *
 * `TRANSPORT_VPN` is checked **first**: a VPN network also lists the
 * transport it runs over, and the default network is then the VPN, so it is
 * reported as "other". Then Wi-Fi, cellular and Ethernet; anything else is
 * "other".
 *
 * ## Seeding the first report
 *
 * `registerDefaultNetworkCallback` delivers the current default network's
 * callbacks asynchronously on the `frust-connectivity` thread, and delivers
 * nothing at all when there is no default network. So after registering,
 * [start] checks whether any callback has reported yet; if none has, it reads
 * `activeNetwork` + `getNetworkCapabilities` synchronously and reports that
 * state once. An initial report therefore always arrives, and the callback's
 * own later report of the same network is collapsed by the Rust state
 * machine. Reports are serialized under one lock, so a seed can never be
 * reported after a callback that already ran.
 *
 * ## Failures
 *
 * Nothing here throws into `ConnectivityManager` or across JNI: every callback
 * body and every native call is wrapped and logged under [TAG].
 */
object FrustConnectivityHost {
    private const val TAG = "FrustConnectivity"

    /** `transport` code: Wi-Fi. */
    private const val TRANSPORT_CODE_WIFI = 0

    /** `transport` code: a cellular radio. */
    private const val TRANSPORT_CODE_CELLULAR = 1

    /** `transport` code: wired Ethernet. */
    private const val TRANSPORT_CODE_ETHERNET = 2

    /** `transport` code: a VPN, or any transport not named above. */
    private const val TRANSPORT_CODE_OTHER = 3

    /** The application [Context], installed by [FrustConnectivityInitProvider]. */
    @Volatile
    private var appContext: Context? = null

    /** Guards [started]: [start] is idempotent. */
    private val startLock = Any()

    /** Whether the callback is registered. Guarded by [startLock]. */
    private var started = false

    /** Serializes every report into Rust, seed and callback alike. */
    private val reportLock = Any()

    /** Whether anything has been reported yet. Guarded by [reportLock]. */
    private var reported = false

    /**
     * Install the application [Context]. Called at process start by
     * [FrustConnectivityInitProvider]; first writer wins.
     */
    @JvmStatic
    fun installApplicationContext(ctx: Context) {
        if (appContext == null) appContext = ctx.applicationContext ?: ctx
    }

    /**
     * Register the default-network callback on a dedicated
     * `frust-connectivity` [HandlerThread], then seed the first report if no
     * callback has reported yet (class doc). Idempotent: a second call returns
     * `true` without registering again. Returns `false` when there is no
     * application context yet or the registration fails (logged).
     */
    @JvmStatic
    fun start(): Boolean {
        synchronized(startLock) {
            if (started) return true
            val ctx = appContext
            if (ctx == null) {
                Log.w(TAG, "frust-connectivity: no application context; is the init provider merged?")
                return false
            }
            val cm = ctx.getSystemService(ConnectivityManager::class.java)
            if (cm == null) {
                Log.w(TAG, "frust-connectivity: no ConnectivityManager service")
                return false
            }
            val thread = HandlerThread("frust-connectivity")
            try {
                thread.start()
                cm.registerDefaultNetworkCallback(DefaultNetworkCallback(), Handler(thread.looper))
            } catch (t: Throwable) {
                Log.e(TAG, "frust-connectivity: registerDefaultNetworkCallback failed", t)
                thread.quitSafely()
                return false
            }
            started = true
            seed(cm)
            return true
        }
    }

    /** Report the current default network once, unless a callback already has. */
    private fun seed(cm: ConnectivityManager) {
        synchronized(reportLock) {
            if (reported) return
            try {
                val network = cm.activeNetwork
                val caps = network?.let { cm.getNetworkCapabilities(it) }
                if (network != null && caps != null) {
                    sendLocked(network, caps)
                } else {
                    sendUnusableLocked()
                }
            } catch (t: Throwable) {
                Log.e(TAG, "frust-connectivity: reading the active network failed", t)
            }
        }
    }

    private fun report(network: Network, caps: NetworkCapabilities) {
        synchronized(reportLock) { sendLocked(network, caps) }
    }

    private fun reportUnusable() {
        synchronized(reportLock) { sendUnusableLocked() }
    }

    /** Callers hold [reportLock]. */
    private fun sendLocked(network: Network, caps: NetworkCapabilities) {
        val usable =
            caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) &&
                caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)
        if (!usable) {
            sendUnusableLocked()
            return
        }
        val expensive = !caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED)
        deliver(true, network.networkHandle, transportCode(caps), expensive)
    }

    /** Callers hold [reportLock]. */
    private fun sendUnusableLocked() {
        deliver(false, 0L, TRANSPORT_CODE_OTHER, false)
    }

    /** Callers hold [reportLock]. */
    private fun deliver(usable: Boolean, handle: Long, transport: Int, expensive: Boolean) {
        reported = true
        try {
            nativeOnConnectivityEvent(usable, handle, transport, expensive)
        } catch (t: Throwable) {
            Log.e(TAG, "frust-connectivity: failed to deliver a connectivity report", t)
        }
    }

    /** VPN first (class doc's *Transport*). */
    private fun transportCode(caps: NetworkCapabilities): Int =
        when {
            caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN) -> TRANSPORT_CODE_OTHER
            caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> TRANSPORT_CODE_WIFI
            caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> TRANSPORT_CODE_CELLULAR
            caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> TRANSPORT_CODE_ETHERNET
            else -> TRANSPORT_CODE_OTHER
        }

    /**
     * Tracks the current default network and its last capabilities. Every
     * callback runs on the one `frust-connectivity` thread, so the fields need
     * no lock.
     */
    private class DefaultNetworkCallback : ConnectivityManager.NetworkCallback() {
        private var network: Network? = null
        private var capabilities: NetworkCapabilities? = null

        /**
         * Record the network only: its capabilities may not be known yet (the
         * Android guide's warning against reading them here), so the report
         * waits for [onCapabilitiesChanged], which always follows.
         */
        override fun onAvailable(network: Network) {
            try {
                if (network != this.network) {
                    this.network = network
                    capabilities = null
                }
            } catch (t: Throwable) {
                Log.e(TAG, "frust-connectivity: onAvailable failed", t)
            }
        }

        override fun onCapabilitiesChanged(network: Network, networkCapabilities: NetworkCapabilities) {
            try {
                this.network = network
                capabilities = networkCapabilities
                report(network, networkCapabilities)
            } catch (t: Throwable) {
                Log.e(TAG, "frust-connectivity: onCapabilitiesChanged failed", t)
            }
        }

        /**
         * A lost network that is not the tracked default is stale (a switch
         * already reported its successor), so only the tracked one reports.
         */
        override fun onLost(network: Network) {
            try {
                if (network != this.network) return
                Log.d(TAG, "frust-connectivity: default network lost (last capabilities: $capabilities)")
                this.network = null
                capabilities = null
                reportUnusable()
            } catch (t: Throwable) {
                Log.e(TAG, "frust-connectivity: onLost failed", t)
            }
        }
    }

    // --- Kotlin -> Rust ----------------------------------------------------
    //
    // Implemented in `plugins/connectivity/src/android.rs` as
    // `Java_dev_frust_connectivity_FrustConnectivityHost_nativeOnConnectivityEvent`.
    // It resolves against the app's already-loaded native library: [start] is
    // only ever called from Rust, so the library is loaded before any report.

    /**
     * One raw default-network report. The Rust side decodes it, feeds its
     * dedup state machine and returns; listeners run on its own thread.
     */
    @JvmStatic
    private external fun nativeOnConnectivityEvent(
        usable: Boolean,
        handle: Long,
        transport: Int,
        expensive: Boolean,
    )
}

/**
 * Installs the application [Context] into [FrustConnectivityHost] at process
 * start — the library self-initialization mechanism `plugins/iap`'s
 * `FrustIapInitProvider` uses. Declared `exported="false"` with an
 * `${applicationId}`-scoped authority; it provides no data.
 */
class FrustConnectivityInitProvider : ContentProvider() {
    override fun onCreate(): Boolean {
        context?.let { FrustConnectivityHost.installApplicationContext(it) }
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
