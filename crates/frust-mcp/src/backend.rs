//! The seam the MCP tool layer drives.
//!
//! [`SessionBackend`] is everything a consumer needs from whatever is
//! supervising app sessions: fourteen methods, no more — the eleven `tools/`
//! drives, plus three an orchestration consumer (`frust-dap`) needs and the
//! tool layer does not. [`crate::SessionEngine`] is this crate's own
//! implementation (and the one the server wires up), but the tools never name
//! it — they hold an `Arc<dyn SessionBackend>`, so an embedder that already
//! owns running sessions can hand its own supervisor in instead.
//!
//! The last three carry **default implementations that refuse**, so a backend
//! predating them still compiles and still answers honestly: no event feed, a
//! typed "does not serve widget trees" error rather than an empty tree, and no
//! project root rather than a guessed one.
//!
//! # Why the trait is sync
//!
//! Every method here is blocking-or-cheap, never `async`. Two reasons, and
//! they point the same way:
//!
//! - An `async` trait method needs either `async-trait` (a dependency this
//!   crate's charter does not carry — see `docs/CLI_ARCHITECTURE.md`) or a
//!   hand-boxed `Pin<Box<dyn Future>>` return per method. Neither buys
//!   anything: the work behind these calls is `frust-drive`'s, which is sync
//!   and tokio-free by charter.
//! - The tool layer is where the async/blocking bridge belongs anyway. It runs
//!   inside a tokio runtime and already wraps every unbounded call in
//!   [`tokio::task::spawn_blocking`]; a backend that returned futures would
//!   just move that decision somewhere it cannot see the runtime.
//!
//! Each method below says which of the two kinds it is. **Blocking** ones have
//! no wall-clock bound (an `adb` call against a wedged device) and must be
//! issued from `spawn_blocking`; the rest take a short lock and are safe to
//! call directly from async code.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use frust_devtools_protocol::{FrameStats, Method, RpcError};
use frust_drive::build_info::BuildMode;
use frust_drive::devices::Device;
use frust_drive::devtools_client::{DevtoolsClient, DevtoolsRpcError};
use frust_drive::process::ProcessRunner;

use crate::engine::{LatestMetrics, RunTarget, SessionEventFeed, SessionId, SessionSnapshot};

/// A shared backend handle: what the handler holds and every tool call
/// borrows, cloned into a [`tokio::task::spawn_blocking`] closure where a
/// blocking method is called.
pub type SharedBackend = Arc<dyn SessionBackend>;

/// What the MCP tools need from a session supervisor.
///
/// Object-safe on purpose — the whole point is `Arc<dyn SessionBackend>`.
/// `Send + Sync` because one backend is shared by every MCP session the
/// transport creates, concurrently.
pub trait SessionBackend: Send + Sync {
    /// **Blocking.** Every discoverable device, plus the non-fatal notes
    /// discovery produced (a missing SDK, an unauthorized device).
    fn list_devices(&self) -> (Vec<Device>, Vec<String>);

    /// A snapshot of every session, oldest id first.
    fn sessions(&self) -> Vec<SessionSnapshot>;

    /// One session's snapshot, or `None` for an unknown id.
    fn session(&self, id: SessionId) -> Option<SessionSnapshot>;

    /// Launches `target` in `mode` and returns its session id
    /// **immediately** — the build/install/launch chain runs elsewhere. Never
    /// fails: a launch that cannot even spawn lands as a failed session.
    fn run_app(&self, target: RunTarget, mode: BuildMode) -> SessionId;

    /// **Blocking.** Stops the session and releases everything it held.
    /// `Err` only for an unknown id.
    ///
    /// "Blocking" describes how long the *call* may take, not what has
    /// finished when it returns: an implementer may treat a stop as a
    /// **request** — the session's terminal state, and the best-effort
    /// OS-level app termination that goes with it (`am force-stop`,
    /// `simctl terminate`), may still be in flight. Returning does not
    /// certify the app is already gone from the device.
    fn stop_app(&self, id: SessionId) -> Result<()>;

    /// **Blocking.** Stops the session and relaunches the same target and
    /// mode as a new one, returning the new id. The old id keeps reporting
    /// its final state. The stop half carries
    /// [`stop_app`](Self::stop_app)'s request semantics.
    fn restart_app(&self, id: SessionId) -> Result<SessionId>;

    /// The session's retained log lines: the most recent `tail`, or every
    /// retained line when `tail` is `None`. `None` for an unknown id.
    fn logs(&self, id: SessionId, tail: Option<usize>) -> Option<Vec<String>>;

    /// The session's retained frame-stats samples, oldest first.
    fn frame_ring(&self, id: SessionId) -> Option<Vec<FrameStats>>;

    /// The latest sample of each system-metrics kind — all-empty for a target
    /// that cannot sample.
    fn latest_metrics(&self, id: SessionId) -> Option<LatestMetrics>;

    /// The session's connected devtools client, for a tool that needs to
    /// issue a request. Resolving it is cheap; every call *on* it blocks.
    fn devtools_client(&self, id: SessionId) -> Option<Arc<DevtoolsClient>>;

