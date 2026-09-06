//! [`DevtoolsBackend`] — the seam between the wire service and whatever holds
//! the live app state (a shell, in production; a fake, in tests).
//!
//! The trait is **plain synchronous Rust**: no `async fn`, no `Future`, no
//! tokio type in any signature. A shell implements it the way it would
//! implement any other trait, and the service does the async work behind it
//! (`crate::hop`). See the crate doc's *Threading & blocking model* for what
//! that costs and guarantees.

use frust_devtools_protocol::{
    Capability, HandshakeInfo, InputScrollParams, InputTapParams, MetricsSnapshot,
    PROTOCOL_VERSION, RpcError, ScreenshotResult, WidgetProps, WidgetTreeDump,
};

/// The app identity a service announces at handshake, supplied by whoever
/// starts the service (the shell knows its own app name and framework
/// version; the backend does not have to).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub app_name: String,
    pub frust_version: String,
}

impl AppInfo {
    pub fn new(app_name: impl Into<String>, frust_version: impl Into<String>) -> Self {
        Self {
            app_name: app_name.into(),
            frust_version: frust_version.into(),
        }
    }
}

/// Why a backend call could not be satisfied. Each variant maps to exactly one
/// JSON-RPC error code (see [`BackendError::to_rpc_error`]), so a backend
/// picks the variant and never has to know the code.
///
/// Hand-rolled `Display`/`Error` rather than `thiserror`-derived (the usual
/// convention in `docs/CODE_STANDARDS.md`'s Error Handling) because this crate
/// keeps a deliberately minimal dependency set — the same trade its sibling
/// `frust-devtools-protocol` makes for `DecodeError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// The backend does not implement this capability at all (the default
    /// `screenshot` answer). Distinct from a transient failure: a client that
    /// reads the handshake capability set should never have asked.
    NotSupported(String),
    /// The capability exists but nothing can serve it right now (no attached
    /// window, no live widget tree yet, app backgrounded).
    Unavailable(String),
    /// The request itself was wrong (an id that names no widget, an
    /// out-of-range coordinate).
    InvalidRequest(String),
    /// Anything else the backend failed at.
    Internal(String),
}

impl BackendError {
    pub fn not_supported(what: impl Into<String>) -> Self {
        BackendError::NotSupported(what.into())
    }

    pub fn unavailable(what: impl Into<String>) -> Self {
        BackendError::Unavailable(what.into())
    }

    pub fn invalid_request(what: impl Into<String>) -> Self {
        BackendError::InvalidRequest(what.into())
    }

    pub fn internal(what: impl Into<String>) -> Self {
        BackendError::Internal(what.into())
    }

    /// The wire error this failure becomes.
    pub fn to_rpc_error(&self) -> RpcError {
        match self {
            BackendError::NotSupported(m) => RpcError::new(RpcError::NOT_SUPPORTED, m.clone()),
            BackendError::Unavailable(m) => RpcError::new(RpcError::INTERNAL_ERROR, m.clone()),
            BackendError::InvalidRequest(m) => RpcError::new(RpcError::INVALID_PARAMS, m.clone()),
            BackendError::Internal(m) => RpcError::new(RpcError::INTERNAL_ERROR, m.clone()),
        }
    }
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::NotSupported(m) => write!(f, "not supported: {m}"),
            BackendError::Unavailable(m) => write!(f, "unavailable: {m}"),
            BackendError::InvalidRequest(m) => write!(f, "invalid request: {m}"),
            BackendError::Internal(m) => write!(f, "internal backend error: {m}"),
        }
    }
}

impl std::error::Error for BackendError {}

