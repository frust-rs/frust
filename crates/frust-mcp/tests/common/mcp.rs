//! A minimal MCP client over raw HTTP.
//!
//! Drives the server over a `tokio::net::TcpStream` rather than an HTTP
//! client crate, matching `frust-mcp`'s dependency charter (rmcp/axum/tokio/
//! tokio-util/base64/serde/serde_json/log only). One connection per request
//! with `Connection: close`, which is all the streamable-HTTP transport needs
//! for a request/response exchange — the MCP session is carried by the
//! `Mcp-Session-Id` header, not by the socket.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use frust_mcp::{McpConfig, SessionEngine};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};
use tokio_util::sync::CancellationToken;

pub const MCP_PATH: &str = "/mcp";
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// A failure deadline for one HTTP exchange — never a pacing device.
const HTTP_DEADLINE: Duration = Duration::from_secs(10);

/// A running `frust-mcp` server plus an initialized MCP session against it.
pub struct McpTestServer {
    pub addr: SocketAddr,
    pub session_id: String,
    cancel: CancellationToken,
    join: JoinHandle<anyhow::Result<()>>,
}

impl McpTestServer {
    /// Starts the server on an OS-assigned port over `engine`, then runs the
    /// `initialize` / `notifications/initialized` handshake so tools can be
    /// called immediately.
    pub async fn start(engine: Arc<SessionEngine>) -> McpTestServer {
        let (addr, cancel, join) = start_server(engine).await;

        let host = format!("127.0.0.1:{}", addr.port());
        let response = roundtrip(addr, &initialize_request(&host), "serverInfo").await;
        assert!(
            response.starts_with("HTTP/1.1 200"),
            "initialize response was: {response}"
        );
        let init = json_rpc_response(&response, 1);
        assert_eq!(
            init["result"]["serverInfo"]["name"],
            serde_json::json!("frust-mcp"),
            "unexpected initialize response: {init}"
        );
        let session_id = session_id_header(&response);

        let request = post(
            &host,
            &session_id,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        );
        let response = roundtrip(addr, &request, "\r\n\r\n").await;
        assert!(
            response.starts_with("HTTP/1.1 202"),
            "notifications/initialized response was: {response}"
        );

        McpTestServer {
            addr,
            session_id,
            cancel,
            join,
        }
    }

    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.addr.port())
    }

    /// `tools/list`, as the `result` object.
    pub async fn list_tools(&self) -> Value {
        let id = next_request_id();
        let body = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/list","params":{{}}}}"#);
        let request = post(&self.host(), &self.session_id, &body);
        request_json(self.addr, &request, id).await["result"].clone()
    }

    /// `tools/call`, as the `result` object (an in-band tool failure is a
    /// result with `isError: true`, not an error, so this never panics on
    /// one).
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Value {
        // A fresh id per call so a response can never be mistaken for a
        // previous exchange's.
        let id = next_request_id();
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments },
        })
        .to_string();
        let request = post(&self.host(), &self.session_id, &body);
        let message = request_json(self.addr, &request, id).await;
        assert!(
            message.get("error").is_none(),
            "tool `{name}` failed at the protocol level (it should have returned an in-band \
             tool error instead): {message}"
        );
        message["result"].clone()
    }

    /// `tools/call`, asserting the tool succeeded, returning its structured
    /// content.
    pub async fn call_ok(&self, name: &str, arguments: Value) -> Value {
        let result = self.call_tool(name, arguments).await;
        assert_ne!(
            result["isError"],
            serde_json::json!(true),
            "tool `{name}` reported an error: {result}"
        );
        result["structuredContent"].clone()
    }

    /// `tools/call`, asserting the tool reported an in-band failure,
    /// returning its structured content.
    pub async fn call_err(&self, name: &str, arguments: Value) -> Value {
        let result = self.call_tool(name, arguments).await;
        assert_eq!(
            result["isError"],
            serde_json::json!(true),
            "tool `{name}` was expected to fail, but succeeded: {result}"
        );
        result["structuredContent"].clone()
    }

    /// Cancels the server and asserts it shut down cleanly.
    pub async fn shutdown(self) {
        shutdown_server(self.cancel, self.join).await;
    }
}

