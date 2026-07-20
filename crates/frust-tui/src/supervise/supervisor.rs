//! The async session supervisor (PLAN D2/D3).
//!
//! [`Supervisor`] owns one supervised session per spawned process. Each
//! session's blocking [`StreamHandle`] (from `frust-drive`'s cancellable
//! `spawn_streaming` seam) is drained on a dedicated std thread that bridges
//! every stdout line — and every inferred [`SessionState`] change — into a
//! single tokio mpsc the engine will `select!` on (TUI2-03). The supervisor
//! owns the kill path: [`Supervisor::stop`] routes to
//! [`StreamHandle::kill`]'s group-kill.
//!
//! # Why a std thread, not a tokio task
//!
//! [`StreamHandle::lines`] is a blocking [`LineReceiver`]; draining
//! it is a blocking loop that must not sit on a tokio worker. A plain std
//! thread per session keeps the blocking recv off the async runtime while
//! still feeding the async channel (`tokio::sync::mpsc::UnboundedSender::send`
//! is a non-blocking, runtime-free call). This is the "spawn_blocking / std
//! thread" split PLAN D2 calls for.
//!
//! # Kill boundary (PLAN D3's known limitation)
//!
//! `stop` is prompt for anything spawned through `spawn_streaming` (the
//! desktop `cargo run` preview, or any streaming phase): killing the child
//! closes its stdout pipe, which unblocks the drain loop immediately. A
//! device session's multi-phase drive pipeline (build → install → launch →
//! logcat) is only killable once it reaches its streaming phase — a kill
//! requested mid-Gradle takes effect when streaming begins, not instantly.
//! Wiring that device path is TUI2-04; this module ships the core plus the
//! fully-killable desktop path.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result};
use frust_drive::process::{LineReceiver, ProcessRunner, StreamHandle};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use super::session::{
    LaunchPlan, SessionEvent, SessionEventKind, SessionId, SessionSpec, SessionState, infer_state,
};

/// One live (or terminated) session's supervisor-side bookkeeping.
struct SessionEntry {
    /// The drive's kill handle, shared with the drain thread (which reaps the
    /// child at EOF). `stop` locks it briefly to send the kill signal.
    handle: Arc<Mutex<StreamHandle>>,
    /// Set by `stop` before killing, so the drain thread reports [`Killed`]
    /// rather than [`Exited`] at EOF.
    ///
    /// [`Killed`]: SessionState::Killed
    /// [`Exited`]: SessionState::Exited
    killed: Arc<AtomicBool>,
    /// The drain thread; joined on `Drop` so a torn-down supervisor never
    /// leaks a running supervision thread.
    worker: Option<JoinHandle<()>>,
}

/// Owns every supervised session and the single event channel they feed.
///
/// Construct with [`Supervisor::new`], which hands back the
/// [`UnboundedReceiver`] the engine drains. Start sessions with
/// [`Supervisor::start`] (a [`SessionSpec`]) or [`Supervisor::start_with_plan`]
/// (a purpose-built [`LaunchPlan`]); stop them with [`Supervisor::stop`] /
/// [`Supervisor::stop_all`].
pub struct Supervisor {
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    events_tx: UnboundedSender<SessionEvent>,
    next_id: u64,
    sessions: HashMap<SessionId, SessionEntry>,
}

impl Supervisor {
    /// Build a supervisor over `runner`, returning it paired with the event
    /// receiver the engine `select!`s on. Production passes
    /// `Arc::new(frust_drive::process::RealProcessRunner)`; tests pass an
    /// `Arc<FakeProcessRunner>` scripted with `with_stream`/
    /// `with_hanging_stream`.
    pub fn new(
        runner: Arc<dyn ProcessRunner + Send + Sync>,
    ) -> (Self, UnboundedReceiver<SessionEvent>) {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let supervisor = Self {
            runner,
            events_tx,
            next_id: 0,
            sessions: HashMap::new(),
        };
        (supervisor, events_rx)
    }

