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
//! still feeding the async channel through `try_send` — a non-blocking,
//! runtime-free call. This is the "spawn_blocking / std thread" split PLAN D2
//! calls for.
//!
//! # Bounded channel + overflow policy (drop-newest, counted)
//!
//! The engine channel is **bounded** ([`SESSION_CHANNEL_CAP`]) rather than
//! unbounded, so a runaway session can never grow it without limit. The drain
//! threads send with [`Sender::try_send`] **only** — never a blocking send —
//! so a full channel (a stalled/behind engine) can never wedge a drain
//! thread, and [`Supervisor`]'s `Drop` join stays prompt. On a full channel a
//! line batch is **dropped (newest first)** and its lines are added to a
//! per-session cumulative counter, surfaced to the engine as
//! [`SessionEventKind::Dropped`] the moment the channel has room again — the
//! same drop-oldest-vs-drop-newest tradeoff `frust-drive`'s
//! [`LineReceiver`] ring makes one layer down, mirrored here for the second
//! hop. Coalescing (a burst becomes one [`SessionEventKind::Lines`] batch)
//! keeps the channel near-empty in practice, so an overflow only happens under
//! a pathological flood against a wedged consumer.
//!
//! # Kill boundary (PLAN D3's known limitation)
//!
//! `stop` is prompt for anything spawned through `spawn_streaming` (the
//! desktop `cargo run` preview, or a device session's logcat/console streaming
//! phase): killing the child closes its stdout pipe, which unblocks the drain
//! loop immediately. Once a session is streaming, the stop→kill→EOF path is
//! **unconditional** — there is no window in which a `stop` can be observed
//! yet the just-installed stream keeps running: the device stream is installed
//! and rechecked against the cancel flag under a single continuous lock hold
//! (see [`DeviceControl::install_logcat`]), and `stop`'s kill takes that same
//! lock. A device session's multi-phase drive pipeline (build →
//! install → launch → logcat) is only *promptly* killable once it reaches its
//! streaming phase — a kill requested mid-Gradle sets the pipeline's cancel
//! flag, which the drive checks at each phase boundary, so it takes effect at
//! the next boundary (a blocking Gradle/xcodebuild phase can't be interrupted
//! mid-flight), not instantly. The device path is driven by
//! [`Supervisor::start_device`] over `frust-drive`'s `android_run`/`ios_run`
//! cancellable `spawn_session` seams; the desktop path is fully killable.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result};
use frust_drive::devices::{Kind, Platform};
use frust_drive::process::{LineReceiver, ProcessRunner, StreamHandle};
use frust_drive::{android_run, ios_run};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{self, Receiver, Sender};

use super::session::{
    DevicePlan, DeviceTarget, LaunchPlan, SessionEvent, SessionEventKind, SessionId, SessionSpec,
    SessionState, infer_state,
};

/// Bounded capacity of the single engine event channel every session feeds.
///
/// Low-hundreds by design: each slot carries a whole coalesced
/// [`SessionEventKind::Lines`] batch (a burst of output), not a single line,
/// so this bounds the number of *bursts* in flight rather than lines — deep
/// enough to absorb many concurrent sessions' bursts against a momentarily
/// behind engine, small enough that a wedged consumer can't let memory grow
/// without limit. Overflow is drop-newest + counted, never a blocking send
/// (see the module docs' overflow policy).
const SESSION_CHANNEL_CAP: usize = 256;

/// One live (or terminated) session's supervisor-side bookkeeping.
struct SessionEntry {
    /// How `stop` terminates this session — a single streaming child (desktop
    /// `cargo run`) or a multi-phase device pipeline.
    killer: Killer,
    /// The supervision thread; joined on `Drop` so a torn-down supervisor
    /// never leaks a running supervision thread.
    worker: Option<JoinHandle<()>>,
}

