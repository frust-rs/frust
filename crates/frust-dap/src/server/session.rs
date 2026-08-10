//! # Per-connection DAP session
//!
//! One [`run_session`] call drives exactly one DAP client connection: a read
//! loop over the Content-Length codec, the lifecycle handshake, and a writer
//! task fed by an outgoing channel.
//!
//! ## Read loop and writer task are separate
//!
//! Responses and events are never written from the read loop. Both go into one
//! `mpsc` channel that a spawned writer task drains, which buys two properties
//! the adapter seam depends on:
//!
//! - An adapter can push `output`/`exited`/`terminated` events from a
//!   background pump task **while the read loop is parked** inside a
//!   long-running request (a device build can take minutes).
//! - Sequence numbers are stamped by the writer, in wire order, so `seq` is
//!   monotonic by construction rather than by discipline at every send site.
//!
//! ## Lifecycle enforcement
//!
//! `initialize` must come first. A request that arrives before it gets a
//! `success: false` response naming the problem — never a dropped connection.
//! The same is true of an unknown or unsupported command: the client is told,
//! the connection stays up. The only things that end a session are EOF, a
//! `disconnect` request, an unrecoverable framing error (the stream cannot be
//! resynchronised after one), and the pre-handshake timeout below.
//!
//! ## Adapter seam
//!
//! Everything past `initialize` is routed to a [`DapAdapter`], which the
//! caller builds per connection from an [`EventSender`]. The trait's async
//! methods are native RPITIT with an explicit `Send` bound, so a session
//! future stays spawnable without an `async-trait` dependency.

use std::ops::ControlFlow;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, BufReader};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::protocol::codec::{CodecError, read_message, write_message};
use crate::protocol::types::{
    Capabilities, DapEvent, DapMessage, DapRequest, DapResponse, InitializeRequestArguments,
};

// ─────────────────────────────────────────────────────────────────────────────
// Tunables
// ─────────────────────────────────────────────────────────────────────────────

/// Capacity of the outgoing response/event channel.
///
/// Deep enough that a burst of app-log `output` events does not stall an
/// adapter's pump task; a client slow enough to fill it applies back-pressure
/// to that pump rather than to the read loop.
const OUTGOING_CAPACITY: usize = 256;

/// How long a connection may stay silent before sending `initialize`.
///
/// Guards the TCP transport against a connection that occupies a concurrency
/// slot without ever speaking DAP (a port scanner, an abandoned editor). Only
/// armed before the handshake — an *initialized* session is never closed on
/// idleness, since a launched app can legitimately be quiet for hours.
const INIT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long teardown waits for the writer task to flush what is queued.
///
/// The writer ends when the last [`EventSender`] is dropped. An adapter that
/// leaves a pump task holding a clone must not be able to wedge session
/// teardown, so the wait is bounded and the task is aborted after it.
const WRITER_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

// ─────────────────────────────────────────────────────────────────────────────
// Adapter seam
// ─────────────────────────────────────────────────────────────────────────────

/// What an adapter answers a routed request with.
///
/// Deliberately narrower than [`DapResponse`]: the session owns `seq`,
/// `request_seq`, and the echoed `command`, so an adapter cannot get the
/// correlation fields wrong.
#[derive(Debug, Clone)]
pub struct AdapterResponse {
    /// Whether the request was handled successfully.
    pub success: bool,

    /// Human-readable explanation. Required in spirit when `success` is
    /// `false` — it is what the client shows the user.
    pub message: Option<String>,

    /// Command-specific response body.
    pub body: Option<serde_json::Value>,
}

impl AdapterResponse {
    /// A success carrying a body.
    pub fn success(body: Option<serde_json::Value>) -> Self {
        Self {
            success: true,
            message: None,
            body,
        }
    }

    /// A bodiless success — the plain acknowledgement most lifecycle commands
    /// answer with.
    pub fn ok() -> Self {
        Self::success(None)
    }

