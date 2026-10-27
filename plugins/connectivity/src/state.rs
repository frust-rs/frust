//! The dedup state machine between the platform arms and the listeners.
//!
//! A platform arm reports the raw state of the default network as often as
//! its OS tells it anything — Android's `NetworkCallback` fires for capability
//! and link-property changes, Network.framework re-delivers the path on every
//! interface tweak. [`report`] collapses that stream into the two events an
//! app cares about, so a listener sees each transition exactly once.
//!
//! # Rules
//!
//! - **The first report after the monitor starts** decides the initial state.
//!   A usable network emits `Available { initial: true }`. An unusable one
//!   emits **nothing**: the app starts offline, [`current`] answers `None`,
//!   and the initial report counts as delivered, so the next usable network is
//!   `Available { initial: false }`. There is no "initially lost" event
//!   because nothing was lost.
//! - **A later usable report** of the same network on the same transport only
//!   refreshes the stored path (its `expensive`/`constrained` flags may have
//!   changed) and emits nothing. A different network, or the same network on a
//!   different transport, emits `Available { initial: false }`.
//! - **A later unusable report** emits `Lost` if the last event was
//!   `Available`, and nothing otherwise — two unusable reports in a row are one
//!   `Lost`.
//!
//! # Ordering
//!
//! One mutex guards the state, and the event is enqueued while it is held, so
//! two platform threads reporting at once can never enqueue their events in
//! the opposite order from the state transitions they made. Enqueueing never
//! blocks (see `event.rs`), so holding the lock across it is cheap.

use std::sync::{Mutex, MutexGuard};

use crate::{ConnectivityEvent, NetworkPath, event};

/// What a platform arm observed about the default network.
// Only the Android/Apple arms (and tests) report; a no-op build never does.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum RawReport {
    /// A usable default network (Android: `INTERNET` + `VALIDATED`; Apple: a
    /// satisfied path).
    Usable(NetworkPath),
    /// No usable default network.
    Unusable,
}

struct State {
    /// Whether the first report after start has been seen.
    initial_delivered: bool,
    /// The path of the last `Available` emitted, refreshed in place by
    /// same-network reports; `None` before any report, and after `Lost`.
    current: Option<NetworkPath>,
}

static STATE: Mutex<State> = Mutex::new(State {
    initial_delivered: false,
    current: None,
});

fn state() -> MutexGuard<'static, State> {
    STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Feed one raw observation in. Callable from any thread; never blocks on a
