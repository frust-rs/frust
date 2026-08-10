//! The session engine — the headless analogue of `frust-tui`'s supervise
//! layer, and the one place this crate owns running apps.
//!
//! A [`SessionEngine`] launches app sessions (desktop / Android / iOS
//! Simulator), retains each one's log tail, discovers and connects its
//! in-app devtools service, keeps a frame-stats ring and the latest system
//! metrics, and tears the whole set down. The tool layer sits *on top* of
//! this: it reads [`SessionSnapshot`]s and calls the lifecycle methods,
//! holding one `Arc<SessionEngine>` shared across every MCP session.
//!
//! # Sync library, async server
//!
//! `frust-drive` is deliberately sync and tokio-free
//! (`docs/CLI_ARCHITECTURE.md`). Two rules follow, and neither is optional:
//!
//! - Every drive call made from async context goes through
//!   [`tokio::task::spawn_blocking`] — never directly on a runtime worker.
//!   The blocking calls here are not "usually fast": an `adb` invocation
//!   against an unresponsive device has no wall-clock bound at all.
//! - Per-session background work is plain `std::thread`, with `std::sync`
//!   primitives, matching the drive's own threading idioms rather than
//!   wrapping them in a second async layer.
//!
//! The state readers ([`SessionEngine::sessions`], [`SessionEngine::logs`],
//! …) are the exception: they take a short, non-blocking lock and are safe to
//! call directly from async code.
//!
//! # What the engine cannot know
//!
//! `frust_drive::process::StreamHandle` does not expose a spawned child's
//! pid, so **system metrics are Android-only** — see [`metrics`]'s module doc.
//! For the same reason [`SessionState::Exited`] carries a success flag rather
//! than a real exit code.

mod devtools;
mod launch;
mod metrics;
mod ring;
mod session;

pub use session::{LatestMetrics, RunTarget, SessionId, SessionSnapshot, SessionState};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result};
use frust_devtools_protocol::FrameStats;
use frust_drive::build_info::BuildMode;
use frust_drive::devices::{Device, default_discoverers, discover_all};
use frust_drive::devtools_client::{DevtoolsClient, adb_forward_remove};
use frust_drive::process::{ProcessRunner, RealProcessRunner};

use session::Session;

/// Retained log lines per session, drop-oldest — the same bound and the same
/// reasoning as `frust_drive::process`'s `LINE_BUFFER_CAP`: a `logcat` tail
/// this deep is already far more than an agent reads back, and the cap is
/// what keeps a long-lived session's memory flat.
pub const LOG_RING_CAP: usize = 10_000;

/// Retained frame-stats samples per session, drop-oldest. Ten seconds of
/// 60fps: enough for the performance tool to aggregate a meaningful window
/// on demand, small enough to keep per session for free.
pub const FRAME_RING_CAP: usize = 600;

/// How long teardown waits for a session's threads before detaching them.
///
/// A session thread can be inside an *uninterruptible* drive phase — a Gradle
/// build honors the cancel flag only at phase boundaries — and a stop request
/// from an agent must not park for the rest of that build. Past the deadline
/// the threads are left to wind down on their own; they touch nothing but
/// their own session.
const TEARDOWN_DEADLINE: Duration = Duration::from_secs(5);

/// The shared process runner every session's threads shell out through.
type Runner = Arc<dyn ProcessRunner + Send + Sync>;

/// What a session launched after [`SessionEngine::shutdown`] reports as its
/// failure reason — a refusal an agent can read, rather than a session that
/// silently outlives the server that was supposed to own it.
const SHUTTING_DOWN: &str =
    "the server is shutting down; no new session was started. Restart the server to run an app.";

/// Owns every supervised app session.
///
/// Wrap in an `Arc` and share it — one engine per server process, read and
/// driven concurrently by every MCP session.
///
/// **Drop does not stop sessions.** Dropping the engine detaches, mirroring
/// `StreamHandle`'s own detach-not-kill contract; call
/// [`shutdown`](Self::shutdown) (which [`crate::run`] does on cancellation)
/// to actually tear running apps down.
pub struct SessionEngine {
    runner: Runner,
    project_root: PathBuf,
    sessions: Mutex<BTreeMap<SessionId, Arc<Session>>>,
    next_id: AtomicU64,
    /// Set by [`shutdown`](SessionEngine::shutdown) *before* it snapshots the
    /// session map, so a `run_app` racing a Ctrl-C is either refused outright
    /// or torn down by the loser of the race — never left running past the
    /// sweep that was meant to end it.
    closed: AtomicBool,
}

