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
//! This engine is therefore **sync itself** — it is the blocking side of that
//! seam, not a wrapper around it. [`list_devices`](SessionEngine::list_devices),
//! [`stop_app`](SessionEngine::stop_app), and
//! [`restart_app`](SessionEngine::restart_app) block for as long as the device
//! takes, and an async caller reaches them through `spawn_blocking` (the tool
//! layer does exactly that, per [`crate::backend`]). The state readers
//! ([`sessions`](SessionEngine::sessions), [`logs`](SessionEngine::logs), …)
//! are the exception: they take a short, non-blocking lock and are safe to
//! call directly from async code.
//!
//! [`shutdown`](SessionEngine::shutdown) is the one `async` method left, and
//! deliberately so: it belongs to the *server's* lifetime rather than to any
//! tool, and it owns the `spawn_blocking` its own sweep and detached-teardown
//! join need.
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

pub use session::{
    LatestMetrics, LogSubscription, RunTarget, SessionEvent, SessionEventFeed, SessionEventSink,
    SessionId, SessionSnapshot, SessionState,
};

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
use frust_drive::ios_run::simctl;
use frust_drive::process::{ProcessRunner, RealProcessRunner};

use crate::backend::SessionBackend;
use session::Session;

/// Retained log lines per session, drop-oldest — the same bound and the same
/// reasoning as `frust_drive::process`'s `LINE_BUFFER_CAP`: a `logcat` tail
/// this deep is already far more than an agent reads back, and the cap is
/// what keeps a long-lived session's memory flat.
pub const LOG_RING_CAP: usize = 10_000;

/// How many log lines a [`LogSubscription`]'s channel holds before the
/// session's own ingest starts dropping them.
///
/// Generous on purpose — a subscriber only has to keep up on average, not
/// line for line — but **bounded**, because the alternative is an ingest that
/// either blocks the session's drain thread on a consumer it does not control
/// or grows a queue without limit. Overflow is reported in band (see
/// [`LogSubscription`]), never silently.
pub const LOG_SUBSCRIPTION_CAP: usize = 4096;

/// Retained frame-stats samples per session, drop-oldest. Ten seconds of
/// 60fps: enough for the performance tool to aggregate a meaningful window
/// on demand, small enough to keep per session for free.
pub const FRAME_RING_CAP: usize = 600;

