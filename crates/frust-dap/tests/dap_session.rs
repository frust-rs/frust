//! End-to-end headless DAP session test.
//!
//! Drives a full session through the real codec + [`run_session`] state
//! machine + [`OrchestrationAdapter`] stack over a `tokio::io::duplex` pair —
//! the same shape a stdio-connected editor drives, minus the process
//! boundary. The engine underneath is backed by
//! `frust_drive::process::FakeProcessRunner` (via the `#[doc(hidden)]`
//! `OrchestrationAdapter::with_engine` test seam — mirroring
//! `crates/frust-dap/src/adapter/mod.rs`'s own tests, since driving the
//! engine from an integration-test context needs the same seam those tests
//! already use), so nothing here shells out for real.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use frust_dap::codec::{CodecError, read_message, write_message};
use frust_dap::{
    Capabilities, DapEvent, DapMessage, DapRequest, DapResponse, EventSender, OrchestrationAdapter,
    run_session,
};
use frust_drive::build_info::BuildMode;
use frust_drive::process::FakeProcessRunner;
use frust_mcp::SessionEngine;
use frust_mcp::engine::{RunTarget, SessionState};
use tokio::io::{BufReader, DuplexStream};
use tokio::task::JoinHandle;

/// The exact invocation `frust_drive::desktop_run` resolves a Debug desktop
/// session to — the key the scripted stream is registered under. Drift here
/// (a change to the mode -> cargo-args funnel) must fail this test loudly
/// rather than silently launching nothing; see
/// `crates/frust-dap/src/adapter/mod.rs`'s own tests for the same constant.
const DEBUG_DESKTOP_INVOCATION: &str =
    "cargo run --features frust/perf-trace --features frust/devtools";

/// A failure deadline, never a pacing device.
const DEADLINE: Duration = Duration::from_secs(10);

const DUPLEX_BUFFER: usize = 8192;

