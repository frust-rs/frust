//! `frust mcp` — serves the MCP (Model Context Protocol) Streamable HTTP
//! endpoint AI agents use to drive/diagnose Frust apps.
//!
//! Mirrors `commands::tui`: frust-mcp constructs its own internals
//! (transport, handler, Ctrl-C wiring) and owns its own async runtime entry
//! point (`frust_mcp::run`), so this handler is a thin `Runtime::new` +
//! `block_on` shim, not a construction site of its own.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

/// Launches the MCP server. Returns 0 on success. Ctrl-C is handled inside
/// `frust_mcp::run` itself (graceful shutdown, `Ok(())`) — this only
/// propagates a genuine failure (e.g. bind failure, or a missing `project`
/// directory checked up front).
pub fn run(port: Option<u16>, project: Option<PathBuf>) -> Result<u8> {
    let project_root = match project {
        Some(dir) => dir,
        None => std::env::current_dir().context("reading current directory")?,
    };
    if !project_root.exists() {
        bail!(
            "project directory `{}` does not exist",
            project_root.display()
        );
    }

    let mut config = frust_mcp::McpConfig::new(project_root);
    if let Some(port) = port {
        config = config.with_port(port);
    }

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(frust_mcp::run(config))?;
    Ok(0)
}
