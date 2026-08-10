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
//! Today this is a skeleton crate: the only tool is `ping`
//! (`{"ok": true}` structured content), proving the rmcp tool-router
//! plumbing works end to end. Session control (backed by `frust-drive`) and
//! the DevTools diagnosis / app-driving tool families land in later work,
//! behind the same [`McpConfig`]/[`run`] seam.

mod config;
mod handler;
mod server;

pub use config::{DEFAULT_MCP_PORT, McpConfig};

use std::net::SocketAddr;

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
    server::serve(config, None, cancel).await
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
    server::serve(config, Some(ready), cancel).await
}
