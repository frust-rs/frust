//! End-to-end headless DAP session test.
//!
//! Drives a full session through the real codec + [`run_session`] state
//! machine + [`OrchestrationAdapter`] stack over a `tokio::io::duplex` pair —
//! the same shape an attached editor drives, minus the socket. The backend
//! underneath is a real `frust_mcp::SessionEngine` (the reference
//! `SessionBackend`) over `frust_drive::process::FakeProcessRunner`, so
//! nothing here shells out for real: exactly the seam an embedding host fills
//! with its own supervisor.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use frust_dap::codec::{CodecError, read_message, write_message};
use frust_dap::{
    Capabilities, DapEvent, DapMessage, DapRequest, DapResponse, EventSender, OrchestrationAdapter,
    SharedBackend, run_session,
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
/// against `backend` — the one thing `frust_dap::serve_embedded` hands each
/// connection, and the thing a launch reads its project root back off.
fn spawn_session_with_backend(
    backend: SharedBackend,
) -> (TestClient, JoinHandle<std::result::Result<(), CodecError>>) {
    let (server_reader, client_writer) = tokio::io::duplex(DUPLEX_BUFFER);
    let (client_reader, server_writer) = tokio::io::duplex(DUPLEX_BUFFER);

    let handle = tokio::spawn(async move {
        run_session(
            server_reader,
            server_writer,
            move |events: EventSender| OrchestrationAdapter::new(events, backend),
            tokio_util::sync::CancellationToken::new(),
        )
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

/// A `launch.json`-shaped configuration — including a `projectRoot` the server
/// will *not* honor, since the embedding host owns that decision.
fn launch_arguments(project_root: &str) -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "projectRoot": project_root,
        "device": "desktop",
        "mode": "debug",
    }))
}

/// The full lifecycle script: `initialize` -> `launch` -> `threads` -> an
/// unknown command -> `disconnect`, asserting on the wire messages at every
/// step and on the backend's own state at the end.
#[tokio::test]
async fn a_headless_client_drives_a_full_session_over_a_duplex_pair() {
    let project_root = tempdir_fixture("full-session");
    let runner = Arc::new(FakeProcessRunner::new().with_hanging_stream(
        DEBUG_DESKTOP_INVOCATION,
        vec!["booting the app", "ready for input"],
    ));
    let engine = Arc::new(SessionEngine::with_runner(project_root.clone(), runner));
    let backend: SharedBackend = Arc::clone(&engine) as SharedBackend;

    let (mut client, handle) = spawn_session_with_backend(backend);

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

    // ── 2. launch -> success response; the client's own projectRoot is
    //        refused in the console, and output events carry the fake app's
    //        lines ─────────────────────────────────────────────────────────
    client
        .request(2, "launch", launch_arguments("/home/attacker/evil"))
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
    let ignored = before
        .iter()
        .filter_map(output_text)
        .find(|text| text.contains("Ignoring"))
        .expect("the ignored client projectRoot is surfaced in the console");
    assert!(ignored.contains("/home/attacker/evil"), "{ignored}");
    assert!(
        before.iter().any(|event| output_text(event)
            .is_some_and(|t| t.contains(&project_root.display().to_string()))),
        "the banner names the host's root, the one the build used: {before:?}"
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

    // ── 5. disconnect -> response; the app is stopped; stream ends cleanly ─
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

    // The teardown proof, both halves: the launched app is stopped — not
    // merely told to stop — and the host's backend is still open for business,
    // because a DAP client leaving must not take the workbench's session world
    // with it.
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
        !matches!(
            engine.session(probe).map(|s| s.state),
            Some(SessionState::Failed { .. })
        ),
        "the DAP teardown shut the host's backend down: {:?}",
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

    engine.shutdown().await;
}

/// The app ending on its own is reported over the wire — the event feed's
/// terminal `Exited`, not a poll of session state — as the app's last lines,
/// then `exited`, then `terminated`, in that order.
#[tokio::test]
async fn an_app_that_exits_on_its_own_reports_its_last_output_then_exited_then_terminated() {
    let project_root = tempdir_fixture("app-exit");
    let runner = Arc::new(FakeProcessRunner::new().with_stream(
        DEBUG_DESKTOP_INVOCATION,
        vec!["working", "all done"],
        true,
    ));
    let engine = Arc::new(SessionEngine::with_runner(project_root.clone(), runner));

    let (mut client, handle) = spawn_session_with_backend(Arc::clone(&engine) as SharedBackend);
    client.initialize().await;

    client
        .request(
            2,
            "launch",
            launch_arguments(&project_root.display().to_string()),
        )
        .await;
    let (launch, _) = client.next_response().await;
    assert!(launch.success, "{:?}", launch.message);

    // Collect everything up to `terminated`: the ordering is the assertion.
    let mut seen: Vec<DapEvent> = Vec::new();
    loop {
        match client.next().await {
            DapMessage::Event(event) => {
                let done = event.event == "terminated";
                seen.push(event);
                if done {
                    break;
                }
            }
            other => panic!("expected an event, got {other:?}"),
        }
    }

    let index = |name: &str| {
        seen.iter()
            .position(|event| event.event == name)
            .unwrap_or_else(|| panic!("no {name} event in {seen:?}"))
    };
    let last_output = seen
        .iter()
        .position(|event| output_text(event).is_some_and(|t| t.contains("all done")))
        .expect("the app's last line reached the client");
    assert!(
        last_output < index("exited"),
        "the app's final output must land before its exit: {seen:?}"
    );
    assert!(
        index("exited") < index("terminated"),
        "exited precedes terminated: {seen:?}"
    );
    assert_eq!(
        seen[index("exited")].body.as_ref().expect("exited body")["exitCode"],
        0,
        "a successful run exits 0"
    );

    client.request(3, "disconnect", None).await;
    let (disconnect, more) = client.next_response().await;
    assert!(disconnect.success);
    assert!(
        !more.iter().any(|event| event.event == "terminated"),
        "terminated is reported once per connection: {more:?}"
    );
    join(handle).await;

    engine.shutdown().await;
}
