//! # DAP server entry points
//!
//! [`serve`] runs the DAP server in whichever [`TransportMode`] it is given:
//! one session over stdin/stdout, or a loopback TCP listener serving one
//! session per accepted connection.
//!
//! ## Per-connection, not per-server, state
//!
//! Every connection gets its own adapter from the caller's factory and its own
//! session task. There is no cross-client state, no fan-out bus, and no
//! server-level event channel: an orchestration session belongs to exactly one
//! client, and a second client drives a second app.
//!
//! ## Trust model
//!
//! The TCP transport binds `127.0.0.1` and nothing else — the address is not
//! configurable and there is no DAP-level authentication, the same deliberate
//! loopback-only stance `frust-mcp` takes. Any local process that can reach
//! the port can drive a build/launch through it.

pub mod session;

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::{Semaphore, oneshot};

use crate::protocol::codec::CodecError;
use crate::transport::TransportMode;
use crate::transport::stdio::run_stdio_session;

pub use session::{AdapterResponse, DapAdapter, EventSender, run_session};

/// The only address the TCP transport ever binds: loopback, `127.0.0.1`.
///
/// Hard-coded rather than configurable, mirroring `frust-mcp`'s
/// `McpConfig`: a DAP session can build, install, and launch code on a device,
/// and it is unauthenticated — exposing that on a network interface is not an
/// option worth offering.
const BIND_ADDR: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);

/// Maximum number of DAP client connections served at once.
///
/// A DAP server serves an editor, not a crowd; the cap exists so a
/// misbehaving client or a port scanner cannot grow tasks without bound.
/// Connections past it are closed immediately.
const MAX_CONCURRENT_CLIENTS: usize = 4;

/// Back-off after a failed `accept`, so a persistent OS-level failure (file
/// descriptor exhaustion) cannot spin the accept loop.
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(100);

/// Failures a DAP server start can report.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("DAP server failed to bind 127.0.0.1:{port}: {source}")]
    Bind {
        /// The requested port (`0` = OS-assigned).
        port: u16,
        /// The underlying bind/address failure.
        #[source]
        source: std::io::Error,
    },

    #[error("DAP session failed: {0}")]
    Session(#[from] CodecError),
}

/// Run the DAP server on the given transport until it ends.
///
/// `make_adapter` is invoked once per connection with that connection's
/// [`EventSender`]. Stdio mode returns when its single session ends; TCP mode
/// runs until the process is stopped (or the listener fails to bind).
pub async fn serve<A, F>(mode: TransportMode, make_adapter: F) -> Result<(), ServerError>
where
    A: DapAdapter,
    F: Fn(EventSender) -> A + Send + Sync + 'static,
{
    match mode {
        TransportMode::Stdio => {
            run_stdio_session(make_adapter).await?;
            Ok(())
        }
        TransportMode::Tcp { port } => serve_tcp(port, make_adapter, None).await,
    }
}

