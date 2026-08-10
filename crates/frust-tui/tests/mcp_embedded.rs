//! The embedded MCP server, end to end against a real workbench event loop.
//!
//! Two things are under test, and neither is mocked:
//!
//! 1. **One session world.** An MCP client speaking the real Streamable-HTTP
//!    transport over a real `frust_mcp::serve_embedded` calls `run_app` /
//!    `list_sessions` / `stop_app`, and every effect is observed on the
//!    *workbench's own* `AppState` — through a mirror the loop publishes,
//!    never through the backend being tested.
//! 2. **Server lifecycle.** `Engine::start_mcp` / `stop_mcp` bind and release
//!    a real loopback port, and the client registry reflects a client
//!    connecting and disconnecting.
//!
//! **No process is ever really spawned** — the workbench loop supervises a
//! scripted `FakeProcessRunner` (`docs/CODE_STANDARDS.md`'s `ProcessRunner`
//! contract).
//!
//! **No sleeps.** Every wait is on something the workbench, the server, or
//! the registry itself produces: a ready signal, an HTTP response, a message
//! on the engine channel, or a bounded yield-until-condition poll on state
//! the loop publishes. The only durations here are failure deadlines.
//!
//! The HTTP client is deliberately blocking `std::net` inside
//! `spawn_blocking`: it keeps this crate's tokio feature set unchanged and
//! models exactly how a `SessionBackend` call reaches the workbench — from a
//! thread that is not the one running the event loop.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use frust_drive::process::{FakeProcessRunner, ProcessRunner};
use frust_mcp::{ClientRegistry, SharedBackend};
use frust_tui::engine::{AppState, Effect, Engine, Message, Screen};
use frust_tui::supervise::{
    McpServeCtx, McpSessionRecords, McpStatus, SessionState, Supervisor, TuiSessionBackend,
    mcp_backend::MAX_ADHOC_SESSION_ID, serve_command,
};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// A failure deadline for anything this test waits on — never a pacing
/// device.
const DEADLINE: Duration = Duration::from_secs(20);

/// The exact `cargo run` invocation a Debug desktop session resolves to
/// (`SessionSpec::launch_plan`), and therefore the key the scripted fake
/// stream is registered under. If that funnel changes, this key stops
/// matching and the test fails loudly rather than silently launching nothing.
const DEBUG_DESKTOP_INVOCATION: &str =
    "cargo run --features frust/perf-trace --features frust/devtools";

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

// ── A workbench event loop, without a terminal ──────────────────────────────

/// One session as the **workbench's own model** holds it — the independent
/// evidence this test checks the MCP effects against.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MirroredSession {
    id: u64,
    target_label: String,
    state: SessionState,
}

/// A running workbench: an `Engine`, a `Supervisor` over a scripted runner,
/// and the same `serve_command` dispatch `crate::runner` performs — on its
/// own thread with its own current-thread runtime, so a blocking backend call
/// is never on the thread that answers it (`supervise::mcp_backend`'s
/// deadlock note).
struct Workbench {
    tx: UnboundedSender<Message>,
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    sessions: Arc<Mutex<Vec<MirroredSession>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Workbench {
    fn start(project_root: PathBuf, runner: Arc<dyn ProcessRunner + Send + Sync>) -> Self {
        let state = AppState {
            screen: Screen::Workbench,
            project_root: Some(project_root.clone()),
            projects: vec![project_root],
            ..AppState::default()
        };

        let engine = Engine::new(state);
        let tx = engine.sender();
        let sessions = Arc::new(Mutex::new(Vec::new()));

        let thread = {
            let runner = Arc::clone(&runner);
            let sessions = Arc::clone(&sessions);
            std::thread::Builder::new()
                .name("frust-tui-test-workbench".to_string())
                .spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("building the workbench test runtime");
                    rt.block_on(run_workbench(engine, runner, sessions));
                })
                .expect("spawning the workbench test thread")
        };

