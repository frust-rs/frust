//! rmcp `ServerHandler` implementation.
//!
//! The streamable-HTTP transport's service factory (`server.rs`) constructs
//! one [`McpHandler`] per MCP session. Today the only tool is `ping`, wired
//! through rmcp's `#[tool_router]`/`#[tool]`/`#[tool_handler]` macros — this
//! is the crate's proof that the macro plumbing (tool listing, dispatch,
//! structured-content results) works end to end. The engine wiring
//! (`frust-drive` session control) and the diagnosis/driving tool families
//! land in later work, behind the same router.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Json;
use rmcp::model::{Implementation, ServerCapabilities, ServerInfo};
use rmcp::schemars::{self, JsonSchema};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use serde::Serialize;

const SERVER_INSTRUCTIONS: &str = "Control and diagnosis for a running frust app, over the MCP \
     Streamable HTTP transport on 127.0.0.1 only. This is an early skeleton: today the only tool \
     is `ping`, a health check proving the server is reachable. Session control (reload/restart/ \
     stop), DevTools diagnosis (widget tree, frame stats, logs), and app-driving tool families \
     (tap/scroll/enter-text) land in later work.";

/// Structured result of the `ping` tool.
#[derive(Debug, Serialize, JsonSchema)]
struct PingResult {
    /// Always `true` — `ping` fails the whole tool call (an MCP error
    /// response) rather than ever returning `false`.
    ok: bool,
}

/// MCP server handler. Stateless today (no fields beyond the tool router)
/// since there is no engine to hold a handle to yet.
#[derive(Debug, Clone)]
pub(crate) struct McpHandler {
    tool_router: ToolRouter<Self>,
}

impl Default for McpHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl McpHandler {
    pub(crate) fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    /// Health check: proves the MCP tool-call plumbing (list, dispatch,
    /// structured-content result) is wired up end to end, independent of
    /// any running frust app.
    #[tool(
        description = "Health check: returns { ok: true } if frust-mcp's tool router is reachable and working."
    )]
    async fn ping(&self) -> Json<PingResult> {
        Json(PingResult { ok: true })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("frust-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(SERVER_INSTRUCTIONS)
    }
}
