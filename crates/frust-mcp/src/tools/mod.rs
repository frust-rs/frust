//! The MCP tool layer over [`crate::backend::SessionBackend`].
//!
//! Three families, one module each: [`session`] (device/session lifecycle and
//! logs), [`driving`] (find/tap/scroll/type against the running app), and
//! [`diagnosis`] (widget tree, frame aggregation, metrics, screenshot).
//! `handler.rs` holds only the `#[tool]` registrations and forwards straight
//! into these functions, so tool *logic* is unit-testable without the rmcp
//! stack in the way.
//!
//! # Two error channels, and which one a failure belongs in
//!
//! A malformed *call* (a missing required argument, a non-numeric `x`) is
//! rmcp's own protocol error — the macro layer rejects it before a tool body
//! runs, and nothing here needs to handle it.
//!
//! Everything else is **in-band**: a [`ToolError`] returned as
//! `Err(Json(..))`, which rmcp renders as a `CallToolResult` carrying the
//! error as structured content with `isError` set. An agent reads it, learns
//! what to do differently, and retries — which is why an ambiguous widget
//! query lists its [`candidates`](ToolError::candidates) and an ambiguous
//! session lists the live [`sessions`](ToolError::sessions) rather than just
//! saying "ambiguous".
//!
//! One consequence is deliberate: a tool declaring an output schema (every
//! `Json<T>`-returning one does) answers a failure with a payload of a
//! *different* shape than that schema. That matches MCP's own split — an
//! `isError` result is not the tool's declared output — and mirrors the
//! fdemon-pro server this layer was ported from.
//!
//! # Blocking calls
//!
//! This layer is where the async/blocking bridge lives. [`SessionBackend`] is
//! a sync trait (see [`crate::backend`]'s rationale), `frust-drive`'s devtools
//! client and process runner are sync too, and all three are unbounded in the
//! worst case (a device that stopped answering). Every call into one goes
//! through [`tokio::task::spawn_blocking`], with the [`SharedBackend`] cloned
//! into the closure — see [`with_devtools`], `session`'s device/lifecycle
//! tools, and `diagnosis`'s adb fallback. The cheap readers (`sessions`,
//! `logs`, `frame_ring`, …) take a short lock and are called directly.

pub(crate) mod diagnosis;
pub(crate) mod driving;
pub(crate) mod session;

use std::sync::Arc;

use frust_devtools_protocol::{RectPx, WidgetNode};
use frust_drive::devtools_client::DevtoolsClient;
use rmcp::handler::server::wrapper::Json;
use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;

use crate::backend::SharedBackend;
use crate::engine::{SessionId, SessionSnapshot, SessionState};

/// What every tool in this layer returns: a typed success payload, or an
/// in-band [`ToolError`] (see the module doc's two-error-channels note).
pub(crate) type ToolResult<T> = Result<Json<T>, Json<ToolError>>;

/// An in-band tool failure: a message written for an agent to act on, plus
/// the disambiguation lists that make "which one did you mean?" answerable
/// without a second exploratory call.
#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct ToolError {
    /// Always `false` — present so a client that only reads structured
    /// content (ignoring `isError`) still cannot mistake this for a result.
    pub success: bool,
    /// What went wrong and what to do about it.
    pub error: String,
    /// The widgets an ambiguous query matched, for the caller to pick from.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<WidgetMatch>,
    /// The sessions an omitted/unknown `session_id` could have meant.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<SessionRef>,
}

impl ToolError {
    pub(crate) fn new(error: impl Into<String>) -> Self {
        Self {
            success: false,
            error: error.into(),
            candidates: Vec::new(),
            sessions: Vec::new(),
        }
    }

    pub(crate) fn with_candidates(mut self, candidates: Vec<WidgetMatch>) -> Self {
        self.candidates = candidates;
        self
    }

    pub(crate) fn with_sessions(mut self, sessions: Vec<SessionRef>) -> Self {
        self.sessions = sessions;
        self
    }
}

/// A compact session reference — what an ambiguity error lists so the caller
/// can pass a concrete `session_id` next time.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct SessionRef {
    pub id: u64,
    /// `desktop`, `android:<serial>`, or `ios-sim:<udid>`.
    pub target: String,
    pub state: &'static str,
}

/// One widget from a `widget_tree` round trip, flattened for an agent:
/// identity, geometry, and the point `tap` would use.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub(crate) struct WidgetMatch {
    /// The retained-tree id `widget_props` takes.
    pub id: u64,
    pub type_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_label: Option<String>,
    /// Logical px, the same coordinate space `tap`/`scroll` take. `null` for
    /// a widget the app reported without bounds (not laid out yet).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<Rect>,
    /// The centre of `bounds` — directly tappable, no conversion needed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub center: Option<Point>,
}

