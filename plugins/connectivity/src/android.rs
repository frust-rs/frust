//! The Android arm: `ConnectivityManager.registerDefaultNetworkCallback`
//! through the plugin's `dev.frust.connectivity.FrustConnectivityHost`
//! (`plugins/connectivity/platform/android/`), reporting into
//! [`crate::state::report`].
//!
//! # The frozen contract
//!
//! **`FrustConnectivityHost.kt` and this module build to this table —
//! changing it means changing both files together.** The package
//! `dev.frust.connectivity` is baked into the JNI export's symbol name, so it
//! never moves once shipped.
//!
//! | Direction | Member | Signature (Java) |
//! |---|---|---|
//! | Rust → Kotlin | `start` (static) | `() -> boolean` — registers the callback; idempotent; `false` without an application context or on a failed registration |
//! | Kotlin → Rust | [`Java_dev_frust_connectivity_FrustConnectivityHost_nativeOnConnectivityEvent`] | `(boolean usable, long handle, int transport, boolean expensive) -> void` |
//!
//! `transport` is [`TRANSPORT_CODE_WIFI`], [`TRANSPORT_CODE_CELLULAR`],
//! [`TRANSPORT_CODE_ETHERNET`] or [`TRANSPORT_CODE_OTHER`]; an unknown code
//! decodes as [`Transport::Other`]. With `usable == false` the other three
//! arguments are ignored.
//!
//! # What the host reports
//!
//! - **Usable** = the default network's capabilities carry `INTERNET` and
//!   `VALIDATED`. Its `Network.networkHandle` becomes the [`NetworkId`],
//!   `expensive` is `!NOT_METERED`, and `constrained` is always `false`:
//!   constrained is Apple's Low Data Mode, and Android's Data Saver is a
//!   per-app background restriction, not a property of the network.
//! - **VPN → [`Transport::Other`]**: the host checks `TRANSPORT_VPN` first,
//!   because a VPN network also lists the transport it runs over and is then
//!   itself the default network.
//! - `onAvailable` only records the network (capabilities may not be known
//!   yet); `onCapabilitiesChanged` reports usable or unusable; `onLost` of the
//!   tracked default reports unusable. On a Wi-Fi↔cellular switch the new
//!   network's `onCapabilitiesChanged` reports (repeats are collapsed by the
//!   state machine), so one `Available`; the old
//!   network's later `onLost`, if one comes, is not the tracked one and
//!   reports nothing.
//!
//! # Seeding the first report (R2)
//!
//! The callback reports asynchronously on the host's `frust-connectivity`
//! thread, and not at all when the device has no default network. So once
//! registered, `start` checks whether any callback has reported yet and, if
//! not, reads `activeNetwork` + `getNetworkCapabilities` synchronously and
//! reports that once: an initial report always arrives (an offline start
//! reports unusable, which [`crate::state`] treats as the initial state). The
//! callback's own later report of the same network is collapsed by the state
//! machine, and the host serializes every report under one lock, so the seed
//! can never land after a callback that already reported.
//!
//! # Failure handling
//!
//! [`start`] logs once and gives up when the platform handle is not
//! installed, the Gradle module is not wired into the app (the class load
//! throws), or the host refuses; [`crate::Connectivity::current`] then stays
//! `None` for the process. The export upgrades its [`EnvUnowned`] through
//! [`EnvUnowned::with_env`], which wraps the body in `catch_unwind`, so
//! nothing unwinds into the JVM; it only decodes and calls
//! [`crate::state::report`], which never blocks on a listener.

use jni::errors::LogErrorAndDefault;
use jni::objects::{JClass, JObject, JValue};
use jni::sys::{jboolean, jint, jlong};
use jni::{Env, EnvUnowned, jni_sig, jni_str};

use crate::state::{self, RawReport};
use crate::{NetworkId, NetworkPath, Transport};

/// The host's class name in the dotted form `ClassLoader.loadClass` expects.
const HOST_CLASS_BINARY: &str = "dev.frust.connectivity.FrustConnectivityHost";