impl SessionEngine {
    /// An engine for `project_root`, shelling out for real.
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        Self::with_runner(project_root, Arc::new(RealProcessRunner))
    }

    /// An engine with an injected runner — the seam tests drive with
    /// `frust_drive::process::FakeProcessRunner` so no `cargo`/`adb` is ever
    /// invoked for real (`docs/CODE_STANDARDS.md`'s `ProcessRunner`
    /// contract).
    pub fn with_runner(project_root: impl Into<PathBuf>, runner: Runner) -> Self {
        Self {
            runner,
            project_root: project_root.into(),
            sessions: Mutex::new(BTreeMap::new()),
            next_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
        }
    }

    /// The project root sessions default to.
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// The shared process runner every session shells out through — the seam
    /// a tool needing its own one-shot invocation (the `screenshot` tool's
    /// `adb exec-out screencap` fallback) borrows rather than reaching for
    /// `std::process::Command` (`docs/CODE_STANDARDS.md`'s `ProcessRunner`
    /// anti-pattern). Every call through it **blocks**, so issue it from
    /// [`tokio::task::spawn_blocking`], exactly as this engine does.
    pub fn runner(&self) -> Arc<dyn ProcessRunner + Send + Sync> {
        Arc::clone(&self.runner)
    }

    /// Every discoverable device, plus the non-fatal notes discovery
    /// produced (a missing SDK, an unauthorized device) — `frust-drive`
    /// aggregates independent discoverers and never lets one failing
    /// discoverer blank the list.
    pub async fn list_devices(&self) -> (Vec<Device>, Vec<String>) {
        let runner = Arc::clone(&self.runner);
        tokio::task::spawn_blocking(move || {
            let discoverers = default_discoverers();
            discover_all(runner.as_ref(), &discoverers)
        })
        .await
        .unwrap_or_else(|err| (Vec::new(), vec![format!("device discovery failed: {err}")]))
    }

    /// Launches `target` in `mode` and returns its session id **immediately**
    /// — the build/install/launch chain runs on the session's own thread.
    ///
    /// This never fails: a launch that cannot even spawn lands as
    /// [`SessionState::Failed`] on the session, which is the only way a
    /// device launch (whose failure arrives minutes later) could report it
    /// anyway. Await progress with [`wait_for`](Self::wait_for).
    ///
    /// Every session launches at the engine's own configured
    /// [`project_root`](Self::project_root) — there is no per-call override
    /// (a client-suppliable build directory would let any caller run
    /// `cargo`, and therefore arbitrary `build.rs`/proc-macro code, anywhere
    /// readable).
    ///
    /// After [`shutdown`](Self::shutdown) it launches nothing: the returned
    /// session is already [`SessionState::Failed`], carrying the refusal
    /// reason the tool layer reports in band.
    pub fn run_app(&self, target: RunTarget, mode: BuildMode) -> SessionId {
        let root = self.project_root.clone();
        let id = SessionId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let session = Arc::new(Session::new(id, target, mode, root));

        if self.closed.load(Ordering::SeqCst) {
            // Nothing spawned, so there is nothing to tear down — but the
            // session is still recorded, because a caller that got an id back
            // must be able to read why it never ran.
            session.request_stop();
            session.set_state_if_live(SessionState::Failed {
                reason: SHUTTING_DOWN.to_string(),
            });
            self.insert(id, session);
            return id;
        }

        // Register the launch thread *before* the session becomes reachable:
        // teardown can only find a session that is in the map, and by the time
        // it can, the thread it must join is already queued. The id is not
        // observable until this returns, so the reorder changes nothing else.
        let handle = launch::spawn(Arc::clone(&session), Arc::clone(&self.runner));
        session.add_thread(handle);
        self.insert(id, Arc::clone(&session));

        if self.closed.load(Ordering::SeqCst) {
            // A `shutdown` snapshotted the map before this insert landed, so
            // its sweep will never see this session: tear it down here instead
            // of leaving a launch running past the server that owns it.
            self.tear_down_detached(session);
        }
        id
    }

    /// Stops the session: kills its process, removes any `adb forward`,
    /// closes the devtools connection, and joins its threads — all off the
    /// async runtime, since every step of it can block.
    pub async fn stop_app(&self, id: SessionId) -> Result<()> {
        let session = self.session_arc(id)?;
        let runner = Arc::clone(&self.runner);
        tokio::task::spawn_blocking(move || teardown(&session, runner.as_ref()))
            .await
            .context("the session teardown task failed")
    }

    /// Stops the session and launches a new one with the same target and
    /// mode (always at the engine's configured project root), returning the
    /// new session's id. The old id keeps reporting its final (stopped)
    /// state.
    pub async fn restart_app(&self, id: SessionId) -> Result<SessionId> {
        let session = self.session_arc(id)?;
        let target = session.target.clone();
        let mode = session.mode;
        drop(session);

        self.stop_app(id).await?;
        Ok(self.run_app(target, mode))
    }

    /// Tears every session down. Wired into [`crate::run`]'s cancellation, so
    /// a Ctrl-C on the MCP server does not leave orphaned preview windows or
    /// `adb forward`s behind.
    ///
    /// Closes the engine to new launches first, so a `run_app` racing this
    /// sweep is refused (or, if it already inserted its session, torn down by
    /// `run_app` itself) rather than starting an app nothing will ever stop.
    pub async fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let sessions: Vec<Arc<Session>> = self
            .sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .cloned()
            .collect();
        if sessions.is_empty() {
            return;
        }
        let runner = Arc::clone(&self.runner);
        let _ = tokio::task::spawn_blocking(move || {
            for session in sessions {
                teardown(&session, runner.as_ref());
            }
        })
        .await;
    }

    /// A snapshot of every session, oldest id first.
    pub fn sessions(&self) -> Vec<SessionSnapshot> {
        self.sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .map(|session| session.snapshot())
            .collect()
    }

    /// One session's snapshot, or `None` for an unknown id.
    pub fn session(&self, id: SessionId) -> Option<SessionSnapshot> {
        self.lookup(id).map(|session| session.snapshot())
    }

    /// The session's retained log lines: the most recent `tail`, or every
    /// retained line when `tail` is `None`.
    pub fn logs(&self, id: SessionId, tail: Option<usize>) -> Option<Vec<String>> {
        self.lookup(id).map(|session| session.logs(tail))
    }

    /// The session's retained frame-stats samples, oldest first.
    pub fn frame_ring(&self, id: SessionId) -> Option<Vec<FrameStats>> {
        self.lookup(id).map(|session| session.frames())
    }

    /// The latest sample of each system-metrics kind — all-empty for a
    /// target that cannot sample (see [`metrics`]'s module doc).
    pub fn latest_metrics(&self, id: SessionId) -> Option<LatestMetrics> {
        self.lookup(id).map(|session| session.latest_metrics())
    }

    /// The session's connected devtools client, for a tool that needs to
    /// issue a request (widget tree, screenshot, input injection). `Send +
    /// Sync`, and every call is bounded by the connection's own per-request
    /// timeout — but each call still *blocks*, so issue it from
    /// [`tokio::task::spawn_blocking`].
    pub fn devtools_client(&self, id: SessionId) -> Option<Arc<DevtoolsClient>> {
        self.lookup(id)
            .and_then(|session| session.devtools_client())
    }

    /// Blocks (off the runtime) until `predicate` holds for the session's
    /// snapshot or `timeout` elapses, returning the last snapshot seen —
    /// `None` only for an unknown id.
    ///
    /// The wait is on the session's own change signal, not a poll: it settles
    /// the instant the engine records the transition.
    pub async fn wait_for(
        &self,
        id: SessionId,
        timeout: Duration,
        predicate: impl Fn(&SessionSnapshot) -> bool + Send + 'static,
    ) -> Option<SessionSnapshot> {
        let session = self.lookup(id)?;
        tokio::task::spawn_blocking(move || session.wait_for(&predicate, timeout))
            .await
            .ok()
    }

    fn insert(&self, id: SessionId, session: Arc<Session>) {
        self.sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, session);
    }

    /// Tears `session` down on a thread of its own — the late half of the
    /// shutdown race. Detached because [`run_app`](Self::run_app) is sync and
    /// reached from a runtime worker, while teardown blocks throughout (a
    /// kill, an `adb` call, a bounded join); the thread touches nothing but
    /// this one session.
    fn tear_down_detached(&self, session: Arc<Session>) {
        session.request_stop();
        let runner = Arc::clone(&self.runner);
        thread::spawn(move || teardown(&session, runner.as_ref()));
    }

    fn lookup(&self, id: SessionId) -> Option<Arc<Session>> {
        self.sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&id)
            .cloned()
    }

    fn session_arc(&self, id: SessionId) -> Result<Arc<Session>> {
        self.lookup(id)
            .with_context(|| format!("no such session: {id}"))
    }
}