/// The kill path for a session, differing by launch shape.
enum Killer {
    /// A single streaming child (the desktop `cargo run` preview, or any
    /// `LaunchPlan`): killing closes its stdout pipe and unblocks the drain.
    Stream {
        /// The drive's kill handle, shared with the drain thread (which reaps
        /// the child at EOF). `stop` locks it briefly to send the kill signal.
        handle: Arc<Mutex<StreamHandle>>,
        /// Set by `stop` before killing, so the drain thread reports
        /// [`Killed`](SessionState::Killed) rather than
        /// [`Exited`](SessionState::Exited) at EOF.
        killed: Arc<AtomicBool>,
    },
    /// A multi-phase device pipeline (build → install → launch → logcat).
    /// `stop` sets its cancel flag (abandoning the pipeline at the next phase
    /// boundary) and kills the logcat stream if it has begun.
    Device(Arc<DeviceControl>),
}

/// The cancel primitive for a supervised device session. `stop` sets `cancel`
/// (checked between the drive pipeline's phases — a stop mid-build takes
/// effect at the next boundary, matching the module doc's kill boundary) and
/// kills the logcat [`StreamHandle`] once the pipeline has reached its
/// streaming phase (prompt from there on).
struct DeviceControl {
    cancel: AtomicBool,
    /// The logcat/console stream, installed by the device worker once the
    /// build/install/launch core completes; `None` until then.
    logcat: Mutex<Option<StreamHandle>>,
}

impl DeviceControl {
    fn new() -> Self {
        Self {
            cancel: AtomicBool::new(false),
            logcat: Mutex::new(None),
        }
    }

    /// Request cancellation: flag the pipeline and, if streaming has begun,
    /// group-kill the logcat stream (prompt). Idempotent.
    fn stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(handle) = lock_logcat(&self.logcat).as_mut() {
            handle.kill();
        }
    }

    /// Install the just-started logcat/console stream and hand back its line
    /// receiver — the single-lock-hold install+recheck that closes the
    /// device-stop race (G2).
    ///
    /// Under **one continuous hold** of the same `logcat` mutex `stop`'s kill
    /// takes: store the handle, then re-check `cancel`; if a `stop` already set
    /// it (its kill found an empty slot because the stream didn't exist yet),
    /// kill the just-stored handle now. This leaves no window in which a
    /// `stop` is observed yet the stream keeps running — the two orderings are
    /// (a) `stop` first: it sets `cancel`, finds no handle, returns; this
    /// install then sees `cancel` under the lock and kills; (b) install first:
    /// it stores the handle and sees `cancel` clear; `stop` then takes the
    /// lock and kills the present handle. Either way the stream is killed
    /// exactly once and the drain sees EOF promptly.
    fn install_logcat(&self, handle: StreamHandle) -> LineReceiver {
        let mut slot = lock_logcat(&self.logcat);
        let lines = handle.lines.clone();
        *slot = Some(handle);
        if self.cancel.load(Ordering::SeqCst)
            && let Some(installed) = slot.as_mut()
        {
            installed.kill();
        }
        lines
    }
}

/// Owns every supervised session and the single event channel they feed.
///
/// Construct with [`Supervisor::new`], which hands back the bounded
/// [`Receiver`] the engine drains. Start sessions with
/// [`Supervisor::start`] (a [`SessionSpec`]) or [`Supervisor::start_with_plan`]
/// (a purpose-built [`LaunchPlan`]); stop them with [`Supervisor::stop`] /
/// [`Supervisor::stop_all`].
pub struct Supervisor {
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    events_tx: Sender<SessionEvent>,
    next_id: u64,
    sessions: HashMap<SessionId, SessionEntry>,
}

impl Supervisor {
    /// Build a supervisor over `runner`, returning it paired with the event
    /// receiver the engine `select!`s on. The channel is bounded
    /// ([`SESSION_CHANNEL_CAP`]) with a drop-newest overflow policy (module
    /// docs). Production passes
    /// `Arc::new(frust_drive::process::RealProcessRunner)`; tests pass an
    /// `Arc<FakeProcessRunner>` scripted with `with_stream`/
    /// `with_hanging_stream`.
    pub fn new(runner: Arc<dyn ProcessRunner + Send + Sync>) -> (Self, Receiver<SessionEvent>) {
        let (events_tx, events_rx) = mpsc::channel(SESSION_CHANNEL_CAP);
        let supervisor = Self {
            runner,
            events_tx,
            next_id: 0,
            sessions: HashMap::new(),
        };
        (supervisor, events_rx)
    }

