//! The Apple arm (iOS and macOS): one Network.framework `nw_path_monitor` on
//! a private serial queue, reporting into [`crate::state::report`].
//!
//! Network.framework's path API is plain C, so it is declared by hand below
//! rather than through a binding crate: `block2` builds the update handler,
//! `dispatch2` the queue it runs on. macOS and iOS share every line.
//!
//! # One monitor per process
//!
//! [`start`] creates the monitor, a serial queue labelled
//! [`QUEUE_LABEL`], and the update-handler block, then starts the monitor
//! and leaks all three on purpose: the monitor reports for the life of the
//! process and is never cancelled, so nothing would ever release them.
//! Network.framework calls the handler once with the current path right after
//! the start, and again on every path change.
//!
//! # Translation
//!
//! A path whose status is `nw_path_status_satisfied` is
//! [`RawReport::Usable`]; every other status (`unsatisfied`, `satisfiable`,
//! `invalid`) is [`RawReport::Unusable`]. The transport is the first of Wi-Fi,
//! cellular, wired that the path uses, else [`Transport::Other`] (a VPN-only
//! or loopback path). `expensive` and `constrained` are
//! `nw_path_is_expensive`/`nw_path_is_constrained` — both inside the iOS 15 /
//! macOS 11 deployment floors, so no availability check guards them.
//!
//! The [`NetworkId`] is **synthetic**: Apple exposes no stable network
//! identity, so the id packs the path's first (preferred) interface — its
//! index shifted left by 8, or'ed with its `nw_interface_type_t` (see
//! [`interface_id`]). A satisfied path that lists no interface gets a hash of
//! its transport instead. Network.framework re-delivers the path for changes
//! that keep the same interface; the state machine's `(id, transport)` dedup
//! collapses those, refreshing only `expensive`/`constrained`.
//!
//! # Borrowed objects
//!
//! The `nw_path_t` handed to the update handler is valid only while the
//! handler runs, and each `nw_interface_t` only while the enumeration block
//! runs. Both are read in place and never stored, retained, or returned: the
//! handler copies plain values out ([`read_path`]) before it returns.
//!
//! # No unwind into libdispatch
//!
//! The handler body runs under `catch_unwind`; a panic is logged once and the
//! update is dropped, never unwound into the libdispatch frame that called
//! the block.
//!
//! # Suspension, entitlements, linking
//!
//! A suspended iOS app receives no path updates; the monitor delivers the then
//! current path when the app resumes, which is why the crate docs pair the
//! listener with the lifecycle callbacks. Path monitoring needs no entitlement
//! and no `Info.plist` key on either platform. The `#[link]` attribute below
//! links Network.framework for a macOS binary; an iOS app links this crate as
//! part of a static library, where that attribute does not reach the final
//! link, so the plugin registry contributes `-framework Network` there.

use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::ffi::c_void;
use std::hash::{Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use block2::{Block, RcBlock, StackBlock};
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};

use crate::state::{self, RawReport};
use crate::{NetworkId, NetworkPath, Transport};

/// The label of the serial queue every path update runs on.
const QUEUE_LABEL: &str = "dev.frust.connectivity";

// The Network.framework object types are opaque, reference-counted objects.
// They stay raw pointers (not newtypes) because a `block2` block argument
// must be an `objc2` `Encode` type, and a raw `c_void` pointer is one.

/// `nw_path_monitor_t`.
type NwPathMonitor = *mut c_void;
/// `nw_path_t`: borrowed for the duration of the update handler only.
type NwPath = *mut c_void;
/// `nw_interface_t`: borrowed for the duration of the enumeration block only.
type NwInterface = *mut c_void;
/// `nw_path_status_t`, a C enum with no negative value (`unsigned int`).
type NwPathStatus = u32;
/// `nw_interface_type_t`, a C enum with no negative value (`unsigned int`).
type NwInterfaceType = u32;

