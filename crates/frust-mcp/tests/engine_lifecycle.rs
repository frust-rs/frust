//! End-to-end tests for [`frust_mcp::SessionEngine`]: a desktop session
//! launched through a scripted `FakeProcessRunner`, whose output announces a
//! devtools service that a hand-rolled NDJSON fixture server on real loopback
//! answers.
//!
//! **No process is ever really spawned** — every external invocation goes
//! through the injected `ProcessRunner`, per `docs/CODE_STANDARDS.md`.
//!
//! **The fixture server speaks the protocol leaf only.** It reimplements just
//! enough JSON-RPC framing to drive the client, exactly as `frust-drive`'s
//! own `devtools_client` tests do, and deliberately does *not* depend on
//! `frust-devtools` — the tooling-isolation charter
//! (`docs/ARCHITECTURE.md`) forbids that edge from this crate, dev-dependency
//! or not.
//!
//! **No sleeps.** Every wait is on something the engine itself produces: a
//! session state transition, a log line landing in the ring, a frame arriving
//! — all through `SessionEngine::wait_for`, which blocks on the session's own
//! change signal. The only durations here are failure deadlines.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use frust_devtools_protocol::{
    AckResult, Capability, FrameStats, HandshakeInfo, HandshakeParams, Notification,
    PROTOCOL_VERSION, Request, Response, RpcError, encode_line, format_discovery_line,
    format_failure_line,
};
use frust_drive::build_info::BuildMode;
use frust_drive::process::FakeProcessRunner;
use frust_mcp::SessionEngine;
use frust_mcp::engine::{RunTarget, SessionSnapshot, SessionState};

/// The exact `cargo run` invocation `frust_drive::desktop_run::desktop_plan`
/// resolves a Debug desktop session to — the key the scripted fake stream is
/// registered under. If the drive's mode→args funnel ever changes, this key
/// stops matching and every test here fails loudly rather than silently
/// launching nothing.
const DEBUG_DESKTOP_INVOCATION: &str =
    "cargo run --features frust/perf-trace --features frust/devtools";

/// How long a test waits for a condition the engine must produce before
/// declaring failure. Generous: it is a failure deadline, never a pacing
/// device.
const DEADLINE: Duration = Duration::from_secs(10);

/// The token the fixture server requires at handshake — the stand-in for one
/// recovered from a real discovery line.
const FIXTURE_TOKEN: &str = "0123456789abcdef0123456789abcdef";

/// A hand-rolled NDJSON devtools server: enforces the same handshake-token
/// gate the real service does, answers `handshake`/`frame_stats_subscribe`
/// with canned results, and pushes one `frame_stats` notification right after
/// acking a subscribe.
///
/// It accepts **more than one** connection, because the engine's connect
/// thread opens a bounded `connect_timeout` reachability probe and drops it
/// before the client's own connect (see `engine::devtools`'s module doc). The
/// probe sends nothing, so it is served and discarded; the thread returns
/// once a connection that actually spoke closes — which is also what proves
/// the engine dropped its client at teardown.
fn spawn_fixture_server() -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
    let addr = listener.local_addr().expect("fixture server addr");
    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            if serve_fixture_connection(stream) {
                break;
            }
        }
    });
    (addr, handle)
}

/// Serves one fixture connection until EOF; returns whether it carried any
/// request at all (`false` for the engine's reachability probe).
fn serve_fixture_connection(stream: std::net::TcpStream) -> bool {
    let mut writer = stream.try_clone().expect("clone fixture stream");
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let mut authenticated = false;
    let mut served_any = false;
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return served_any;
        }
        let Ok(request) = serde_json::from_str::<Request>(line.trim_end()) else {
            continue;
        };
        served_any = true;

        if request.method == "handshake" {
            let presented = serde_json::from_value::<HandshakeParams>(request.params.clone())
                .ok()
                .and_then(|params| params.token);
            authenticated = presented.as_deref() == Some(FIXTURE_TOKEN);
        }
        if !authenticated {
            let response = Response::error(
                request.id,
                RpcError::unauthorized("present the devtools token at handshake"),
            );
            let _ = writeln!(writer, "{}", encode_line(&response));
            continue;
        }

        let result = if request.method == "handshake" {
            serde_json::to_value(HandshakeInfo {
                app_name: "fixture-app".into(),
                frust_version: "0.0.0".into(),
                protocol_version: PROTOCOL_VERSION,
                capabilities: vec![
                    Capability::WidgetTree,
                    Capability::FrameStats,
                    Capability::Input,
                ],
            })
            .expect("encode handshake result")
        } else {
            serde_json::to_value(AckResult { ok: true }).expect("encode ack")
        };
        let response = Response::success(request.id, result);
        let _ = writeln!(writer, "{}", encode_line(&response));

        if request.method == "frame_stats_subscribe" {
            let notification = Notification::new(
                "frame_stats",
                serde_json::to_value(fixture_frame()).expect("encode frame stats"),
            );
            let _ = writeln!(writer, "{}", encode_line(&notification));
        }
    }
}

