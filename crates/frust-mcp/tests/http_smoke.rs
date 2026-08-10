//! HTTP smoke test for `frust-mcp`'s streamable-HTTP transport wiring: the
//! Host-header DNS-rebinding guard, and the `ping` tool routed end to end
//! through the full rmcp stack (initialize -> notifications/initialized ->
//! tools/call).
//!
//! Drives the server over a raw `std`/`tokio` `TcpStream` rather than an
//! HTTP client crate, matching `frust-mcp`'s dependency charter (rmcp/axum/
//! tokio/tokio-util/base64/serde/serde_json/log only).

use std::net::SocketAddr;
use std::time::Duration;

use frust_mcp::McpConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;
use tokio::time::{Instant, timeout};
use tokio_util::sync::CancellationToken;

const MCP_PATH: &str = "/mcp";
const PROTOCOL_VERSION: &str = "2025-06-18";

/// Starts the server on an OS-assigned port and returns its address, the
/// cancellation token that shuts it down, and the task running it.
async fn start_server() -> (
    SocketAddr,
    CancellationToken,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let config = McpConfig::new(std::env::temp_dir()).with_port(0);
    let (ready_tx, ready_rx) = oneshot::channel();
    let cancel = CancellationToken::new();
    let join = tokio::spawn(frust_mcp::run_with_ready(config, ready_tx, cancel.clone()));
    let addr = timeout(Duration::from_secs(5), ready_rx)
        .await
        .expect("server did not report ready in time")
        .expect("ready channel dropped before send");
    (addr, cancel, join)
}

/// Sends `request` to `addr` and reads the response until it contains
/// `until` or the peer closes the connection.
async fn roundtrip(addr: SocketAddr, request: &str, until: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .expect("failed to connect to frust-mcp");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("failed to write request");

    let mut response = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut buf = [0u8; 4096];
    loop {
        let read = timeout(deadline - Instant::now(), stream.read(&mut buf))
            .await
            .expect("timed out waiting for HTTP response")
            .expect("read failed");
        if read == 0 {
            break;
        }
        response.extend_from_slice(&buf[..read]);
        if String::from_utf8_lossy(&response).contains(until) {
            break;
        }
    }
    String::from_utf8_lossy(&response).into_owned()
}

fn initialize_request(host: &str) -> String {
    let body = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"{PROTOCOL_VERSION}","capabilities":{{}},"clientInfo":{{"name":"frust-mcp-smoke-test","version":"0.0.0"}}}}}}"#
    );
    format!(
        "POST {MCP_PATH} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Accept: application/json, text/event-stream\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len(),
    )
}

fn notifications_initialized_request(host: &str, session_id: &str) -> String {
    let body = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    format!(
        "POST {MCP_PATH} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Accept: application/json, text/event-stream\r\n\
         Content-Type: application/json\r\n\
         Mcp-Session-Id: {session_id}\r\n\
         MCP-Protocol-Version: {PROTOCOL_VERSION}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len(),
    )
}

fn tools_call_ping_request(host: &str, session_id: &str) -> String {
    let body =
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"ping","arguments":{}}}"#;
    format!(
        "POST {MCP_PATH} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Accept: application/json, text/event-stream\r\n\
         Content-Type: application/json\r\n\
         Mcp-Session-Id: {session_id}\r\n\
         MCP-Protocol-Version: {PROTOCOL_VERSION}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len(),
    )
}

/// Extracts the `Mcp-Session-Id` response header (case-insensitive) from a
/// raw HTTP response.
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

/// Scans an SSE (or plain JSON) HTTP response body for a `data: <json>` line
/// whose top-level `id` field matches `id`, and parses it — the proof the
/// response is a parseable MCP (JSON-RPC) message, not just a 200 status.
fn json_rpc_response(response: &str, id: u64) -> serde_json::Value {
    for line in response.lines() {
        let Some(data) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
            continue;
        };
        if value.get("id").and_then(serde_json::Value::as_u64) == Some(id) {
            return value;
        }
    }
    panic!("no JSON-RPC response with id={id} found in: {response}");
}

#[tokio::test]
async fn initialize_and_ping_tool_roundtrip_through_the_full_stack() {
    let (addr, cancel, join) = start_server().await;
    let host = format!("127.0.0.1:{}", addr.port());

    // initialize
    let request = initialize_request(&host);
    let response = roundtrip(addr, &request, "serverInfo").await;
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "initialize response was: {response}"
    );
    let init_result = json_rpc_response(&response, 1);
    assert_eq!(
        init_result["result"]["serverInfo"]["name"],
        serde_json::json!("frust-mcp"),
        "unexpected initialize response: {init_result}"
    );
    let session_id = session_id_header(&response);

    // notifications/initialized (no id, no response body expected — a 202)
    let request = notifications_initialized_request(&host, &session_id);
    let response = roundtrip(addr, &request, "\r\n\r\n").await;
    assert!(
        response.starts_with("HTTP/1.1 202"),
        "notifications/initialized response was: {response}"
    );

    // tools/call ping — proves the #[tool_router]/#[tool] macro plumbing
    // end to end, through the full rmcp session stack.
    let request = tools_call_ping_request(&host, &session_id);
    let response = roundtrip(addr, &request, "\"id\":2").await;
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "tools/call response was: {response}"
    );
    let call_result = json_rpc_response(&response, 2);
    let structured = &call_result["result"]["structuredContent"];
    assert_eq!(
        structured["ok"],
        serde_json::json!(true),
        "unexpected ping result: {call_result}"
    );

    // Cancellation shuts the server down cleanly: the run task must return,
    // not hang.
    cancel.cancel();
    timeout(Duration::from_secs(5), join)
        .await
        .expect("server task did not shut down within the deadline")
        .expect("server task panicked")
        .expect("server task returned an error");
}

#[tokio::test]
async fn host_header_outside_the_loopback_allowlist_is_refused() {
    let (addr, cancel, join) = start_server().await;

    let request = initialize_request("evil.example");
    let response = roundtrip(addr, &request, "\r\n\r\n").await;
    assert!(
        response.starts_with("HTTP/1.1 403"),
        "expected the DNS-rebinding guard to reject a non-loopback Host header, got: {response}"
    );

    cancel.cancel();
    timeout(Duration::from_secs(5), join)
        .await
        .expect("server task did not shut down within the deadline")
        .expect("server task panicked")
        .expect("server task returned an error");
}

#[tokio::test]
async fn cancellation_shuts_the_server_down_cleanly() {
    let (addr, cancel, join) = start_server().await;

    // The server accepts connections while running.
    let probe = tokio::net::TcpStream::connect(addr).await;
    assert!(probe.is_ok(), "server should be reachable before cancel");
    drop(probe);

    cancel.cancel();
    timeout(Duration::from_secs(5), join)
        .await
        .expect("server task did not shut down within the deadline (hung)")
        .expect("server task panicked")
        .expect("server task returned an error");

    // After shutdown, new connections are refused.
    let probe = tokio::net::TcpStream::connect(addr).await;
    assert!(probe.is_err(), "server should be unreachable after cancel");
}