impl WidgetMatch {
    pub(crate) fn from_node(node: &WidgetNode) -> Self {
        Self {
            id: node.id,
            type_name: node.type_name.clone(),
            debug_label: node.debug_label.clone(),
            bounds: node.bounds.map(Rect::from),
            center: node.bounds.map(|bounds| Point {
                x: bounds.x + bounds.width / 2.0,
                y: bounds.y + bounds.height / 2.0,
            }),
        }
    }

    /// Whether this widget occupies a tappable area — a zero-sized (or
    /// bounds-less) node is in the tree but cannot be hit by a synthesized
    /// tap, so targeting it would silently do nothing.
    pub(crate) fn is_tappable(&self) -> bool {
        self.bounds
            .is_some_and(|bounds| bounds.width > 0.0 && bounds.height > 0.0)
    }
}

/// A logical-px rectangle (the serde/schema mirror of the protocol leaf's
/// `RectPx`, which carries no `JsonSchema` derive of its own).
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
pub(crate) struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<RectPx> for Rect {
    fn from(rect: RectPx) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }
}

/// A logical-px point.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
pub(crate) struct Point {
    pub x: f64,
    pub y: f64,
}

/// The short name a [`SessionState`] reports as.
pub(crate) fn state_name(state: &SessionState) -> &'static str {
    match state {
        SessionState::Launching => "launching",
        SessionState::Running => "running",
        SessionState::DevtoolsConnected => "devtools_connected",
        SessionState::Exited { .. } => "exited",
        SessionState::Failed { .. } => "failed",
    }
}

pub(crate) fn session_ref(snapshot: &SessionSnapshot) -> SessionRef {
    SessionRef {
        id: snapshot.id.0,
        target: snapshot.target.label(),
        state: state_name(&snapshot.state),
    }
}

/// Resolves the session a tool acts on.
///
/// An explicit `session_id` always wins (and an unknown one is an error
/// listing what does exist). With none given the rule is "the obvious one":
/// the single still-live session, or — when nothing is live — the single
/// session that ever ran. Anything else is ambiguous and says so, listing the
/// candidates rather than picking for the caller.
///
/// The two ambiguities are reported **differently**, because they need
/// different next steps: several *running* sessions want a `session_id`, while
/// several *ended* ones mean nothing is running at all — an agent told "3
/// sessions are running" about three dead ones would keep driving a corpse.
/// A dead session is never counted as running.
pub(crate) fn resolve_session(
    backend: &SharedBackend,
    requested: Option<u64>,
) -> Result<SessionSnapshot, ToolError> {
    let all = backend.sessions();
    if let Some(id) = requested {
        return backend.session(SessionId(id)).ok_or_else(|| {
            ToolError::new(format!(
                "no such session: {id}. Call list_sessions for the ids that exist, \
                 or run_app to start one."
            ))
            .with_sessions(all.iter().map(session_ref).collect())
        });
    }

    let live: Vec<&SessionSnapshot> = all.iter().filter(|s| !s.state.is_terminal()).collect();
    match live.as_slice() {
        [only] => return Ok((*only).clone()),
        [] => {}
        many => {
            return Err(ToolError::new(format!(
                "{} sessions are running — pass session_id to say which one.",
                many.len()
            ))
            .with_sessions(many.iter().map(|s| session_ref(s)).collect()));
        }
    }

    // Nothing is live: the single session that ever ran is still the obvious
    // one (reading a crashed session's logs is exactly what an agent does
    // next), but several ended ones are ambiguous in their own right.
    match all.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err(ToolError::new(
            "no sessions yet — call run_app first (target \"desktop\", or a device id \
             from list_devices).",
        )),
        ended => Err(ToolError::new(format!(
            "no session is running; {} have ended — pass session_id to act on one of \
             them, or run_app to start a new one.",
            ended.len()
        ))
        .with_sessions(ended.iter().map(session_ref).collect())),
    }
}

/// Why a session has no devtools connection, phrased for an agent.
pub(crate) fn no_devtools_error(snapshot: &SessionSnapshot) -> ToolError {
    let state = state_name(&snapshot.state);
    let detail = match (&snapshot.state, &snapshot.devtools_error) {
        (SessionState::Failed { reason }, _) => format!(" The launch failed: {reason}"),
        (_, Some(reason)) => format!(" The app reported: {reason}"),
        _ => String::new(),
    };
    ToolError::new(format!(
        "session {} is not connected to a devtools service (state: {state}).{detail} \
         Inspection and input need a debug/profile build with the devtools service \
         running; check app_logs for the discovery line.",
        snapshot.id
    ))
}