/// The C `bool` an enumeration block returns, as a `u8` holding 0 or 1.
///
/// `block2` admits only `objc2`-encodable block return types, which excludes
/// Rust `bool`. A `u8` of 0 or 1 is ABI-identical to C `_Bool` as a return
/// value on every Apple target (one byte, zero-extended), so the framework
/// reads it as the `bool` it declares.
type CBool = u8;
/// C `false`, as a [`CBool`].
const C_FALSE: CBool = 0;

/// `nw_path_status_satisfied`: the path has a usable route.
const NW_PATH_STATUS_SATISFIED: NwPathStatus = 1;
/// `nw_interface_type_wifi`.
const NW_INTERFACE_TYPE_WIFI: NwInterfaceType = 1;
/// `nw_interface_type_cellular`.
const NW_INTERFACE_TYPE_CELLULAR: NwInterfaceType = 2;
/// `nw_interface_type_wired`.
const NW_INTERFACE_TYPE_WIRED: NwInterfaceType = 3;

// Checked against Network.framework's `path_monitor.h`, `path.h` and
// `interface.h` in the iPhoneOS 27.0 and MacOSX 27.0 SDKs (identical in both).
// Every function is iOS 12 / macOS 10.14 except `nw_path_is_constrained`
// (iOS 13 / macOS 10.15); all are below this workspace's floors.
#[link(name = "Network", kind = "framework")]
unsafe extern "C" {
    /// Returns a retained monitor, or null on failure.
    fn nw_path_monitor_create() -> NwPathMonitor;
    fn nw_path_monitor_set_queue(monitor: NwPathMonitor, queue: &DispatchQueue);
    /// `nw_path_monitor_update_handler_t` is `void (^)(nw_path_t)`.
    fn nw_path_monitor_set_update_handler(monitor: NwPathMonitor, handler: &Block<dyn Fn(NwPath)>);
    fn nw_path_monitor_start(monitor: NwPathMonitor);
    fn nw_path_get_status(path: NwPath) -> NwPathStatus;
    fn nw_path_uses_interface_type(path: NwPath, interface_type: NwInterfaceType) -> bool;
    /// `nw_path_enumerate_interfaces_block_t` is `bool (^)(nw_interface_t)`,
    /// `NW_NOESCAPE`; returning `false` stops the enumeration. The block
    /// returns [`CBool`] (see there).
    fn nw_path_enumerate_interfaces(
        path: NwPath,
        enumerate_block: &Block<dyn Fn(NwInterface) -> CBool + '_>,
    );
    fn nw_path_is_expensive(path: NwPath) -> bool;
    fn nw_path_is_constrained(path: NwPath) -> bool;
    fn nw_interface_get_index(interface: NwInterface) -> u32;
    fn nw_interface_get_type(interface: NwInterface) -> NwInterfaceType;
}

/// Start the platform monitor. Idempotent: the crate calls it once per
/// process, and a second call is a no-op.
pub(crate) fn start() {
    static STARTED: Once = Once::new();
    STARTED.call_once(start_monitor);
}

fn start_monitor() {
    // SAFETY: `nw_path_monitor_create` takes no arguments; it returns a
    // retained monitor or null, checked below.
    let monitor = unsafe { nw_path_monitor_create() };
    if monitor.is_null() {
        log::error!(
            "frust-connectivity: nw_path_monitor_create returned null; no connectivity events \
             will be reported"
        );
        return;
    }
    let queue = DispatchQueue::new(QUEUE_LABEL, DispatchQueueAttr::SERIAL);
    let handler = RcBlock::new(|path: NwPath| on_path_update(path));
    // SAFETY: `monitor` is the live, non-null monitor created above; `queue`
    // and `handler` are live for these calls (and leaked below, so for the
    // process). The queue and the handler are set before the start, as
    // `path_monitor.h` requires; the handler has the `void (^)(nw_path_t)`
    // shape the monitor calls.
    unsafe {
        nw_path_monitor_set_queue(monitor, &queue);
        nw_path_monitor_set_update_handler(monitor, &handler);
        nw_path_monitor_start(monitor);
    }
    // Leaked on purpose (see the module docs): the monitor is never cancelled,
    // so its retain from `nw_path_monitor_create` is never released either,
    // and our references to the queue and the handler block stay with it.
    let _ = DispatchRetained::into_raw(queue);
    std::mem::forget(handler);
}

