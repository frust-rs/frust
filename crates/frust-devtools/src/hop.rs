//! The async→sync hand-off between a connection task and the
//! [`DevtoolsBackend`](crate::DevtoolsBackend).
//!
//! # Shape
//!
//! A connection task builds a [`Call`], sends it down a **bounded** `mpsc`
//! together with a `oneshot` reply sender, and awaits the reply under a
//! timeout. A dedicated OS thread (`backend_thread`) owns the backend, pulls
//! calls off the queue with `blocking_recv`, runs the plain sync trait method,
//! and answers the `oneshot`.
//!
//! # Why this shape
//!
//! The alternative — an `async` trait — would put tokio in every shell's
//! signature and still not solve the real problem, which is that the answers
//! live on a UI thread the service does not own. With this shape a shell
//! implements ordinary sync methods, hops to its own UI thread however it
//! likes inside them, and never sees a `Future`.
//!
//! # Blocking model
//!
//! One call runs at a time, in arrival order. If the shell's UI thread is busy
//! or frozen, the in-flight call blocks the backend thread and later calls
//! queue behind it:
//!
//! - each waiting client gets `INTERNAL_ERROR` after
//!   [`ServiceConfig::backend_timeout`](crate::ServiceConfig) — a client is
//!   never left hanging, and the connection stays usable;
//! - the queue is bounded, so a flood of requests cannot grow memory without
//!   bound; enqueue is covered by the same timeout;
//! - `handshake` never enters this queue at all (answered from the cached
//!   [`HandshakeInfo`](frust_devtools_protocol::HandshakeInfo)), so a client
//!   can always identify a frozen app;
//! - a timed-out call is **not** cancelled — the backend thread still runs it
//!   when the UI thread frees up, and drops the reply nobody is waiting for.
//!   For the read-only methods that is harmless; for `input_*` it means an
//!   injection a client gave up on may still land later.

use std::time::Duration;

use frust_devtools_protocol::{
    InputScrollParams, InputTapParams, MetricsSnapshot, RpcError, ScreenshotResult, WidgetProps,
    WidgetTreeDump,
};
use tokio::sync::{mpsc, oneshot};

use crate::backend::{BackendError, DevtoolsBackend};

/// One backend method invocation, with its already-decoded params.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Call {
    WidgetTree,
    WidgetProps(u64),
    MetricsSnapshot,
    InputTap(InputTapParams),
    InputScroll(InputScrollParams),
    InputText(String),
    Screenshot,
}

impl Call {
    /// The wire method name this call came from — diagnostics only.
    fn name(&self) -> &'static str {
        match self {
            Call::WidgetTree => "widget_tree",
            Call::WidgetProps(_) => "widget_props",
            Call::MetricsSnapshot => "metrics_snapshot",
            Call::InputTap(_) => "input_tap",
            Call::InputScroll(_) => "input_scroll",
            Call::InputText(_) => "input_text",
            Call::Screenshot => "screenshot",
        }
    }
}

/// What a [`Call`] produced. The dispatcher turns this into a
/// [`Response`](frust_devtools_protocol::Response); keeping it typed here (as
/// opposed to a pre-serialized `Value`) keeps every JSON concern in one module.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CallOutcome {
    WidgetTree(WidgetTreeDump),
    WidgetProps(WidgetProps),
    Metrics(MetricsSnapshot),
    /// An `input_*` call the backend accepted.
    Ack,
    Screenshot(ScreenshotResult),
    Failed(RpcError),
}

struct BackendCall {
    call: Call,
    reply: oneshot::Sender<CallOutcome>,
}

/// The connection-task side of the hand-off. Cheap to clone-by-`Arc`; every
/// connection shares one.
pub(crate) struct BackendClient {
    tx: mpsc::Sender<BackendCall>,
    timeout: Duration,
}

