//! rmcp `ServerHandler` implementation.
//!
//! The streamable-HTTP transport's service factory (`server.rs`) constructs
//! one [`McpHandler`] per MCP session, each holding a clone of the one
//! process-wide [`SharedBackend`] — MCP sessions are connections, not app
//! sessions, and every one of them drives the same supervised apps. The
//! backend is held as `Arc<dyn SessionBackend>`, never as a concrete engine:
//! nothing from here down names what is actually supervising the apps.
//!
//! Dispatch here is deliberately thin: each `#[tool]` body parses nothing,
//! decides nothing, and forwards straight into [`crate::tools`], where the
//! logic lives and is unit-tested without the rmcp stack. What this module
//! *does* own is the agent-facing contract — every tool's name, argument
//! schema (derived from its `Parameters` type), and description. A
//! description here is the only documentation an agent gets, so each one
//! states what the tool does, what it defaults to, and what it costs.

use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::schemars::{self, JsonSchema};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use serde::Serialize;

use crate::SharedBackend;
use crate::clients::ClientGuard;
use crate::tools::{ToolError, ToolResult, diagnosis, driving, session};

const SERVER_INSTRUCTIONS: &str = "Control and diagnosis for frust apps, over the MCP \
     Streamable HTTP transport on 127.0.0.1 only. Start with list_devices and run_app (target \
     \"desktop\" or a device id), then poll list_sessions until a session reports \
     devtools_connected — every inspection and input tool needs that connection. From there: \
     widget_tree and find_widgets to see the UI, tap/scroll/enter_text to drive it, \
     widget_props for one widget's details, performance and metrics for frame timings and \
     resource use, screenshot for a picture, app_logs for the app's own output. Tools that \
     take session_id default to the only running session; when several are running they say \
     so and list the ids. Failures come back as tool results with an `error` message written \
     to be acted on (an ambiguous widget query lists its candidates), not as protocol errors.";

/// Structured result of the `ping` tool.
#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct PingResult {
    /// Always `true` — `ping` fails the whole tool call (an MCP error
    /// response) rather than ever returning `false`.
    ok: bool,
    /// How many app sessions the server is supervising right now.
    sessions: usize,
}

/// MCP server handler: the tool router plus the shared backend every tool
/// acts through.
#[derive(Clone)]
pub(crate) struct McpHandler {
    backend: SharedBackend,
    tool_router: ToolRouter<Self>,
    /// Per-session client registration; `None` until the service factory
    /// calls [`McpHandler::with_client_guard`], `Some` for every handler
    /// produced for a live MCP session. `Arc`-wrapped so cloning the
    /// handler (rmcp does this per request, not per session) never
    /// double-registers or double-drops the one guard a session owns.
    client_guard: Option<Arc<ClientGuard>>,
}

#[tool_router]
impl McpHandler {
    pub(crate) fn new(backend: SharedBackend) -> Self {
        Self {
            backend,
            tool_router: Self::tool_router(),
            client_guard: None,
        }
    }

    /// Attaches the per-session client registration — called once, by the
    /// service factory, right after `new` (`server.rs`'s `serve_embedded`).
    pub(crate) fn with_client_guard(mut self, guard: ClientGuard) -> Self {
        self.client_guard = Some(Arc::new(guard));
        self
    }

