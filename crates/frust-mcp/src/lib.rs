//! `frust-mcp` — an MCP (Model Context Protocol) server exposing frust app
//! control and diagnosis to AI agents, over the Streamable HTTP transport on
//! `127.0.0.1` only (never configurable).
//!
//! ## Usage
//!
//! ```no_run
//! # async fn example() -> anyhow::Result<()> {
//! let config = frust_mcp::McpConfig::new(std::env::current_dir()?);
//! frust_mcp::run(config).await
//! # }
//! ```
//!
//! The crate is in two halves. [`engine`] is the session engine: it launches
//! and supervises real app sessions through `frust-drive`, and owns their
//! logs, devtools connections, frame stats, and metrics. The tool layer
//! (`tools`, surfaced through the rmcp handler) sits on top of it in three
//! families — session lifecycle, app driving, and diagnosis — and is the
//! whole agent-facing surface.
//!
//! The two halves meet at [`backend::SessionBackend`], not at the engine type:
//! the tools drive an `Arc<dyn SessionBackend>`, of which [`SessionEngine`] is
//! this crate's own (and the server's default) implementation.

pub mod backend;
pub mod engine;

mod clients;
mod config;
mod handler;
mod server;
mod tools;

pub use backend::{SessionBackend, SharedBackend};
pub use clients::{ClientEntry, ClientRegistry};
pub use config::{DEFAULT_MCP_PORT, McpConfig};
pub use engine::SessionEngine;
pub use server::serve_embedded;

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// Run the MCP server until interrupted (Ctrl-C) or bind fails.
///
/// Binds `127.0.0.1:config.port`, prints the one endpoint line
/// (`frust-mcp listening on http://127.0.0.1:<port>/mcp`) once listening,
/// and serves until a Ctrl-C signal cancels it — at which point it shuts
/// down gracefully and returns `Ok(())`.
pub async fn run(config: McpConfig) -> anyhow::Result<()> {
    let cancel = CancellationToken::new();
    let ctrl_c_cancel = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            log::info!("frust-mcp: Ctrl-C received, shutting down");
            ctrl_c_cancel.cancel();
        }
    });
    let engine = Arc::new(SessionEngine::new(config.project_root.clone()));
    serve_with_engine(config, engine, None, cancel).await
}

/// Test/embedding seam: run with a caller-owned [`CancellationToken`] (so
/// shutdown is triggered directly rather than via Ctrl-C), notifying `ready`
/// with the actual bound [`SocketAddr`] once listening — how a caller
/// discovers the OS-assigned port from `config.port == 0`.
#[doc(hidden)]
pub async fn run_with_ready(
    config: McpConfig,
    ready: oneshot::Sender<SocketAddr>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let engine = Arc::new(SessionEngine::new(config.project_root.clone()));
    serve_with_engine(config, engine, Some(ready), cancel).await
}

/// Test/embedding seam: [`run_with_ready`] against a caller-built engine.
///
/// The engine is what a test scripts (`SessionEngine::with_runner` over a
/// `FakeProcessRunner`), so the tool layer can be driven end to end over a
/// real MCP connection without ever launching a real app. The caller keeps
/// its own `Arc`, and every MCP session the server creates shares this one.
#[doc(hidden)]
pub async fn run_with_engine(
    config: McpConfig,
    engine: Arc<SessionEngine>,
    ready: oneshot::Sender<SocketAddr>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    serve_with_engine(config, engine, Some(ready), cancel).await
}

/// Serves with a live [`SessionEngine`] for the server's whole lifetime, and
/// tears every session it launched down on the way out.
///
/// The shutdown runs whether the server stopped cleanly or errored: a
/// cancelled MCP server must not leave orphaned preview windows, running
/// device apps, or `adb forward`s behind. `shutdown` is deliberately *not* a
/// [`SessionBackend`] method — it belongs to the server's lifetime, not to any
/// tool — so this is the one place that keeps the concrete engine while the
/// tool layer below it sees only the trait.
async fn serve_with_engine(
    config: McpConfig,
    engine: Arc<SessionEngine>,
    ready: Option<oneshot::Sender<SocketAddr>>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    // One allocation, two views of it: everything below sees the trait, this
    // function keeps the engine for the `shutdown` the trait does not carry.
    let backend: SharedBackend = engine.clone();
    let result = server::serve(config, backend, ready, cancel).await;
    engine.shutdown().await;
    result
}
