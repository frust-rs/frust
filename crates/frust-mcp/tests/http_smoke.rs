//! Transport-level tests for `frust-mcp`'s streamable-HTTP wiring: the
//! Host-header DNS-rebinding guard, graceful cancellation, and the `ping`
//! tool routed end to end through the full rmcp stack (initialize ->
//! notifications/initialized -> tools/call).
//!
//! The tool families themselves are covered by `tool_families.rs`; this file
//! stays about the transport. Both drive the server over raw `tokio`
//! `TcpStream`s (see `common/mcp.rs`), matching `frust-mcp`'s dependency
//! charter.

mod common;

use std::sync::Arc;

use common::TEST_PROJECT_ROOT;
use common::mcp::{McpTestServer, initialize_request, roundtrip, shutdown_server, start_server};
use frust_drive::process::FakeProcessRunner;
use frust_mcp::SessionEngine;
use serde_json::json;

/// An engine that can never launch anything — these tests never start a
/// session, they only exercise the transport.
fn idle_engine() -> Arc<SessionEngine> {
    Arc::new(SessionEngine::with_runner(
        TEST_PROJECT_ROOT,
        Arc::new(FakeProcessRunner::new()),
    ))
}

#[tokio::test]
async fn initialize_and_ping_tool_roundtrip_through_the_full_stack() {
    // `start` performs initialize (asserting the serverInfo identity) and
    // notifications/initialized.
    let mcp = McpTestServer::start(idle_engine()).await;

    // tools/call ping — proves the #[tool_router]/#[tool] macro plumbing
    // end to end, through the full rmcp session stack.
    let ping = mcp.call_ok("ping", json!({})).await;
    assert_eq!(ping["ok"], json!(true), "unexpected ping result: {ping}");
    assert_eq!(ping["sessions"], json!(0));

    // Cancellation shuts the server down cleanly: the run task must return,
    // not hang.
    mcp.shutdown().await;
}

#[tokio::test]
async fn host_header_outside_the_loopback_allowlist_is_refused() {
    let (addr, cancel, join) = start_server(idle_engine()).await;

    let request = initialize_request("evil.example");
    let response = roundtrip(addr, &request, "\r\n\r\n").await;
    assert!(
        response.starts_with("HTTP/1.1 403"),
        "expected the DNS-rebinding guard to reject a non-loopback Host header, got: {response}"
    );

    shutdown_server(cancel, join).await;
}

#[tokio::test]
async fn cancellation_shuts_the_server_down_cleanly() {
    let (addr, cancel, join) = start_server(idle_engine()).await;

    // The server accepts connections while running.
    let probe = tokio::net::TcpStream::connect(addr).await;
    assert!(probe.is_ok(), "server should be reachable before cancel");
    drop(probe);

    shutdown_server(cancel, join).await;

    // After shutdown, new connections are refused.
    let probe = tokio::net::TcpStream::connect(addr).await;
    assert!(probe.is_err(), "server should be unreachable after cancel");
}