    /// A failure carrying an actionable message.
    ///
    /// A failed response is an *in-band* answer: the client learns the command
    /// did not work and the session carries on.
    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: Some(message.into()),
            body: None,
        }
    }

    /// The canonical answer to a command the adapter does not implement.
    ///
    /// Every adapter's dispatch should end in this rather than in a panic or a
    /// silent drop — an unknown command is a routine event (DAP clients probe
    /// for capabilities and vary by version), not a protocol violation.
    pub fn unsupported(command: &str) -> Self {
        Self::failure(format!("unsupported DAP request '{command}'"))
    }
}

/// The seam between the session state machine and whatever it is driving.
///
/// The session owns the DAP lifecycle (`initialize`, the `initialized` event,
/// sequence numbering, framing) and hands the adapter everything else.
///
/// Async methods are native RPITIT with an explicit `Send` bound rather than
/// `async-trait` boxing; implementations write plain `async fn`.
pub trait DapAdapter: Send + 'static {
    /// Capabilities to advertise in the `initialize` response.
    ///
    /// Called once per connection, by the session, while handling
    /// `initialize` — an adapter never answers that request itself.
    fn capabilities(&self) -> Capabilities;

    /// Handle one post-handshake request.
    ///
    /// `command` is the raw DAP command name (including custom `frust*`
    /// requests). Returning [`AdapterResponse::unsupported`] for anything
    /// unrecognised keeps the connection alive.
    fn handle_request(
        &mut self,
        command: &str,
        arguments: Option<serde_json::Value>,
    ) -> impl std::future::Future<Output = AdapterResponse> + Send;

    /// Tear down whatever the connection owned.
    ///
    /// Called exactly once per session, after the read loop ends for *any*
    /// reason — `disconnect`, EOF, a framing error, or the pre-handshake
    /// timeout. Since a `disconnect` request is routed to
    /// [`handle_request`](DapAdapter::handle_request) first, an adapter that
    /// tears down there too must make teardown idempotent.
    fn on_disconnect(&mut self) -> impl std::future::Future<Output = ()> + Send;
}

/// An adapter's handle for pushing server-originated events at any time.
///
/// Cloneable and `Send`, so an adapter can hand clones to background pump
/// tasks (log forwarding, exit watching). Sends are ordered with the session's
/// own responses: both share one channel and one writer.
#[derive(Debug, Clone)]
pub struct EventSender {
    tx: mpsc::Sender<DapMessage>,
}

impl EventSender {
    fn new(tx: mpsc::Sender<DapMessage>) -> Self {
        Self { tx }
    }

    /// Queue an event for the client.
    ///
    /// Returns `false` when the session has already ended (the writer is
    /// gone) — a pump task should treat that as "stop pumping", not as an
    /// error worth reporting.
    pub async fn send(&self, event: DapEvent) -> bool {
        self.tx.send(DapMessage::Event(event)).await.is_ok()
    }

    /// Queue an `output` event (`console` for tool progress, `stdout`/`stderr`
    /// for app output).
    pub async fn output(&self, category: &str, text: &str) -> bool {
        self.send(DapEvent::output(category, text)).await
    }

    /// Queue an `exited` event carrying the debuggee's exit code.
    pub async fn exited(&self, exit_code: i64) -> bool {
        self.send(DapEvent::exited(exit_code)).await
    }

    /// Queue a `terminated` event.
    pub async fn terminated(&self) -> bool {
        self.send(DapEvent::terminated()).await
    }

    /// Whether the session has ended and further sends would be dropped.
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Session
// ─────────────────────────────────────────────────────────────────────────────

/// Where a connection is in the DAP lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionState {
    /// Connected; `initialize` not yet received.
    Uninitialized,
    /// `initialize` answered; requests route to the adapter.
    Initialized,
    /// `disconnect` answered; the read loop is on its way out.
    Disconnecting,
}

struct Session<A: DapAdapter> {
    adapter: A,
    state: SessionState,
    out_tx: mpsc::Sender<DapMessage>,
}

