//! The embedded entry point: a DAP server inside its host.
//!
//! [`serve_embedded`] is the whole public surface a host needs. It owns no
//! runtime, no session supervisor, and no project of its own — the host passes
//! all three in — which is what makes a DAP client and the host's own UI drive
//! *the same* app sessions rather than two worlds that happen to look alike.
//!
//! It is the direct counterpart of `frust_mcp::serve_embedded`, down to the
//! argument order and the registry's guard discipline: a host that already
//! starts and stops the MCP server on a [`CancellationToken`] starts and stops
//! this one the same way.

use std::path::PathBuf;
use std::sync::Arc;

use frust_mcp::SharedBackend;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::{ServerError, serve_tcp};
use crate::adapter::OrchestrationAdapter;
use crate::clients::DapClientRegistry;

/// Serve DAP clients on `127.0.0.1:<bind_port>` over the host's own session
/// backend, until `cancel` fires.
///
/// - `backend` is shared by every connection this call accepts, and with
///   whatever else the host drives it from (its UI, an MCP client): a `launch`
///   here produces a session the host sees, and a stop there is an exit this
///   server reports.
/// - `project_root` is the **host's** project directory, and the only one a
///   session ever builds from. A client's own `launchArguments.projectRoot` is
///   never honored — the listener is unauthenticated loopback, and a
///   client-chosen build directory is arbitrary local code execution
///   (`cargo` runs `build.rs`, proc macros, and a `.cargo/config.toml`
///   `[target.*.runner]` out of it). A client that asks for a different one
///   is told, in its Debug Console, which directory was used instead.
/// - `bind_port` of `0` asks the OS for an ephemeral port; `ready`, when
///   given, is notified with the resolved port once the listener is up.
/// - `cancel` stops the accept loop *and* every live session: each is spawned
///   under a child token, so one `cancel()` breaks their read loops into the
///   same `on_disconnect` teardown a clean disconnect runs. Teardown stops the
///   app that connection launched and nothing else — the host's other
///   sessions, and the backend itself, outlive any DAP client.
///
/// `registry` is shared across every connection: one `DapClientGuard` (see
/// [`crate::clients`]) is registered per connection and moved into that
/// connection's adapter, so its `Drop` is the disconnect signal. Handing the
/// same registry back in across repeated start/stop cycles is safe — nothing
/// here is process-global, and a registry with no live guards left in it is
/// indistinguishable from a fresh one.
///
/// Returns [`ServerError::Bind`] if the port is taken; otherwise `Ok(())` once
/// the loop has stopped and its sessions have drained. (An embedder-facing
/// entry point in a crate whose charter excludes `anyhow` — the error is this
/// crate's own, and converts into a host's `anyhow` chain with `?`.)
pub async fn serve_embedded(
    backend: SharedBackend,
    registry: DapClientRegistry,
    project_root: PathBuf,
    bind_port: u16,
    ready: Option<oneshot::Sender<u16>>,
    cancel: CancellationToken,
) -> Result<(), ServerError> {
    // Both are cloned once per connection by the factory below; the `Arc`
    // keeps that from copying the path for every editor that attaches.
    let project_root = Arc::new(project_root);

    log::debug!(
        "frust-dap: embedding a DAP server rooted at {}",
        project_root.display()
    );

    serve_tcp(
        bind_port,
        move |events| {
            let guard = registry.register();
            OrchestrationAdapter::new(events, Arc::clone(&backend), PathBuf::clone(&project_root))
                .with_client_guard(guard)
        },
        ready,
        cancel,
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::sync::Arc;
    use std::time::Duration;

    use frust_drive::process::FakeProcessRunner;
    use frust_mcp::SessionEngine;
    use tokio::io::BufReader;
    use tokio::net::TcpStream;

    use super::*;
    use crate::protocol::codec::{read_message, write_message};
    use crate::protocol::types::{DapMessage, DapRequest};

    /// A failure deadline, never a pacing device.
    const DEADLINE: Duration = Duration::from_secs(5);

    fn backend() -> SharedBackend {
        Arc::new(SessionEngine::with_runner(
            "/tmp/frust-dap-embedded-test",
            Arc::new(FakeProcessRunner::new()),
        ))
    }

    /// The embed contract end to end: a client connects, is counted while it
    /// is connected — under the name it gave in `initialize` — and is gone
    /// from the registry once its session ends.
    #[tokio::test]
    async fn a_connected_client_is_registered_by_name_and_deregistered_at_the_end() {
        let registry = DapClientRegistry::new();
        let cancel = CancellationToken::new();
        let (ready_tx, ready_rx) = oneshot::channel();

        let server = tokio::spawn(serve_embedded(
            backend(),
            registry.clone(),
            PathBuf::from("/tmp/frust-dap-embedded-test"),
            0,
            Some(ready_tx),
            cancel.clone(),
        ));

        let port = tokio::time::timeout(DEADLINE, ready_rx)
            .await
            .expect("the listener came up")
            .expect("ready channel closed");
        assert_eq!(registry.count(), 0, "no client has connected yet");

        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .expect("connect to the embedded DAP listener");
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);

        write_message(
            &mut writer,
            &DapMessage::Request(DapRequest {
                seq: 1,
                command: "initialize".into(),
                arguments: Some(serde_json::json!({
                    "clientID": "vscode",
                    "clientName": "Visual Studio Code",
                })),
            }),
        )
        .await
        .expect("write initialize");

        let response = tokio::time::timeout(DEADLINE, read_message(&mut reader))
            .await
            .expect("initialize answered")
            .expect("read ok")
            .expect("not EOF");
        assert!(matches!(&response, DapMessage::Response(r) if r.success));

        let entries = registry.snapshot();
        let [entry] = entries.as_slice() else {
            panic!("expected exactly one registered client, got {entries:?}");
        };
        assert_eq!(entry.client_name.as_deref(), Some("Visual Studio Code"));
        assert_eq!(entry.client_id.as_deref(), Some("vscode"));

        // Cancellation is the host stopping the server: the accept loop ends,
        // the live session tears down, and its registration goes with it.
        cancel.cancel();
        let served = tokio::time::timeout(DEADLINE, server)
            .await
            .expect("serve_embedded returned after cancellation")
            .expect("the server task panicked");
        assert!(
            served.is_ok(),
            "a cancelled server ends cleanly: {served:?}"
        );
        assert_eq!(registry.count(), 0, "the guard deregistered the client");
    }

    /// A port already in use is reported, not panicked on — the host decides
    /// what to tell the user.
    #[tokio::test]
    async fn a_taken_port_is_reported_as_a_bind_failure() {
        let squatter = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind squatter");
        let port = squatter.local_addr().expect("local addr").port();

        let result = serve_embedded(
            backend(),
            DapClientRegistry::new(),
            PathBuf::from("/tmp/frust-dap-embedded-test"),
            port,
            None,
            CancellationToken::new(),
        )
        .await;

        match result {
            Err(ServerError::Bind { port: reported, .. }) => assert_eq!(reported, port),
            other => panic!("expected a bind failure, got {other:?}"),
        }
    }
}