/// The one frame sample the fixture server pushes; asserted field-for-field
/// so a silent re-shaping of the payload cannot pass.
fn fixture_frame() -> FrameStats {
    FrameStats {
        n: 7,
        total_us: 12_345,
        rebuild_us: 100,
        layout_us: 200,
        paint_us: 300,
        encode_us: 400,
        acquire_us: 500,
        submit_us: 600,
        skipped: false,
    }
}

/// An engine whose desktop launch replays `lines` and then hangs — a live
/// session that only ends when something kills it, like a real preview.
fn engine_with_hanging_desktop_stream(lines: Vec<String>) -> SessionEngine {
    let runner = FakeProcessRunner::new().with_hanging_stream(DEBUG_DESKTOP_INVOCATION, lines);
    SessionEngine::with_runner("/tmp/frust-mcp-engine-test", Arc::new(runner))
}

async fn await_snapshot(
    engine: &SessionEngine,
    id: frust_mcp::engine::SessionId,
    predicate: impl Fn(&SessionSnapshot) -> bool + Send + Clone + 'static,
) -> SessionSnapshot {
    let check = predicate.clone();
    let snapshot = engine
        .wait_for(id, DEADLINE, predicate)
        .await
        .expect("the session must exist");
    assert!(
        check(&snapshot),
        "condition not reached within {DEADLINE:?}; last snapshot: {snapshot:?}"
    );
    snapshot
}

/// The headline lifecycle: launch → the app announces a devtools service →
/// the engine connects and handshakes → `stop_app` kills and joins cleanly.
#[tokio::test]
async fn desktop_session_connects_devtools_then_stops_cleanly() {
    let (addr, server) = spawn_fixture_server();
    let engine = engine_with_hanging_desktop_stream(vec![
        "Compiling frust v0.1.0".to_string(),
        format_discovery_line(addr.port(), Some(FIXTURE_TOKEN)),
    ]);

    let id = engine.run_app(RunTarget::Desktop, BuildMode::Debug, None);
    let snapshot =
        await_snapshot(&engine, id, |s| s.state == SessionState::DevtoolsConnected).await;

    // The handshake's declared capabilities are recorded on the session, so a
    // tool can check "does this app support X?" without a round trip.
    let handshake = snapshot
        .devtools_handshake
        .as_ref()
        .expect("a connected session carries its handshake");
    assert_eq!(handshake.app_name, "fixture-app");
    assert_eq!(
        snapshot.devtools_capabilities(),
        Some(
            [
                Capability::WidgetTree,
                Capability::FrameStats,
                Capability::Input
            ]
            .as_slice()
        )
    );
    // Desktop connects directly to the discovered port — no `adb forward`.
    assert_eq!(snapshot.devtools_port, Some(addr.port()));
    assert!(snapshot.devtools_error.is_none());
    // The client is reachable for the tool layer.
    assert!(engine.devtools_client(id).is_some());
    // Desktop has no pid seam, so metrics can never start — the tool layer
    // must render that honestly rather than as a zeroed reading.
    assert!(!snapshot.metrics_sampling);

    engine.stop_app(id).await.expect("stop_app");
    let stopped = engine
        .session(id)
        .expect("the session outlives its process");
    assert_eq!(stopped.state, SessionState::Exited { success: false });

    drop(engine);
    // The fixture server's accept loop ends when the client's socket closes;
    // joining it proves the engine really did drop the connection.
    server.join().expect("fixture server thread");
}

/// A session whose app reports that its devtools service could not start must
/// carry the reason **verbatim** — an agent can only act on the per-app
/// network-toggle case if it reads exactly what the app said.
#[tokio::test]
async fn devtools_failure_line_surfaces_verbatim() {
    const REASON: &str = "Connection refused (os error 111)";
    let engine = engine_with_hanging_desktop_stream(vec![
        "Compiling frust v0.1.0".to_string(),
        format_failure_line(REASON),
    ]);

    let id = engine.run_app(RunTarget::Desktop, BuildMode::Debug, None);
    let snapshot = await_snapshot(&engine, id, |s| s.devtools_error.is_some()).await;

    assert_eq!(snapshot.devtools_error.as_deref(), Some(REASON));
    // The app itself is fine — only its debug service is missing.
    assert_eq!(snapshot.state, SessionState::Running);
    assert!(snapshot.devtools_handshake.is_none());

    engine.stop_app(id).await.expect("stop_app");
}