/// Runs one blocking devtools request off the runtime and maps its failure
/// modes onto agent-readable messages.
///
/// `what` names the operation for the message (`"widget_tree"`, `"tap"`, …).
pub(crate) async fn with_devtools<T, F>(
    backend: &SharedBackend,
    snapshot: &SessionSnapshot,
    what: &'static str,
    call: F,
) -> Result<T, ToolError>
where
    F: FnOnce(&DevtoolsClient) -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    let Some(client) = backend.devtools_client(snapshot.id) else {
        return Err(no_devtools_error(snapshot));
    };
    devtools_call(client, what, call).await
}

/// [`with_devtools`] against an already-resolved client — for a caller that
/// holds one (the screenshot decision chain) and must not re-resolve it.
pub(crate) async fn devtools_call<T, F>(
    client: Arc<DevtoolsClient>,
    what: &'static str,
    call: F,
) -> Result<T, ToolError>
where
    F: FnOnce(&DevtoolsClient) -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(move || call(&client)).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(devtools_error(what, &err)),
        Err(err) => Err(ToolError::new(format!("the {what} task failed: {err}"))),
    }
}

/// Maps a devtools client failure onto an actionable message, keeping the
/// two rejections a caller must treat differently distinguishable.
pub(crate) fn devtools_error(what: &str, err: &anyhow::Error) -> ToolError {
    if frust_drive::devtools_client::is_not_supported(err) {
        return ToolError::new(format!(
            "the app's devtools service does not support {what} — it declared no matching \
             capability at handshake. list_sessions reports the capabilities it did declare."
        ));
    }
    if frust_drive::devtools_client::is_unauthorized(err) {
        return ToolError::new(format!(
            "the app's devtools service rejected {what} as unauthorized — the connection's \
             token is stale. Restart the session (restart_app) to reconnect."
        ));
    }
    ToolError::new(format!("{what} failed: {err:#}"))
}

/// Microseconds to milliseconds, rounded to two decimals — every duration
/// this layer reports goes through here so an agent never has to divide.
pub(crate) fn us_to_ms(micros: u64) -> f64 {
    round2(micros as f64 / 1000.0)
}

pub(crate) fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::SessionEngine;
    use frust_devtools_protocol::RectPx;

    /// The reference backend, behind the trait object every tool sees.
    fn backend() -> SharedBackend {
        Arc::new(SessionEngine::new("/tmp/frust-mcp-tools-test"))
    }

    fn node(id: u64, bounds: Option<RectPx>) -> WidgetNode {
        WidgetNode {
            id,
            type_name: "ButtonWidget".to_string(),
            debug_label: Some("Save".to_string()),
            bounds,
            children: Vec::new(),
        }
    }

    #[test]
    fn a_widget_match_centers_its_bounds() {
        let matched = WidgetMatch::from_node(&node(
            3,
            Some(RectPx {
                x: 10.0,
                y: 20.0,
                width: 100.0,
                height: 40.0,
            }),
        ));
        let center = matched.center.expect("bounds yield a center");
        assert_eq!((center.x, center.y), (60.0, 40.0));
        assert!(matched.is_tappable());
    }

    #[test]
    fn a_boundsless_or_empty_widget_is_not_tappable() {
        assert!(!WidgetMatch::from_node(&node(1, None)).is_tappable());
        assert!(
            !WidgetMatch::from_node(&node(
                2,
                Some(RectPx {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 12.0,
                }),
            ))
            .is_tappable()
        );
    }

    #[test]
    fn microseconds_render_as_rounded_milliseconds() {
        assert_eq!(us_to_ms(16_667), 16.67);
        assert_eq!(us_to_ms(0), 0.0);
    }

    #[test]
    fn resolving_a_session_with_none_running_names_the_next_step() {
        let err = resolve_session(&backend(), None).expect_err("no sessions exist");
        assert!(err.error.contains("run_app"), "unhelpful: {}", err.error);
        assert!(err.sessions.is_empty());
    }

    #[test]
    fn resolving_an_unknown_id_lists_what_exists() {
        let err = resolve_session(&backend(), Some(42)).expect_err("id 42 does not exist");
        assert!(err.error.contains("no such session: 42"));
    }
}