    /// Start a session from a [`SessionSpec`], resolving it to a
    /// [`LaunchPlan`] first (see [`SessionSpec::launch_plan`]). Errors if the
    /// spec can't be resolved (e.g. an unwired device target) or the process
    /// fails to spawn.
    pub fn start(&mut self, spec: &SessionSpec) -> Result<SessionId> {
        let plan = spec.launch_plan()?;
        self.start_with_plan(plan)
    }

    /// Start a session from an already-resolved [`LaunchPlan`] — the general
    /// seam (TUI2-04 builds device plans and hands them here). Spawns the
    /// process through the cancellable `spawn_streaming` seam and begins
    /// supervising it; the returned [`SessionId`] tags every [`SessionEvent`]
    /// the session emits.
    pub fn start_with_plan(&mut self, plan: LaunchPlan) -> Result<SessionId> {
        let id = SessionId(self.next_id);

        // Borrow the owned plan pieces for the spawn call.
        let arg_refs: Vec<&str> = plan.args.iter().map(String::as_str).collect();
        let env_refs: Vec<(&str, &str)> = plan
            .env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let handle = self
            .runner
            .spawn_streaming(
                &plan.program,
                &arg_refs,
                Some(plan.cwd.as_path()),
                &env_refs,
            )
            .with_context(|| format!("failed to start session `{}`", plan.program))?;

        // Clone the line receiver for the drain thread (`LineReceiver` is an
        // `Arc` over the shared ring; clones compete for lines and only the
        // drain thread ever receives). `kill`/`wait` never touch `lines`, so
        // the handle stays fully functional for the kill path while the
        // drain thread owns the receive side.
        let lines = handle.lines.clone();
        let handle = Arc::new(Mutex::new(handle));
        let killed = Arc::new(AtomicBool::new(false));

        let worker = {
            let handle = Arc::clone(&handle);
            let killed = Arc::clone(&killed);
            let events = self.events_tx.clone();
            thread::Builder::new()
                .name(format!("frust-tui-session-{}", id.0))
                .spawn(move || drain_session(id, lines, handle, killed, events))
                .context("spawning session supervision thread")?
        };

        self.sessions.insert(
            id,
            SessionEntry {
                handle,
                killed,
                worker: Some(worker),
            },
        );
        self.next_id += 1;
        Ok(id)
    }

    /// Stop a session: mark it killed, then group-kill its process. The drain
    /// thread observes the closed stdout pipe, reaps the child, and emits a
    /// final [`SessionState::Killed`]. Idempotent and safe on an
    /// already-exited session (the underlying [`StreamHandle::kill`] is a
    /// no-op then); an unknown id is ignored.
    pub fn stop(&mut self, id: SessionId) {
        if let Some(entry) = self.sessions.get(&id) {
            entry.killed.store(true, Ordering::SeqCst);
            lock(&entry.handle).kill();
        }
    }

    /// Stop every session (the supervisor's job on quit — PLAN D3).
    pub fn stop_all(&mut self) {
        let ids: Vec<SessionId> = self.sessions.keys().copied().collect();
        for id in ids {
            self.stop(id);
        }
    }

    /// The ids of every session the supervisor is tracking (running or
    /// already terminated), for the engine's session bookkeeping.
    pub fn session_ids(&self) -> impl Iterator<Item = SessionId> + '_ {
        self.sessions.keys().copied()
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        // Kill every still-running session, then join the drain threads so a
        // dropped supervisor leaves no orphaned process or thread behind.
        self.stop_all();
        for (_, mut entry) in self.sessions.drain() {
            if let Some(worker) = entry.worker.take() {
                let _ = worker.join();
            }
        }
    }
}

