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
//! logs, devtools connections, frame stats, and metrics. The MCP tool layer
//! (today only `ping`) sits on top of it and is still being filled in — the
//! DevTools diagnosis and app-driving tool families land in later work,
//! behind the same [`McpConfig`]/[`run`] seam.

pub mod engine;

mod config;
mod handler;
mod server;

pub use config::{DEFAULT_MCP_PORT, McpConfig};
pub use engine::SessionEngine;

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
    serve_with_engine(config, None, cancel).await
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
    serve_with_engine(config, Some(ready), cancel).await
}

/// Serves with a live [`SessionEngine`] for the server's whole lifetime, and
/// tears every session it launched down on the way out.
///
/// The shutdown runs whether the server stopped cleanly or errored: a
/// cancelled MCP server must not leave orphaned preview windows, running
/// device apps, or `adb forward`s behind. The tool layer takes its own clone
/// of this same `Arc`.
async fn serve_with_engine(
    config: McpConfig,
    ready: Option<oneshot::Sender<SocketAddr>>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let engine = Arc::new(SessionEngine::new(config.project_root.clone()));
    let result = server::serve(config, ready, cancel).await;
    engine.shutdown().await;
    result
}