/// Run one DAP session over an async reader/writer pair.
///
/// `make_adapter` is called once, with the [`EventSender`] this connection's
/// adapter pushes events through. The call returns when the client
/// disconnects, sends `disconnect`, closes the stream, or fails the
/// pre-handshake timeout — after the adapter's
/// [`on_disconnect`](DapAdapter::on_disconnect) hook has run and the queued
/// output has been flushed.
///
/// `Err` is reserved for a genuine write failure: a read error or a vanished
/// client is a routine end-of-session, logged and reported as `Ok`.
///
/// `cancel` is the shutdown signal: firing it breaks the read loop into the
/// same [`on_disconnect`](DapAdapter::on_disconnect) teardown an EOF or a
/// `disconnect` request runs, so a Ctrl-C/SIGTERM of the server tears the
/// session's app down instead of orphaning it.
pub async fn run_session<R, W, A, F>(
    reader: R,
    writer: W,
    make_adapter: F,
    cancel: CancellationToken,
) -> Result<(), CodecError>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send + 'static,
    A: DapAdapter,
    F: FnOnce(EventSender) -> A,
{
    let (out_tx, out_rx) = mpsc::channel::<DapMessage>(OUTGOING_CAPACITY);
    let mut writer_task = tokio::spawn(write_loop(writer, out_rx));

    let adapter = make_adapter(EventSender::new(out_tx.clone()));
    let mut session = Session {
        adapter,
        state: SessionState::Uninitialized,
        out_tx,
    };

    session.read_loop(reader, cancel).await;

    // Runs for every exit path, exactly once — this is the hook an adapter
    // stops a launched app from.
    session.adapter.on_disconnect().await;

    // Dropping the session drops both senders (the loop's and the adapter's),
    // which is what lets the writer task finish draining.
    drop(session);

    match tokio::time::timeout(WRITER_DRAIN_TIMEOUT, &mut writer_task).await {
        Ok(Ok(result)) => result,
        Ok(Err(join_error)) => {
            log::error!("DAP writer task failed: {join_error}");
            Ok(())
        }
        Err(_) => {
            log::warn!(
                "DAP writer did not finish within {}s of teardown; abandoning queued output",
                WRITER_DRAIN_TIMEOUT.as_secs()
            );
            writer_task.abort();
            Ok(())
        }
    }
}

/// Drain the outgoing channel, stamping each message with the next sequence
/// number and framing it onto the wire.
///
/// Single-writer by construction: nothing else ever touches `writer`, so
/// `seq` ordering and frame integrity need no lock.
async fn write_loop<W>(mut writer: W, mut rx: mpsc::Receiver<DapMessage>) -> Result<(), CodecError>
where
    W: AsyncWrite + Unpin + Send,
{
    let mut next_seq: i64 = 1;

    while let Some(mut message) = rx.recv().await {
        match &mut message {
            DapMessage::Request(request) => request.seq = next_seq,
            DapMessage::Response(response) => response.seq = next_seq,
            DapMessage::Event(event) => event.seq = next_seq,
        }
        next_seq = next_seq.saturating_add(1);

        write_message(&mut writer, &message).await?;
    }

    Ok(())
}

impl<A: DapAdapter> Session<A> {
    /// Read requests until the connection ends.
    ///
    /// The `select!` below is cancellation-safe by shape, not by luck: every
    /// other arm terminates the loop, so a half-read message can never be
    /// dropped by a competing branch waking first. `cancel` is one such arm —
    /// a shutdown signal ends the loop exactly like EOF, flowing into the same
    /// `on_disconnect` teardown [`run_session`] runs after it returns.
    async fn read_loop<R>(&mut self, reader: R, cancel: CancellationToken)
    where
        R: AsyncRead + Unpin + Send,
    {
        let mut reader = BufReader::new(reader);

        let init_deadline = tokio::time::sleep(INIT_TIMEOUT);
        tokio::pin!(init_deadline);

        loop {
            tokio::select! {
                result = read_message(&mut reader) => {
                    match result {
                        Ok(Some(DapMessage::Request(request))) => {
                            log::debug!("DAP ← {} (seq={})", request.command, request.seq);
                            if self.handle_request(&request).await.is_break() {
                                break;
                            }
                        }
                        Ok(Some(_)) => {
                            log::debug!("DAP: ignoring non-request message from client");
                        }
                        Ok(None) => {
                            log::debug!("DAP client disconnected (EOF)");
                            break;
                        }
                        Err(error) => {
                            // A framing error desynchronises the stream; there
                            // is no defined way to resume mid-frame.
                            log::warn!("DAP: unreadable message, closing connection: {error}");
                            break;
                        }
                    }
                }

                () = &mut init_deadline, if self.state == SessionState::Uninitialized => {
                    log::warn!(
                        "DAP client sent no initialize within {}s; closing connection",
                        INIT_TIMEOUT.as_secs()
                    );
                    break;
                }

                () = cancel.cancelled() => {
                    log::info!("DAP: shutdown signal received; closing connection and tearing down");
                    break;
                }
            }
        }
    }

