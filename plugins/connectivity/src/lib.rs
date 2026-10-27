//! `frust-connectivity`: default-network availability and change events —
//! Android `ConnectivityManager` and Apple Network.framework behind one
//! platform-independent API.
//!
//! # Charter: a platform plugin
//!
//! Like `frust-iap` and `frust-camera`, this is a **platform plugin**: it
//! depends on `frust-plugin` plus FFI crates only, and carries no other
//! `frust-*` framework dependency. An app adds it to its own `Cargo.toml`
//! alongside `frust`; the `frust` facade does not re-export it.
//!
//! # What it reports
//!
//! [`Connectivity::set_listener`] delivers a [`ConnectivityEvent`] when the
//! process's **default network** becomes usable
//! ([`ConnectivityEvent::Available`]), changes to a different network or
//! transport (`Available` again), or goes away
//! ([`ConnectivityEvent::Lost`]). [`Connectivity::current`] answers the latest
//! state at any time. "Usable" means Android `INTERNET` + `VALIDATED`, or an
//! Apple path whose status is satisfied. Repeated platform reports of the same
//! state are collapsed, so each transition arrives once.
//!
//! The platform monitor starts on the first call to either function — once
//! per process, never stopped. A usable first report after it starts is the
//! one marked `initial: true`; a device that starts offline emits nothing
//! until a network appears, and that `Available` has `initial: false`. A
//! listener registered later receives only later transitions, so read
//! [`Connectivity::current`] for the state at registration time.
//!
//! ```
//! use std::sync::mpsc;
//! use frust_connectivity::{Connectivity, ConnectivityEvent};
//!
//! let (tx, rx) = mpsc::channel();
//! // Keep the handle: dropping it unregisters the listener.
//! let _handle = Connectivity::set_listener(Box::new(move |event| {
//!     // Hand off and return; never block here.
//!     let _ = tx.send(event);
//! }));
//! if Connectivity::current().is_none() {
//!     // Offline, or no report yet.
//! }
//! # drop(rx);
//! ```
//!
//! # Threading contract
//!
//! A listener runs on the plugin's own `frust-connectivity-events` thread —
//! one thread for the whole process, never the UI thread and never the thread
//! the platform reported on, hence `Send + Sync`. It **must not block** and
//! must not write a signal directly: hand the event off (a channel,
//! `frust_reactive::use_task`, a write scheduled onto the UI thread) and
//! return. Every later event queues behind that one thread, in the order the
//! platform reported them; each listener sees each event, in registration
//! order. A listener that blocks anyway costs events rather than memory: the
//! queue is bounded and a full queue drops the newest event with a logged
//! warning, while [`Connectivity::current`] stays correct. A listener may
//! register another listener or drop a [`ListenerHandle`] from inside its
//! callback; a listener panic is caught and logged.
//!
//! # No events in the background
//!
//! While the app is suspended (iOS) or frozen/cached (Android) the OS
//! delivers no network callbacks, so a change that happens then produces no
//! event, and the first event after the app returns may already describe a
//! newer state. Pair the listener with `frust::AppLifecycleListener`'s
//! `on_restart`/`on_resume` and re-check [`Connectivity::current`] there.
//!
//! # Platforms
//!
//! | Platform | Backend | Minimum |
//! |---|---|---|
//! | Android | `ConnectivityManager` default-network callback | minSdk 29 |
//! | iOS | Network.framework `nw_path_monitor` | iOS 12 |
//! | macOS | Network.framework `nw_path_monitor` | macOS 10.14 |
//! | Linux, Windows, web | none: no events, `current()` is always `None` | — |
//!
//! On the no-op platforms nothing is ever reported: listeners are accepted and
//! never called. That is "unknown", not "offline" — do not gate network work
//! on `current()` there.

mod event;
mod state;
mod types;

#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
mod noop;

#[cfg(target_os = "android")]
use android as platform;
#[cfg(any(target_os = "ios", target_os = "macos"))]
use apple as platform;
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
use noop as platform;

use std::sync::OnceLock;

pub use event::ListenerHandle;
pub use types::{ConnectivityEvent, NetworkId, NetworkPath, Transport};

/// Default-network availability for this process. All functions are
/// associated: there is one default network, so there is no handle to hold.
pub struct Connectivity;

impl Connectivity {
    /// Register `listener` for every later [`ConnectivityEvent`], start the
    /// platform monitor if it is not running yet, and return the handle that
    /// keeps the listener registered.
    ///
    /// **Dropping the [`ListenerHandle`] unregisters the listener**, so
    /// `let _ = Connectivity::set_listener(cb);` unregisters it immediately.
    /// See the crate docs for the threading contract `listener` must honour.
    /// Several listeners may be registered at once; each sees every event.
    pub fn set_listener(listener: Box<dyn Fn(ConnectivityEvent) + Send + Sync>) -> ListenerHandle {
        let handle = event::register(listener);
        ensure_started();
        handle
    }

    /// The current default network: the path of the last
    /// [`ConnectivityEvent::Available`], refreshed when only its
    /// `expensive`/`constrained` flags change. `None` before the platform's
    /// first report, after [`ConnectivityEvent::Lost`], and always on a
    /// platform without a backend. Starts the platform monitor if it is not
    /// running yet, so a first call right after launch is usually `None`.
    ///
    /// Never blocks; callable from any thread.
    #[must_use]
    pub fn current() -> Option<NetworkPath> {
        ensure_started();
        state::current()
    }
}

static STARTED: OnceLock<()> = OnceLock::new();

/// Start the platform monitor, once per process.
fn ensure_started() {
    STARTED.get_or_init(|| {
        // Tests drive `state::report` directly; a real monitor would race
        // them with the host's actual network.
        if !cfg!(test) {
            platform::start();
        }
    });
}

/// Serialize a test over the process-global state and start it clean: no
/// queued events, no listeners, no reports.
#[cfg(test)]
pub(crate) fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let guard = TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    event::flush();
    event::clear_listeners();
    state::reset();
    guard
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::state::{RawReport, report};

    fn wifi() -> NetworkPath {
        NetworkPath {
            id: NetworkId::new(7),
            transport: Transport::Wifi,
            expensive: false,
            constrained: false,
        }
    }

    #[test]
    fn set_listener_delivers_reports_until_its_handle_drops() {
        let _guard = test_guard();
        let (tx, rx) = mpsc::channel();
        let handle = Connectivity::set_listener(Box::new(move |event| tx.send(event).unwrap()));
        report(RawReport::Usable(wifi()));
        event::flush();
        assert_eq!(
            rx.try_iter().collect::<Vec<_>>(),
            [ConnectivityEvent::Available {
                path: wifi(),
                initial: true,
            }]
        );
        drop(handle);
        report(RawReport::Unusable);
        event::flush();
        assert!(rx.try_recv().is_err(), "the dropped listener saw nothing");
    }

    #[test]
    fn current_reads_the_reported_state() {
        let _guard = test_guard();
        assert_eq!(Connectivity::current(), None, "no report yet");
        report(RawReport::Usable(wifi()));
        assert_eq!(Connectivity::current(), Some(wifi()));
        report(RawReport::Unusable);
        assert_eq!(Connectivity::current(), None, "after Lost");
    }

    #[test]
    fn network_id_round_trips_its_raw_value() {
        assert_eq!(NetworkId::new(42).as_u64(), 42);
        assert_eq!(NetworkId::new(42), NetworkId::new(42));
        assert_ne!(NetworkId::new(42), NetworkId::new(43));
    }
}