        Self {
            tx,
            runner,
            sessions,
            thread: Some(thread),
        }
    }

    fn backend(&self) -> SharedBackend {
        Arc::new(TuiSessionBackend::new(
            self.tx.clone(),
            Arc::clone(&self.runner),
        ))
    }

    fn sessions(&self) -> Vec<MirroredSession> {
        self.sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Yields (never sleeps) until the workbench's own session mirror
    /// satisfies `predicate`.
    async fn wait_for(&self, what: &str, predicate: impl Fn(&[MirroredSession]) -> bool) {
        let deadline = Instant::now() + DEADLINE;
        loop {
            let sessions = self.sessions();
            if predicate(&sessions) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the workbench never reached `{what}` within {DEADLINE:?}; \
                 its sessions were {sessions:?}"
            );
            tokio::task::yield_now().await;
        }
    }
}

impl Drop for Workbench {
    fn drop(&mut self) {
        let _ = self.tx.send(Message::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The event loop itself: the same message routing `crate::runner::run_loop`
/// performs, minus the terminal, the effects (nothing here emits one this
/// test needs), and the tick.
async fn run_workbench(
    mut engine: Engine,
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    mirror: Arc<Mutex<Vec<MirroredSession>>>,
) {
    let mut rx = engine.take_receiver();
    let (mut supervisor, mut session_rx) = Supervisor::new(runner);
    let mut records = McpSessionRecords::new();
    let mut next_adhoc_id = MAX_ADHOC_SESSION_ID;

    while !engine.state.should_quit {
        tokio::select! {
            Some(msg) = rx.recv() => {
                if let Message::Mcp(command) = msg {
                    serve_command(command, &mut McpServeCtx {
                        state: &engine.state,
                        supervisor: &mut supervisor,
                        records: &mut records,
                        tx: &engine.sender(),
                        next_adhoc_id: &mut next_adhoc_id,
                    });
                } else {
                    engine.handle(msg);
                }
            }
            Some(event) = session_rx.recv() => {
                engine.handle(Message::Session(event));
            }
            else => break,
        }
        publish(&engine, &mirror);
    }
}

fn publish(engine: &Engine, mirror: &Arc<Mutex<Vec<MirroredSession>>>) {
    let sessions: Vec<MirroredSession> = engine
        .state
        .sessions
        .iter()
        .map(|view| MirroredSession {
            id: view.id.0,
            target_label: view.target_label.clone(),
            state: view.state.clone(),
        })
        .collect();
    *mirror.lock().unwrap_or_else(|p| p.into_inner()) = sessions;
}

// ── A minimal MCP client over raw HTTP ──────────────────────────────────────

/// An initialized MCP session against `addr`.
struct McpClient {
    addr: SocketAddr,
    session_id: String,
}

impl McpClient {
    /// Runs the `initialize` / `notifications/initialized` handshake, so
    /// tools can be called immediately.
    async fn connect(addr: SocketAddr) -> McpClient {
        let response = exchange(addr, initialize_request(addr), "serverInfo").await;
        assert!(
            response.starts_with("HTTP/1.1 200"),
            "initialize response was: {response}"
        );
        let session_id = session_id_header(&response);

        let request = post(
            addr,
            &session_id,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        );
        let response = exchange(addr, request, "\r\n\r\n").await;
        assert!(
            response.starts_with("HTTP/1.1 202"),
            "notifications/initialized response was: {response}"
        );

        McpClient { addr, session_id }
    }

    /// Calls one tool, returning the whole raw HTTP response (the JSON-RPC
    /// message is in an SSE `data:` line inside it).
    async fn call_tool(&self, name: &str, arguments: &str) -> String {
        let id = next_request_id();
        let body = format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#
        );
        let request = post(self.addr, &self.session_id, &body);
        let response = exchange(self.addr, request, &format!("\"id\":{id}")).await;
        assert!(
            !response.contains(r#""isError":true"#),
            "tool `{name}` reported an error: {response}"
        );
        response
    }

    /// Closes the MCP session the way a client disconnecting does.
    async fn disconnect(self) {
        let request = format!(
            "DELETE /mcp HTTP/1.1\r\n\
             Host: 127.0.0.1:{port}\r\n\
             Mcp-Session-Id: {session}\r\n\
             MCP-Protocol-Version: {MCP_PROTOCOL_VERSION}\r\n\
             Connection: close\r\n\r\n",
            port = self.addr.port(),
            session = self.session_id,
        );
        exchange(self.addr, request, "\r\n").await;
    }
}

fn next_request_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(100);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn initialize_request(addr: SocketAddr) -> String {
    let body = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"{MCP_PROTOCOL_VERSION}","capabilities":{{}},"clientInfo":{{"name":"frust-tui-test","version":"0.0.0"}}}}}}"#
    );
    format!(
        "POST /mcp HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Accept: application/json, text/event-stream\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\r\n{body}",
        port = addr.port(),
        len = body.len(),
    )
}

