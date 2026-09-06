//! MCP server configuration.

use std::path::PathBuf;

/// Default port the MCP server listens on.
///
/// `0` is also a valid value (not the default): it lets the OS pick an
/// ephemeral port, which is how the crate's own tests avoid a fixed-port
/// collision (see `run_with_ready`, `lib.rs`).
pub const DEFAULT_MCP_PORT: u16 = 4848;

/// Configuration for [`crate::run`].
///
/// Unlike the fdemon-pro server this crate was ported from, the bind address
/// is **not** configurable — it is always `127.0.0.1`, mirroring
/// `frust-devtools`'s loopback-only stance (`docs/DEVTOOLS_ARCHITECTURE.md`).
/// The engine-control surface this crate exposes never listens on a
/// non-loopback interface, full stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConfig {
    /// TCP port to listen on. `0` lets the OS pick an ephemeral port.
    pub port: u16,
    /// The frust project root the server's tools operate against.
    ///
    /// Unused today (no tool reads it yet) — carried on the config now so
    /// the engine wiring (a later task) has a stable seam rather than a
    /// breaking signature change.
    pub project_root: PathBuf,
}

impl McpConfig {
    /// A config for `project_root`, defaulting `port` to [`DEFAULT_MCP_PORT`].
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        Self {
            port: DEFAULT_MCP_PORT,
            project_root: project_root.into(),
        }
    }

    /// Overrides the port (e.g. `0` for an OS-assigned port in tests).
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_defaults_to_the_standard_port() {
        let config = McpConfig::new("/tmp/frust-mcp-test-project");
        assert_eq!(config.port, DEFAULT_MCP_PORT);
        assert_eq!(
            config.project_root,
            PathBuf::from("/tmp/frust-mcp-test-project")
        );
    }

    #[test]
    fn with_port_overrides_the_default() {
        let config = McpConfig::new("/tmp/frust-mcp-test-project").with_port(0);
        assert_eq!(config.port, 0);
    }
}