    /// Start a session from a [`SessionSpec`], dispatching on its target: a
    /// [`DeviceTarget::Desktop`] resolves to a [`LaunchPlan`] and spawns a
    /// single streaming `cargo run` child; a [`DeviceTarget::Device`] drives
    /// the multi-phase device pipeline (build → install → launch → logcat) on
    /// its own supervision thread (see [`Supervisor::start_device`]). Errors
    /// if the process/thread fails to spawn.
    pub fn start(&mut self, spec: &SessionSpec) -> Result<SessionId> {
        match &spec.target {
            DeviceTarget::Desktop => {
                let plan = spec.launch_plan()?;
                self.start_with_plan(plan)
            }
            DeviceTarget::Device(device) => self.start_device(DevicePlan {
                project_root: spec.project_root.clone(),
                device: device.clone(),
                build: spec.build.clone(),
            }),
        }
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
                killer: Killer::Stream { handle, killed },
                worker: Some(worker),
            },
        );
        self.next_id += 1;
        Ok(id)
    }

    /// Start a supervised **device** session from a resolved [`DevicePlan`]:
    /// the multi-phase drive pipeline (build → install → launch → logcat, per
    /// the plan's `device` platform/kind) runs on a dedicated std thread,
    /// bridging every phase line — and the inferred [`SessionState`] changes —
    /// into the same engine channel a desktop session feeds. The returned
    /// [`SessionId`] tags every event; [`Supervisor::stop`] cancels the
    /// pipeline (prompt once streaming, at the next phase boundary before it —
    /// see [`DeviceControl`] and the module doc's kill boundary).
    pub fn start_device(&mut self, plan: DevicePlan) -> Result<SessionId> {
        let id = SessionId(self.next_id);
        let control = Arc::new(DeviceControl::new());
        let runner = Arc::clone(&self.runner);
        let events = self.events_tx.clone();

        let worker = {
            let control = Arc::clone(&control);
            thread::Builder::new()
                .name(format!("frust-tui-device-{}", id.0))
                .spawn(move || run_device_session(id, plan, runner, control, events))
                .context("spawning device session supervision thread")?
        };

        self.sessions.insert(
            id,
            SessionEntry {
                killer: Killer::Device(control),
                worker: Some(worker),
            },
        );
        self.next_id += 1;
        Ok(id)
    }

    /// Stop a session. For a streaming (desktop) session: mark it killed, then
    /// group-kill its process — the drain thread observes the closed stdout
    /// pipe, reaps the child, and emits a final [`SessionState::Killed`]. For
    /// a device session: cancel the pipeline and kill its logcat stream if
    /// streaming. Idempotent and safe on an already-exited session; an unknown
    /// id is ignored.
    pub fn stop(&mut self, id: SessionId) {
        if let Some(entry) = self.sessions.get(&id) {
            match &entry.killer {
                Killer::Stream { handle, killed } => {
                    killed.store(true, Ordering::SeqCst);
                    lock(handle).kill();
                }
                Killer::Device(control) => control.stop(),
            }
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

/// A drain thread's non-blocking sender into the bounded engine channel,
/// enforcing the drop-newest overflow policy (module docs) and the per-session
/// dropped-line accounting.
///
/// Every send goes through [`Sender::try_send`] — never a blocking send — so a
/// full channel can never wedge a drain thread and [`Supervisor`]'s `Drop`
/// join stays prompt. A failed `Ok`-path send returns `Err(())`, the "engine
/// dropped the receiver, stop" signal every caller bails on.
struct SessionSender {
    tx: Sender<SessionEvent>,
    id: SessionId,
    /// Cumulative output lines dropped so far because the channel was full.
    dropped: u64,
    /// The last `dropped` value actually delivered as a
    /// [`SessionEventKind::Dropped`], so we only re-report an increase.
    reported: u64,
}

impl SessionSender {
    fn new(tx: Sender<SessionEvent>, id: SessionId) -> Self {
        Self {
            tx,
            id,
            dropped: 0,
            reported: 0,
        }
    }

    /// Send one coalesced batch. On a full channel, drop it (newest first) and
    /// add its lines to the cumulative dropped counter; on success, flush any
    /// pending dropped count. An empty batch only flushes. `Err(())` once the
    /// receiver is gone.
    fn send_lines(&mut self, lines: Vec<String>) -> Result<(), ()> {
        if lines.is_empty() {
            return self.flush_dropped();
        }
        let n = lines.len() as u64;
        match self.tx.try_send(SessionEvent {
            id: self.id,
            kind: SessionEventKind::Lines(lines),
        }) {
            Ok(()) => self.flush_dropped(),
            Err(TrySendError::Full(_)) => {
                self.dropped += n;
                Ok(())
            }
            Err(TrySendError::Closed(_)) => Err(()),
        }
    }

    /// Send a lifecycle state change. Like every send it is non-blocking
    /// (`try_send`); a state lost to a full channel is possible only under the
    /// pathological flood the module docs describe (batch coalescing keeps the
    /// channel near-empty otherwise), and is preferred over ever blocking the
    /// drain thread — which would stall `Drop`'s join. `Err(())` once the
    /// receiver is gone.
    fn send_state(&mut self, state: SessionState) -> Result<(), ()> {
        match self.tx.try_send(SessionEvent {
            id: self.id,
            kind: SessionEventKind::State(state),
        }) {
            Ok(()) => self.flush_dropped(),
            Err(TrySendError::Full(_)) => Ok(()),
            Err(TrySendError::Closed(_)) => Err(()),
        }
    }

    /// If more lines have been dropped than last reported, try (non-blocking)
    /// to deliver the new cumulative count. A full channel leaves it pending
    /// for the next successful send; `Err(())` once the receiver is gone.
    fn flush_dropped(&mut self) -> Result<(), ()> {
        if self.dropped > self.reported {
            match self.tx.try_send(SessionEvent {
                id: self.id,
                kind: SessionEventKind::Dropped(self.dropped),
            }) {
                Ok(()) => self.reported = self.dropped,
                Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Closed(_)) => return Err(()),
            }
        }
        Ok(())
    }
}

/// Send one coalesced batch of lines and, for every phase transition the batch
/// announces (forward-only, across the batch in order), the new state —
/// keeping the desktop drain and the device pipeline/logcat feeds identical.
/// Lines go out first, then any state transitions, matching the engine's
/// "line then glyph" ordering. Returns `Err(())` once the engine has dropped
/// the receiver.
fn feed_lines(
    sender: &mut SessionSender,
    state: &mut SessionState,
    batch: Vec<String>,
) -> Result<(), ()> {
    let mut transitions = Vec::new();
    for line in &batch {
        if let Some(next) = infer_state(state, line) {
            *state = next.clone();
            transitions.push(next);
        }
    }
    sender.send_lines(batch)?;
    for t in transitions {
        sender.send_state(t)?;
    }
    Ok(())
}

/// Drain a [`LineReceiver`] to EOF, coalescing each burst into one batch: block
/// on `recv` for the next line, then drain everything already buffered via
/// `try_recv` into the same batch before sending. `try_recv`'s natural
/// emptying is also the cancel-observation point — a killed stream closes,
/// unblocking the `recv`. Returns `Ok(())` at EOF (the stream closed) or
/// `Err(())` if the engine dropped the receiver mid-drain (the caller then
/// bails without emitting a terminal state).
fn drain_receiver(
    sender: &mut SessionSender,
    state: &mut SessionState,
    lines: &LineReceiver,
) -> Result<(), ()> {
    while let Ok(first) = lines.recv() {
        let mut batch = vec![first];
        while let Ok(line) = lines.try_recv() {
            batch.push(line);
        }
        feed_lines(sender, state, batch)?;
    }
    Ok(())
}

/// The per-session drain loop, run on its own std thread: forward every line
/// (coalesced into batches) and inferred state change into the engine channel,
/// then reap the child and emit the terminal state.
fn drain_session(
    id: SessionId,
    lines: LineReceiver,
    handle: Arc<Mutex<StreamHandle>>,
    killed: Arc<AtomicBool>,
    events: Sender<SessionEvent>,
) {
    let mut sender = SessionSender::new(events, id);
    let mut state = SessionState::Configuring;
    // The engine dropping the receiver (shutdown) makes every send fail; bail
    // out immediately when that happens — there is nothing left to report to,
    // and the child is reaped via the handle's own drop-detach path.
    if sender.send_state(state.clone()).is_err() {
        return;
    }

    if drain_receiver(&mut sender, &mut state, &lines).is_err() {
        return;
    }

    // stdout hit EOF: the process exited on its own or a `stop` killed it.
    // `wait` reaps it (idempotent with the kill path) and yields exit success.
    let success = lock(&handle).wait();
    let terminal = if killed.load(Ordering::SeqCst) {
        SessionState::Killed
    } else {
        SessionState::Exited(success)
    };
    let _ = sender.send_state(terminal);
}

/// Lock a device session's logcat slot, recovering from a poisoned mutex the
/// same way [`lock`] does for the streaming kill handle.
fn lock_logcat(
    slot: &Mutex<Option<StreamHandle>>,
) -> std::sync::MutexGuard<'_, Option<StreamHandle>> {
    slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The per-device-session pipeline loop, run on its own std thread: drive the
/// drive's multi-phase device pipeline (feeding phase lines through
/// [`feed_lines`]), then — once it hands back the logcat/console stream —
/// install it under the stop-race-closing lock hold
/// ([`DeviceControl::install_logcat`]) and drain it the same way
/// [`drain_session`] drains a desktop child, and emit the terminal state. A
/// cancellation observed before streaming (or a killed stream) reports
/// [`SessionState::Killed`]; a pipeline error reports the error as a line then
/// [`SessionState::Exited(false)`].
fn run_device_session(
    id: SessionId,
    plan: DevicePlan,
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    control: Arc<DeviceControl>,
    events: Sender<SessionEvent>,
) {
    let mut sender = SessionSender::new(events, id);
    let mut state = SessionState::Configuring;
    if sender.send_state(state.clone()).is_err() {
        return;
    }

    // Run build → install → launch, streaming phase lines; the closure borrows
    // `sender`/`state` only for the pipeline's duration (it returns the logcat
    // handle, after which both are used again for the drain below).
    let pipeline = {
        let sender = &mut sender;
        let state = &mut state;
        let mut on_line = |line: &str| {
            let _ = feed_lines(sender, state, vec![line.to_string()]);
        };
        launch_device_stream(&plan, runner.as_ref(), &control.cancel, &mut on_line)
    };

    let terminal = match pipeline {
        Ok(Some(handle)) => {
            // Install the stream under one continuous lock hold, closing the
            // stop race (G2): a `stop` observed just before or after this
            // point kills the stream exactly once, so the drain always sees
            // EOF rather than blocking forever (see `install_logcat`).
            let lines = control.install_logcat(handle);

            if drain_receiver(&mut sender, &mut state, &lines).is_err() {
                return;
            }

            // Reap the logcat child (idempotent with `stop`'s kill).
            let success = lock_logcat(&control.logcat)
                .as_mut()
                .map(StreamHandle::wait)
                .unwrap_or(false);
            if control.cancel.load(Ordering::SeqCst) {
                SessionState::Killed
            } else {
                SessionState::Exited(success)
            }
        }
        // Cancelled before the streaming phase began.
        Ok(None) => SessionState::Killed,
        Err(err) => {
            let _ = sender.send_lines(vec![format!("error: {err:#}")]);
            SessionState::Exited(false)
        }
    };
    let _ = sender.send_state(terminal);
}

/// Dispatch the drive's cancellable device pipeline by the plan's device
/// platform/kind, returning the logcat/console [`StreamHandle`] to drain
/// (`Ok(Some)`), a cancellation before streaming (`Ok(None)`), or a pipeline
/// error (`Err`).
fn launch_device_stream(
    plan: &DevicePlan,
    runner: &dyn ProcessRunner,
    cancel: &AtomicBool,
    on_line: &mut dyn FnMut(&str),
) -> Result<Option<StreamHandle>> {
    match (plan.device.platform, plan.device.kind) {
        (Platform::Android, _) => android_run::spawn_session(
            runner,
            &plan.project_root,
            &plan.device,
            &plan.build,
            on_line,
            cancel,
        ),
        (Platform::Ios, Kind::PhysicalDevice) => ios_run::spawn_physical_session(
            runner,
            &plan.project_root,
            &plan.device,
            &plan.build,
            on_line,
            cancel,
        ),
        (Platform::Ios, _) => ios_run::spawn_session(
            runner,
            &plan.project_root,
            &plan.device,
            &plan.build,
            on_line,
            cancel,
        ),
    }
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
    async fn next_for(rx: &mut Receiver<SessionEvent>, want_id: SessionId) -> SessionEvent {
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
        rx: &mut Receiver<SessionEvent>,
        want_id: SessionId,
    ) -> (Vec<String>, SessionState) {
        let mut lines = Vec::new();
        loop {
            let ev = next_for(rx, want_id).await;
            match ev.kind {
                SessionEventKind::Lines(mut ls) => lines.append(&mut ls),
                SessionEventKind::Dropped(_) => {}
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
        rx: &mut Receiver<SessionEvent>,
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
                SessionEventKind::Lines(mut ls) => {
                    if let Some(v) = lines.get_mut(&ev.id) {
                        v.append(&mut ls);
                    }
                }
                SessionEventKind::Dropped(_) => {}
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

    // ── Target dispatch (TUI2-04) ───────────────────────────────────────────

    use frust_drive::build_info::{BuildInfo, BuildMode};
    use frust_drive::devices::{Device, Kind, Platform};

    fn desktop_spec(root: &str) -> SessionSpec {
        SessionSpec {
            project_root: PathBuf::from(root),
            target: DeviceTarget::Desktop,
            build: BuildInfo {
                mode: BuildMode::Debug,
                flavor: None,
                defines: HashMap::new(),
                build_name: None,
                build_number: None,
            },
        }
    }

    /// `start` dispatches a desktop spec to the single-stream path; N specs →
    /// N concurrent sessions, each terminating independently.
    #[tokio::test]
    async fn start_launches_a_desktop_session_per_spec() {
        let runner = FakeProcessRunner::new().with_stream(
            "cargo run",
            ["   Compiling app", "     Running `app`", "hello"],
            true,
        );
        let (mut sup, mut rx) = Supervisor::new(Arc::new(runner));
        let a = sup.start(&desktop_spec("/tmp/a")).unwrap();
        let b = sup.start(&desktop_spec("/tmp/b")).unwrap();
        assert_ne!(a, b);

        let results = drain_all_to_terminal(&mut rx, &[a, b]).await;
        assert_eq!(results[&a].1, SessionState::Exited(true));
        assert_eq!(results[&b].1, SessionState::Exited(true));
        // Each session saw the run's lines (its own copy of the stream).
        assert!(results[&a].0.iter().any(|l| l == "hello"));
    }

    /// `start` dispatches a device spec to the multi-phase pipeline. With a
    /// project root that has no `frust.toml`, the drive pipeline errors at
    /// project detection — the device worker surfaces the error as a line and
    /// terminates `Exited(false)` (no real `adb`/env needed; the plumbing is
    /// what's under test).
    #[tokio::test]
    async fn start_device_surfaces_a_pipeline_error_as_a_line() {
        let device = Device {
            id: "emulator-5554".into(),
            name: "Pixel 7".into(),
            platform: Platform::Android,
            kind: Kind::Emulator,
            os_version: None,
            connection_state: None,
        };
        let spec = SessionSpec {
            project_root: PathBuf::from("/frust-tui-nonexistent-project-xyz"),
            target: DeviceTarget::Device(device),
            build: BuildInfo {
                mode: BuildMode::Debug,
                flavor: None,
                defines: HashMap::new(),
                build_name: None,
                build_number: None,
            },
        };
        let (mut sup, mut rx) = Supervisor::new(Arc::new(FakeProcessRunner::new()));
        let id = sup.start(&spec).unwrap();

        let (lines, terminal) = drain_to_terminal(&mut rx, id).await;
        assert_eq!(terminal, SessionState::Exited(false));
        assert!(
            lines.iter().any(|l| l.starts_with("error:")),
            "expected a surfaced pipeline error line, got {lines:?}"
        );
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

    // ── G2: device-stop race (install+recheck under one lock) ───────────────

    /// Spawn a hanging fake stream directly and hand back its [`StreamHandle`],
    /// the raw material for the install-race tests below.
    fn hanging_handle(key_cmd: &str, key_args: &[&str]) -> StreamHandle {
        let runner = FakeProcessRunner::new()
            .with_hanging_stream(invocation(key_cmd, key_args), ["streaming…"]);
        runner
            .spawn_streaming(key_cmd, key_args, None, &[])
            .expect("fake hanging stream spawns")
    }

    fn invocation(cmd: &str, args: &[&str]) -> String {
        std::iter::once(cmd)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The classic G2 gap: a `stop` arrives *before* the logcat handle exists,
    /// so its kill finds an empty slot and sets only `cancel`. `install_logcat`
    /// must — under the same lock — observe `cancel` and kill the
    /// just-installed stream, so its drain sees EOF promptly instead of
    /// blocking forever. No second `stop` is needed.
    #[test]
    fn install_logcat_closes_the_stop_race_when_stop_arrives_first() {
        let handle = hanging_handle("adb", &["logcat"]);
        let control = DeviceControl::new();

        // stop races ahead of the stream: cancel set, empty slot, nothing killed.
        control.stop();
        assert!(control.cancel.load(Ordering::SeqCst));

        // Install under one continuous lock hold — it must kill on seeing cancel.
        let lines = control.install_logcat(handle);

        let (tx, _rx) = mpsc::channel(SESSION_CHANNEL_CAP);
        let mut sender = SessionSender::new(tx, SessionId(0));
        let mut state = SessionState::Running;
        let started = std::time::Instant::now();
        drain_receiver(&mut sender, &mut state, &lines).expect("receiver stays connected");
        assert!(
            started.elapsed() < PROMPT_STOP,
            "a stream killed by install_logcat must drain to EOF promptly, took {:?}",
            started.elapsed()
        );
    }

    /// The reverse ordering: install first (cancel still clear, stream left
    /// running), then `stop` kills the now-installed stream. Draining is prompt
    /// and the double path (install's recheck + stop's kill) never
    /// double-kills into a panic.
    #[test]
    fn stop_kills_a_stream_installed_before_it() {
        let handle = hanging_handle("adb", &["logcat"]);
        let control = Arc::new(DeviceControl::new());

        let lines = control.install_logcat(handle);
        assert!(!control.cancel.load(Ordering::SeqCst), "not cancelled yet");

        control.stop(); // now kills the installed stream

        let (tx, _rx) = mpsc::channel(SESSION_CHANNEL_CAP);
        let mut sender = SessionSender::new(tx, SessionId(0));
        let mut state = SessionState::Running;
        let started = std::time::Instant::now();
        drain_receiver(&mut sender, &mut state, &lines).expect("receiver stays connected");
        assert!(
            started.elapsed() < PROMPT_STOP,
            "a stopped stream must drain to EOF promptly, took {:?}",
            started.elapsed()
        );
    }

    // ── G6: batch coalescing + bounded-channel overflow ─────────────────────

    /// A burst of buffered lines drains into exactly ONE coalesced
    /// [`SessionEventKind::Lines`] batch — the whole point of the recv-then-
    /// try_recv coalescing loop. Deterministic: `wait` joins the fake
    /// producer, so every line is buffered (and the stream closed) before the
    /// drain begins.
    #[test]
    fn a_buffered_burst_coalesces_into_one_lines_batch() {
        let runner = FakeProcessRunner::new().with_stream(
            "cargo run",
            ["   Compiling app", "     Running `app`", "hello", "world"],
            true,
        );
        let mut handle = runner
            .spawn_streaming("cargo", &["run"], None, &[])
            .expect("fake stream spawns");
        let lines = handle.lines.clone();
        handle.wait(); // all lines pushed + buffer closed → a fully-buffered burst

        let (tx, mut rx) = mpsc::channel(SESSION_CHANNEL_CAP);
        let mut sender = SessionSender::new(tx, SessionId(0));
        let mut state = SessionState::Configuring;
        drain_receiver(&mut sender, &mut state, &lines).expect("receiver stays connected");
        drop(sender);

        let mut batches: Vec<Vec<String>> = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let SessionEventKind::Lines(ls) = ev.kind {
                batches.push(ls);
            }
        }
        assert_eq!(
            batches.len(),
            1,
            "a fully-buffered burst coalesces into one batch, got {batches:?}"
        );
        assert_eq!(
            batches[0],
            vec![
                "   Compiling app".to_string(),
                "     Running `app`".to_string(),
                "hello".to_string(),
                "world".to_string(),
            ]
        );
    }

    /// Overflow policy: with the consumer paused, a full bounded channel drops
    /// the newest batches (never blocking the sender) and counts their lines;
    /// the cumulative count surfaces as a [`SessionEventKind::Dropped`] the
    /// moment there is room again. This is the drain thread's non-blocking
    /// guarantee (Drop's join stays prompt) exercised at the send primitive.
    #[test]
    fn a_full_channel_drops_newest_batches_and_counts_them() {
        // A tiny channel so we can fill it deterministically; consumer paused.
        let (tx, mut rx) = mpsc::channel(2);
        let mut sender = SessionSender::new(tx, SessionId(7));

        // Fill both slots with the two oldest batches.
        sender.send_lines(vec!["a".into()]).expect("connected");
        sender.send_lines(vec!["b".into()]).expect("connected");

        // Full now: the newest batches are dropped, their lines counted — and
        // crucially each call RETURNS (never blocks).
        sender
            .send_lines(vec!["c1".into(), "c2".into()])
            .expect("connected");
        sender.send_lines(vec!["d".into()]).expect("connected");
        assert_eq!(sender.dropped, 3, "c1, c2, d were dropped while full");

        // Drop-newest, not drop-oldest: the two oldest batches survived.
        let first = rx.try_recv().expect("a batch is queued");
        assert_eq!(first.kind, SessionEventKind::Lines(vec!["a".into()]));

        // With room again, the next send flushes the cumulative dropped count.
        sender.send_lines(Vec::new()).expect("connected");
        let mut dropped_seen = None;
        while let Ok(ev) = rx.try_recv() {
            if let SessionEventKind::Dropped(n) = ev.kind {
                dropped_seen = Some(n);
            }
        }
        assert_eq!(
            dropped_seen,
            Some(3),
            "the cumulative dropped count surfaces once the channel has room"
        );
    }

    /// A closed receiver (engine gone) turns every send into the `Err(())`
    /// "stop draining" signal — the drain loops bail rather than spin.
    #[test]
    fn sends_report_the_receiver_going_away() {
        let (tx, rx) = mpsc::channel(SESSION_CHANNEL_CAP);
        let mut sender = SessionSender::new(tx, SessionId(1));
        drop(rx);
        assert_eq!(sender.send_lines(vec!["x".into()]), Err(()));
        assert_eq!(sender.send_state(SessionState::Running), Err(()));
    }
}