    /// Answer one request, reporting whether the session should keep reading.
    async fn handle_request(&mut self, request: &DapRequest) -> ControlFlow<()> {
        match request.command.as_str() {
            "initialize" => self.handle_initialize(request).await,

            // A disconnect is never refused, not even before the handshake:
            // a client asking to leave always gets to leave.
            "disconnect" if self.state == SessionState::Uninitialized => {
                self.state = SessionState::Disconnecting;
                let _ = self
                    .send(DapMessage::Response(DapResponse::success(request, None)))
                    .await;
                ControlFlow::Break(())
            }

            command if self.state == SessionState::Uninitialized => {
                let message = format!(
                    "'{command}' arrived before 'initialize'; the DAP lifecycle requires \
                     'initialize' first"
                );
                log::warn!("DAP: {message}");
                self.send(DapMessage::Response(DapResponse::error(request, message)))
                    .await
            }

            command => {
                let response = self
                    .adapter
                    .handle_request(command, request.arguments.clone())
                    .await;
                let flow = self.send_adapter_response(request, response).await;

                if command == "disconnect" {
                    self.state = SessionState::Disconnecting;
                    return ControlFlow::Break(());
                }
                flow
            }
        }
    }

    /// Answer `initialize`: capabilities from the adapter, then the
    /// `initialized` event, in that order.
    async fn handle_initialize(&mut self, request: &DapRequest) -> ControlFlow<()> {
        if self.state != SessionState::Uninitialized {
            log::warn!("DAP: repeated initialize on an already-initialized connection");
            return self
                .send(DapMessage::Response(DapResponse::error(
                    request,
                    "initialize has already been called on this connection",
                )))
                .await;
        }

        if let Some(client) = request.arguments.as_ref().and_then(|args| {
            serde_json::from_value::<InitializeRequestArguments>(args.clone()).ok()
        }) {
            log::info!(
                "DAP client connected: {} (id: {})",
                client.client_name.as_deref().unwrap_or("<unnamed>"),
                client.client_id.as_deref().unwrap_or("<none>"),
            );
        }

        self.state = SessionState::Initialized;

        let body = match serde_json::to_value(self.adapter.capabilities()) {
            Ok(value) => value,
            Err(error) => {
                // An empty capability set is a valid answer; a lost handshake
                // is not.
                log::error!("DAP: failed to serialize capabilities: {error}");
                serde_json::Value::Object(serde_json::Map::new())
            }
        };

        if self
            .send(DapMessage::Response(DapResponse::success(
                request,
                Some(body),
            )))
            .await
            .is_break()
        {
            return ControlFlow::Break(());
        }

        self.send(DapMessage::Event(DapEvent::initialized())).await
    }

    /// Fold an [`AdapterResponse`] into a correlated [`DapResponse`].
    async fn send_adapter_response(
        &mut self,
        request: &DapRequest,
        response: AdapterResponse,
    ) -> ControlFlow<()> {
        let dap_response = DapResponse {
            seq: 0, // stamped by the writer
            request_seq: request.seq,
            success: response.success,
            command: request.command.clone(),
            message: response.message,
            body: response.body,
        };

        if !dap_response.success {
            log::debug!(
                "DAP → failed {} (req_seq={}): {}",
                dap_response.command,
                dap_response.request_seq,
                dap_response.message.as_deref().unwrap_or("<no message>"),
            );
        }

        self.send(DapMessage::Response(dap_response)).await
    }