fn post(addr: SocketAddr, session_id: &str, body: &str) -> String {
    format!(
        "POST /mcp HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Accept: application/json, text/event-stream\r\n\
         Content-Type: application/json\r\n\
         Mcp-Session-Id: {session_id}\r\n\
         MCP-Protocol-Version: {MCP_PROTOCOL_VERSION}\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\r\n{body}",
        port = addr.port(),
        len = body.len(),
    )
}

fn session_id_header(response: &str) -> String {
    response
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("mcp-session-id")
                .then(|| value.trim().to_string())
        })
        .unwrap_or_else(|| panic!("no Mcp-Session-Id header in response: {response}"))
}

/// One blocking HTTP exchange, off the runtime: write `request`, then read
/// until `until` appears or the peer closes.
async fn exchange(addr: SocketAddr, request: String, until: &str) -> String {
    let until = until.to_string();
    tokio::task::spawn_blocking(move || {
        let mut stream =
            TcpStream::connect_timeout(&addr, DEADLINE).expect("connecting to the MCP server");
        stream
            .set_read_timeout(Some(DEADLINE))
            .expect("setting the read timeout");
        stream
            .write_all(request.as_bytes())
            .expect("writing the request");

        let mut response = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let read = stream.read(&mut buf).expect("reading the response");
            if read == 0 {
                break;
            }
            response.extend_from_slice(&buf[..read]);
            if String::from_utf8_lossy(&response).contains(&until) {
                break;
            }
        }
        String::from_utf8_lossy(&response).into_owned()
    })
    .await
    .expect("the HTTP exchange task panicked")
}

// ── Tests ───────────────────────────────────────────────────────────────────

