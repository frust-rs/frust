//! Async session supervision.
//!
//! The layer between the TEA engine and `frust-drive`'s pipelines: it wraps
//! the drive's cancellable `spawn_streaming` seam, bridges each session's
//! stdout lines and inferred lifecycle-state changes into one tokio mpsc the
//! engine `select!`s on, and owns the kill/cancel path.
//!
//! - [`session`] is the pure value vocabulary — [`SessionId`], [`SessionSpec`]
//!   (project × device × mode), the [`SessionState`] machine, [`SessionEvent`],
//!   and the [`LaunchPlan`] a spec resolves into.
//! - [`progress`] is a second pure vocabulary layer — [`PhaseLabel`] and its
//!   extraction from streamed output (`phase_from_output_line`), the
//!   transient build/install/launch status line's source of truth.
//! - [`supervisor`] is the moving part — [`Supervisor`] owns a supervision
//!   thread per session and the single event channel.
//! - [`devtools_bridge`] is the second moving part — [`DevtoolsBridge`] owns
//!   one devtools connection thread per session (workbook §B12's DevTools
//!   mode), reporting into the engine's own message channel.
//! - [`mcp_backend`] is the seam an **embedded MCP server** drives — a
//!   [`TuiSessionBackend`] answering `frust-mcp`'s `SessionBackend` over
//!   these same sessions, so an agent and the user share one session world
//!   rather than each running the app once.
//! - [`session_feeds`] holds that seam's two *deferred-answer* registries —
//!   the open [`SessionSubscribers`] feeds a DAP client's output/exit pumps
//!   read, and the [`PendingWidgetTrees`] a `widget_tree` pull is answered
//!   from once the devtools bridge reports back.
//! - [`dap_server`] is the embedded **DAP** server's handle/status pair
//!   ([`DapServerHandle`]/[`DapStatus`]), the exact counterpart of
//!   `mcp_backend`'s [`McpServerHandle`]/[`McpStatus`].
//! - [`metrics_bridge`] is the third moving part — [`MetricsBridge`] owns
//!   one System/Network metrics-sampling thread per session (workbook
//!   §B12's System/Network tabs), a sibling to `devtools_bridge` rather than
//!   part of it — see that module's doc for why.
//!
//! The desktop `cargo run` path and the multi-phase device pipeline (build →
//! install → launch → logcat, via `frust-drive`'s `android_run`/`ios_run`
//! cancellable seams) both feed the same channel; [`Supervisor::start`]
//! dispatches on the [`SessionSpec`]'s target. The module is unit-tested
//! against a scripted `FakeProcessRunner`.
//!
//! # Bridge teardown
//!
//! Both bridges hand their stopped threads back as a [`Teardown`] instead of
//! joining inline, so the *waiting* happens wherever the caller can afford it
//! (`crate::runner` hands it to `tokio::task::spawn_blocking`) and the event
//! loop is never the thing that blocks. The wait itself is bounded and
//! detaches on expiry — see [`Teardown`] for why an unbounded join here is a
//! whole-TUI freeze.

mod dap_server;
mod devtools_bridge;
pub mod mcp_backend;
mod metrics_bridge;
mod progress;
mod session;
pub mod session_feeds;
mod supervisor;

use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub use dap_server::{DapServerHandle, DapStatus};
pub use devtools_bridge::DevtoolsBridge;
pub use mcp_backend::{
    EmbeddedError, McpCommand, McpServeCtx, McpServerHandle, McpSessionRecords, McpStatus,
    TreeRefusal, TuiSessionBackend, mcp_session_state, serve_command,
};
pub use metrics_bridge::MetricsBridge;
pub use progress::{PhaseLabel, phase_from_output_line};
pub use session::{
    DevicePlan, DeviceTarget, LaunchError, LaunchPlan, SessionEvent, SessionEventKind, SessionId,
    SessionSpec, SessionState,
};
pub use session_feeds::{PendingWidgetTrees, SessionCursor, SessionSubscribers};
pub use supervisor::Supervisor;

/// How long a [`Teardown`] waits for its signalled thread before detaching it.
///
/// Generous against every step a bridge thread can be inside when it is
/// signalled and that this side actually bounds (`devtools_bridge`'s
/// coalescing window, `metrics_bridge`'s poll slice — both tens of
/// milliseconds), and short enough that the one place the wait is taken
/// inline — quit, see [`Teardown::wait_until`] — is not felt as a hang.
const TEARDOWN_DEADLINE: Duration = Duration::from_millis(500);

/// Spawn a bridge thread whose completion can be waited on with a deadline.
///
/// The returned receiver's paired `Sender` is moved into the thread and never
/// sent on: the channel *disconnecting* is the thread's "I have returned"
/// signal, which lets [`Teardown`] wait with one blocking receive instead of
/// polling [`JoinHandle::is_finished`] — and, unlike a bare `join`, with a
/// bound.
fn spawn_tracked(
    name: String,
    body: impl FnOnce() + Send + 'static,
) -> std::io::Result<(JoinHandle<()>, mpsc::Receiver<()>)> {
    let (done_tx, done) = mpsc::channel::<()>();
    let worker = thread::Builder::new().name(name).spawn(move || {
        // Held only so it drops (and so disconnects `done`) when the thread
        // returns, panic or not.
        let _done_tx = done_tx;
        body();
    })?;
    Ok((worker, done))
}

