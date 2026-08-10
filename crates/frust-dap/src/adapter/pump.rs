//! The two background tasks a launched session owns.
//!
//! - The **log pump** turns every line the engine ingests into a DAP `output`
//!   event (`stdout` category).
//! - The **exit watch** turns the session's terminal state into `exited` plus
//!   `terminated`.
//!
//! ## Why the log pump is two tasks
//!
//! `frust_mcp::engine::LogSubscription` is a blocking, `Send`-but-not-`Sync`
//! receiver — it has to be owned by one thread and read with a blocking
//! `recv`. So the reader half runs on [`tokio::task::spawn_blocking`] and
//! hands lines to an async forwarder over a bounded channel. Back-pressure
//! composes the right way round: a DAP client that stops reading stalls the
//! forwarder, which fills the bridge, which parks the blocking reader, which
//! makes the *subscription* drop lines with its own in-band marker — never the
//! session's ingest thread.
//!
//! ## Silencing
//!
//! Both tasks share one cancel flag. It is raised before the adapter itself
//! stops or restarts the session, because the engine reports a
//! deliberately-stopped session exactly like an app that died on its own: the
//! watch must not announce an `exited` for a stop the client asked for.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use frust_mcp::SessionEngine;
use frust_mcp::engine::{SessionId, SessionState};
use tokio::task::{AbortHandle, JoinHandle};

use super::TerminatedOnce;
use crate::server::EventSender;

/// How long a pump blocks on its source before re-checking the cancel flag.
///
/// The exit watch does not *poll* on this interval — it waits on the session's
/// own change signal and settles the instant the engine records the
/// transition; the tick only bounds how long a cancelled watch (or a blocking
/// log reader whose feed never closes) stays parked, which in turn bounds how
/// long runtime shutdown can wait for it.
const PUMP_TICK: Duration = Duration::from_millis(200);

/// Lines buffered between the blocking subscription reader and the async
/// forwarder. Deep enough that a build's output burst never parks the reader,
/// shallow enough that a stalled client is felt by the subscription (which
/// reports its loss in band) rather than hoarded here.
const BRIDGE_CAPACITY: usize = 256;

/// How long the exit watch lets the log pump drain before announcing the exit.
///
/// The app's last lines are the ones a developer wants most, so `exited` waits
/// for them — but only for a bounded moment: a client that has stopped reading
/// must not be able to suppress the exit events entirely.
const LOG_FLUSH_GRACE: Duration = Duration::from_secs(2);

/// Exit code reported for a session that ended successfully.
///
/// The engine's `SessionState::Exited` carries a **success flag, not a real
/// exit code** — `frust_drive::process::StreamHandle` does not expose one — so
/// every DAP `exited` event from this adapter is 0 or 1 and nothing else.
const EXIT_OK: i64 = 0;

/// Exit code reported for a session that failed, was killed, or never
/// launched.
const EXIT_FAILED: i64 = 1;

/// The background tasks belonging to one launched session.
pub(crate) struct Pumps {
    cancel: Arc<AtomicBool>,
    log: AbortHandle,
    watch: AbortHandle,
}

impl Pumps {
    /// Starts the log pump and the exit watch for `session`.
    pub(crate) fn spawn(
        engine: Arc<SessionEngine>,
        session: SessionId,
        events: EventSender,
        terminated: TerminatedOnce,
    ) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let log = spawn_log_pump(
            Arc::clone(&engine),
            session,
            events.clone(),
            Arc::clone(&cancel),
        );
        let log_abort = log.abort_handle();
        let watch = tokio::spawn(watch_exit(
            engine,
            session,
            events,
            terminated,
            Arc::clone(&cancel),
            log,
        ));
        Self {
            cancel,
            log: log_abort,
            watch: watch.abort_handle(),
        }
    }

    /// Silences both tasks without waiting for them.
    ///
    /// Raised *before* the adapter stops or restarts the session, so the
    /// engine-side transition that stop produces is never reported as the
    /// app exiting on its own.
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
        self.log.abort();
        self.watch.abort();
    }
}

/// Forwards the session's log lines as `output` events until the feed ends.
fn spawn_log_pump(
    engine: Arc<SessionEngine>,
    session: SessionId,
    events: EventSender,
    cancel: Arc<AtomicBool>,
) -> JoinHandle<()> {
    let (bridge_tx, mut bridge_rx) = tokio::sync::mpsc::channel::<String>(BRIDGE_CAPACITY);

    // The subscription is opened *inside* the blocking task: it is not `Sync`,
    // so the thread that reads it must also be the thread that owns it — and
    // opening it there loses nothing, since the feed opens with the session's
    // retained backlog rather than with whatever arrives next.
    tokio::task::spawn_blocking(move || {
        let Some(subscription) = engine.subscribe_logs(session) else {
            log::debug!("DAP: session {session} vanished before its log pump started");
            return;
        };
        while !cancel.load(Ordering::SeqCst) {
            match subscription.recv_timeout(PUMP_TICK) {
                Ok(line) => {
                    if bridge_tx.blocking_send(line).is_err() {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });

    tokio::spawn(async move {
        while let Some(line) = bridge_rx.recv().await {
            // DAP `output` bodies are concatenated verbatim by the client;
            // engine log lines carry no terminator of their own.
            if !events.output("stdout", &format!("{line}\n")).await {
                return;
            }
        }
    })
}

/// Waits for the session to reach a terminal state, then reports it.
///
/// `logs` is the log pump's handle rather than a second flag: awaiting it is
/// what makes the app's final output land *before* `exited`, and holding it
/// here means the pump outlives the watch only when the watch was cancelled.
async fn watch_exit(
    engine: Arc<SessionEngine>,
    session: SessionId,
    events: EventSender,
    terminated: TerminatedOnce,
    cancel: Arc<AtomicBool>,
    logs: JoinHandle<()>,
) {
    let state = loop {
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        let Some(snapshot) = engine
            .wait_for(session, PUMP_TICK, |snapshot| snapshot.state.is_terminal())
            .await
        else {
            // The engine no longer knows this session; there is nothing left
            // to report and nothing left to wait for.
            log::debug!("DAP: session {session} is no longer known to the engine");
            return;
        };
        if snapshot.state.is_terminal() {
            break snapshot.state;
        }
    };

    if cancel.load(Ordering::SeqCst) {
        return;
    }
    let _ = tokio::time::timeout(LOG_FLUSH_GRACE, logs).await;

    let code = match &state {
        SessionState::Exited { success: true } => EXIT_OK,
        SessionState::Exited { success: false } => EXIT_FAILED,
        SessionState::Failed { reason } => {
            // The engine reports a launch that never spawned the same way it
            // reports one that died minutes into a device build — either way
            // the reason is the only thing the developer can act on.
            let _ = events
                .output("console", &format!("Launch failed: {reason}\n"))
                .await;
            EXIT_FAILED
        }
        // `is_terminal` admits exactly the two arms above; a live state here
        // would mean the loop broke early, which it cannot.
        other => {
            log::error!("DAP: session {session} reported non-terminal state {other:?} as final");
            EXIT_FAILED
        }
    };

    let _ = events.exited(code).await;
    terminated.emit(&events).await;
}