/// Lock a session's kill handle, recovering from a poisoned mutex (a panicked
/// prior holder) rather than propagating — the supervisor must still be able
/// to kill and reap a child even if some other path panicked mid-lock.
fn lock(handle: &Mutex<StreamHandle>) -> std::sync::MutexGuard<'_, StreamHandle> {
    handle
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The per-session drain loop, run on its own std thread: forward every line
/// and inferred state change into the engine channel, then reap the child and
/// emit the terminal state.
fn drain_session(
    id: SessionId,
    lines: LineReceiver,
    handle: Arc<Mutex<StreamHandle>>,
    killed: Arc<AtomicBool>,
    events: UnboundedSender<SessionEvent>,
) {
    let mut state = SessionState::Configuring;
    // The engine dropping the receiver (shutdown) makes every `send` fail;
    // bail out of the loop immediately when that happens — there is nothing
    // left to report to, and the child is reaped via the handle's own
    // drop-detach path.
    if emit(&events, id, SessionEventKind::State(state.clone())).is_err() {
        return;
    }

    while let Ok(line) = lines.recv() {
        if emit(&events, id, SessionEventKind::Line(line.clone())).is_err() {
            return;
        }
        if let Some(next) = infer_state(&state, &line) {
            state = next;
            if emit(&events, id, SessionEventKind::State(state.clone())).is_err() {
                return;
            }
        }
    }

    // stdout hit EOF: the process exited on its own or a `stop` killed it.
    // `wait` reaps it (idempotent with the kill path) and yields exit success.
    let success = lock(&handle).wait();
    let terminal = if killed.load(Ordering::SeqCst) {
        SessionState::Killed
    } else {
        SessionState::Exited(success)
    };
    let _ = emit(&events, id, SessionEventKind::State(terminal));
}