/// Starts the server on an OS-assigned port over `engine`, with no MCP
/// handshake — the seam a transport-level test (Host-header guard,
/// cancellation) drives directly.
pub async fn start_server(
    engine: Arc<SessionEngine>,
) -> (
    SocketAddr,
    CancellationToken,
    JoinHandle<anyhow::Result<()>>,
) {
    let config = McpConfig::new(engine.project_root().to_path_buf()).with_port(0);
    let (ready_tx, ready_rx) = oneshot::channel();
    let cancel = CancellationToken::new();
    let join = tokio::spawn(frust_mcp::run_with_engine(
        config,
        engine,
        ready_tx,
        cancel.clone(),
    ));
    let addr = timeout(Duration::from_secs(5), ready_rx)
        .await
        .expect("server did not report ready in time")
        .expect("ready channel dropped before send");
    (addr, cancel, join)
}

/// Cancels a server started by [`start_server`] and asserts it shut down
/// cleanly rather than hanging or erroring.
pub async fn shutdown_server(cancel: CancellationToken, join: JoinHandle<anyhow::Result<()>>) {
    cancel.cancel();
    timeout(Duration::from_secs(10), join)
        .await
        .expect("server task did not shut down within the deadline")
        .expect("server task panicked")
        .expect("server task returned an error");
}

fn next_request_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(100);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Sends `request` to `addr` and reads until a **complete** JSON-RPC message
/// with `id` has arrived (or the peer closes), returning that message.
///
/// Termination is on a successful parse rather than on a substring: an SSE
/// body arrives in chunks, and a payload as large as a widget-tree dump is
/// routinely split mid-JSON, so stopping at the first sight of `"id":<n>`
/// would hand back half a message.
pub async fn request_json(addr: SocketAddr, request: &str, id: u64) -> Value {
    let response = read_response(addr, request, |text| {
        find_json_rpc_response(text, id).is_some()
    })
    .await;
    find_json_rpc_response(&response, id)
        .unwrap_or_else(|| panic!("no JSON-RPC response with id={id} found in: {response}"))
}

/// Sends `request` to `addr` and reads the response until it contains
/// `until` or the peer closes the connection. For header-only exchanges (a
/// 202/403) where there is no JSON-RPC body to wait for.
pub async fn roundtrip(addr: SocketAddr, request: &str, until: &str) -> String {
    read_response(addr, request, |text| text.contains(until)).await
}

async fn read_response(addr: SocketAddr, request: &str, done: impl Fn(&str) -> bool) -> String {
    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .expect("failed to connect to frust-mcp");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("failed to write request");

    let mut response = Vec::new();
    let deadline = Instant::now() + HTTP_DEADLINE;
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
        if done(&String::from_utf8_lossy(&response)) {
            break;
        }
    }
    String::from_utf8_lossy(&response).into_owned()
}

/// An `initialize` POST with an arbitrary `Host` header — the parameter the
/// DNS-rebinding-guard test varies.
pub fn initialize_request(host: &str) -> String {
    let body = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"{PROTOCOL_VERSION}","capabilities":{{}},"clientInfo":{{"name":"frust-mcp-test","version":"0.0.0"}}}}}}"#
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

/// A POST carrying an established MCP session id.
pub fn post(host: &str, session_id: &str, body: &str) -> String {
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
pub fn session_id_header(response: &str) -> String {
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
pub fn json_rpc_response(response: &str, id: u64) -> Value {
    find_json_rpc_response(response, id)
        .unwrap_or_else(|| panic!("no JSON-RPC response with id={id} found in: {response}"))
}

fn find_json_rpc_response(response: &str, id: u64) -> Option<Value> {
    response.lines().find_map(|line| {
        let data = line.trim().strip_prefix("data:")?.trim();
        if data.is_empty() {
            return None;
        }
        let value = serde_json::from_str::<Value>(data).ok()?;
        (value.get("id").and_then(Value::as_u64) == Some(id)).then_some(value)
    })
}