/// Tears one session down, in the only order that works.
///
/// Blocking throughout — callers reach it through
/// [`tokio::task::spawn_blocking`], never a runtime worker.
fn teardown(session: &Arc<Session>, runner: &dyn ProcessRunner) {
    session.request_stop();
    // Take every handle out in one short critical section: nothing below runs
    // with a session lock held, because a session thread may be waiting for
    // the same lock and would then never reach the join.
    let mut resources = session.take_resources();

    // Android: killing the `logcat` stream ends our *view* of the app, not
    // the app — ask the OS to stop it. Best-effort: a device that has already
    // gone away must not block the rest of teardown.
    if let (Some(serial), Some(package)) = (
        session.target.android_serial(),
        session.snapshot().android_package,
    ) {
        let _ = runner.run(
            "adb",
            &["-s", serial, "shell", "am", "force-stop", &package],
        );
    }

    // Closing the devtools socket also ends the frame-stats subscription the
    // connect thread is draining.
    session.drop_devtools_client();

    if let Some(stream) = resources.stream.as_mut() {
        stream.kill();
        stream.wait();
    }

    if let Some((serial, local_port)) = resources.forward.take() {
        let _ = adb_forward_remove(runner, &serial, local_port);
    }

    join_bounded(std::mem::take(&mut resources.threads));
    session.clear_metrics_sampling();
    session.set_state_if_live(SessionState::Exited { success: false });
}

