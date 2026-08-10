//! The seam the MCP tool layer drives.
//!
//! [`SessionBackend`] is everything `tools/` needs from whatever is
//! supervising app sessions: eleven methods, no more. [`crate::SessionEngine`]
//! is this crate's own implementation (and the one the server wires up), but
//! the tools never name it — they hold an `Arc<dyn SessionBackend>`, so an
//! embedder that already owns running sessions can hand its own supervisor in
//! instead.
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

use std::sync::Arc;

use anyhow::Result;
use frust_devtools_protocol::FrameStats;
use frust_drive::build_info::BuildMode;
use frust_drive::devices::Device;
use frust_drive::devtools_client::DevtoolsClient;
use frust_drive::process::ProcessRunner;

use crate::engine::{LatestMetrics, RunTarget, SessionId, SessionSnapshot};

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

    /// The shared process runner, for a tool needing its own one-shot
    /// invocation (`screenshot`'s `adb exec-out screencap` fallback) rather
    /// than reaching for `std::process::Command` (`docs/CODE_STANDARDS.md`'s
    /// `ProcessRunner` anti-pattern). Every call through it blocks.
    fn runner(&self) -> Arc<dyn ProcessRunner + Send + Sync>;
}
