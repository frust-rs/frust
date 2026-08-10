//! `frust dap [--port N]` — Debug Adapter Protocol server for IDE integration.
//!
//! Bridges a DAP client (VS Code debugger, Zed, Helix, nvim-dap) to Frust's
//! app-supervision engine. Stdio mode (the default) runs the server's single
//! session over the process's own stdin/stdout; --port runs a loopback TCP
//! listener instead.
//!
//! **Stdout discipline:** In stdio mode, stdout *is* the DAP wire protocol —
//! this handler prints NOTHING to stdout, ever. Human-facing errors go to stderr.

use anyhow::Result;
use frust_dap::{DapConfig, TransportMode, run_blocking};
use frust_drive::process::ProcessRunner;
use std::sync::Arc;

/// Runs a DAP server to completion. Returns the process exit code.
///
/// The port parameter selects the transport:
/// - `None` → stdio (default; one session over stdin/stdout)
/// - `Some(port)` → TCP loopback listener (one session per connection)
///
/// Stdout is reserved for the DAP protocol in stdio mode and must not be
/// written to by anything in this handler. All diagnostics go to stderr via
/// the logger, which this handler does not install (it relies on the caller
/// to configure logging if desired).
pub fn run_in(runner: Arc<dyn ProcessRunner + Send + Sync>, port: Option<u16>) -> Result<u8> {
    let mode = match port {
        Some(p) => TransportMode::Tcp { port: p },
        None => TransportMode::Stdio,
    };

    let config = DapConfig::new(mode, runner);
    run_blocking(config).map_err(anyhow::Error::from)
}