impl BackendClient {
    /// Runs `call` on the backend thread and awaits its answer.
    ///
    /// Never fails in a way the caller has to handle: a timeout, a full queue,
    /// or a dead backend thread all come back as
    /// [`CallOutcome::Failed`] so the connection can reply and move on.
    pub(crate) async fn call(&self, call: Call) -> CallOutcome {
        let name = call.name();
        let (reply, answer) = oneshot::channel();
        let send_and_wait = async {
            // A bounded queue: awaiting here is backpressure, not a leak, and
            // the timeout below covers the wait.
            if self.tx.send(BackendCall { call, reply }).await.is_err() {
                return None;
            }
            answer.await.ok()
        };

        match tokio::time::timeout(self.timeout, send_and_wait).await {
            Ok(Some(outcome)) => outcome,
            Ok(None) => {
                log::warn!("frust-devtools: backend thread is gone, failing `{name}`");
                CallOutcome::Failed(RpcError::new(
                    RpcError::INTERNAL_ERROR,
                    "devtools backend is no longer running",
                ))
            }
            Err(_) => {
                let ms = self.timeout.as_millis();
                log::warn!("frust-devtools: `{name}` timed out after {ms}ms");
                CallOutcome::Failed(RpcError::new(
                    RpcError::INTERNAL_ERROR,
                    format!(
                        "`{name}` timed out after {ms}ms — the app's UI thread is busy or blocked"
                    ),
                ))
            }
        }
    }
}

/// Moves `backend` onto its own thread and returns the client half.
///
/// The thread exits when every [`BackendClient`] is dropped (the channel
/// closes). It is deliberately **not** joined at shutdown: a backend wedged on
/// a frozen UI thread must not be able to wedge `ServiceHandle::shutdown` too
/// (see [`crate::ServiceHandle::shutdown`]).
pub(crate) fn spawn_backend_thread<B: DevtoolsBackend>(
    backend: B,
    queue_depth: usize,
    timeout: Duration,
) -> BackendClient {
    let (tx, mut rx) = mpsc::channel::<BackendCall>(queue_depth.max(1));
    let spawned = std::thread::Builder::new()
        .name("frust-devtools-backend".to_string())
        .spawn(move || {
            while let Some(BackendCall { call, reply }) = rx.blocking_recv() {
                // The reply receiver is gone whenever the client timed out or
                // disconnected; running the call anyway (rather than skipping
                // it) keeps arrival order the single contract, and dropping
                // the answer is free.
                let _ = reply.send(execute(&backend, call));
            }
        });

    if let Err(e) = spawned {
        // Nothing to unwind here: the client below will answer every call with
        // "backend is no longer running" once `rx` drops, which is exactly the
        // behavior a wedged-but-alive backend already has.
        log::error!("frust-devtools: failed to spawn the backend thread: {e}");
    }

    BackendClient { tx, timeout }
}

fn execute<B: DevtoolsBackend>(backend: &B, call: Call) -> CallOutcome {
    match call {
        Call::WidgetTree => CallOutcome::WidgetTree(backend.widget_tree()),
        Call::WidgetProps(id) => match backend.widget_props(id) {
            Some(props) => CallOutcome::WidgetProps(props),
            None => CallOutcome::Failed(
                BackendError::invalid_request(format!("no widget with id {id}")).to_rpc_error(),
            ),
        },
        Call::MetricsSnapshot => CallOutcome::Metrics(backend.metrics_snapshot()),
        Call::InputTap(params) => ack(backend.inject_tap(params)),
        Call::InputScroll(params) => ack(backend.inject_scroll(params)),
        Call::InputText(text) => ack(backend.inject_text(&text)),
        Call::Screenshot => match backend.screenshot() {
            Ok(shot) => CallOutcome::Screenshot(shot),
            Err(e) => CallOutcome::Failed(e.to_rpc_error()),
        },
    }
}

fn ack(result: Result<(), BackendError>) -> CallOutcome {
    match result {
        Ok(()) => CallOutcome::Ack,
        Err(e) => CallOutcome::Failed(e.to_rpc_error()),
    }
}
