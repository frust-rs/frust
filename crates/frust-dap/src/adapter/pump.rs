//! The background pump a launched session owns.
//!
//! One feed, two tasks: the backend's `SessionEvent` stream becomes DAP
//! `output` events for every log line, and `exited` + `terminated` for the
//! session's end.
//!
//! ## Why the pump is two tasks
//!
//! `frust_mcp::engine::SessionEventFeed` is a blocking, `Send`-but-not-`Sync`
//! receiver — it has to be owned by one thread and read with a blocking
//! `recv`. So the reader half runs on [`tokio::task::spawn_blocking`] and
//! hands events to an async forwarder over a bounded channel. Back-pressure
//! composes the right way round: a DAP client that stops reading stalls the
//! forwarder, which fills the bridge, which parks the blocking reader, which
//! makes the *feed* drop events with its own in-band marker — never the
//! session's ingest thread.
//!
//! ## Why the exit is not watched separately
//!
//! The feed carries the exit as its last event, after the app's final lines,
//! so "the app's output lands before `exited`" is a property of the stream
//! rather than of a flush grace this module has to guess at — and there is no
//! polling of session state at all.
//!
//! ## Silencing
//!
//! Both tasks share one cancel flag. It is raised before the adapter itself
//! stops or restarts the session, because the backend reports a
//! deliberately-stopped session exactly like an app that died on its own: the
//! forwarder must not announce an `exited` for a stop the client asked for.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use frust_mcp::SharedBackend;
use frust_mcp::engine::{SessionEvent, SessionId, SessionState};
use tokio::task::AbortHandle;

use super::TerminatedOnce;
use crate::server::EventSender;

/// How long the blocking reader parks on the feed before re-checking the
/// cancel flag.
///
/// Not a poll interval: an event wakes the `recv` immediately. The tick only
/// bounds how long a *cancelled* reader (or one whose feed never closes) stays
/// parked, which in turn bounds how long a host's runtime shutdown can wait on
/// it.
const PUMP_TICK: Duration = Duration::from_millis(200);

/// Events buffered between the blocking feed reader and the async forwarder.
/// Deep enough that a build's output burst never parks the reader, shallow
/// enough that a stalled client is felt by the feed (which reports its loss in
/// band) rather than hoarded here.
const BRIDGE_CAPACITY: usize = 256;

/// Exit code reported for a session that ended successfully.
///
/// The backend's `SessionState::Exited` carries a **success flag, not a real
/// exit code** — `frust_drive::process::StreamHandle` does not expose one — so
/// every DAP `exited` event from this adapter is 0 or 1 and nothing else.
const EXIT_OK: i64 = 0;

/// Exit code reported for a session that failed, was killed, or never
/// launched.
const EXIT_FAILED: i64 = 1;

/// The background tasks belonging to one launched session.
pub(crate) struct Pumps {
    cancel: Arc<AtomicBool>,
    forward: AbortHandle,
}

impl Pumps {
    /// Starts the event pump for `session`.
    pub(crate) fn spawn(
        backend: SharedBackend,
        session: SessionId,
        events: EventSender,
        terminated: TerminatedOnce,
    ) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let (bridge_tx, bridge_rx) = tokio::sync::mpsc::channel::<SessionEvent>(BRIDGE_CAPACITY);

        // The feed is opened *inside* the blocking task: subscribing is a
        // backend call like any other (it may reach a supervisor that is not
        // this process's own data structure), and the feed is not `Sync`, so
        // the thread that reads it may as well be the thread that opens it.
        // Nothing is lost by opening late — the feed starts from the session's
        // retained backlog, not from whatever arrives next.
        let reader_cancel = Arc::clone(&cancel);
        tokio::task::spawn_blocking(move || {
            read_feed(backend, session, &bridge_tx, &reader_cancel);
        });

        let forward = tokio::spawn(forward_events(
            bridge_rx,
            events,
            terminated,
            Arc::clone(&cancel),
        ));

        Self {
            cancel,
            forward: forward.abort_handle(),
        }
    }

    /// Silences both tasks without waiting for them.
    ///
    /// Raised *before* the adapter stops or restarts the session, so the
    /// backend-side transition that stop produces is never reported as the app
    /// exiting on its own.
    pub(crate) fn silence(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Silences and then drops both tasks.
    ///
    /// The abort is what releases a forwarder parked on a client that stopped
    /// reading; the blocking reader behind it then observes a closed bridge
    /// and returns on its own.
    pub(crate) fn stop(self) {
        self.silence();
        self.forward.abort();
    }
}

/// Drains the session's event feed into the bridge until it ends.
///
/// Blocking throughout — this is the body of a [`tokio::task::spawn_blocking`]
/// closure and must never be awaited on a runtime thread.
fn read_feed(
    backend: SharedBackend,
    session: SessionId,
    bridge: &tokio::sync::mpsc::Sender<SessionEvent>,
    cancel: &AtomicBool,
) {
    let Some(feed) = backend.subscribe_session_events(session) else {
        // Either the session vanished, or this backend serves no event feed
        // at all: the connection stays usable, it just carries no app output
        // and reports no exit of its own.
        log::warn!(
            "DAP: no session-event feed for session {session}; this connection will report \
             neither the app's output nor its exit"
        );
        return;
    };

    while !cancel.load(Ordering::SeqCst) {
        match feed.recv_timeout(PUMP_TICK) {
            Ok(event) => {
                // The feed sends nothing after `Exited`, so neither does this.
                let last = matches!(event, SessionEvent::Exited { .. });
                if bridge.blocking_send(event).is_err() || last {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Turns the bridged events into DAP events, in the order the feed produced
/// them — which is what makes the app's final output land *before* `exited`.
async fn forward_events(
    mut bridge: tokio::sync::mpsc::Receiver<SessionEvent>,
    events: EventSender,
    terminated: TerminatedOnce,
    cancel: Arc<AtomicBool>,
) {
    while let Some(event) = bridge.recv().await {
        match event {
            SessionEvent::Log(line) => {
                // DAP `output` bodies are concatenated verbatim by the client;
                // backend log lines carry no terminator of their own.
                if !events.output("stdout", &format!("{line}\n")).await {
                    return;
                }
            }
            SessionEvent::Exited { state } => {
                if !cancel.load(Ordering::SeqCst) {
                    report_exit(&events, &terminated, &state).await;
                }
                return;
            }
        }
    }
}

/// Reports one terminal state as `exited` plus (at most once) `terminated`.
async fn report_exit(events: &EventSender, terminated: &TerminatedOnce, state: &SessionState) {
    let code = match state {
        SessionState::Exited { success: true } => EXIT_OK,
        SessionState::Exited { success: false } => EXIT_FAILED,
        SessionState::Failed { reason } => {
            // A launch that never spawned is reported the same way one that
            // died minutes into a device build is — either way the reason is
            // the only thing the developer can act on.
            let _ = events
                .output("console", &format!("Launch failed: {reason}\n"))
                .await;
            EXIT_FAILED
        }
        // The feed only ever sends a terminal state here; a live one would
        // mean the backend contract was broken, not that the app is fine.
        other => {
            log::error!("DAP: a session event reported non-terminal state {other:?} as final");
            EXIT_FAILED
        }
    };

    let _ = events.exited(code).await;
    terminated.emit(events).await;
}