/// A bridge thread that has been told to stop but has not necessarily
/// finished yet, handed to the caller so the waiting happens off the event
/// loop.
///
/// # Why the wait is bounded, and why it detaches
///
/// A bridge thread can be parked in a call this side cannot bound: a metrics
/// sampler's `adb shell` probe (`frust_drive::process`'s `run` has no
/// wall-clock timeout) or a devtools round trip against a device that stopped
/// answering. Joining such a thread from `crate::runner`'s `select!` loop
/// freezes the *whole* workbench — no repaint, no input, no quit — and a
/// session's terminal transition tears both bridges down, so an ordinary
/// Android run is enough to reach it. So the wait is capped at
/// [`TEARDOWN_DEADLINE`] and the thread is **detached** past it: it exits on
/// its own once the call it is stuck in finally returns.
///
/// The residual is a leaked thread per wedged teardown, bounded by the number
/// of sessions torn down while a device is unresponsive, and it costs nothing
/// but its stack — each bridge mutes or abandons the detached thread's
/// reporting path before handing the [`Teardown`] over, so a straggler can
/// neither be joined later nor race a stale report into the engine.
#[must_use = "a Teardown is the bounded wait for a stopped bridge thread; dropping it takes that wait inline"]
pub struct Teardown {
    /// Names the thread in the detach report.
    label: String,
    /// Disconnects when the thread returns — see [`spawn_tracked`].
    done: mpsc::Receiver<()>,
    /// Taken by the first wait, so [`Teardown::wait`] and the [`Drop`]
    /// backstop below can't both wait for the same thread.
    worker: Option<JoinHandle<()>>,
}

impl Teardown {
    /// Wrap an already-signalled thread. Callers signal *first* (and stop
    /// listening to the thread), then hand the thread here: constructing a
    /// `Teardown` is not itself a stop request.
    fn new(label: String, done: mpsc::Receiver<()>, worker: JoinHandle<()>) -> Self {
        Self {
            label,
            done,
            worker: Some(worker),
        }
    }

    /// Wait the thread out against a fresh [`TEARDOWN_DEADLINE`], detaching it
    /// if it doesn't finish in time.
    pub fn wait(mut self) {
        self.wait_until(Instant::now() + TEARDOWN_DEADLINE);
    }

    /// [`Self::wait`] against a caller-owned deadline — how a bridge tears
    /// every thread down at once on quit: they were all signalled up front and
    /// wind down in parallel, so the quit path waits *once*, not once per
    /// session.
    fn wait_until(&mut self, deadline: Instant) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        match self.done.recv_timeout(remaining) {
            // The thread dropped its sender on the way out, so this join
            // returns immediately.
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = worker.join();
            }
            // Nothing is ever *sent* on this channel, so `Ok` is unreachable
            // in practice and treated as the timeout it would follow: the
            // thread is stuck somewhere this side cannot bound. Detach it.
            Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {
                eprintln!(
                    "frust-tui: {} did not stop before its teardown deadline; \
                     leaving it detached (it exits when its blocked call returns)",
                    self.label
                );
                drop(worker);
            }
        }
    }
}

impl Drop for Teardown {
    fn drop(&mut self) {
        // The backstop for a caller that never placed the wait: still bounded,
        // still detaching — a no-op once `wait`/`wait_until` has run.
        self.wait_until(Instant::now() + TEARDOWN_DEADLINE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex};

    /// A thread parked exactly the way a wedged `adb` probe parks one: it
    /// ignores every stop signal until the test releases it.
    fn parked_thread(release: Arc<(Mutex<bool>, Condvar)>) -> Teardown {
        let (worker, done) = spawn_tracked("teardown-test-parked".to_string(), move || {
            let (lock, ready) = &*release;
            let mut released = lock.lock().unwrap_or_else(|p| p.into_inner());
            while !*released {
                released = ready.wait(released).unwrap_or_else(|p| p.into_inner());
            }
        })
        .expect("spawn the parked test thread");
        Teardown::new("the parked test thread".to_string(), done, worker)
    }

    fn release(gate: &Arc<(Mutex<bool>, Condvar)>) {
        let (lock, ready) = &**gate;
        *lock.lock().unwrap_or_else(|p| p.into_inner()) = true;
        ready.notify_all();
    }

    #[test]
    fn waiting_on_a_finished_thread_returns_at_once() {
        let done_flag = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&done_flag);
        let (worker, done) = spawn_tracked("teardown-test-quick".to_string(), move || {
            flag.store(true, Ordering::SeqCst);
        })
        .expect("spawn the quick test thread");
        let teardown = Teardown::new("the quick test thread".to_string(), done, worker);

        let started = Instant::now();
        teardown.wait();
        assert!(
            started.elapsed() < TEARDOWN_DEADLINE,
            "a thread that finishes is joined, never waited out: took {:?}",
            started.elapsed()
        );
        assert!(
            done_flag.load(Ordering::SeqCst),
            "the body ran to completion"
        );
    }

    #[test]
    fn a_wedged_thread_is_detached_at_the_deadline_rather_than_joined_forever() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let teardown = parked_thread(Arc::clone(&gate));

        let started = Instant::now();
        teardown.wait();
        let waited = started.elapsed();
        assert!(
            waited >= TEARDOWN_DEADLINE,
            "the wait gives the thread its full deadline: took {waited:?}"
        );
        assert!(
            waited < TEARDOWN_DEADLINE * 4,
            "the wait is bounded by the deadline, not by the thread: took {waited:?}"
        );

        // Let the (detached) thread go, so the test binary leaves nothing
        // parked behind it.
        release(&gate);
    }
}