/// How many **terminal** sessions the engine retains.
///
/// A stopped session stays readable — its final state and its log tail are
/// exactly what an agent inspects after a crash — but that history cannot
/// grow without bound on a server an agent drives for hours. Past this many,
/// the oldest terminal sessions are dropped on the next insert. A live
/// session is **never** evicted, whatever the count: dropping one would leave
/// a running app with no handle to stop it.
pub const TERMINAL_SESSION_CAP: usize = 32;

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
    /// session map, and read by [`run_app`](SessionEngine::run_app) **inside**
    /// the same `sessions` critical section that spawns the launch — the two
    /// are therefore totally ordered, and a `run_app` racing a Ctrl-C either
    /// launches nothing or leaves a session the sweep's snapshot contains.
    closed: AtomicBool,
    /// Every off-runtime teardown thread still in flight, for
    /// [`shutdown`](SessionEngine::shutdown) to join before it returns — a
    /// teardown nothing joins is a kill that may still be pending when the
    /// server thinks it has finished stopping.
    detached: Mutex<Vec<JoinHandle<()>>>,
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
            detached: Mutex::new(Vec::new()),
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
    ///
    /// **Blocks** (it shells out to `adb`/`xcrun`): call it from
    /// [`tokio::task::spawn_blocking`].
    pub fn list_devices(&self) -> (Vec<Device>, Vec<String>) {
        let discoverers = default_discoverers();
        discover_all(self.runner.as_ref(), &discoverers)
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
    /// readable). `frust-dap` builds a *fresh* engine per `launch` from the
    /// request's `projectRoot`, so it confines that client-chosen root to its
    /// stdio transport (where the client spawned the process); its
    /// unauthenticated TCP transport ignores the client root and launches from
    /// the server's cwd (see `docs/LIMITATIONS.md`
    /// `dap-tcp-unauthenticated-v1`).
    ///
    /// After [`shutdown`](Self::shutdown) it launches nothing: the returned
    /// session is already [`SessionState::Failed`], carrying the refusal
    /// reason the tool layer reports in band.
    pub fn run_app(&self, target: RunTarget, mode: BuildMode) -> SessionId {
        let root = self.project_root.clone();
        let id = SessionId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let session = Arc::new(Session::new(id, target, mode, root));

        // The closed check, the launch, and the insert happen in **one**
        // `sessions` critical section, and [`shutdown`](Self::shutdown) takes
        // the same lock to snapshot — so the two are totally ordered and only
        // two interleavings exist. Either this call ran first, and the sweep's
        // snapshot holds a session whose launch thread is already registered
        // (so teardown joins it, and the join is what makes the process dead
        // before `shutdown` returns); or the sweep ran first, and the load
        // below sees the flag it set and launches nothing at all. There is no
        // window that starts an app the sweep cannot find.
        let mut sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        if self.closed.load(Ordering::SeqCst) {
            // Nothing spawned, so there is nothing to tear down — but the
            // session is still recorded, because a caller that got an id back
            // must be able to read why it never ran.
            session.request_stop();
            session.set_state_if_live(SessionState::Failed {
                reason: SHUTTING_DOWN.to_string(),
            });
            insert_retaining(&mut sessions, id, session);
            return id;
        }

        // Register the launch thread *before* the session becomes reachable:
        // teardown can only find a session that is in the map, and by the time
        // it can, the thread it must join is already queued. The id is not
        // observable until this returns, so the reorder changes nothing else.
        let handle = launch::spawn(Arc::clone(&session), Arc::clone(&self.runner));
        session.add_thread(handle);
        insert_retaining(&mut sessions, id, Arc::clone(&session));
        drop(sessions);

        if self.closed.load(Ordering::SeqCst) {
            // A `shutdown` landed after the critical section above, so its own
            // sweep already holds this session — this is a backstop, not the
            // path that ends the app, and it costs only a redundant
            // (idempotent) teardown that runs concurrently with the sweep's.
            self.tear_down_detached(session);
        }
        id
    }

    /// Stops the session: kills its process, removes any `adb forward`,
    /// closes the devtools connection, and joins its threads.
    ///
    /// **Blocks** through every one of those steps: call it from
    /// [`tokio::task::spawn_blocking`], never on a runtime worker.
    pub fn stop_app(&self, id: SessionId) -> Result<()> {
        let session = self.session_arc(id)?;
        teardown(&session, self.runner.as_ref());
        Ok(())
    }

    /// Stops the session and launches a new one with the same target and
    /// mode (always at the engine's configured project root), returning the
    /// new session's id. The old id keeps reporting its final (stopped)
    /// state.
    ///
    /// **Blocks** for the stop half, exactly as [`stop_app`](Self::stop_app)
    /// does.
    pub fn restart_app(&self, id: SessionId) -> Result<SessionId> {
        let session = self.session_arc(id)?;
        let target = session.target.clone();
        let mode = session.mode;
        drop(session);

        self.stop_app(id)?;
        Ok(self.run_app(target, mode))
    }

    /// Tears every session down. Wired into [`crate::run`]'s cancellation, so
    /// a Ctrl-C on the MCP server does not leave orphaned preview windows or
    /// `adb forward`s behind.
    ///
    /// Closes the engine to new launches first, so a `run_app` racing this
    /// sweep is refused (or, having already inserted its session under the
    /// same lock, swept here) rather than starting an app nothing will ever
    /// stop — see [`run_app`](Self::run_app)'s ordering note.
    ///
    /// When this returns, every teardown it is responsible for has finished or
    /// hit [`TEARDOWN_DEADLINE`]: the sweep's own teardowns run to completion,
    /// and every off-thread teardown registered along the way is joined on the
    /// way out — including on the path where the snapshot was empty, which is
    /// exactly the path a launch racing this sweep registers one on.
    pub async fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let sessions: Vec<Arc<Session>> = self
            .sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .cloned()
            .collect();
        if !sessions.is_empty() {
            let runner = Arc::clone(&self.runner);
            let _ = tokio::task::spawn_blocking(move || {
                for session in sessions {
                    teardown(&session, runner.as_ref());
                }
            })
            .await;
        }
        self.join_detached().await;
    }

    /// Drains the off-thread teardowns registered so far and bounded-joins
    /// them, off the runtime (every one of them blocks on a kill, an `adb`
    /// call, or a join of its own).
    async fn join_detached(&self) {
        let detached =
            std::mem::take(&mut *self.detached.lock().unwrap_or_else(|p| p.into_inner()));
        if detached.is_empty() {
            return;
        }
        let _ = tokio::task::spawn_blocking(move || join_bounded(detached)).await;
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

    /// Streams the session's log lines as they arrive — the push seam a
    /// consumer that must not miss output (a DAP adapter forwarding it to an
    /// editor) uses instead of re-polling [`logs`](Self::logs) and diffing.
    ///
    /// The feed opens with the session's retained lines and continues live,
    /// with no gap and no duplicate across the boundary: the seed is taken and
    /// the feed registered inside the same critical section a log ingest holds
    /// (see [`LogSubscription`] for the full contract — bounded channel,
    /// in-band loss markers, redacted lines, close on session end).
    ///
    /// **One subscriber per session.** A second call replaces the first, whose
    /// receiver then observes a closed channel.
    ///
    /// `None` for an unknown session id.
    pub fn subscribe_logs(&self, id: SessionId) -> Option<LogSubscription> {
        self.lookup(id).map(|session| session.subscribe_logs())
    }

    /// Streams the session's log lines **and its end** as they happen — the
    /// [`subscribe_logs`](Self::subscribe_logs) feed plus the terminal
    /// [`SessionEvent::Exited`], so a sync consumer needs neither a second
    /// (async) [`wait_for`](Self::wait_for) nor a poll to learn the app died.
    ///
    /// Same gapless seed-then-live contract, same in-band loss markers, same
    /// one-subscriber-per-session rule (see [`SessionEventFeed`]). An
    /// already-terminal session hands back its retained lines followed
    /// immediately by its `Exited`.
    ///
    /// `None` for an unknown session id.
    pub fn subscribe_session_events(&self, id: SessionId) -> Option<SessionEventFeed> {
        self.lookup(id).map(|session| session.subscribe_events())
    }

    /// One widget-tree dump from the session's devtools service, as the raw
    /// JSON of `frust_devtools_protocol::WidgetTreeDump`.
    ///
    /// **Blocks** (it is a request/response round trip over the devtools
    /// connection, bounded only by that connection's own per-request timeout):
    /// call it from [`tokio::task::spawn_blocking`].
    ///
    /// The error is the devtools client's own, unwrapped — so
    /// `frust_drive::devtools_client::is_not_supported`/`is_unauthorized`
    /// still classify it — plus this engine's own "not connected" refusal for
    /// a session with no client.
    pub fn fetch_widget_tree(&self, id: SessionId) -> Result<serde_json::Value> {
        let client = self.devtools_client(id).with_context(|| {
            format!("session {id} is not connected to a devtools service; no widget tree to read")
        })?;
        let dump = client.widget_tree()?;
        serde_json::to_value(&dump).context("serializing the widget tree")
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

    /// Tears `session` down on a thread of its own — the late half of the
    /// shutdown race. Off-thread because [`run_app`](Self::run_app) is sync
    /// and reached from a runtime worker, while teardown blocks throughout (a
    /// kill, an `adb` call, a bounded join); the thread touches nothing but
    /// this one session.
    ///
    /// The handle is **registered**, not dropped: a
    /// [`shutdown`](Self::shutdown) that returns while one of these is still
    /// killing a process has not actually stopped the server's apps.
    fn tear_down_detached(&self, session: Arc<Session>) {
        session.request_stop();
        let runner = Arc::clone(&self.runner);
        let handle = thread::spawn(move || teardown(&session, runner.as_ref()));
        self.detached
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(handle);
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

/// The reference [`SessionBackend`]: every method forwards to the inherent one
/// above, which is why the trait's method set was chosen to match. The tool
/// layer sees only this impl — nothing in `tools/` names [`SessionEngine`].
impl SessionBackend for SessionEngine {
    fn list_devices(&self) -> (Vec<Device>, Vec<String>) {
        SessionEngine::list_devices(self)
    }

    fn sessions(&self) -> Vec<SessionSnapshot> {
        SessionEngine::sessions(self)
    }

    fn session(&self, id: SessionId) -> Option<SessionSnapshot> {
        SessionEngine::session(self, id)
    }

    fn run_app(&self, target: RunTarget, mode: BuildMode) -> SessionId {
        SessionEngine::run_app(self, target, mode)
    }

    fn stop_app(&self, id: SessionId) -> Result<()> {
        SessionEngine::stop_app(self, id)
    }

    fn restart_app(&self, id: SessionId) -> Result<SessionId> {
        SessionEngine::restart_app(self, id)
    }

    fn logs(&self, id: SessionId, tail: Option<usize>) -> Option<Vec<String>> {
        SessionEngine::logs(self, id, tail)
    }

    fn frame_ring(&self, id: SessionId) -> Option<Vec<FrameStats>> {
        SessionEngine::frame_ring(self, id)
    }

    fn latest_metrics(&self, id: SessionId) -> Option<LatestMetrics> {
        SessionEngine::latest_metrics(self, id)
    }

    fn devtools_client(&self, id: SessionId) -> Option<Arc<DevtoolsClient>> {
        SessionEngine::devtools_client(self, id)
    }

    fn subscribe_session_events(&self, id: SessionId) -> Option<SessionEventFeed> {
        SessionEngine::subscribe_session_events(self, id)
    }

    fn fetch_widget_tree(&self, id: SessionId) -> Result<serde_json::Value> {
        SessionEngine::fetch_widget_tree(self, id)
    }

    fn runner(&self) -> Arc<dyn ProcessRunner + Send + Sync> {
        SessionEngine::runner(self)
    }
}

/// Inserts a session and enforces [`TERMINAL_SESSION_CAP`] on what is left.
///
/// Eviction is oldest-terminal-first (a `BTreeMap` over the monotonic
/// [`SessionId`] iterates in launch order) and skips every live session, so a
/// long-lived server's history stays bounded without a running app ever losing
/// the handle that stops it. Retention is enforced here, on insert, rather
/// than when a session *becomes* terminal: a session count only grows by an
/// insert, so that is the only moment the cap can be newly exceeded by more
/// than the sessions still running.
fn insert_retaining(
    sessions: &mut BTreeMap<SessionId, Arc<Session>>,
    id: SessionId,
    session: Arc<Session>,
) {
    sessions.insert(id, session);
    let terminal: Vec<SessionId> = sessions
        .iter()
        .filter(|(_, session)| session.is_terminal())
        .map(|(id, _)| *id)
        .collect();
    let excess = terminal.len().saturating_sub(TERMINAL_SESSION_CAP);
    for id in terminal.into_iter().take(excess) {
        sessions.remove(&id);
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
    let snapshot = session.snapshot();

    // Android: killing the `logcat` stream ends our *view* of the app, not
    // the app — ask the OS to stop it. Best-effort: a device that has already
    // gone away must not block the rest of teardown.
    if let (Some(serial), Some(package)) =
        (session.target.android_serial(), &snapshot.android_package)
    {
        let _ = runner.run("adb", &["-s", serial, "shell", "am", "force-stop", package]);
    }

    // iOS Simulator: the same gap, one platform over. Killing the
    // `simctl launch` stream ends the foreground console bridge, not the app
    // running inside the simulator — `simctl terminate` is what stops it, and
    // it is best-effort for the same reason (the app may already be gone, or
    // the simulator shut down).
    if let (Some(udid), Some(bundle_id)) =
        (session.target.ios_simulator_udid(), &snapshot.ios_bundle_id)
    {
        simctl::terminate(runner, udid, bundle_id);
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
    // Nothing will ever be ingested for this session again, so the log
    // subscription ends here — explicitly, because a session that was already
    // terminal never passes through the state transition that closes it.
    session.close_log_subscription();
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

#[cfg(test)]
mod tests {
    use frust_drive::devices::{Device, Kind, Platform};
    use frust_drive::process::{FakeProcessRunner, TryRecvError};

    use super::test_support::RecordingRunner;
    use super::*;

    /// A project root no test ever writes to — it only ever reaches the fake
    /// runner.
    const TEST_PROJECT_ROOT: &str = "/tmp/frust-mcp-engine-unit-test";

    /// The scripted invocation the fake runner hands a hanging stream back
    /// for.
    const FAKE_STREAM_KEY: &str = "app --run";

    fn session(id: u64, target: RunTarget) -> Arc<Session> {
        Arc::new(Session::new(
            SessionId(id),
            target,
            BuildMode::Debug,
            PathBuf::from(TEST_PROJECT_ROOT),
        ))
    }

    /// Registers a hand-built session in the engine's own map — what
    /// `run_app` does minus the launch, so a feed test drives the seam rather
    /// than a scripted build pipeline.
    fn insert(engine: &SessionEngine, session: Arc<Session>) {
        let mut sessions = engine.sessions.lock().unwrap_or_else(|p| p.into_inner());
        insert_retaining(&mut sessions, session.id, session);
    }

    fn engine() -> SessionEngine {
        SessionEngine::with_runner(TEST_PROJECT_ROOT, Arc::new(FakeProcessRunner::new()))
    }

    fn simulator() -> Device {
        Device {
            id: "AAAA-BBBB".to_string(),
            name: "iPhone 15".to_string(),
            platform: Platform::Ios,
            kind: Kind::Simulator,
            os_version: Some("17.5".to_string()),
            connection_state: None,
        }
    }

    /// A shutdown whose own snapshot is empty still has work to finish: the
    /// off-thread teardown a launch racing it registered. Asserted with no
    /// poll and no second deadline — the kill has either happened by the time
    /// `shutdown` returns, or the engine returned while still killing.
    #[tokio::test]
    async fn shutdown_joins_an_off_thread_teardown_with_no_sessions_of_its_own() {
        let recorder = Arc::new(RecordingRunner::new(
            FakeProcessRunner::new().with_hanging_stream(FAKE_STREAM_KEY, Vec::<String>::new()),
        ));
        let engine = SessionEngine::with_runner(TEST_PROJECT_ROOT, Arc::clone(&recorder) as Runner);

        // A session the engine's own map never held — the shape the loser of a
        // launch/shutdown race leaves behind.
        let session = session(1, RunTarget::Desktop);
        let stream = recorder
            .spawn_streaming("app", &["--run"], None, &[])
            .expect("the scripted stream spawns");
        assert!(session.set_stream(stream).is_none());
        engine.tear_down_detached(session);

        engine.shutdown().await;

        assert!(
            engine.sessions().is_empty(),
            "the fixture session was never in the map — the empty-snapshot path is the one under test"
        );
        let spawned = recorder.spawned();
        let [lines] = spawned.as_slice() else {
            panic!(
                "expected exactly one spawned process, got {}",
                spawned.len()
            );
        };
        // A scripted hanging stream closes its line buffer only once `kill`
        // (and the `wait` that reaps it) has run, so a *disconnected* receiver
        // read without any waiting at all is exactly "the teardown finished
        // before `shutdown` returned".
        assert_eq!(
            lines.try_recv(),
            Err(TryRecvError::Disconnected),
            "shutdown returned while a teardown it registered was still running"
        );
    }

    /// Killing the `simctl launch` stream ends the console bridge, not the app
    /// in the simulator — teardown has to terminate it by bundle id.
    #[test]
    fn tearing_down_an_ios_simulator_session_terminates_the_app() {
        let recorder = Arc::new(RecordingRunner::new(FakeProcessRunner::new()));
        let session = session(1, RunTarget::IosSimulator(simulator()));
        session.note_ios_bundle_id("dev.f0x.myapp".to_string());

        teardown(&session, recorder.as_ref());

        assert!(
            recorder
                .runs()
                .iter()
                .any(|run| run == "xcrun simctl terminate AAAA-BBBB dev.f0x.myapp"),
            "teardown left the simulator app running: {:?}",
            recorder.runs()
        );
    }

    /// Nothing to terminate is not the same as terminating nothing: a session
    /// that never reached a launch has no bundle id, and teardown must not
    /// invent one.
    #[test]
    fn an_ios_session_that_never_launched_terminates_nothing() {
        let recorder = Arc::new(RecordingRunner::new(FakeProcessRunner::new()));
        let session = session(1, RunTarget::IosSimulator(simulator()));

        teardown(&session, recorder.as_ref());

        assert!(
            !recorder.runs().iter().any(|run| run.contains("terminate")),
            "{:?}",
            recorder.runs()
        );
    }

    /// The engine reaches the tool layer only as an `Arc<dyn SessionBackend>`,
    /// so the coercion itself is a contract: the trait must stay object-safe,
    /// and this engine must keep satisfying it. Pinned here rather than left to
    /// whichever call site happens to compile.
    #[test]
    fn the_engine_drives_through_the_backend_trait_object() {
        let backend: Arc<dyn SessionBackend> = Arc::new(SessionEngine::with_runner(
            TEST_PROJECT_ROOT,
            Arc::new(FakeProcessRunner::new()),
        ));

        assert!(backend.sessions().is_empty());
        assert!(backend.session(SessionId(1)).is_none());
        assert!(backend.logs(SessionId(1), None).is_none());
        assert!(backend.frame_ring(SessionId(1)).is_none());
        assert!(backend.latest_metrics(SessionId(1)).is_none());
        assert!(backend.devtools_client(SessionId(1)).is_none());
        assert!(backend.stop_app(SessionId(1)).is_err());
        assert!(backend.restart_app(SessionId(1)).is_err());
    }

    /// The whole point of the event feed in one pass: the lines already
    /// retained, then the ones ingested after subscribing, then the session's
    /// end — and nothing after it.
    #[test]
    fn a_session_event_feed_delivers_the_backlog_then_live_lines_then_the_exit() {
        let engine = engine();
        let session = session(1, RunTarget::Desktop);
        session.push_log("before subscribing".to_string());
        insert(&engine, Arc::clone(&session));

        let feed = engine
            .subscribe_session_events(SessionId(1))
            .expect("the session exists");
        session.push_log("after subscribing".to_string());
        session.set_state_if_live(SessionState::Exited { success: true });

        assert_eq!(
            feed.recv().expect("the seeded line"),
            SessionEvent::Log("before subscribing".to_string())
        );
        assert_eq!(
            feed.recv().expect("the live line"),
            SessionEvent::Log("after subscribing".to_string())
        );
        assert_eq!(
            feed.recv().expect("the exit"),
            SessionEvent::Exited {
                state: SessionState::Exited { success: true },
            }
        );
        assert!(
            feed.recv().is_err(),
            "the feed must end with the exit, not stay open"
        );
        assert!(engine.subscribe_session_events(SessionId(9)).is_none());
    }

    /// A session that ended before anyone subscribed still owes the
    /// subscriber an ending — otherwise a consumer waits forever on an app
    /// that is already gone.
    #[test]
    fn subscribing_to_an_already_ended_session_hands_back_its_exit_at_once() {
        let engine = engine();
        let session = session(1, RunTarget::Desktop);
        session.push_log("it crashed".to_string());
        session.set_state_if_live(SessionState::Failed {
            reason: "the build failed".to_string(),
        });
        insert(&engine, Arc::clone(&session));

        let feed = engine
            .subscribe_session_events(SessionId(1))
            .expect("a terminal session is still readable");
        assert_eq!(
            feed.recv().expect("the retained line"),
            SessionEvent::Log("it crashed".to_string())
        );
        assert_eq!(
            feed.recv().expect("the exit"),
            SessionEvent::Exited {
                state: SessionState::Failed {
                    reason: "the build failed".to_string(),
                },
            }
        );
        assert!(feed.recv().is_err());
    }

    /// Loss is reported in band and never silently: a consumer that stops
    /// reading gets a `[frust] <N> event(s) dropped` marker at the point the
    /// loss happened, ahead of the next line that fits.
    #[test]
    fn a_slow_consumer_is_told_how_many_events_it_missed() {
        let engine = engine();
        let session = session(1, RunTarget::Desktop);
        insert(&engine, Arc::clone(&session));
        let feed = engine
            .subscribe_session_events(SessionId(1))
            .expect("the session exists");

        let overflow = 5;
        for index in 0..(LOG_SUBSCRIPTION_CAP + overflow) {
            session.push_log(format!("line {index}"));
        }
        for index in 0..LOG_SUBSCRIPTION_CAP {
            assert_eq!(
                feed.try_recv().expect("a buffered line"),
                SessionEvent::Log(format!("line {index}")),
            );
        }
        // The marker rides ahead of the next line the channel accepts.
        session.push_log("caught up".to_string());
        assert_eq!(
            feed.try_recv().expect("the loss marker"),
            SessionEvent::Log(format!(
                "[frust] {overflow} event(s) dropped (slow consumer)"
            ))
        );
        assert_eq!(
            feed.try_recv().expect("the line after the marker"),
            SessionEvent::Log("caught up".to_string())
        );
    }

    /// The exit is the one event that may never be lost to overflow, so the
    /// feed holds slots back for it: a consumer that read *nothing* at all
    /// still finds the marker and the exit at the end of what it buffered.
    #[test]
    fn an_overflowing_feed_still_delivers_the_exit_it_owes() {
        let engine = engine();
        let session = session(1, RunTarget::Desktop);
        insert(&engine, Arc::clone(&session));
        let feed = engine
            .subscribe_session_events(SessionId(1))
            .expect("the session exists");

        for index in 0..(LOG_SUBSCRIPTION_CAP * 2) {
            session.push_log(format!("line {index}"));
        }
        session.set_state_if_live(SessionState::Exited { success: false });

        let mut events = Vec::new();
        while let Ok(event) = feed.try_recv() {
            events.push(event);
        }
        assert_eq!(
            events.last(),
            Some(&SessionEvent::Exited {
                state: SessionState::Exited { success: false },
            }),
            "the exit must survive an overflowing feed"
        );
        assert!(
            events.iter().any(|event| matches!(
                event,
                SessionEvent::Log(line) if line.contains("event(s) dropped")
            )),
            "a feed that dropped lines must say so in band"
        );
    }

    /// Without a devtools connection there is no tree to read, and the engine
    /// says which session it means rather than answering with an empty one.
    #[test]
    fn fetching_a_widget_tree_without_a_devtools_connection_is_an_error() {
        let engine = engine();
        insert(&engine, session(1, RunTarget::Desktop));

        let err = engine
            .fetch_widget_tree(SessionId(1))
            .expect_err("nothing is connected");
        assert!(
            format!("{err:#}").contains("session 1 is not connected"),
            "unhelpful: {err:#}"
        );
    }

    /// Retention evicts the oldest *terminal* sessions past the cap and never
    /// a live one, whatever the order they were launched in.
    #[test]
    fn retention_evicts_the_oldest_terminal_sessions_but_never_a_live_one() {
        let mut sessions: BTreeMap<SessionId, Arc<Session>> = BTreeMap::new();
        // The oldest session of all is still running: eviction must skip it.
        let live = SessionId(1);
        insert_retaining(&mut sessions, live, session(1, RunTarget::Desktop));

        let ended: Vec<SessionId> = (2..=(TERMINAL_SESSION_CAP as u64 + 3))
            .map(|id| {
                let session = session(id, RunTarget::Desktop);
                session.set_state_if_live(SessionState::Exited { success: true });
                insert_retaining(&mut sessions, SessionId(id), session);
                SessionId(id)
            })
            .collect();

        assert!(
            sessions.contains_key(&live),
            "a live session must never be evicted for capacity"
        );
        let retained_terminal = sessions.len() - 1;
        assert_eq!(retained_terminal, TERMINAL_SESSION_CAP);
        // The survivors are the most recent ones, in launch order.
        let expected: Vec<SessionId> = ended[ended.len() - TERMINAL_SESSION_CAP..].to_vec();
        let retained: Vec<SessionId> = sessions.keys().copied().filter(|id| *id != live).collect();
        assert_eq!(retained, expected);
    }
}