/// A tempdir fixture no test ever writes into — it only ever reaches the
/// fake runner, and `SessionEngine`'s desktop path never touches the
/// filesystem itself.
fn tempdir_fixture(tag: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "frust-dap-session-test-{tag}-{}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A scripted DAP client over the far end of a duplex pair — frames/unframes
/// [`DapMessage`]s through the crate's real codec, exactly as an editor's DAP
/// client would.
struct TestClient {
    reader: BufReader<DuplexStream>,
    writer: DuplexStream,
}

impl TestClient {
    async fn request(&mut self, seq: i64, command: &str, arguments: Option<serde_json::Value>) {
        write_message(
            &mut self.writer,
            &DapMessage::Request(DapRequest {
                seq,
                command: command.to_owned(),
                arguments,
            }),
        )
        .await
        .expect("client write");
    }

    /// Next message, failing the test on timeout or EOF.
    async fn next(&mut self) -> DapMessage {
        tokio::time::timeout(DEADLINE, read_message(&mut self.reader))
            .await
            .expect("timed out waiting for a DAP message")
            .expect("client read")
            .expect("expected a message, got EOF")
    }

    /// The next response, collecting (and returning) every event that
    /// arrives ahead of it.
    async fn next_response(&mut self) -> (DapResponse, Vec<DapEvent>) {
        let mut events = Vec::new();
        loop {
            match self.next().await {
                DapMessage::Response(response) => return (response, events),
                DapMessage::Event(event) => events.push(event),
                other => panic!("unexpected message from the server: {other:?}"),
            }
        }
    }

    /// Reads events until one carries `needle` in its `output` body, failing
    /// on the deadline rather than hanging.
    async fn wait_for_output(&mut self, needle: &str) -> Vec<DapEvent> {
        let needle = needle.to_owned();
        let mut seen = Vec::new();
        loop {
            match self.next().await {
                DapMessage::Event(event) => {
                    let hit = output_text(&event).is_some_and(|t| t.contains(&needle));
                    seen.push(event);
                    if hit {
                        return seen;
                    }
                }
                other => panic!("expected an event, got {other:?}"),
            }
        }
    }

    /// The full handshake: `initialize` -> response, then the `initialized`
    /// event, in that order — asserting the two capabilities orchestration-v1
    /// promises.
    async fn initialize(&mut self) -> DapResponse {
        self.request(
            1,
            "initialize",
            Some(serde_json::json!({"clientID": "test", "clientName": "headless test client"})),
        )
        .await;
        let (response, before) = self.next_response().await;
        assert!(response.success, "initialize must succeed");
        assert!(
            before.is_empty(),
            "no event may precede the initialize response: {before:?}"
        );

        match self.next().await {
            DapMessage::Event(event) => {
                assert_eq!(
                    event.event, "initialized",
                    "the initialized event must follow the response, not precede it"
                );
            }
            other => panic!("expected the initialized event, got {other:?}"),
        }

        response
    }
}

/// The `output` body's text, for an event that carries one.
fn output_text(event: &DapEvent) -> Option<&str> {
    (event.event == "output")
        .then(|| event.body.as_ref()?["output"].as_str())
        .flatten()
}

/// A session over a duplex pair, driving a real [`OrchestrationAdapter`]
/// against `engine`.
fn spawn_session_with_engine(
    engine: Arc<SessionEngine>,
) -> (TestClient, JoinHandle<std::result::Result<(), CodecError>>) {
    let (server_reader, client_writer) = tokio::io::duplex(DUPLEX_BUFFER);
    let (client_reader, server_writer) = tokio::io::duplex(DUPLEX_BUFFER);

    let handle = tokio::spawn(async move {
        run_session(server_reader, server_writer, move |events: EventSender| {
            OrchestrationAdapter::with_engine(events, engine)
        })
        .await
    });

    (
        TestClient {
            reader: BufReader::new(client_reader),
            writer: client_writer,
        },
        handle,
    )
}

async fn join(handle: JoinHandle<std::result::Result<(), CodecError>>) {
    tokio::time::timeout(DEADLINE, handle)
        .await
        .expect("the session did not end")
        .expect("the session task panicked")
        .expect("session I/O");
}

fn launch_arguments(project_root: &Path) -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "projectRoot": project_root.to_string_lossy(),
        "device": "desktop",
        "mode": "debug",
    }))
}

