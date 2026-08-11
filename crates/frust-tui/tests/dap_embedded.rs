//! The embedded DAP server against a workbench with **no project open**.
//!
//! Where a debug launch builds is not decided when the server starts: the
//! adapter reads it back off the host's `SessionBackend` per launch. So the
//! answer can be "nowhere" — the workbench's project was closed after the
//! server came up — and the only honest response is a refusal, in band, naming
//! what the developer has to do about it.
//!
//! This test lives here rather than in `frust-dap` because a `SessionBackend`
//! that reports no project has to be written somewhere, and `frust-dap`'s
//! charter (`crates/frust-dap/Cargo.toml`) excludes the `anyhow` its fallible
//! methods are typed in. The workbench's own backend is the real thing anyway.
//!
//! **No process is ever really spawned** — the loop below supervises a
//! `FakeProcessRunner` (`docs/CODE_STANDARDS.md`'s `ProcessRunner` contract),
//! and the launch is refused long before anything would be run.
//!
//! The DAP client is deliberately blocking `std::net` inside
//! `spawn_blocking`: it models exactly how a real editor's requests reach the
//! server, and how a `SessionBackend` call reaches the workbench — from a
//! thread that is not the one running the event loop.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use frust_dap::DapClientRegistry;
use frust_drive::process::FakeProcessRunner;
use frust_mcp::SharedBackend;
use frust_tui::engine::{AppState, Message};
use frust_tui::supervise::{
    DevtoolsBridge, McpServeCtx, McpSessionRecords, PendingWidgetTrees, SessionSubscribers,
    Supervisor, TuiSessionBackend, mcp_backend::MAX_ADHOC_SESSION_ID, serve_command,
};
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// A failure deadline for anything this test waits on — never a pacing device.
const DEADLINE: Duration = Duration::from_secs(20);

/// One Content-Length-framed DAP message, as an editor writes it.
fn frame(body: &str) -> String {
    format!("Content-Length: {}\r\n\r\n{body}", body.len())
}

/// A launch refused for want of a project says so in band, and says what to do
/// about it — the workbench-side half of the rule that a launch builds in the
/// project the workbench is on *right now*.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_launch_with_no_project_open_is_refused_in_band() {
    // The workbench's model: started, but with no project open — the state a
    // DAP client can find the host in at any moment.
    let state = AppState::default();
    assert!(
        state.project_root.is_none(),
        "sanity: this test is about a workbench with nothing open"
    );

    let runner = Arc::new(FakeProcessRunner::new());
    let (tx, mut rx) = unbounded_channel::<Message>();
    let backend: SharedBackend =
        Arc::new(TuiSessionBackend::new(tx.clone(), Arc::clone(&runner) as _));

    // The event loop, trimmed to the one thing this test needs answered: the
    // `SessionBackend` commands the servers post into the engine channel.
    let pump = tokio::spawn(async move {
        let (mut supervisor, _events) = Supervisor::new(Arc::clone(&runner) as _);
        let mut records = McpSessionRecords::new();
        let mut next_adhoc_id = MAX_ADHOC_SESSION_ID;
        let mut subscribers = SessionSubscribers::new();
        let mut pending_trees = PendingWidgetTrees::new();
        let mut devtools = DevtoolsBridge::new(Arc::clone(&runner) as _);
        while let Some(message) = rx.recv().await {
            if let Message::Mcp(command) = message {
                serve_command(
                    command,
                    &mut McpServeCtx {
                        state: &state,
                        supervisor: &mut supervisor,
                        records: &mut records,
                        tx: &tx,
                        next_adhoc_id: &mut next_adhoc_id,
                        subscribers: &mut subscribers,
                        devtools: &mut devtools,
                        pending_trees: &mut pending_trees,
                    },
                );
            }
        }
    });

    let cancel = CancellationToken::new();
    let (ready_tx, ready_rx) = oneshot::channel();
    let server = tokio::spawn(frust_dap::serve_embedded(
        backend,
        DapClientRegistry::new(),
        0,
        Some(ready_tx),
        cancel.clone(),
    ));
    let port = tokio::time::timeout(DEADLINE, ready_rx)
        .await
        .expect("the embedded DAP listener did not come up in time")
        .expect("the ready channel was dropped before a send");

    let transcript = tokio::task::spawn_blocking(move || {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let mut stream = TcpStream::connect(addr).expect("connect to the embedded DAP listener");
        stream
            .set_read_timeout(Some(DEADLINE))
            .expect("setting a read deadline");
        for request in [
            r#"{"type":"request","seq":1,"command":"initialize","arguments":{"clientID":"test"}}"#,
            r#"{"type":"request","seq":2,"command":"launch","arguments":{"device":"desktop","mode":"debug"}}"#,
        ] {
            stream
                .write_all(frame(request).as_bytes())
                .expect("client write");
        }

        // Read until the launch answer arrives; the read deadline above turns a
        // server that never answers into a failure rather than a hang.
        let mut transcript = String::new();
        let mut buffer = [0u8; 4096];
        loop {
            let read = stream.read(&mut buffer).expect("client read");
            if read == 0 {
                break;
            }
            transcript.push_str(&String::from_utf8_lossy(&buffer[..read]));
            if transcript.contains(r#""command":"launch""#) {
                break;
            }
        }
        transcript
    })
    .await
    .expect("the DAP client panicked");

    assert!(
        transcript.contains(r#""success":false"#),
        "the launch must be refused, not answered: {transcript}"
    );
    assert!(
        transcript.contains("no project is open"),
        "the refusal must name the missing project: {transcript}"
    );
    assert!(
        transcript.contains("'launch' again"),
        "the refusal must say what would fix it: {transcript}"
    );

    cancel.cancel();
    tokio::time::timeout(DEADLINE, server)
        .await
        .expect("the server returned after cancellation")
        .expect("the server task panicked")
        .expect("a cancelled server ends cleanly");
    pump.abort();
}