/// Send one event, mapping the tokio send error to `()` (the only failure is a
/// dropped receiver, which every caller treats as "engine gone, stop").
fn emit(
    events: &UnboundedSender<SessionEvent>,
    id: SessionId,
    kind: SessionEventKind,
) -> Result<(), ()> {
    events.send(SessionEvent { id, kind }).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_drive::process::FakeProcessRunner;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::time::timeout;

    /// A generous ceiling so a genuinely wedged test still fails (rather than
    /// hanging the suite forever) while never flaking on thread-scheduling
    /// latency under a loaded `cargo test --workspace` run — a success returns
    /// the moment the event arrives, so this only bounds the failure path.
    const RECV_TIMEOUT: Duration = Duration::from_secs(30);

    /// A tighter bound for the "kill is *prompt*" assertion: even under load a
    /// killed fake stream (no real sleep) unblocks in well under this, proving
    /// `stop` didn't block on the process's own lifetime.
    const PROMPT_STOP: Duration = Duration::from_secs(10);

    fn plan(program: &str, args: &[&str]) -> LaunchPlan {
        LaunchPlan {
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: PathBuf::from("."),
            env: Vec::new(),
        }
    }

    /// Receive the next event for `want_id`, ignoring events for other
    /// sessions, within [`RECV_TIMEOUT`].
    async fn next_for(
        rx: &mut UnboundedReceiver<SessionEvent>,
        want_id: SessionId,
    ) -> SessionEvent {
        loop {
            let ev = timeout(RECV_TIMEOUT, rx.recv())
                .await
                .expect("timed out waiting for a session event")
                .expect("event channel closed unexpectedly");
            if ev.id == want_id {
                return ev;
            }
        }
    }

    /// Drain events for `want_id` until a terminal state, collecting every
    /// line seen and returning `(lines, terminal_state)`. Only sound when
    /// `want_id` is the *only* session on the channel — [`next_for`] discards
    /// other ids, so multi-session tests must use [`drain_all_to_terminal`]
    /// instead (which never throws another session's events away).
    async fn drain_to_terminal(
        rx: &mut UnboundedReceiver<SessionEvent>,
        want_id: SessionId,
    ) -> (Vec<String>, SessionState) {
        let mut lines = Vec::new();
        loop {
            let ev = next_for(rx, want_id).await;
            match ev.kind {
                SessionEventKind::Line(l) => lines.push(l),
                SessionEventKind::State(s) if s.is_terminal() => return (lines, s),
                SessionEventKind::State(_) => {}
            }
        }
    }

    /// Drain the single shared channel once, routing each event to its session
    /// by id, until *every* id in `ids` has reached a terminal state. Returns
    /// each session's collected lines + terminal state.
    ///
    /// The right tool when several sessions share one channel: a per-id drain
    /// ([`drain_to_terminal`]) would discard — and thereby lose — another
    /// session's terminal event while scanning for the first, wedging the
    /// second drain forever.
    async fn drain_all_to_terminal(
        rx: &mut UnboundedReceiver<SessionEvent>,
        ids: &[SessionId],
    ) -> HashMap<SessionId, (Vec<String>, SessionState)> {
        let mut lines: HashMap<SessionId, Vec<String>> =
            ids.iter().map(|id| (*id, Vec::new())).collect();
        let mut terminals: HashMap<SessionId, SessionState> = HashMap::new();
        while terminals.len() < ids.len() {
            let ev = timeout(RECV_TIMEOUT, rx.recv())
                .await
                .expect("timed out waiting for a session event")
                .expect("event channel closed unexpectedly");
            match ev.kind {
                SessionEventKind::Line(l) => {
                    if let Some(v) = lines.get_mut(&ev.id) {
                        v.push(l);
                    }
                }
                SessionEventKind::State(s) if s.is_terminal() => {
                    terminals.insert(ev.id, s);
                }
                SessionEventKind::State(_) => {}
            }
        }
        ids.iter()
            .map(|id| {
                (
                    *id,
                    (lines.remove(id).unwrap(), terminals.remove(id).unwrap()),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn session_reaches_running_and_lines_arrive_in_order() {
        let runner = FakeProcessRunner::new().with_stream(
            "cargo run",
            [
                "   Compiling app v0.1.0",
                "    Finished dev",
                "     Running `target/debug/app`",
                "hello from the app",
            ],
            true,
        );
        let (mut sup, mut rx) = Supervisor::new(Arc::new(runner));
        let id = sup.start_with_plan(plan("cargo", &["run"])).unwrap();

        // First event is always the initial Configuring state.
        assert_eq!(
            next_for(&mut rx, id).await.kind,
            SessionEventKind::State(SessionState::Configuring)
        );

        let (lines, terminal) = drain_to_terminal(&mut rx, id).await;
        assert_eq!(
            lines,
            vec![
                "   Compiling app v0.1.0".to_string(),
                "    Finished dev".to_string(),
                "     Running `target/debug/app`".to_string(),
                "hello from the app".to_string(),
            ]
        );
        assert_eq!(terminal, SessionState::Exited(true));
    }

    #[tokio::test]
    async fn state_machine_walks_the_build_phases() {
        let runner = FakeProcessRunner::new().with_stream(
            "gradlew run",
            [
                "Building `it.f0x.app`…",
                "Installing on Pixel 7…",
                "Launching it.f0x.app…",
            ],
            true,
        );
        let (mut sup, mut rx) = Supervisor::new(Arc::new(runner));
        let id = sup.start_with_plan(plan("gradlew", &["run"])).unwrap();

        let mut states = Vec::new();
        loop {
            let ev = next_for(&mut rx, id).await;
            if let SessionEventKind::State(s) = ev.kind {
                let terminal = s.is_terminal();
                states.push(s);
                if terminal {
                    break;
                }
            }
        }
        assert_eq!(
            states,
            vec![
                SessionState::Configuring,
                SessionState::Building,
                SessionState::Installing,
                SessionState::Running,
                SessionState::Exited(true),
            ]
        );
    }

    #[tokio::test]
    async fn stop_kills_a_hung_session_promptly() {
        let runner =
            FakeProcessRunner::new().with_hanging_stream("cargo run", ["     Running `app`"]);
        let (mut sup, mut rx) = Supervisor::new(Arc::new(runner));
        let id = sup.start_with_plan(plan("cargo", &["run"])).unwrap();

        // Wait until the session is actually Running before stopping it.
        loop {
            let ev = next_for(&mut rx, id).await;
            if ev.kind == SessionEventKind::State(SessionState::Running) {
                break;
            }
        }

        let started = std::time::Instant::now();
        sup.stop(id);

        // The terminal state must be Killed, delivered well within the bound.
        let (_lines, terminal) = drain_to_terminal(&mut rx, id).await;
        assert_eq!(terminal, SessionState::Killed);
        assert!(
            started.elapsed() < PROMPT_STOP,
            "stop() should terminate a hung session promptly, took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn concurrent_sessions_do_not_cross_streams() {
        let runner = FakeProcessRunner::new()
            .with_stream("cargo run", ["desktop-a", "     Running `a`"], true)
            .with_stream("adb logcat", ["device-b", "Launching b…"], true);
        let (mut sup, mut rx) = Supervisor::new(Arc::new(runner));

        let a = sup.start_with_plan(plan("cargo", &["run"])).unwrap();
        let b = sup.start_with_plan(plan("adb", &["logcat"])).unwrap();
        assert_ne!(a, b);

        let results = drain_all_to_terminal(&mut rx, &[a, b]).await;
        let (a_lines, a_term) = &results[&a];
        let (b_lines, b_term) = &results[&b];

        // Each session's lines belong only to it — no cross-contamination.
        assert_eq!(
            *a_lines,
            vec!["desktop-a".to_string(), "     Running `a`".to_string()]
        );
        assert_eq!(
            *b_lines,
            vec!["device-b".to_string(), "Launching b…".to_string()]
        );
        assert_eq!(*a_term, SessionState::Exited(true));
        assert_eq!(*b_term, SessionState::Exited(true));
    }

    #[tokio::test]
    async fn failed_exit_surfaces_as_unsuccessful() {
        let runner = FakeProcessRunner::new().with_stream("cargo run", ["boom"], false);
        let (mut sup, mut rx) = Supervisor::new(Arc::new(runner));
        let id = sup.start_with_plan(plan("cargo", &["run"])).unwrap();
        let (_lines, terminal) = drain_to_terminal(&mut rx, id).await;
        assert_eq!(terminal, SessionState::Exited(false));
    }

    #[tokio::test]
    async fn double_stop_is_a_noop() {
        let runner = FakeProcessRunner::new().with_hanging_stream("cargo run", Vec::<&str>::new());
        let (mut sup, mut rx) = Supervisor::new(Arc::new(runner));
        let id = sup.start_with_plan(plan("cargo", &["run"])).unwrap();

        sup.stop(id);
        let (_lines, terminal) = drain_to_terminal(&mut rx, id).await;
        assert_eq!(terminal, SessionState::Killed);
        // A second stop after the session already terminated must not panic.
        sup.stop(id);
    }

    #[tokio::test]
    async fn start_errors_on_unregistered_invocation() {
        let runner = FakeProcessRunner::new();
        let (mut sup, _rx) = Supervisor::new(Arc::new(runner));
        assert!(sup.start_with_plan(plan("cargo", &["run"])).is_err());
    }

    #[tokio::test]
    async fn stop_all_kills_every_session() {
        let runner = FakeProcessRunner::new()
            .with_hanging_stream("cargo run", Vec::<&str>::new())
            .with_hanging_stream("adb logcat", Vec::<&str>::new());
        let (mut sup, mut rx) = Supervisor::new(Arc::new(runner));
        let a = sup.start_with_plan(plan("cargo", &["run"])).unwrap();
        let b = sup.start_with_plan(plan("adb", &["logcat"])).unwrap();

        sup.stop_all();

        let results = drain_all_to_terminal(&mut rx, &[a, b]).await;
        assert_eq!(results[&a].1, SessionState::Killed);
        assert_eq!(results[&b].1, SessionState::Killed);
    }
}