    /// A push feed of the session's log lines and its end — what a consumer
    /// that must not miss output *or* the app's exit reads instead of
    /// re-polling [`logs`](Self::logs) and diffing (`frust-dap`'s pumps).
    ///
    /// Sync and blocking-recv on purpose: the exit is the one thing this
    /// trait could otherwise only report through an `async` `wait_for`, which
    /// the sync charter above rules out. See
    /// [`SessionEventFeed`] for the delivery contract (seeded then live,
    /// bounded, in-band loss markers, terminal `Exited` then close).
    ///
    /// **Defaults to `None`** — the same answer an unknown id gets, and what a
    /// backend that has no such feed to give should keep returning.
    fn subscribe_session_events(&self, _id: SessionId) -> Option<SessionEventFeed> {
        None
    }

    /// **Blocking.** One widget-tree dump for the session, as the raw JSON of
    /// `frust_devtools_protocol::WidgetTreeDump` — for a consumer that cannot
    /// reach [`devtools_client`](Self::devtools_client) because the backend
    /// (not the session) owns the connection.
    ///
    /// **Defaults to a refusal**, typed as the devtools layer types its own:
    /// a `frust_drive::devtools_client::DevtoolsRpcError` carrying
    /// `RpcError::NOT_SUPPORTED`, so the caller's existing
    /// `is_not_supported` check classifies it exactly like an app that
    /// declared no `widget_tree` capability. Nothing here invents a second
    /// error vocabulary for "no".
    fn fetch_widget_tree(&self, id: SessionId) -> Result<serde_json::Value> {
        Err(anyhow::Error::new(DevtoolsRpcError {
            method: Method::WidgetTree.to_string(),
            code: RpcError::NOT_SUPPORTED,
            message: format!(
                "this backend does not serve widget trees (session {id}); read the tree \
                 through its own devtools connection instead"
            ),
        }))
    }

    /// The directory a [`run_app`](Self::run_app) issued **now** would build
    /// from — the backend's own project, answered at the moment it is asked.
    ///
    /// Read, never cached: a backend whose project can change under it (a
    /// workbench whose user switches projects) answers with the one the next
    /// launch will actually use, so a consumer that *displays* or *compares*
    /// the root — `frust-dap`'s launch banner and its ignored-`projectRoot`
    /// note — never names a directory nothing builds from. `None` means there
    /// is no project to build in at all, which a consumer must treat as a
    /// refusal rather than substituting a root of its own.
    ///
    /// Cheap in this crate's own engine (a field read). An embedder answering
    /// from its own event loop takes a short, bounded round trip instead — the
    /// same shape as [`session`](Self::session) — so a caller inside a runtime
    /// should issue it the way it issues every other backend call.
    ///
    /// **Defaults to `None`**: a backend that never told anyone where it
    /// builds says so, rather than having a root inferred for it.
    fn project_root(&self) -> Option<PathBuf> {
        None
    }

    /// The shared process runner, for a tool needing its own one-shot
    /// invocation (`screenshot`'s `adb exec-out screencap` fallback) rather
    /// than reaching for `std::process::Command` (`docs/CODE_STANDARDS.md`'s
    /// `ProcessRunner` anti-pattern). Every call through it blocks.
    fn runner(&self) -> Arc<dyn ProcessRunner + Send + Sync>;
}

#[cfg(test)]
mod tests {
    use frust_drive::process::RealProcessRunner;

    use super::*;

    /// A backend implementing only the eleven required methods — the shape of
    /// an embedder's own supervisor written before the last three existed. It
    /// is here to pin that such a backend still *compiles*, and that the three
    /// defaults refuse honestly rather than answering with an empty feed, an
    /// empty tree, or a guessed project root.
    struct MinimalBackend;

    impl SessionBackend for MinimalBackend {
        fn list_devices(&self) -> (Vec<Device>, Vec<String>) {
            (Vec::new(), Vec::new())
        }

        fn sessions(&self) -> Vec<SessionSnapshot> {
            Vec::new()
        }

        fn session(&self, _id: SessionId) -> Option<SessionSnapshot> {
            None
        }

        fn run_app(&self, _target: RunTarget, _mode: BuildMode) -> SessionId {
            SessionId(1)
        }

        fn stop_app(&self, _id: SessionId) -> Result<()> {
            Ok(())
        }

        fn restart_app(&self, _id: SessionId) -> Result<SessionId> {
            Ok(SessionId(1))
        }

        fn logs(&self, _id: SessionId, _tail: Option<usize>) -> Option<Vec<String>> {
            None
        }

        fn frame_ring(&self, _id: SessionId) -> Option<Vec<FrameStats>> {
            None
        }

        fn latest_metrics(&self, _id: SessionId) -> Option<LatestMetrics> {
            None
        }

        fn devtools_client(&self, _id: SessionId) -> Option<Arc<DevtoolsClient>> {
            None
        }

        fn runner(&self) -> Arc<dyn ProcessRunner + Send + Sync> {
            Arc::new(RealProcessRunner)
        }
    }

    #[test]
    fn a_backend_that_implements_no_default_refuses_all_three() {
        let backend: SharedBackend = Arc::new(MinimalBackend);

        assert!(backend.subscribe_session_events(SessionId(1)).is_none());
        assert!(
            backend.project_root().is_none(),
            "a backend that names no project must not have one inferred for it"
        );

        let err = backend
            .fetch_widget_tree(SessionId(1))
            .expect_err("the default refuses");
        assert!(
            frust_drive::devtools_client::is_not_supported(&err),
            "the refusal must classify as not-supported: {err:#}"
        );
        assert!(
            format!("{err:#}").contains("does not serve widget trees"),
            "unhelpful: {err:#}"
        );
    }
}
