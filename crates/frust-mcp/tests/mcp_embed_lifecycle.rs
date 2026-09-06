//! `serve_embedded` + `ClientRegistry`: the runtime-toggled embedding entry
//! point a host (e.g. `frust-tui`) starts and stops on its own, over its own
//! caller-owned backend and registry.
//!
//! **No sleeps.** Every wait is on something the server or the registry
//! itself produces: the ready signal, an HTTP response, or a bounded
//! yield-until-condition poll on the registry's own state (there is no
//! change-notification channel to await instead — see `clients.rs`'s
//! module doc on why a guard's `Drop` is the only disconnect signal at
//! all). The only durations here are failure deadlines.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use common::TEST_PROJECT_ROOT;
use common::mcp::{initialize_request, post, roundtrip, session_id_header};
use frust_drive::process::FakeProcessRunner;
use frust_mcp::{ClientRegistry, SessionEngine, SharedBackend};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};
use tokio_util::sync::CancellationToken;

/// A failure deadline for waiting on a registry/server condition — never a
/// pacing device.
const DEADLINE: Duration = Duration::from_secs(10);

fn idle_backend() -> SharedBackend {
    Arc::new(SessionEngine::with_runner(
        TEST_PROJECT_ROOT,
        Arc::new(FakeProcessRunner::new()),
    ))
}

/// Starts `serve_embedded` on an OS-assigned port over `registry`, returning
/// once the ready signal fires.
async fn start_embedded(
    registry: ClientRegistry,
) -> (
    SocketAddr,
    CancellationToken,
    JoinHandle<anyhow::Result<()>>,
) {
    let (ready_tx, ready_rx) = oneshot::channel();
    let cancel = CancellationToken::new();
    let join = tokio::spawn(frust_mcp::serve_embedded(
        idle_backend(),
        registry,
        0,
        Some(ready_tx),
        cancel.clone(),
    ));
    let addr = timeout(DEADLINE, ready_rx)
        .await
        .expect("server did not report ready in time")
        .expect("ready channel dropped before send");
    (addr, cancel, join)
}

/// Cancels a server started by [`start_embedded`] and asserts it shut down
/// cleanly rather than hanging or erroring.
async fn shutdown_embedded(cancel: CancellationToken, join: JoinHandle<anyhow::Result<()>>) {
    cancel.cancel();
    timeout(DEADLINE, join)
        .await
        .expect("server task did not shut down within the deadline")
        .expect("server task panicked")
        .expect("server task returned an error");
}

/// Spins (yielding to the executor, never sleeping) until `registry.count()`
/// equals `expected` or `DEADLINE` elapses.
async fn wait_for_count(registry: &ClientRegistry, expected: usize) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if registry.count() == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "registry count did not reach {expected} within {DEADLINE:?} (still {})",
            registry.count()
        );
        tokio::task::yield_now().await;
    }
}

/// Opens an MCP session against `addr` (the `initialize` handshake only —
/// enough for the service factory to have run and registered its client
/// guard) and returns the assigned session id.
async fn open_session(addr: SocketAddr) -> String {
    let host = format!("127.0.0.1:{}", addr.port());
    let response = roundtrip(addr, &initialize_request(&host), "serverInfo").await;
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "initialize response was: {response}"
    );
    session_id_header(&response)
}

/// Closes an MCP session with the standard client-disconnect path (HTTP
/// `DELETE`), the clean-close half of what drops a session's `ClientGuard`.
async fn close_session(addr: SocketAddr, session_id: &str) {
    let host = format!("127.0.0.1:{}", addr.port());
    let request = format!(
        "DELETE /mcp HTTP/1.1\r\n\
         Host: {host}\r\n\
         Mcp-Session-Id: {session_id}\r\n\
         MCP-Protocol-Version: 2025-06-18\r\n\
         Connection: close\r\n\r\n"
    );
    roundtrip(addr, &request, "\r\n").await;
}

#[tokio::test]
async fn opening_and_closing_a_session_registers_and_removes_a_client() {
    let registry = ClientRegistry::new();
    let (addr, cancel, join) = start_embedded(registry.clone()).await;
    assert_eq!(registry.count(), 0, "no client before any session opens");

    let session_id = open_session(addr).await;
    wait_for_count(&registry, 1).await;
    let snapshot = registry.snapshot();
    assert_eq!(
        snapshot.len(),
        1,
        "exactly one populated entry: {snapshot:?}"
    );
    assert!(
        snapshot[0].connected_at <= std::time::SystemTime::now(),
        "connect stamp must not be in the future"
    );

    close_session(addr, &session_id).await;
    wait_for_count(&registry, 0).await;
    assert!(registry.snapshot().is_empty());

    shutdown_embedded(cancel, join).await;
}

#[tokio::test]
async fn start_cancel_start_again_on_the_same_registry_works() {
    let registry = ClientRegistry::new();

    let (addr, cancel, join) = start_embedded(registry.clone()).await;
    assert!(
        tokio::net::TcpStream::connect(addr).await.is_ok(),
        "server should be reachable before cancel"
    );
    shutdown_embedded(cancel, join).await;
    assert!(
        tokio::net::TcpStream::connect(addr).await.is_err(),
        "server should be unreachable after cancel"
    );

    // Same registry, second lifecycle: a session opened and closed on the
    // first run must not leave a stale count behind for the second.
    assert_eq!(registry.count(), 0);
    let (addr, cancel, join) = start_embedded(registry.clone()).await;
    let session_id = open_session(addr).await;
    wait_for_count(&registry, 1).await;

    close_session(addr, &session_id).await;
    wait_for_count(&registry, 0).await;
    shutdown_embedded(cancel, join).await;
}

// Exercises `post`/`roundtrip` re-export from `common::mcp` so the shared
// helper module stays linked even though this file drives the transport
// with its own open/close helpers above.
#[tokio::test]
async fn a_second_client_raises_the_count_to_two() {
    let registry = ClientRegistry::new();
    let (addr, cancel, join) = start_embedded(registry.clone()).await;

    let first = open_session(addr).await;
    wait_for_count(&registry, 1).await;
    let second = open_session(addr).await;
    wait_for_count(&registry, 2).await;

    // `post`/`roundtrip` prove a session id from `open_session` is usable
    // for a real request, not just accepted by `initialize`.
    let list_tools = post(
        &format!("127.0.0.1:{}", addr.port()),
        &first,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
    );
    let response = roundtrip(addr, &list_tools, "\r\n\r\n").await;
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "response was: {response}"
    );

    close_session(addr, &first).await;
    wait_for_count(&registry, 1).await;
    close_session(addr, &second).await;
    wait_for_count(&registry, 0).await;

    shutdown_embedded(cancel, join).await;
}