    /// Queue one outgoing message, reporting whether the writer is still there.
    ///
    /// Takes `&mut self` rather than `&self` on purpose: a shared borrow would
    /// require `Session<A>: Sync` for the session future to be `Send`, and the
    /// TCP transport spawns that future.
    async fn send(&mut self, message: DapMessage) -> ControlFlow<()> {
        if self.out_tx.send(message).await.is_err() {
            log::debug!("DAP: outgoing channel closed; ending session");
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use tokio::io::DuplexStream;
    use tokio::task::JoinHandle;

    use super::*;

    const READ_TIMEOUT: Duration = Duration::from_secs(2);
    const DUPLEX_BUFFER: usize = 8192;

    /// What the fake adapter saw, observable from the test after the session
    /// has moved the adapter into its own future.
    #[derive(Debug, Clone, Default)]
    struct Recorder {
        commands: Arc<Mutex<Vec<String>>>,
        disconnects: Arc<AtomicUsize>,
    }

    impl Recorder {
        fn commands(&self) -> Vec<String> {
            self.commands.lock().expect("recorder poisoned").clone()
        }

        fn disconnects(&self) -> usize {
            self.disconnects.load(Ordering::SeqCst)
        }
    }

    struct FakeAdapter {
        events: EventSender,
        recorder: Recorder,
    }

    impl DapAdapter for FakeAdapter {
        fn capabilities(&self) -> Capabilities {
            Capabilities::frust_defaults()
        }

        async fn handle_request(
            &mut self,
            command: &str,
            _arguments: Option<serde_json::Value>,
        ) -> AdapterResponse {
            self.recorder
                .commands
                .lock()
                .expect("recorder poisoned")
                .push(command.to_owned());

            match command {
                // Pushes an event before answering — the interleaving case.
                "launch" => {
                    self.events.output("console", "building\n").await;
                    AdapterResponse::ok()
                }
                // Pushes an event, then stays busy long enough for the test to
                // prove the writer is not blocked behind the read loop.
                "slowLaunch" => {
                    self.events.output("console", "building\n").await;
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    AdapterResponse::ok()
                }
                "threads" => AdapterResponse::success(Some(serde_json::json!({
                    "threads": [{ "id": 1, "name": "app" }]
                }))),
                "disconnect" => AdapterResponse::ok(),
                other => AdapterResponse::unsupported(other),
            }
        }

        async fn on_disconnect(&mut self) {
            self.recorder.disconnects.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// A scripted DAP client over the far end of a duplex pair.
    struct TestClient {
        reader: BufReader<DuplexStream>,
        writer: DuplexStream,
    }

    impl TestClient {
        async fn request(&mut self, seq: i64, command: &str, arguments: Option<serde_json::Value>) {
            let message = DapMessage::Request(DapRequest {
                seq,
                command: command.to_owned(),
                arguments,
            });
            write_message(&mut self.writer, &message)
                .await
                .expect("client write");
        }

        /// Write bytes the codec cannot make sense of.
        async fn write_raw(&mut self, bytes: &[u8]) {
            use tokio::io::AsyncWriteExt;
            self.writer
                .write_all(bytes)
                .await
                .expect("raw client write");
            self.writer.flush().await.expect("raw client flush");
        }

        /// Next message, failing the test on timeout or EOF.
        async fn next(&mut self) -> DapMessage {
            self.next_within(READ_TIMEOUT)
                .await
                .expect("expected a message, got EOF")
        }

        async fn next_within(&mut self, budget: Duration) -> Option<DapMessage> {
            tokio::time::timeout(budget, read_message(&mut self.reader))
                .await
                .expect("timed out waiting for a DAP message")
                .expect("client read")
        }

        async fn next_response(&mut self) -> DapResponse {
            match self.next().await {
                DapMessage::Response(response) => response,
                other => panic!("expected a response, got {other:?}"),
            }
        }

        async fn next_event(&mut self) -> DapEvent {
            match self.next().await {
                DapMessage::Event(event) => event,
                other => panic!("expected an event, got {other:?}"),
            }
        }

        /// Close the client's write half, which the session sees as EOF.
        fn hang_up(self) -> BufReader<DuplexStream> {
            let TestClient { reader, writer } = self;
            drop(writer);
            reader
        }
    }

    fn spawn_session() -> (
        TestClient,
        Recorder,
        JoinHandle<std::result::Result<(), CodecError>>,
    ) {
        let (client, recorder, _cancel, handle) = spawn_session_with_cancel();
        (client, recorder, handle)
    }

    /// The same as [`spawn_session`], but also hands back the session's
    /// [`CancellationToken`] so a test can drive the shutdown-signal path.
    fn spawn_session_with_cancel() -> (
        TestClient,
        Recorder,
        CancellationToken,
        JoinHandle<std::result::Result<(), CodecError>>,
    ) {
        let (server_reader, client_writer) = tokio::io::duplex(DUPLEX_BUFFER);
        let (client_reader, server_writer) = tokio::io::duplex(DUPLEX_BUFFER);

        let recorder = Recorder::default();
        let adapter_recorder = recorder.clone();
        let cancel = CancellationToken::new();
        let session_cancel = cancel.clone();

        let handle = tokio::spawn(async move {
            run_session(
                server_reader,
                server_writer,
                move |events| FakeAdapter {
                    events,
                    recorder: adapter_recorder,
                },
                session_cancel,
            )
            .await
        });

        let client = TestClient {
            reader: BufReader::new(client_reader),
            writer: client_writer,
        };

        (client, recorder, cancel, handle)
    }

    async fn join(handle: JoinHandle<std::result::Result<(), CodecError>>) {
        tokio::time::timeout(READ_TIMEOUT, handle)
            .await
            .expect("session did not end")
            .expect("session task panicked")
            .expect("session I/O");
    }

    async fn initialize(client: &mut TestClient) {
        client
            .request(
                1,
                "initialize",
                Some(serde_json::json!({"clientID": "test"})),
            )
            .await;
        let response = client.next_response().await;
        assert!(response.success);
        let event = client.next_event().await;
        assert_eq!(event.event, "initialized");
    }

    // ── Handshake ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_initialize_answers_capabilities_then_initialized_event() {
        let (mut client, _recorder, handle) = spawn_session();

        client.request(1, "initialize", None).await;

        let response = client.next_response().await;
        assert!(response.success, "initialize must succeed");
        assert_eq!(response.command, "initialize");
        assert_eq!(response.request_seq, 1);
        let body = response.body.expect("capabilities body");
        assert_eq!(body["supportsConfigurationDoneRequest"], true);
        assert_eq!(body["supportsTerminateRequest"], true);

        let event = client.next_event().await;
        assert_eq!(
            event.event, "initialized",
            "the initialized event must follow the response, not precede it"
        );

        client.request(2, "disconnect", None).await;
        let _ = client.next_response().await;
        join(handle).await;
    }

    #[tokio::test]
    async fn test_initialize_twice_is_refused_without_dropping_the_connection() {
        let (mut client, _recorder, handle) = spawn_session();
        initialize(&mut client).await;

        client.request(2, "initialize", None).await;
        let response = client.next_response().await;
        assert!(!response.success);
        assert!(
            response
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("already"),
            "message should say initialize already happened: {:?}",
            response.message
        );

        // The session survived the refusal.
        client.request(3, "threads", None).await;
        assert!(client.next_response().await.success);

        client.request(4, "disconnect", None).await;
        let _ = client.next_response().await;
        join(handle).await;
    }

    #[tokio::test]
    async fn test_request_before_initialize_is_refused_and_session_survives() {
        let (mut client, recorder, handle) = spawn_session();

        client.request(1, "threads", None).await;
        let response = client.next_response().await;
        assert!(!response.success, "pre-initialize request must fail");
        assert_eq!(response.request_seq, 1);
        assert_eq!(response.command, "threads");
        assert!(
            response
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("initialize"),
            "message should name the missing step: {:?}",
            response.message
        );
        assert!(
            recorder.commands().is_empty(),
            "a pre-initialize request must never reach the adapter"
        );

        // Same connection, now doing it properly.
        client.request(2, "initialize", None).await;
        assert!(client.next_response().await.success);
        assert_eq!(client.next_event().await.event, "initialized");

        client.request(3, "disconnect", None).await;
        let _ = client.next_response().await;
        join(handle).await;
    }

    #[tokio::test]
    async fn test_disconnect_before_initialize_is_accepted() {
        let (mut client, recorder, handle) = spawn_session();

        client.request(1, "disconnect", None).await;
        let response = client.next_response().await;
        assert!(response.success, "a disconnect is never refused");

        join(handle).await;
        assert_eq!(recorder.disconnects(), 1);
    }

    // ── Unknown / hostile input ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_unknown_command_returns_failed_response() {
        let (mut client, _recorder, handle) = spawn_session();
        initialize(&mut client).await;

        client.request(2, "frobnicate", None).await;
        let response = client.next_response().await;
        assert!(!response.success);
        assert_eq!(response.command, "frobnicate");
        assert!(
            response
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("frobnicate"),
            "message should name the command: {:?}",
            response.message
        );

        client.request(3, "disconnect", None).await;
        let _ = client.next_response().await;
        join(handle).await;
    }

    #[tokio::test]
    async fn test_garbage_commands_never_panic_and_never_drop_the_connection() {
        let (mut client, _recorder, handle) = spawn_session();
        initialize(&mut client).await;

        let overlong = "a".repeat(4096);
        let garbage = [
            "",
            " ",
            "../../etc/passwd",
            "{\"not\":\"a command\"}",
            "\u{1f4a5}\u{0}\u{7f}",
            overlong.as_str(),
        ];

        for (index, command) in garbage.iter().enumerate() {
            let seq = 2 + index as i64;
            client
                .request(
                    seq,
                    command,
                    Some(serde_json::json!({"junk": [1, {}, null]})),
                )
                .await;
            let response = client.next_response().await;
            assert!(!response.success, "garbage command {command:?} must fail");
            assert_eq!(response.request_seq, seq);
        }

        // Still a working session afterwards.
        client.request(100, "threads", None).await;
        assert!(client.next_response().await.success);

        client.request(101, "disconnect", None).await;
        let _ = client.next_response().await;
        join(handle).await;
    }

    #[tokio::test]
    async fn test_malformed_framing_ends_the_session_without_panicking() {
        let (mut client, recorder, handle) = spawn_session();
        initialize(&mut client).await;

        // A header the codec cannot parse: the stream is now unrecoverable, so
        // the session is expected to close — cleanly, and through teardown.
        client
            .write_raw(b"Content-Length: not-a-number\r\n\r\n{}")
            .await;

        join(handle).await;
        assert_eq!(recorder.disconnects(), 1);
    }

    // ── Adapter-pushed events ───────────────────────────────────────────────

    #[tokio::test]
    async fn test_adapter_event_precedes_the_response_it_was_pushed_before() {
        let (mut client, _recorder, handle) = spawn_session();
        initialize(&mut client).await;

        client.request(2, "launch", None).await;

        let event = client.next_event().await;
        assert_eq!(event.event, "output");
        assert_eq!(event.body.expect("output body")["output"], "building\n");

        let response = client.next_response().await;
        assert!(response.success);
        assert_eq!(response.command, "launch");

        client.request(3, "disconnect", None).await;
        let _ = client.next_response().await;
        join(handle).await;
    }

    #[tokio::test]
    async fn test_adapter_event_reaches_the_client_while_the_read_loop_is_blocked() {
        let (mut client, _recorder, handle) = spawn_session();
        initialize(&mut client).await;

        // The adapter emits, then stays busy for 400ms.
        client.request(2, "slowLaunch", None).await;

        // The event must arrive well before the request completes — proof the
        // writer is not serialized behind the read loop.
        let event = client
            .next_within(Duration::from_millis(150))
            .await
            .expect("event before EOF");
        match event {
            DapMessage::Event(event) => assert_eq!(event.event, "output"),
            other => panic!("expected the pushed output event first, got {other:?}"),
        }

        let response = client.next_response().await;
        assert!(response.success);

        client.request(3, "disconnect", None).await;
        let _ = client.next_response().await;
        join(handle).await;
    }

    #[tokio::test]
    async fn test_server_sequence_numbers_are_monotonic() {
        let (mut client, _recorder, handle) = spawn_session();

        client.request(1, "initialize", None).await;
        let init_response = client.next_response().await;
        let initialized = client.next_event().await;

        client.request(2, "launch", None).await;
        let output = client.next_event().await;
        let launch_response = client.next_response().await;

        assert_eq!(
            [
                init_response.seq,
                initialized.seq,
                output.seq,
                launch_response.seq
            ],
            [1, 2, 3, 4],
            "server-originated messages carry a monotonic seq in wire order"
        );

        client.request(3, "disconnect", None).await;
        let _ = client.next_response().await;
        join(handle).await;
    }

    // ── Teardown ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_disconnect_answers_then_runs_the_teardown_hook() {
        let (mut client, recorder, handle) = spawn_session();
        initialize(&mut client).await;

        client.request(2, "disconnect", None).await;
        let response = client.next_response().await;
        assert!(response.success);
        assert_eq!(response.command, "disconnect");

        join(handle).await;

        assert_eq!(
            recorder.commands(),
            vec!["disconnect".to_owned()],
            "disconnect routes to the adapter like any other request"
        );
        assert_eq!(recorder.disconnects(), 1, "teardown runs exactly once");
    }

    #[tokio::test]
    async fn test_cancellation_runs_the_teardown_hook() {
        let (mut client, recorder, cancel, handle) = spawn_session_with_cancel();
        initialize(&mut client).await;

        // A shutdown signal (Ctrl-C/SIGTERM) fires the token; the read loop must
        // break into the same idempotent teardown EOF and disconnect run.
        cancel.cancel();

        join(handle).await;
        assert_eq!(
            recorder.disconnects(),
            1,
            "cancellation runs the teardown hook exactly once"
        );

        // The server side closed the connection too — the client sees EOF.
        let mut reader = client.hang_up();
        let trailing = tokio::time::timeout(READ_TIMEOUT, read_message(&mut reader))
            .await
            .expect("stream should close")
            .expect("read ok");
        assert!(trailing.is_none(), "expected EOF, got {trailing:?}");
    }

    #[tokio::test]
    async fn test_client_hangup_runs_the_teardown_hook() {
        let (mut client, recorder, handle) = spawn_session();
        initialize(&mut client).await;

        let mut reader = client.hang_up();

        join(handle).await;
        assert_eq!(recorder.disconnects(), 1);

        // Server side closed too — the client sees EOF, not a hang.
        let trailing = tokio::time::timeout(READ_TIMEOUT, read_message(&mut reader))
            .await
            .expect("stream should close")
            .expect("read ok");
        assert!(trailing.is_none(), "expected EOF, got {trailing:?}");
    }

    // ── AdapterResponse constructors ────────────────────────────────────────

    #[test]
    fn test_adapter_response_constructors() {
        let ok = AdapterResponse::ok();
        assert!(ok.success);
        assert!(ok.body.is_none());

        let with_body = AdapterResponse::success(Some(serde_json::json!({"a": 1})));
        assert!(with_body.success);
        assert_eq!(with_body.body.expect("body")["a"], 1);

        let failed = AdapterResponse::failure("nope");
        assert!(!failed.success);
        assert_eq!(failed.message.as_deref(), Some("nope"));

        let unsupported = AdapterResponse::unsupported("frustWarpDrive");
        assert!(!unsupported.success);
        assert!(
            unsupported
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("frustWarpDrive")
        );
    }
}