/// Whether a handler panic has been logged already: one line, not one per
/// path update.
static PANIC_LOGGED: AtomicBool = AtomicBool::new(false);

/// The update handler's body, on the monitor's serial queue.
fn on_path_update(path: NwPath) {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `path` is the update handler's own argument, which
        // Network.framework keeps valid for the duration of this call — and
        // `read_path` does not let it escape.
        let raw = unsafe { read_path(path) };
        state::report(raw);
    }));
    if outcome.is_err() && !PANIC_LOGGED.swap(true, Ordering::Relaxed) {
        log::error!(
            "frust-connectivity: the path update handler panicked; that update was dropped \
             (logged once)"
        );
    }
}

/// Translate one path into a [`RawReport`], copying every value out of it.
///
/// # Safety
///
/// `path` must be a valid `nw_path_t` for the duration of the call. It is not
/// retained or stored.
unsafe fn read_path(path: NwPath) -> RawReport {
    // SAFETY: `path` is valid for this call (the caller's contract).
    if unsafe { nw_path_get_status(path) } != NW_PATH_STATUS_SATISFIED {
        return RawReport::Unusable;
    }
    // SAFETY: as above, for each call.
    let transport = pick_transport(|kind| unsafe { nw_path_uses_interface_type(path, kind) });
    // SAFETY: as above.
    let primary = unsafe { primary_interface(path) };
    let id = match primary {
        Some((index, kind)) => interface_id(index, kind),
        None => transport_id(transport),
    };
    RawReport::Usable(NetworkPath {
        id,
        transport,
        // SAFETY: as above.
        expensive: unsafe { nw_path_is_expensive(path) },
        // SAFETY: as above; the function is inside the deployment floors.
        constrained: unsafe { nw_path_is_constrained(path) },
    })
}

/// The `(index, type)` of the path's first enumerated interface — the one
/// Network.framework prefers — or `None` if it lists none.
///
/// # Safety
///
/// `path` must be a valid `nw_path_t` for the duration of the call.
unsafe fn primary_interface(path: NwPath) -> Option<(u32, NwInterfaceType)> {
    let found = Cell::new(None);
    // A stack block suffices: the enumeration block is `NW_NOESCAPE`. The
    // body cannot panic, so nothing unwinds through the enumerating frame.
    let enumerate = StackBlock::new(|interface: NwInterface| {
        // SAFETY: `interface` is the enumeration block's own argument, valid
        // for the duration of this call; only plain values are copied out.
        let identity = unsafe {
            (
                nw_interface_get_index(interface),
                nw_interface_get_type(interface),
            )
        };
        found.set(Some(identity));
        // Stop after the first interface.
        C_FALSE
    });
    // SAFETY: `path` is valid for this call (the caller's contract) and
    // `enumerate` is a live block of the `bool (^)(nw_interface_t)` shape,
    // which the non-escaping enumeration never keeps past its return.
    unsafe { nw_path_enumerate_interfaces(path, &enumerate) };
    found.get()
}

/// The path's transport: the first of Wi-Fi, cellular, wired that `uses`
/// reports, else [`Transport::Other`].
fn pick_transport(uses: impl Fn(NwInterfaceType) -> bool) -> Transport {
    if uses(NW_INTERFACE_TYPE_WIFI) {
        Transport::Wifi
    } else if uses(NW_INTERFACE_TYPE_CELLULAR) {
        Transport::Cellular
    } else if uses(NW_INTERFACE_TYPE_WIRED) {
        Transport::Ethernet
    } else {
        Transport::Other
    }
}