/// Accept DAP clients on `127.0.0.1:<port>`, one session per connection.
///
/// `port: 0` asks the OS for an ephemeral port. `ready`, when given, is
/// notified with the resolved port once the listener is up — the seam a test
/// (or an embedder) needs, since an OS-assigned port is otherwise only
/// visible in the log line.
///
/// Returns only on a bind failure: the accept loop itself is infinite, and a
/// caller that needs to stop it drops or aborts the future driving it.
pub async fn serve_tcp<A, F>(
    port: u16,
    make_adapter: F,
    ready: Option<oneshot::Sender<u16>>,
) -> Result<(), ServerError>
where
    A: DapAdapter,
    F: Fn(EventSender) -> A + Send + Sync + 'static,
{
    let addr = SocketAddr::V4(SocketAddrV4::new(BIND_ADDR, port));
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|source| ServerError::Bind { port, source })?;

    let bound_port = listener
        .local_addr()
        .map_err(|source| ServerError::Bind { port, source })?
        .port();

    log::info!("frust-dap listening on 127.0.0.1:{bound_port}");

    if let Some(ready) = ready {
        // A caller that stopped listening is not an error worth failing on.
        let _ = ready.send(bound_port);
    }

    let factory = Arc::new(make_adapter);
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CLIENTS));

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                // Non-blocking: a full server rejects immediately rather than
                // parking the accept loop behind a slot.
                let Ok(permit) = Arc::clone(&semaphore).try_acquire_owned() else {
                    log::warn!(
                        "frust-dap at its {MAX_CONCURRENT_CLIENTS}-client cap; \
                         rejecting connection from {peer}"
                    );
                    drop(stream);
                    continue;
                };

                log::debug!("DAP client connected: {peer}");

                let factory = Arc::clone(&factory);
                tokio::spawn(async move {
                    let (reader, writer) = stream.into_split();

                    match run_session(reader, writer, move |events| factory(events)).await {
                        Ok(()) => log::debug!("DAP client session ended: {peer}"),
                        Err(error) => log::warn!("DAP client session failed ({peer}): {error}"),
                    }

                    drop(permit);
                });
            }
            Err(error) => {
                log::error!("frust-dap failed to accept a connection: {error}");
                tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use tokio::io::BufReader;
    use tokio::net::TcpStream;

    use super::*;
    use crate::protocol::codec::{read_message, write_message};
    use crate::protocol::types::{Capabilities, DapMessage, DapRequest};

    /// Minimal adapter: enough to prove a connection reaches the session.
    struct PingAdapter;

    impl DapAdapter for PingAdapter {
        fn capabilities(&self) -> Capabilities {
            Capabilities::frust_defaults()
        }

        async fn handle_request(
            &mut self,
            command: &str,
            _arguments: Option<serde_json::Value>,
        ) -> AdapterResponse {
            match command {
                "disconnect" => AdapterResponse::ok(),
                other => AdapterResponse::unsupported(other),
            }
        }

        async fn on_disconnect(&mut self) {}
    }

    #[tokio::test]
    async fn test_tcp_serve_binds_loopback_and_completes_a_handshake() {
        let (ready_tx, ready_rx) = oneshot::channel();
        let server = tokio::spawn(serve_tcp(0, |_events| PingAdapter, Some(ready_tx)));

        let port = tokio::time::timeout(Duration::from_secs(2), ready_rx)
            .await
            .expect("listener did not come up")
            .expect("ready channel closed");

        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .expect("connect to the DAP listener");
        let peer = stream.peer_addr().expect("peer addr");
        assert_eq!(
            peer.ip().to_string(),
            "127.0.0.1",
            "the TCP transport binds loopback only"
        );

        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);

        write_message(
            &mut writer,
            &DapMessage::Request(DapRequest {
                seq: 1,
                command: "initialize".into(),
                arguments: None,
            }),
        )
        .await
        .expect("write initialize");

        let response = tokio::time::timeout(Duration::from_secs(2), read_message(&mut reader))
            .await
            .expect("initialize response timed out")
            .expect("read ok")
            .expect("not EOF");
        assert!(matches!(&response, DapMessage::Response(r) if r.success));

        let event = tokio::time::timeout(Duration::from_secs(2), read_message(&mut reader))
            .await
            .expect("initialized event timed out")
            .expect("read ok")
            .expect("not EOF");
        assert!(matches!(&event, DapMessage::Event(e) if e.event == "initialized"));

        write_message(
            &mut writer,
            &DapMessage::Request(DapRequest {
                seq: 2,
                command: "disconnect".into(),
                arguments: None,
            }),
        )
        .await
        .expect("write disconnect");

        let disconnect = tokio::time::timeout(Duration::from_secs(2), read_message(&mut reader))
            .await
            .expect("disconnect response timed out")
            .expect("read ok")
            .expect("not EOF");
        assert!(matches!(&disconnect, DapMessage::Response(r) if r.success));

        server.abort();
    }

    #[tokio::test]
    async fn test_tcp_serve_reports_a_bind_failure() {
        // Occupy a port, then ask the server for the same one.
        let squatter = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind squatter");
        let port = squatter.local_addr().expect("local addr").port();

        let result = serve_tcp(port, |_events| PingAdapter, None).await;

        match result {
            Err(ServerError::Bind { port: reported, .. }) => assert_eq!(reported, port),
            other => panic!("expected a bind failure, got {other:?}"),
        }
    }
}