/// Joins `threads`, giving up (and detaching) after [`TEARDOWN_DEADLINE`].
///
/// The joining itself happens on a throwaway thread so the *caller's* wait is
/// bounded; a `JoinHandle` has no timed join. Past the deadline the joiner —
/// and whatever it is still waiting on — is detached rather than blocking a
/// stop request behind an uninterruptible build. The residual is a thread
/// that outlives its session and then exits on its own.
fn join_bounded(threads: Vec<JoinHandle<()>>) {
    if threads.is_empty() {
        return;
    }
    let (done_tx, done_rx) = mpsc::channel::<()>();
    thread::spawn(move || {
        for thread in threads {
            let _ = thread.join();
        }
        let _ = done_tx.send(());
    });
    let _ = done_rx.recv_timeout(TEARDOWN_DEADLINE);
}

/// The one process fake this module's own unit tests share — sibling modules
/// reach it as `super::test_support`.
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Mutex;

    use frust_drive::process::{
        FakeProcessRunner, LineReceiver, Output, ProcessRunner, StreamHandle,
    };

    use super::*;

    /// Wraps a scripted [`FakeProcessRunner`], recording every one-shot
    /// invocation it is asked to make and a clone of every spawned stream's
    /// line receiver.
    ///
    /// Those two records are how a test asserts cleanup without a real
    /// process: an issued `adb forward --remove` shows up in
    /// [`runs`](Self::runs), and a *killed* process shows up as a closed line
    /// buffer — a scripted hanging stream closes its buffer only once
    /// `StreamHandle::kill` has run.
    pub(crate) struct RecordingRunner {
        inner: FakeProcessRunner,
        runs: Mutex<Vec<String>>,
        spawned: Mutex<Vec<LineReceiver>>,
    }

    impl RecordingRunner {
        pub(crate) fn new(inner: FakeProcessRunner) -> Self {
            Self {
                inner,
                runs: Mutex::new(Vec::new()),
                spawned: Mutex::new(Vec::new()),
            }
        }

        /// Every one-shot invocation so far, as `"<cmd> <args…>"`.
        pub(crate) fn runs(&self) -> Vec<String> {
            self.runs.lock().unwrap_or_else(|p| p.into_inner()).clone()
        }

        /// A receiver over each spawned stream, in spawn order.
        pub(crate) fn spawned(&self) -> Vec<LineReceiver> {
            self.spawned
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
        }
    }

    impl ProcessRunner for RecordingRunner {
        fn run(&self, cmd: &str, args: &[&str]) -> Result<Output> {
            self.runs
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(format!("{cmd} {}", args.join(" ")));
            self.inner.run(cmd, args)
        }

        fn run_streaming(
            &self,
            cmd: &str,
            args: &[&str],
            cwd: Option<&Path>,
            env: &[(&str, &str)],
            on_line: &mut dyn FnMut(&str),
        ) -> Result<Output> {
            self.inner.run_streaming(cmd, args, cwd, env, on_line)
        }

        fn spawn_streaming(
            &self,
            cmd: &str,
            args: &[&str],
            cwd: Option<&Path>,
            env: &[(&str, &str)],
        ) -> Result<StreamHandle> {
            let handle = self.inner.spawn_streaming(cmd, args, cwd, env)?;
            self.spawned
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(handle.lines.clone());
            Ok(handle)
        }
    }
}