/// The synthetic id of a path whose preferred interface has `index` and
/// `kind`: `(index << 8) | kind`. `kind` is a small enum (0..=4), so the low
/// byte holds it whole.
fn interface_id(index: u32, kind: NwInterfaceType) -> NetworkId {
    NetworkId::new((u64::from(index) << 8) | u64::from(kind & 0xff))
}

/// The synthetic id of a satisfied path that lists no interface: a hash of
/// its transport, stable within the process.
fn transport_id(transport: Transport) -> NetworkId {
    let mut hasher = DefaultHasher::new();
    transport.hash(&mut hasher);
    NetworkId::new(hasher.finish())
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::{Connectivity, ConnectivityEvent, event, test_guard};

    #[test]
    fn transport_prefers_wifi_then_cellular_then_wired() {
        assert_eq!(pick_transport(|_| true), Transport::Wifi);
        assert_eq!(
            pick_transport(|kind| kind != NW_INTERFACE_TYPE_WIFI),
            Transport::Cellular
        );
        assert_eq!(
            pick_transport(|kind| kind == NW_INTERFACE_TYPE_WIRED),
            Transport::Ethernet
        );
        assert_eq!(pick_transport(|_| false), Transport::Other);
    }

    #[test]
    fn interface_id_packs_the_index_above_the_type() {
        assert_eq!(
            interface_id(14, NW_INTERFACE_TYPE_WIFI).as_u64(),
            (14 << 8) | 1
        );
        assert_ne!(
            interface_id(14, NW_INTERFACE_TYPE_WIFI),
            interface_id(14, NW_INTERFACE_TYPE_WIRED),
            "same index, different type"
        );
        assert_ne!(
            interface_id(14, NW_INTERFACE_TYPE_WIFI),
            interface_id(15, NW_INTERFACE_TYPE_WIFI),
            "same type, different index"
        );
    }

    #[test]
    fn an_interfaceless_path_gets_a_stable_per_transport_id() {
        assert_eq!(transport_id(Transport::Wifi), transport_id(Transport::Wifi));
        assert_ne!(
            transport_id(Transport::Wifi),
            transport_id(Transport::Cellular)
        );
    }

    /// How long [`apple_live`] listens, in seconds; overridable through
    /// `FRUST_CONNECTIVITY_LIVE_SECS` to leave time to toggle Wi-Fi by hand.
    const LIVE_DEFAULT_SECS: u64 = 5;

    /// Manual: starts the real monitor against the host's network and prints
    /// every event. Run it alone (`cargo test -p frust-connectivity --
    /// --ignored apple_live --nocapture`): the monitor keeps reporting into the
    /// process-global state after it returns, which would race any other test
    /// in the same run. A host with a network prints
    /// `Available { initial: true }` first; toggling Wi-Fi inside the window
    /// prints `Lost` and a later `Available`.
    #[test]
    #[ignore = "manual: reads the host's live network through Network.framework"]
    fn apple_live() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();
        let _handle = event::register(Box::new(move |event| {
            let _ = tx.send(event);
        }));
        // The crate's own start path skips the monitor under `cfg(test)`, so
        // start it directly.
        start();

        let secs = std::env::var("FRUST_CONNECTIVITY_LIVE_SECS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(LIVE_DEFAULT_SECS);
        println!("apple_live: listening for {secs} s");
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut events = Vec::new();
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(event) => {
                    println!("apple_live: {event:?}");
                    events.push(event);
                }
                Err(_) => break,
            }
        }
        println!("apple_live: current() = {:?}", Connectivity::current());

        match events.first() {
            Some(ConnectivityEvent::Available { initial, .. }) => {
                assert!(initial, "the first report after start is the initial one");
            }
            other => panic!(
                "expected Available {{ initial: true }} first, got {other:?}: is the host offline?"
            ),
        }
    }
}