/// The full lifecycle script: `initialize` -> `launch` -> `threads` -> an
/// unknown command -> `disconnect`, asserting on the wire messages at every
/// step and on the engine's own teardown state at the end.
#[tokio::test]
async fn a_headless_client_drives_a_full_session_over_stdio_shaped_duplex() {
    let project_root = tempdir_fixture("full-session");
    let runner = Arc::new(FakeProcessRunner::new().with_hanging_stream(
        DEBUG_DESKTOP_INVOCATION,
        vec!["booting the app", "ready for input"],
    ));
    let engine = Arc::new(SessionEngine::with_runner(project_root.clone(), runner));

    let (mut client, handle) = spawn_session_with_engine(Arc::clone(&engine));

    // ── 1. initialize -> response (capabilities), then initialized event ──
    let init_response = client.initialize().await;
    let body = init_response.body.expect("capabilities body");
    assert_eq!(
        body["supportsConfigurationDoneRequest"], true,
        "orchestration-v1 advertises configurationDone support"
    );
    assert_eq!(
        body["supportsTerminateRequest"], true,
        "orchestration-v1 advertises terminate support"
    );
    let expected_caps = serde_json::to_value(Capabilities::frust_defaults()).unwrap();
    assert_eq!(body, expected_caps);

    // ── 2. launch -> success response; output events carry the fake app's
    //        lines ─────────────────────────────────────────────────────────
    client
        .request(2, "launch", launch_arguments(&project_root))
        .await;
    let (launch_response, before) = client.next_response().await;
    assert!(
        launch_response.success,
        "launch failed: {:?}",
        launch_response.message
    );
    assert_eq!(launch_response.command, "launch");
    assert_eq!(launch_response.request_seq, 2);
    assert!(
        before
            .iter()
            .any(|event| output_text(event).is_some_and(|t| t.contains("Launching desktop"))),
        "the launch banner must precede the response: {before:?}"
    );

    let events = client.wait_for_output("ready for input").await;
    let app_line = events
        .iter()
        .find(|event| output_text(event).is_some_and(|t| t.contains("booting the app")))
        .expect("the app's first line arrived as an output event");
    let app_body = app_line.body.as_ref().expect("output body");
    assert_eq!(
        app_body["category"], "stdout",
        "app-emitted lines are stdout-category output events"
    );

    // ── 3. threads -> the single static thread ─────────────────────────────
    client.request(3, "threads", None).await;
    let (threads_response, _) = client.next_response().await;
    assert!(threads_response.success);
    let threads_body = threads_response.body.expect("threads body");
    let threads = threads_body["threads"].as_array().expect("threads array");
    assert_eq!(
        threads.len(),
        1,
        "orchestration-v1 reports one static thread"
    );
    assert_eq!(threads[0]["id"], 1);
    assert_eq!(threads[0]["name"], "app");

    // ── 4. an unknown command -> success: false; session still alive ──────
    client.request(4, "frobnicate", None).await;
    let (unknown_response, _) = client.next_response().await;
    assert!(
        !unknown_response.success,
        "an unrecognised command must be answered, not silently dropped"
    );
    assert_eq!(unknown_response.command, "frobnicate");
    assert!(
        unknown_response
            .message
            .as_deref()
            .unwrap_or_default()
            .contains("frobnicate"),
        "the refusal must name the command: {:?}",
        unknown_response.message
    );

    // Proof the connection survived: a routine request still answers.
    client.request(5, "configurationDone", None).await;
    let (config_done, _) = client.next_response().await;
    assert!(
        config_done.success,
        "the session must still be alive after the unknown command"
    );

    // ── 5. disconnect -> response; engine teardown is complete; stream ends
    //        cleanly ───────────────────────────────────────────────────────
    client.request(6, "disconnect", None).await;
    let (disconnect_response, _) = client.next_response().await;
    assert!(disconnect_response.success);
    assert_eq!(disconnect_response.command, "disconnect");

    // `on_disconnect` fires after the read loop ends, which is what puts the
    // `terminated` event *after* the disconnect response on this path (see
    // `crates/frust-dap/src/adapter/mod.rs`'s module doc on the ordering).
    match client.next().await {
        DapMessage::Event(event) => assert_eq!(event.event, "terminated"),
        other => panic!("expected the trailing terminated event, got {other:?}"),
    }

    join(handle).await;

    // The engine-level teardown proof: the launched session is terminal, and
    // the engine itself refuses new work — not merely "the app was told to
    // stop".
    let sessions = engine.sessions();
    let [session] = sessions.as_slice() else {
        panic!("expected exactly one session, got {}", sessions.len());
    };
    assert!(
        session.state.is_terminal(),
        "the launched app must be stopped by the time disconnect answers: {:?}",
        session.state
    );

    let probe = engine.run_app(RunTarget::Desktop, BuildMode::Debug);
    assert!(
        matches!(
            engine.session(probe).map(|s| s.state),
            Some(SessionState::Failed { .. })
        ),
        "a shut-down engine must refuse new launches: {:?}",
        engine.session(probe).map(|s| s.state)
    );

    // The wire itself ends cleanly: the server closed its write half, so the
    // client sees EOF rather than a hang.
    let trailing = tokio::time::timeout(DEADLINE, read_message(&mut client.reader))
        .await
        .expect("the stream should close promptly after disconnect")
        .expect("a clean EOF is not a read error");
    assert!(
        trailing.is_none(),
        "expected EOF after disconnect, got {trailing:?}"
    );
}