/// Starts `serve_embedded` over `backend` on an OS-assigned port.
async fn start_embedded(
    backend: SharedBackend,
    registry: ClientRegistry,
) -> (SocketAddr, CancellationToken) {
    let (ready_tx, ready_rx) = oneshot::channel();
    let cancel = CancellationToken::new();
    tokio::spawn(frust_mcp::serve_embedded(
        backend,
        registry,
        0,
        Some(ready_tx),
        cancel.clone(),
    ));
    let addr = tokio::time::timeout(DEADLINE, ready_rx)
        .await
        .expect("the embedded server did not report ready in time")
        .expect("the ready channel was dropped before a send");
    (addr, cancel)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_drives_the_workbenchs_own_sessions() {
    let project_root = PathBuf::from("/tmp/frust-tui-mcp-embedded-test");
    let runner: Arc<dyn ProcessRunner + Send + Sync> = Arc::new(
        FakeProcessRunner::new()
            .with_hanging_stream(DEBUG_DESKTOP_INVOCATION, ["     Running `app`"]),
    );
    let workbench = Workbench::start(project_root.clone(), runner);
    let (addr, cancel) = start_embedded(workbench.backend(), ClientRegistry::new()).await;
    let client = McpClient::connect(addr).await;

    // The agent launches an app…
    let response = client.call_tool("run_app", r#"{"target":"desktop"}"#).await;
    assert!(
        response.contains(r#"\"target\":\"desktop\""#)
            || response.contains(r#""target":"desktop""#),
        "run_app did not report the launched target: {response}"
    );

    // …and it is the *workbench's* session that appears, in its own model.
    workbench
        .wait_for("one registered desktop session", |sessions| {
            sessions.len() == 1 && sessions[0].target_label == "desktop"
        })
        .await;
    workbench
        .wait_for("the session reaches Running", |sessions| {
            sessions.first().map(|s| &s.state) == Some(&SessionState::Running)
        })
        .await;

    // The agent sees the same one session, in the same project.
    let response = client.call_tool("list_sessions", "{}").await;
    let root = project_root.display().to_string();
    assert!(
        response.contains(&root) || response.contains(&root.replace('/', "\\/")),
        "list_sessions did not report the workbench's project root {root}: {response}"
    );

    // Stopping through MCP kills the workbench's own session.
    let id = workbench.sessions()[0].id;
    client
        .call_tool("stop_app", &format!(r#"{{"session_id":{id}}}"#))
        .await;
    workbench
        .wait_for("the session is killed", |sessions| {
            sessions.first().map(|s| &s.state) == Some(&SessionState::Killed)
        })
        .await;

    client.disconnect().await;
    cancel.cancel();
}

/// A tool needing the app's devtools connection is refused with the
/// embedded-mode reason — never a fabricated success.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn devtools_backed_tools_report_the_embedded_mode_refusal() {
    let project_root = PathBuf::from("/tmp/frust-tui-mcp-embedded-devtools-test");
    let runner: Arc<dyn ProcessRunner + Send + Sync> = Arc::new(
        FakeProcessRunner::new()
            .with_hanging_stream(DEBUG_DESKTOP_INVOCATION, ["     Running `app`"]),
    );
    let workbench = Workbench::start(project_root, runner);
    let (addr, cancel) = start_embedded(workbench.backend(), ClientRegistry::new()).await;
    let client = McpClient::connect(addr).await;

    client.call_tool("run_app", r#"{"target":"desktop"}"#).await;
    workbench
        .wait_for("one registered session", |sessions| sessions.len() == 1)
        .await;

    // `widget_tree` needs the devtools client this backend cannot share.
    let id = next_request_id();
    let body = format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"widget_tree","arguments":{{}}}}}}"#
    );
    let response = exchange(
        addr,
        post(addr, &client.session_id, &body),
        &format!("\"id\":{id}"),
    )
    .await;
    assert!(
        response.contains(r#""isError":true"#),
        "widget_tree should have failed in embedded mode: {response}"
    );
    assert!(
        response.contains("workbench owns this session"),
        "the refusal must say why, verbatim: {response}"
    );

    client.disconnect().await;
    cancel.cancel();
}

/// `start_mcp` → `stop_mcp` releases the port, the registry tracks the one
/// client that connected meanwhile, and a second `start_mcp` succeeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_stop_start_releases_the_port_and_tracks_clients() {
    let mut engine = Engine::new(AppState::default());
    let mut rx = engine.take_receiver();
    let backend: SharedBackend = Arc::new(TuiSessionBackend::new(
        engine.sender(),
        Arc::new(FakeProcessRunner::new()),
    ));

    assert!(engine.start_mcp(Arc::clone(&backend), 0), "first start");
    assert!(
        !engine.start_mcp(Arc::clone(&backend), 0),
        "a second server must not start while one is running"
    );
    let port = pump_until_listening(&mut engine, &mut rx).await;

    // A client connects: the registry (and therefore the status) sees it.
    let client = McpClient::connect(SocketAddr::from(([127, 0, 0, 1], port))).await;
    wait_for_clients(&engine, 1).await;
    client.disconnect().await;
    wait_for_clients(&engine, 0).await;

    assert!(engine.stop_mcp(), "a running server stops");
    assert_eq!(engine.mcp_status(), McpStatus::Stopped);
    // Wait for the server task's own report — the point at which the listener
    // is provably closed, rather than a sleep.
    pump_until_stopped(&mut engine, &mut rx).await;
    assert!(
        TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), DEADLINE).is_err(),
        "the port must be free once the server task has returned"
    );

    // A second lifecycle starts cleanly.
    assert!(engine.start_mcp(backend, 0), "restart after a clean stop");
    let second = pump_until_listening(&mut engine, &mut rx).await;
    assert_ne!(second, 0);
    engine.stop_mcp();
    pump_until_stopped(&mut engine, &mut rx).await;
}