/// Every line the session emits is retained, and `logs` hands back the most
/// recent `n` in arrival order. (Eviction past the ring's capacity is pinned
/// at the real `LOG_RING_CAP` by the ring's own unit test — driving 10k lines
/// through a scripted stream would race two independently drop-oldest rings.)
#[tokio::test]
async fn log_ring_retains_lines_and_serves_a_tail() {
    let lines: Vec<String> = (0..12).map(|i| format!("line {i}")).collect();
    let engine = engine_with_hanging_desktop_stream(lines.clone());

    let id = engine.run_app(RunTarget::Desktop, BuildMode::Debug, None);
    let snapshot = await_snapshot(&engine, id, |s| s.log_lines >= 12).await;
    assert_eq!(snapshot.dropped_log_lines, 0);

    assert_eq!(engine.logs(id, None).expect("logs"), lines);
    assert_eq!(
        engine.logs(id, Some(3)).expect("logs"),
        vec![
            "line 9".to_string(),
            "line 10".to_string(),
            "line 11".to_string()
        ]
    );

    engine.stop_app(id).await.expect("stop_app");
}

/// A frame-stats notification pushed by the fixture lands in the session's
/// ring, field for field.
#[tokio::test]
async fn frame_stats_notifications_land_in_the_session_ring() {
    let (addr, server) = spawn_fixture_server();
    let engine = engine_with_hanging_desktop_stream(vec![format_discovery_line(
        addr.port(),
        Some(FIXTURE_TOKEN),
    )]);

    let id = engine.run_app(RunTarget::Desktop, BuildMode::Debug, None);
    let snapshot = await_snapshot(&engine, id, |s| s.frames >= 1).await;
    assert_eq!(snapshot.dropped_frames, 0);
    assert_eq!(
        engine.frame_ring(id).expect("frame ring"),
        vec![fixture_frame()]
    );

    engine.stop_app(id).await.expect("stop_app");
    drop(engine);
    server.join().expect("fixture server thread");
}

/// A launch the runner cannot even spawn ends as `Failed`, carrying the
/// drive's own error chain rather than hanging in `Launching` forever.
#[tokio::test]
async fn an_unspawnable_launch_fails_with_a_reason() {
    // No stream registered for the desktop invocation at all.
    let engine = SessionEngine::with_runner(
        "/tmp/frust-mcp-engine-test",
        Arc::new(FakeProcessRunner::new()),
    );

    let id = engine.run_app(RunTarget::Desktop, BuildMode::Debug, None);
    let snapshot = await_snapshot(&engine, id, |s| s.state.is_terminal()).await;

    match snapshot.state {
        SessionState::Failed { ref reason } => {
            assert!(
                reason.contains("cargo"),
                "the failure must name what could not be spawned, got {reason:?}"
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

/// `shutdown` tears down every live session, so a cancelled MCP server leaves
/// nothing running behind it.
#[tokio::test]
async fn shutdown_stops_every_session() {
    let engine = engine_with_hanging_desktop_stream(vec!["Compiling frust v0.1.0".to_string()]);
    let first = engine.run_app(RunTarget::Desktop, BuildMode::Debug, None);
    let second = engine.run_app(RunTarget::Desktop, BuildMode::Debug, None);
    await_snapshot(&engine, first, |s| s.state == SessionState::Running).await;
    await_snapshot(&engine, second, |s| s.state == SessionState::Running).await;

    engine.shutdown().await;

    for snapshot in engine.sessions() {
        assert!(
            snapshot.state.is_terminal(),
            "session {} survived shutdown: {snapshot:?}",
            snapshot.id
        );
    }
}

/// `restart_app` stops the old session and starts a new one with the same
/// target, mode, and project root.
#[tokio::test]
async fn restart_replaces_the_session_with_an_identical_spec() {
    let engine = engine_with_hanging_desktop_stream(vec!["Compiling frust v0.1.0".to_string()]);
    let first = engine.run_app(RunTarget::Desktop, BuildMode::Debug, None);
    await_snapshot(&engine, first, |s| s.state == SessionState::Running).await;

    let second = engine.restart_app(first).await.expect("restart_app");
    assert_ne!(first, second);
    let restarted = await_snapshot(&engine, second, |s| s.state == SessionState::Running).await;
    assert_eq!(restarted.mode, BuildMode::Debug);
    assert_eq!(restarted.project_root, engine.project_root().to_path_buf());
    assert!(matches!(restarted.target, RunTarget::Desktop));

    // The old id keeps reporting its own final state rather than vanishing.
    let old = engine.session(first).expect("the old session is retained");
    assert!(old.state.is_terminal());

    engine.shutdown().await;
}

/// An unknown id is an error, never a panic — an agent replaying a stale
/// session id must get a clean refusal.
#[tokio::test]
async fn unknown_session_ids_are_refused_cleanly() {
    let engine = SessionEngine::new("/tmp/frust-mcp-engine-test");
    let unknown = frust_mcp::engine::SessionId(9_999);
    assert!(engine.session(unknown).is_none());
    assert!(engine.logs(unknown, None).is_none());
    assert!(engine.frame_ring(unknown).is_none());
    assert!(engine.latest_metrics(unknown).is_none());
    assert!(engine.devtools_client(unknown).is_none());
    assert!(engine.stop_app(unknown).await.is_err());
    assert!(engine.restart_app(unknown).await.is_err());
}