/// `transport` code: Wi-Fi.
const TRANSPORT_CODE_WIFI: jint = 0;
/// `transport` code: a cellular radio.
const TRANSPORT_CODE_CELLULAR: jint = 1;
/// `transport` code: wired Ethernet.
const TRANSPORT_CODE_ETHERNET: jint = 2;
/// `transport` code: a VPN, or any transport the host does not name.
const TRANSPORT_CODE_OTHER: jint = 3;

/// Start the platform monitor. Called once per process.
pub(crate) fn start() {
    match frust_plugin::android::with_jni_env(start_host) {
        Ok(Ok(true)) => log::debug!("frust-connectivity: default-network callback registered"),
        Ok(Ok(false)) => log::warn!(
            "frust-connectivity: FrustConnectivityHost.start refused (no application context, or \
             the registration failed; see logcat tag FrustConnectivity) — no connectivity events \
             this process"
        ),
        Ok(Err(err)) => log::warn!(
            "frust-connectivity: could not start FrustConnectivityHost (is the plugin's Android \
             Gradle module wired into the app?): {err} — no connectivity events this process"
        ),
        Err(err) => log::warn!(
            "frust-connectivity: platform handle unavailable: {err} — no connectivity events this \
             process"
        ),
    }
}

/// Load the host through the application classloader and call its `start`.
///
/// The application `Context`'s classloader is the only one that sees app
/// classes; a bare `FindClass` off the main thread sees the bootstrap loader
/// only. The class is used once, so no global reference is cached. A pending
/// Java exception is described to logcat and cleared before returning, so the
/// next JNI call never runs with one pending.
fn start_host(env: &mut Env<'_>, context: &JObject) -> Result<bool, jni::errors::Error> {
    let result = call_start(env, context);
    if env.exception_check() {
        env.exception_describe();
        env.exception_clear();
    }
    result
}

fn call_start(env: &mut Env<'_>, context: &JObject) -> Result<bool, jni::errors::Error> {
    let loader = env
        .call_method(
            context,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )?
        .l()?;
    let name = env.new_string(HOST_CLASS_BINARY)?;
    let class = env
        .call_method(
            &loader,
            jni_str!("loadClass"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
            &[JValue::Object(&name)],
        )?
        .l()?;
    let class = env.cast_local::<JClass>(class)?;
    env.call_static_method(&class, jni_str!("start"), jni_sig!("()Z"), &[])?
        .z()
}

/// Decode one host report (module doc's contract table).
fn decode(usable: bool, handle: jlong, transport: jint, expensive: bool) -> RawReport {
    if !usable {
        return RawReport::Unusable;
    }
    let transport = match transport {
        TRANSPORT_CODE_WIFI => Transport::Wifi,
        TRANSPORT_CODE_CELLULAR => Transport::Cellular,
        TRANSPORT_CODE_ETHERNET => Transport::Ethernet,
        TRANSPORT_CODE_OTHER => Transport::Other,
        unknown => {
            log::debug!("frust-connectivity: unknown transport code {unknown}, reported as Other");
            Transport::Other
        }
    };
    RawReport::Usable(NetworkPath {
        // `networkHandle` is an opaque bit pattern compared only for
        // equality, so the sign-reinterpreting cast loses nothing.
        id: NetworkId::new(handle as u64),
        transport,
        expensive,
        constrained: false,
    })
}

/// `Java_dev_frust_connectivity_FrustConnectivityHost_nativeOnConnectivityEvent`
/// — one raw default-network report, on the host's `frust-connectivity`
/// thread (or the thread that called `start`, for the seed).
///
/// Decodes and feeds [`state::report`], which enqueues for the plugin's own
/// delivery thread and returns; no listener runs here. A panic is caught by
/// [`EnvUnowned::with_env`] and never unwinds into the JVM.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_frust_connectivity_FrustConnectivityHost_nativeOnConnectivityEvent<
    'local,
>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    usable: jboolean,
    handle: jlong,
    transport: jint,
    expensive: jboolean,
) {
    env.with_env(|_env| {
        state::report(decode(usable, handle, transport, expensive));
        Ok::<(), jni::errors::Error>(())
    })
    .resolve::<LogErrorAndDefault>();
}