/// The workbook §B13 toggle, end to end: the pure transition asks for a
/// start, the runner's enactment binds a real listener a real client reaches,
/// and the *same* message takes it back down.
///
/// The enactment here binds an OS-assigned port rather than
/// `DEFAULT_MCP_PORT` (what `crate::runner` passes), so this test can never
/// collide with a workbench — or another test — already holding the default.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_toggle_starts_and_stops_a_real_server() {
    let mut engine = Engine::new(AppState::default());
    let mut rx = engine.take_receiver();
    let backend: SharedBackend = Arc::new(TuiSessionBackend::new(
        engine.sender(),
        Arc::new(FakeProcessRunner::new()),
    ));

    let out = engine.handle(Message::ToggleMcpServer);
    assert_eq!(out.effect, Some(Effect::StartMcpServer));
    assert_eq!(
        engine.mcp_status(),
        McpStatus::Stopped,
        "the pure transition starts nothing itself"
    );

    assert!(engine.start_mcp(Arc::clone(&backend), 0));
    let port = pump_until_listening(&mut engine, &mut rx).await;
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let client = McpClient::connect(addr).await;
    wait_for_clients(&engine, 1).await;
    client.disconnect().await;
    wait_for_clients(&engine, 0).await;

    let out = engine.handle(Message::ToggleMcpServer);
    assert_eq!(out.effect, Some(Effect::StopMcpServer));
    assert!(engine.stop_mcp());
    pump_until_stopped(&mut engine, &mut rx).await;
    assert_eq!(engine.mcp_status(), McpStatus::Stopped);
    assert!(
        TcpStream::connect_timeout(&addr, DEADLINE).is_err(),
        "the toggle actually released the listener"
    );
}

/// Drains the engine channel (the loop's job) until the server reports the
/// port it bound.
async fn pump_until_listening(
    engine: &mut Engine,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Message>,
) -> u16 {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let msg = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("the server never reported its bound port")
            .expect("the engine channel closed");
        engine.handle(msg);
        if let McpStatus::Listening { port, .. } = engine.mcp_status() {
            return port;
        }
    }
}

/// Drains the engine channel until the server task reports that it stopped.
async fn pump_until_stopped(
    engine: &mut Engine,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Message>,
) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let msg = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("the server never reported that it stopped")
            .expect("the engine channel closed");
        let stopped = matches!(msg, Message::McpStopped(_));
        engine.handle(msg);
        if stopped {
            return;
        }
    }
}

/// Yields until the status reports `expected` connected clients.
async fn wait_for_clients(engine: &Engine, expected: usize) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let McpStatus::Listening { clients, .. } = engine.mcp_status()
            && clients == expected
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the client count never reached {expected}: {:?}",
            engine.mcp_status()
        );
        tokio::task::yield_now().await;
    }
}

/// A session id the workbench never assigned is a typed refusal, not a
/// silent success.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_an_unknown_session_is_refused() {
    let workbench = Workbench::start(
        PathBuf::from("/tmp/frust-tui-mcp-embedded-unknown-test"),
        Arc::new(FakeProcessRunner::new()),
    );
    let backend = workbench.backend();
    let error = tokio::task::spawn_blocking(move || {
        backend
            .stop_app(frust_mcp::engine::SessionId(4242))
            .expect_err("an unknown session cannot be stopped")
            .to_string()
    })
    .await
    .expect("the stop task panicked");
    assert!(
        error.contains("no such session"),
        "unexpected refusal: {error}"
    );
}