    #[tool(
        description = "Health check: returns { ok: true, sessions: <count> } if frust-mcp's tool router is reachable and working. Needs no running app."
    )]
    async fn ping(&self) -> Json<PingResult> {
        Json(PingResult {
            ok: true,
            sessions: self.backend.sessions().len(),
        })
    }

    // ── Session family ──────────────────────────────────────────────────

    #[tool(
        description = "List the devices run_app can target (Android devices/emulators and iOS Simulators), plus any non-fatal discovery notes such as a missing SDK or an unauthorized device. \"desktop\" is always a valid target and is not listed. Shells out to adb/xcrun, so it takes a moment."
    )]
    async fn list_devices(&self) -> ToolResult<session::ListDevicesResult> {
        session::list_devices(&self.backend).await
    }

    #[tool(
        description = "List every app session this server has launched, running or finished: its target, build mode, lifecycle state, devtools connection and declared capabilities, log/frame counts, and (for a failed launch) the error. Poll this after run_app until state is devtools_connected."
    )]
    async fn list_sessions(&self) -> ToolResult<session::ListSessionsResult> {
        session::list_sessions(&self.backend)
    }

    #[tool(
        description = "Launch a frust app and supervise it. `target` is \"desktop\" for a host preview or a device id from list_devices; `mode` is \"debug\" (default) or \"profile\". Every session runs at the project root this server was started in — there is no per-call override. Returns immediately with the new session id — building, installing, and connecting happen in the background, so poll list_sessions (and read app_logs) until the session reports devtools_connected or failed."
    )]
    async fn run_app(
        &self,
        Parameters(args): Parameters<session::RunAppArgs>,
    ) -> ToolResult<session::RunAppResult> {
        session::run_app(&self.backend, args).await
    }

    #[tool(
        description = "Stop a running app session: kill the process, drop its devtools connection, and remove any adb port forward. Defaults to the only running session; pass session_id when several are running. The session id stays valid and keeps reporting its final state."
    )]
    async fn stop_app(
        &self,
        Parameters(args): Parameters<session::SessionArgs>,
    ) -> ToolResult<session::StopAppResult> {
        session::stop_app(&self.backend, args).await
    }

    #[tool(
        description = "Stop a session and relaunch the same target, mode, and project root as a NEW session (frust has no hot reload — this is a full restart, so it rebuilds). Returns both the stopped session and the new one; the new id is what every later tool call should use."
    )]
    async fn restart_app(
        &self,
        Parameters(args): Parameters<session::SessionArgs>,
    ) -> ToolResult<session::RestartAppResult> {
        session::restart_app(&self.backend, args).await
    }

    #[tool(
        description = "Read a session's captured output (build output, app logs, logcat). Returns the most recent `limit` lines (default 100) that match the optional `pattern` (case-insensitive substring, not a regex) and `level` (error/warn/info/debug/trace, matched textually against the line). Also reports how many lines matched in total and how many were evicted for ring capacity."
    )]
    async fn app_logs(
        &self,
        Parameters(args): Parameters<session::AppLogsArgs>,
    ) -> ToolResult<session::AppLogsResult> {
        session::app_logs(&self.backend, args)
    }

    // ── Driving family ──────────────────────────────────────────────────

    #[tool(
        description = "Find widgets on screen by type name and/or debug label (case-insensitive substrings; both filters must match). Fetches the widget tree ONCE and filters here, returning each match's id, type, label, logical-px bounds, and center — the center is directly tappable. Reports the total match count so you can tell a truncated list from a complete one."
    )]
    async fn find_widgets(
        &self,
        Parameters(args): Parameters<driving::FindWidgetsArgs>,
    ) -> ToolResult<driving::FindWidgetsResult> {
        driving::find_widgets(&self.backend, args).await
    }

    #[tool(
        description = "Tap the app, either at logical-px coordinates (x and y together) or at the center of the one widget a query (type_name and/or label) resolves to — one or the other, not both. A query that matches several visible widgets fails with the candidates listed so you can narrow it; a query matching only widgets with no on-screen rect fails too, rather than tapping nothing. The tap goes through the app's real input path, so it hit-tests and routes exactly like a user's."
    )]
    async fn tap(
        &self,
        Parameters(args): Parameters<driving::TapArgs>,
    ) -> ToolResult<driving::TapResult> {
        driving::tap(&self.backend, args).await
    }

    #[tool(
        description = "Scroll the app: a (dx, dy) logical-px delta applied at logical-px (x, y), routed through the app's real input path so it lands on whatever scrollable is under that point. Use find_widgets to get a point inside the list you mean."
    )]
    async fn scroll(
        &self,
        Parameters(args): Parameters<driving::ScrollArgs>,
    ) -> ToolResult<driving::ScrollResult> {
        driving::scroll(&self.backend, args).await
    }

    #[tool(
        description = "Type text into the app. It goes to whatever currently holds FOCUS — there is no target argument — so tap the field you mean first, then call this."
    )]
    async fn enter_text(
        &self,
        Parameters(args): Parameters<driving::EnterTextArgs>,
    ) -> ToolResult<driving::EnterTextResult> {
        driving::enter_text(&self.backend, args).await
    }

    #[tool(
        description = "Read one widget's debug properties by id (ids come from find_widgets or widget_tree). Use this to check a widget's state — text, enabled, checked — after driving the app."
    )]
    async fn widget_props(
        &self,
        Parameters(args): Parameters<driving::WidgetPropsArgs>,
    ) -> ToolResult<driving::WidgetPropsResult> {
        driving::widget_props(&self.backend, args).await
    }

    // ── Diagnosis family ────────────────────────────────────────────────

    #[tool(
        description = "Dump the app's retained widget tree as nested nodes (id, type, debug label, logical-px bounds), capped at `depth` levels below each root (default 12, max 50). Elided subtrees are marked with children_truncated so you can see what was left out and re-query deeper. Prefer find_widgets when you are looking for one specific widget."
    )]
    async fn widget_tree(
        &self,
        Parameters(args): Parameters<diagnosis::WidgetTreeArgs>,
    ) -> ToolResult<diagnosis::WidgetTreeResult> {
        diagnosis::widget_tree(&self.backend, args).await
    }

    #[tool(
        description = "Aggregate the frame-timing samples collected from the running app: fps estimate, frame-time mean/p50/p95/p99/max, jank count and percentage (frames over 16.7ms), per-phase means (rebuild, layout, paint, encode, acquire, submit), and the sample window. A session that has not rendered yet reports no frames rather than zeros. Reads only what is already buffered — no round trip to the app."
    )]
    async fn performance(
        &self,
        Parameters(args): Parameters<session::SessionArgs>,
    ) -> ToolResult<diagnosis::PerformanceResult> {
        diagnosis::performance(&self.backend, args)
    }

    #[tool(
        description = "Report the app's resource use: sampled system metrics (CPU, RSS, network counters, thermal zones) plus the app's own RSS and uptime from its devtools service. System sampling is Android-only — on desktop and the iOS Simulator this server cannot see the app's pid, and says so explicitly instead of reporting zeros. Network counters are namespace/device-wide, never per-process."
    )]
    async fn metrics(
        &self,
        Parameters(args): Parameters<session::SessionArgs>,
    ) -> ToolResult<diagnosis::MetricsResult> {
        diagnosis::metrics(&self.backend, args).await
    }

    #[tool(
        description = "Capture the app's screen as a PNG, returned both as an image and as base64 in the structured result. Uses the app's own devtools screenshot when it declared that capability; otherwise falls back to `adb screencap` for an Android session (which captures the whole device screen, not just the app — the result says which source was used). Not supported for desktop or iOS Simulator sessions without the capability."
    )]
    async fn screenshot(
        &self,
        Parameters(args): Parameters<session::SessionArgs>,
    ) -> Result<CallToolResult, Json<ToolError>> {
        diagnosis::screenshot(&self.backend, args).await
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