/// listener.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn report(raw: RawReport) {
    let mut state = state();
    if let Some(event) = transition(&mut state, raw) {
        event::emit(event);
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn transition(state: &mut State, raw: RawReport) -> Option<ConnectivityEvent> {
    let initial = !state.initial_delivered;
    state.initial_delivered = true;
    match raw {
        RawReport::Usable(path) => {
            let same_network = state
                .current
                .is_some_and(|last| last.id == path.id && last.transport == path.transport);
            state.current = Some(path);
            (!same_network).then_some(ConnectivityEvent::Available { path, initial })
        }
        RawReport::Unusable => state.current.take().map(|_| ConnectivityEvent::Lost),
    }
}

/// The last `Available` path, refreshed by same-network reports; `None` before
/// any usable report and after `Lost`.
pub(crate) fn current() -> Option<NetworkPath> {
    state().current
}

/// Forget every report, as if the monitor had never started.
#[cfg(test)]
pub(crate) fn reset() {
    *state() = State {
        initial_delivered: false,
        current: None,
    };
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::mpsc::{self, Receiver};

    use super::*;
    use crate::event::flush;
    use crate::{ListenerHandle, NetworkId, Transport, test_guard};

    pub(crate) fn path(id: u64, transport: Transport) -> NetworkPath {
        NetworkPath {
            id: NetworkId::new(id),
            transport,
            expensive: false,
            constrained: false,
        }
    }

    fn listen() -> (ListenerHandle, Receiver<ConnectivityEvent>) {
        let (tx, rx) = mpsc::channel();
        let handle = event::register(Box::new(move |event| tx.send(event).unwrap()));
        (handle, rx)
    }

    /// Report `raw`, then everything the listener received for it.
    fn step(rx: &Receiver<ConnectivityEvent>, raw: RawReport) -> Vec<ConnectivityEvent> {
        report(raw);
        flush();
        rx.try_iter().collect()
    }

    fn available(path: NetworkPath, initial: bool) -> ConnectivityEvent {
        ConnectivityEvent::Available { path, initial }
    }

    #[test]
    fn the_initial_usable_report_is_available_initial() {
        let _guard = test_guard();
        let (_handle, rx) = listen();
        let wifi = path(1, Transport::Wifi);
        assert_eq!(step(&rx, RawReport::Usable(wifi)), [available(wifi, true)]);
    }

    #[test]
    fn a_duplicate_usable_report_emits_nothing() {
        let _guard = test_guard();
        let (_handle, rx) = listen();
        let wifi = path(1, Transport::Wifi);
        step(&rx, RawReport::Usable(wifi));
        assert_eq!(step(&rx, RawReport::Usable(wifi)), []);
    }

    #[test]
    fn a_flag_change_on_the_same_network_updates_current_silently() {
        let _guard = test_guard();
        let (_handle, rx) = listen();
        let wifi = path(1, Transport::Wifi);
        step(&rx, RawReport::Usable(wifi));
        let metered = NetworkPath {
            expensive: true,
            constrained: true,
            ..wifi
        };
        assert_eq!(step(&rx, RawReport::Usable(metered)), []);
        assert_eq!(current(), Some(metered));
    }

    #[test]
    fn the_same_id_on_a_different_transport_is_available() {
        let _guard = test_guard();
        let (_handle, rx) = listen();
        step(&rx, RawReport::Usable(path(1, Transport::Wifi)));
        let cellular = path(1, Transport::Cellular);
        assert_eq!(
            step(&rx, RawReport::Usable(cellular)),
            [available(cellular, false)]
        );
    }

    #[test]
    fn a_different_id_is_available() {
        let _guard = test_guard();
        let (_handle, rx) = listen();
        step(&rx, RawReport::Usable(path(1, Transport::Wifi)));
        let other = path(2, Transport::Wifi);
        assert_eq!(
            step(&rx, RawReport::Usable(other)),
            [available(other, false)]
        );
    }

    #[test]
    fn unusable_after_available_is_lost_once() {
        let _guard = test_guard();
        let (_handle, rx) = listen();
        step(&rx, RawReport::Usable(path(1, Transport::Wifi)));
        assert_eq!(step(&rx, RawReport::Unusable), [ConnectivityEvent::Lost]);
        assert_eq!(
            step(&rx, RawReport::Unusable),
            [],
            "two unusable reports, one Lost"
        );
    }

    #[test]
    fn usable_after_lost_is_available_not_initial() {
        let _guard = test_guard();
        let (_handle, rx) = listen();
        let wifi = path(1, Transport::Wifi);
        step(&rx, RawReport::Usable(wifi));
        step(&rx, RawReport::Unusable);
        assert_eq!(step(&rx, RawReport::Usable(wifi)), [available(wifi, false)]);
    }

    #[test]
    fn an_initial_unusable_report_emits_nothing_and_consumes_initial() {
        let _guard = test_guard();
        let (_handle, rx) = listen();
        assert_eq!(step(&rx, RawReport::Unusable), [], "nothing was lost");
        assert_eq!(current(), None);
        let wifi = path(1, Transport::Wifi);
        assert_eq!(
            step(&rx, RawReport::Usable(wifi)),
            [available(wifi, false)],
            "the initial report was the unusable one"
        );
    }

    #[test]
    fn current_tracks_every_transition() {
        let _guard = test_guard();
        let (_handle, rx) = listen();
        assert_eq!(current(), None, "no report yet");
        let wifi = path(1, Transport::Wifi);
        step(&rx, RawReport::Usable(wifi));
        assert_eq!(current(), Some(wifi));
        let cellular = path(2, Transport::Cellular);
        step(&rx, RawReport::Usable(cellular));
        assert_eq!(current(), Some(cellular));
        step(&rx, RawReport::Unusable);
        assert_eq!(current(), None, "after Lost");
    }
}