/// What the devtools service asks the app for.
///
/// # Threading
///
/// Every method here is called from the service's own **backend thread**, one
/// call at a time, never from the frame/UI thread and never concurrently. An
/// implementation that needs UI-thread state does the hop itself (post to the
/// UI thread and block on the answer) — that is the one thing this trait
/// cannot do for it, because only the shell knows what its UI thread is. The
/// service caps every call with a timeout, so a hop that never comes back
/// costs the caller an error response, never a hung client
/// (`crate::hop::BackendClient`).
///
/// `handshake_info` is the exception: it is called **once**, on the thread that
/// calls [`crate::Service::start`], and the answer is cached for the process
/// lifetime — which is what lets `handshake` be answered statelessly at any
/// time, including while the UI thread is wedged.
pub trait DevtoolsBackend: Send + 'static {
    /// Server identity + the capability set clients gate on. Called once at
    /// startup, on the starting thread (see the trait doc).
    ///
    /// The default answers with everything except
    /// [`Capability::Screenshot`], matching the default [`Self::screenshot`]
    /// below — override this together with `screenshot`, never one alone.
    fn handshake_info(&self, app: &AppInfo) -> HandshakeInfo {
        HandshakeInfo {
            app_name: app.app_name.clone(),
            frust_version: app.frust_version.clone(),
            protocol_version: PROTOCOL_VERSION,
            capabilities: vec![
                Capability::WidgetTree,
                Capability::FrameStats,
                Capability::Input,
                Capability::Metrics,
            ],
        }
    }

    /// A snapshot of the retained widget tree.
    ///
    /// Infallible on purpose: "nothing built yet" is an empty
    /// [`WidgetTreeDump::roots`], not an error — a client polling a
    /// just-launched app should see an empty tree, not a failure it has to
    /// distinguish from a broken backend.
    fn widget_tree(&self) -> WidgetTreeDump;

    /// One widget's inspectable properties, or `None` when `id` names no live
    /// widget (the service turns that into `INVALID_PARAMS` — a stale id from
    /// a tree the app has since rebuilt is an ordinary, expected case, not a
    /// backend failure).
    fn widget_props(&self, id: u64) -> Option<WidgetProps>;

    /// Process-level metrics (uptime, best-effort RSS). Infallible for
    /// [`Self::widget_tree`]'s reason — an unavailable RSS is
    /// [`MetricsSnapshot::rss_bytes`] `None`, not a failed call.
    fn metrics_snapshot(&self) -> MetricsSnapshot;

    /// Synthesize a tap at the given logical-px point.
    fn inject_tap(&self, params: InputTapParams) -> Result<(), BackendError>;

    /// Synthesize a scroll at the given logical-px point.
    fn inject_scroll(&self, params: InputScrollParams) -> Result<(), BackendError>;

    /// Synthesize text input against the focused widget.
    fn inject_text(&self, text: &str) -> Result<(), BackendError>;

    /// Capture the current frame as a PNG.
    ///
    /// Defaults to [`BackendError::NotSupported`]: a screenshot needs a
    /// readback path the backend may not have, so it is capability-gated
    /// rather than mandatory (`RpcError::NOT_SUPPORTED` on the wire). A
    /// backend that overrides this must also declare
    /// [`Capability::Screenshot`] in [`Self::handshake_info`].
    fn screenshot(&self) -> Result<ScreenshotResult, BackendError> {
        Err(BackendError::not_supported(
            "screenshot capture is not implemented by this backend",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_backend_error_maps_to_its_own_rpc_code() {
        assert_eq!(
            BackendError::not_supported("x").to_rpc_error().code,
            RpcError::NOT_SUPPORTED
        );
        assert_eq!(
            BackendError::invalid_request("x").to_rpc_error().code,
            RpcError::INVALID_PARAMS
        );
        assert_eq!(
            BackendError::unavailable("x").to_rpc_error().code,
            RpcError::INTERNAL_ERROR
        );
        assert_eq!(
            BackendError::internal("x").to_rpc_error().code,
            RpcError::INTERNAL_ERROR
        );
    }

    #[test]
    fn rpc_error_message_carries_the_backend_message() {
        let err = BackendError::unavailable("no window attached").to_rpc_error();
        assert_eq!(err.message, "no window attached");
    }

    #[test]
    fn backend_error_is_a_std_error() {
        let err = BackendError::internal("boom");
        let _: &dyn std::error::Error = &err;
        assert_eq!(err.to_string(), "internal backend error: boom");
    }
}
