//! One supervised app session: its identity, its observable state, and the
//! background resources ([`crate::engine`] threads, a `StreamHandle`, a
//! devtools connection, a metrics sampler) that have to be torn down with it.
//!
//! # Locking contract
//!
//! A [`Session`] holds exactly three independent locks and they are **never**
//! nested:
//!
//! - `data` — everything a [`SessionSnapshot`] reports, plus the two rings
//!   and the sending halves of the log subscription and the session-event
//!   feed. Every mutation notifies the paired `Condvar`, which is what
//!   [`Session::wait_for`] blocks on, so a caller waiting for "connected" or
//!   "N frames arrived" waits on a condition the engine itself produces
//!   rather than polling a clock. Both feeds live here and nowhere else
//!   *because* the log ring does: seeding a new subscriber and registering
//!   its feed happen in the same critical section as an ingest, which is what
//!   makes the seam gapless (see [`Session::subscribe_logs`] and
//!   [`Session::subscribe_events`]).
//! - `resources` — the teardown-only handles. Held for a short take/insert
//!   only; **never** held while joining a thread, since a session thread may
//!   itself be waiting to lock it. It is also the lock that orders
//!   registration against teardown: every registrar re-reads the stop flag
//!   *inside* this critical section, and teardown sets that flag strictly
//!   before it takes the lock, so a handle is either stored (and torn down)
//!   or handed straight back to its registrar (see
//!   [`Session::set_stream`]).
//! - `devtools` — the connected client. Callers clone the `Arc` out and drop
//!   the lock before issuing a (blocking, timeout-bounded) request.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{
    self, Receiver, RecvError, RecvTimeoutError, SyncSender, TryRecvError, TrySendError,
};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use frust_devtools_protocol::{Capability, Discovery, FrameStats, HandshakeInfo};
use frust_drive::build_info::BuildMode;
use frust_drive::devices::Device;
use frust_drive::devtools_client::DevtoolsClient;
use frust_drive::metrics::{CpuSample, MemSample, MetricsSample, NetSample, ThermalSample};
use frust_drive::process::StreamHandle;

use super::ring::Ring;
use super::{FRAME_RING_CAP, LOG_RING_CAP, LOG_SUBSCRIPTION_CAP};

/// Engine-assigned handle for one supervised session. Opaque and monotonic —
/// never reused within a process, so a stale id from an agent always resolves
/// to "no such session" rather than to somebody else's app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub u64);

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What a session runs the app on.
#[derive(Debug, Clone)]
pub enum RunTarget {
    /// A host `cargo run` preview (`frust_drive::desktop_run`).
    Desktop,
    /// An Android device or emulator (`frust_drive::android_run`).
    Android(Device),
    /// An iOS Simulator (`frust_drive::ios_run`). **Best-effort and
    /// untested** — it compiles and follows the same shape as Android, but
    /// nothing in this repo has run it against a Simulator; see the module
    /// doc on [`crate::engine`].
    IosSimulator(Device),
}

impl RunTarget {
    /// The `adb` serial this target is addressed by, or `None` for anything
    /// that is not an Android device — the discriminator both the devtools
    /// `adb forward` and the metrics sampler key off.
    pub fn android_serial(&self) -> Option<&str> {
        match self {
            RunTarget::Android(device) => Some(device.id.as_str()),
            RunTarget::Desktop | RunTarget::IosSimulator(_) => None,
        }
    }

    /// The Simulator udid this target is addressed by, or `None` for anything
    /// that is not an iOS Simulator — the discriminator teardown's
    /// `simctl terminate` keys off, mirroring
    /// [`android_serial`](Self::android_serial).
    pub fn ios_simulator_udid(&self) -> Option<&str> {
        match self {
            RunTarget::IosSimulator(device) => Some(device.id.as_str()),
            RunTarget::Desktop | RunTarget::Android(_) => None,
        }
    }

    /// A short, stable label for reporting (`desktop`, `android:<serial>`,
    /// `ios-sim:<udid>`).
    pub fn label(&self) -> String {
        match self {
            RunTarget::Desktop => "desktop".to_string(),
            RunTarget::Android(device) => format!("android:{}", device.id),
            RunTarget::IosSimulator(device) => format!("ios-sim:{}", device.id),
        }
    }
}

/// A session's lifecycle position.
///
/// `Launching → Running → DevtoolsConnected` is the happy path; `Exited` and
/// `Failed` are terminal and reachable from any of the three.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// Spawned; for a device target still building/installing/launching.
    Launching,
    /// The app's output stream is live.
    Running,
    /// A devtools discovery line was found, the connection handshook, and
    /// the client is stored on the session.
    DevtoolsConnected,
    /// The process ended. `success` mirrors
    /// `frust_drive::process::StreamHandle::wait` — **not** a real exit
    /// code, which that seam does not expose; a session stopped through
    /// [`crate::engine::SessionEngine::stop_app`] therefore reports
    /// `success: false`, the same as a killed process does.
    Exited { success: bool },
    /// The launch itself failed (build/install/spawn error) — `reason` is
    /// the drive's own error chain, rendered for an agent to read.
    Failed { reason: String },
}

impl SessionState {
    /// Whether no further transition is coming.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            SessionState::Exited { .. } | SessionState::Failed { .. }
        )
    }
}

/// The most recent sample of each system-metrics kind, as
/// `frust_drive::metrics` reports them.
///
/// Thermal readings are per zone (latest reading of each), ordered by zone
/// label. Every field is `None`/empty for a session whose sampler never
/// started — see [`crate::engine`]'s note on the desktop pid gap.
#[derive(Debug, Clone, Default)]
pub struct LatestMetrics {
    pub cpu: Option<CpuSample>,
    pub mem: Option<MemSample>,
    pub net: Option<NetSample>,
    pub thermal: Vec<ThermalSample>,
}

/// An immutable read of one session's observable state — what the tool layer
/// renders. Cheap to take and never held across a blocking call.
#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    pub id: SessionId,
    pub target: RunTarget,
    pub mode: BuildMode,
    pub project_root: PathBuf,
    pub state: SessionState,
    pub started_at: SystemTime,
    /// The app's process id, when the session's own output reported one.
    /// Android only, as the drive's `Streaming logs (pid N)` marker carries
    /// it; kept as the string the marker holds because that is exactly what
    /// `frust_drive::metrics::MetricsSource::AndroidPkg` wants back.
    pub pid: Option<String>,
    /// The installed Android package, from the drive's `Launching <pkg>…`
    /// marker.
    pub android_package: Option<String>,
    /// The bundle id an iOS Simulator session actually launched, as
    /// `frust_drive::ios_run::spawn_session` reported it. iOS Simulator only,
    /// and the one thing teardown needs to `simctl terminate` the app —
    /// killing the `simctl launch` bridge leaves the app itself running.
    pub ios_bundle_id: Option<String>,
    /// The **local** loopback port the devtools client is connected on —
    /// for Android that is the `adb forward` host port, not the device port
    /// the discovery line announced.
    pub devtools_port: Option<u16>,
    /// The handshake the connected devtools service answered with (app name,
    /// frust version, protocol version, declared capabilities).
    pub devtools_handshake: Option<HandshakeInfo>,
    /// Why devtools is not available, verbatim: either the app's own
    /// `frust-devtools: service did not start: <reason>` line (the
    /// per-app-network-toggle case an agent must be able to read as written —
    /// `docs/LIMITATIONS.md` `devtools-android-per-app-network-toggle`), or
    /// this engine's own connect/handshake failure.
    pub devtools_error: Option<String>,
    /// Retained log lines and how many were evicted for capacity.
    pub log_lines: usize,
    pub dropped_log_lines: u64,
    /// Retained frame-stats samples and how many were evicted for capacity.
    pub frames: usize,
    pub dropped_frames: u64,
    /// Whether a metrics sampler is running for this session.
    pub metrics_sampling: bool,
}

impl SessionSnapshot {
    /// The devtools capability set declared at handshake, if connected.
    pub fn devtools_capabilities(&self) -> Option<&[Capability]> {
        self.devtools_handshake
            .as_ref()
            .map(|info| info.capabilities.as_slice())
    }
}

/// The one shape a lost-line marker takes on a log subscription: a line the
/// subscriber reads in band, so loss is always explicit and never silent.
fn dropped_marker(count: u64, reason: &str) -> String {
    format!("[frust] {count} log line(s) dropped ({reason})")
}

/// Lines lost because the subscriber was not reading fast enough.
const SLOW_CONSUMER: &str = "slow consumer";

/// Lines lost at subscribe time: the session's retained backlog was deeper
/// than the subscription channel, so only its newest lines could be seeded.
const OLDER_THAN_BUFFER: &str = "backlog older than the subscription buffer";

/// A live push feed of one session's log lines, handed out by
/// [`SessionEngine::subscribe_logs`](crate::engine::SessionEngine::subscribe_logs).
///
/// **Seeded, then live.** The feed opens with the session's retained log tail
/// and continues with every line ingested afterwards — the seed is taken and
/// the feed registered inside a single critical section, so no line is
/// dropped or repeated at the boundary.
///
/// **Redacted, like the ring.** The subscription is fed where lines enter the
/// log ring, i.e. after the devtools handshake token has been redacted out; a
/// subscriber can never observe that token.
///
/// **Bounded, never blocking.** The channel holds
/// [`LOG_SUBSCRIPTION_CAP`](crate::engine::LOG_SUBSCRIPTION_CAP) lines and the
/// session thread only ever offers into it, so a subscriber that stops reading
/// slows nothing down. It loses lines instead, and is told: the next line the
/// channel accepts is preceded by one
/// `[frust] <N> log line(s) dropped (slow consumer)` marker.
///
/// **Ends with the session.** Once the session is torn down (or ends on its
/// own, or a later `subscribe_logs` call replaces this one), the sending half
/// is dropped: the receiver hands back everything still buffered and then
/// reports a disconnect.
pub struct LogSubscription {
    lines: Receiver<String>,
}

impl LogSubscription {
    /// Blocks until the next line arrives, or the feed ends.
    pub fn recv(&self) -> Result<String, RecvError> {
        self.lines.recv()
    }

    /// Blocks for at most `timeout` waiting for the next line.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<String, RecvTimeoutError> {
        self.lines.recv_timeout(timeout)
    }

    /// The next line if one is already buffered, never blocking.
    pub fn try_recv(&self) -> Result<String, TryRecvError> {
        self.lines.try_recv()
    }

    /// The underlying receiver, for a caller that wants to select/iterate over
    /// it directly.
    pub fn into_receiver(self) -> Receiver<String> {
        self.lines
    }
}

/// The session's own half of a [`LogSubscription`]: the bounded sender plus
/// the count of lines the subscriber was too slow to take.
///
/// Held under the `data` lock and driven only from an ingest, so every send is
/// a `try_send` — a blocking send here would park the session's launch/drain
/// thread on whatever an agent's DAP client is doing.
struct LogFeed {
    lines: SyncSender<String>,
    /// Lines dropped since the last marker the subscriber actually received.
    dropped: u64,
}

impl LogFeed {
    fn new(lines: SyncSender<String>) -> Self {
        Self { lines, dropped: 0 }
    }

    /// Offers `line` to the subscriber. Returns `false` once the receiver is
    /// gone — the caller then clears the slot, so a dropped subscriber costs
    /// the session thread one failed send and nothing more.
    ///
    /// A backlog of dropped lines is announced *before* the line that finally
    /// fits, so the marker lands in the reader's stream at the point the loss
    /// actually happened.
    fn offer(&mut self, line: &str) -> bool {
        if self.dropped > 0 {
            match self
                .lines
                .try_send(dropped_marker(self.dropped, SLOW_CONSUMER))
            {
                Ok(()) => self.dropped = 0,
                // Still full: the marker's own count grows by this line too.
                Err(TrySendError::Full(_)) => {
                    self.dropped = self.dropped.saturating_add(1);
                    return true;
                }
                Err(TrySendError::Disconnected(_)) => return false,
            }
        }
        match self.lines.try_send(line.to_string()) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.dropped = self.dropped.saturating_add(1);
                true
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Announces, as the feed's very first line, a backlog too deep to seed.
    /// Only ever called on an empty channel, so it cannot itself be lost.
    fn note_backlog_skipped(&mut self, skipped: u64) {
        let _ = self
            .lines
            .try_send(dropped_marker(skipped, OLDER_THAN_BUFFER));
    }
}

/// One thing that happened to a session, as a [`SessionEventFeed`] delivers
/// it.
///
/// Deliberately only the two a consumer outside this crate cannot get any
/// other way from a *sync* seam: the log lines (which
/// [`SessionEngine::subscribe_logs`](crate::engine::SessionEngine::subscribe_logs)
/// already pushes) and the session's end (which only `wait_for`, an `async`
/// method, reports today).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// One retained-then-live log line, exactly as the log ring holds it —
    /// including the in-band `[frust] <N> event(s) dropped (…)` markers this
    /// feed reports its own losses with.
    Log(String),
    /// The session reached a terminal state; `state` is the same
    /// [`SessionState::Exited`]/[`SessionState::Failed`] a
    /// [`SessionSnapshot`] would report for it.
    ///
    /// **Terminal for the feed too**: nothing follows it, and the sending
    /// half is dropped immediately after, so the next `recv` reports a
    /// disconnect.
    Exited { state: SessionState },
}

/// Slots held back from [`LOG_SUBSCRIPTION_CAP`] for the feed's own ending: a
/// final loss marker plus the [`SessionEvent::Exited`] itself.
///
/// The exit is the one event that may never be dropped — a consumer that
/// misses it waits for an app that already died — so it is never allowed to
/// compete with log lines for the last slot. Log events therefore occupy at
/// most `LOG_SUBSCRIPTION_CAP` of a channel that is two deeper.
const EVENT_FEED_RESERVED: usize = 2;

/// A live push feed of one session's [`SessionEvent`]s, handed out by
/// [`SessionEngine::subscribe_session_events`](crate::engine::SessionEngine::subscribe_session_events).
///
/// The [`LogSubscription`] contract, plus an ending. Seeded with the retained
/// log tail and continued live under the ring's own lock (no gap, no
/// duplicate); redacted, because it is fed where lines enter the ring;
/// bounded at [`LOG_SUBSCRIPTION_CAP`] log events, dropping rather than
/// blocking the session's ingest thread and reporting every loss in band as a
/// `[frust] <N> event(s) dropped (…)` [`SessionEvent::Log`]; **one subscriber
/// per session**, a second call replacing the first.
///
/// What it adds is the end: the session's terminal state arrives as
/// [`SessionEvent::Exited`] and the sending half is dropped straight after, so
/// a consumer reads the app's last lines, then its exit, then a disconnect —
/// with no second (async) call to `wait_for` and no polling.
///
/// `Send`, not `Sync`: like [`LogSubscription`], one thread owns it and reads
/// it with a blocking `recv`.
pub struct SessionEventFeed {
    events: Receiver<SessionEvent>,
    /// Events sent but not yet handed to the consumer — see
    /// [`EventFeed::offer`] for what the count is for. Decremented here
    /// because this is the only side that knows an event was taken.
    inflight: Arc<AtomicUsize>,
}

impl SessionEventFeed {
    /// Opens a feed plus the [`SessionEventSink`] that drives it, for a
    /// [`SessionBackend`](crate::backend::SessionBackend) implemented
    /// **outside this crate**.
    ///
    /// [`SessionEngine`](crate::SessionEngine) never calls this — it owns
    /// [`EventFeed`] directly, under its own session lock. It exists because
    /// the delivery contract above (bounded log budget, in-band loss markers,
    /// a reserved slot the [`SessionEvent::Exited`] can always be delivered
    /// in) is the *feed's* contract, not the engine's: an embedder that
    /// re-implemented it would be re-implementing exactly the part a consumer
    /// depends on. Handing out the sending half instead keeps one
    /// implementation of it.
    ///
    /// The caller owns the seeding and the ending, in the same order this
    /// crate's own subscribe does: optionally
    /// [`note_backlog_skipped`](SessionEventSink::note_backlog_skipped), then
    /// one [`offer`](SessionEventSink::offer) per retained line, then live
    /// offers, then exactly one [`finish`](SessionEventSink::finish).
    pub fn channel() -> (SessionEventSink, SessionEventFeed) {
        let (sender, events) = mpsc::sync_channel(LOG_SUBSCRIPTION_CAP + EVENT_FEED_RESERVED);
        let inflight = Arc::new(AtomicUsize::new(0));
        let sink = SessionEventSink(EventFeed::new(sender, Arc::clone(&inflight)));
        (sink, SessionEventFeed { events, inflight })
    }

    /// Blocks until the next event arrives, or the feed ends.
    pub fn recv(&self) -> Result<SessionEvent, RecvError> {
        self.took(self.events.recv())
    }

    /// Blocks for at most `timeout` waiting for the next event.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<SessionEvent, RecvTimeoutError> {
        self.took(self.events.recv_timeout(timeout))
    }

    /// The next event if one is already buffered, never blocking.
    pub fn try_recv(&self) -> Result<SessionEvent, TryRecvError> {
        self.took(self.events.try_recv())
    }

    /// Accounts for one taken event, whatever the receive flavor was.
    ///
    /// There is no `into_receiver` counterpart to [`LogSubscription`]'s: a
    /// bare `Receiver` would receive events without this accounting, and the
    /// session would then treat the channel as fuller than it is — eventually
    /// dropping live lines for capacity that is actually free.
    fn took<E>(&self, received: Result<SessionEvent, E>) -> Result<SessionEvent, E> {
        if received.is_ok() {
            self.inflight.fetch_sub(1, Ordering::Relaxed);
        }
        received
    }
}

/// The sending half of a [`SessionEventFeed`] opened with
/// [`SessionEventFeed::channel`] — what an out-of-crate backend pushes a
/// session's lines and its ending into.
///
/// A thin wrapper over the same [`EventFeed`]
/// [`SessionEngine`](crate::SessionEngine) drives, so an embedder gets the
/// feed's whole delivery contract (bounded log budget, in-band loss markers,
/// the reserved exit slot) rather than a bare channel it would have to
/// re-implement that contract on top of.
///
/// Dropping it without [`finish`](Self::finish) closes the feed *without* an
/// [`SessionEvent::Exited`]: the consumer sees a disconnect, which is the
/// honest report for a host that went away mid-session (rather than a
/// fabricated exit status). A host that knows the session ended calls
/// `finish`.
pub struct SessionEventSink(EventFeed);

impl SessionEventSink {
    /// Offers one log line, returning `false` once the consumer's half is
    /// gone (the caller then drops this sink rather than retrying a dead
    /// channel).
    ///
    /// Never blocks: past the log budget the line is counted and reported
    /// in band by the next successful offer, exactly as the engine's own
    /// ingest path behaves.
    pub fn offer(&mut self, line: &str) -> bool {
        self.0.offer(line)
    }

    /// Announces, as the feed's first event, a seed backlog too deep to send
    /// — `skipped` lines the consumer will never see.
    pub fn note_backlog_skipped(&mut self, skipped: u64) {
        self.0.note_backlog_skipped(skipped);
    }

    /// Closes the feed with the session's terminal `state`, preceded by a
    /// marker for anything the consumer missed. Consumes the sink: the
    /// disconnect the consumer sees after the [`SessionEvent::Exited`] is
    /// this returning.
    pub fn finish(self, state: SessionState) {
        self.0.finish(state);
    }
}

/// The session's own half of a [`SessionEventFeed`] — the bounded sender, the
/// slot accounting that keeps the exit deliverable, and the count of events
/// the subscriber was too slow to take.
///
/// Held under the `data` lock and driven only from an ingest or a state
/// transition, so every send is a `try_send`: a blocking send here would park
/// the session's launch/drain thread on whatever a DAP client is doing.
struct EventFeed {
    events: SyncSender<SessionEvent>,
    inflight: Arc<AtomicUsize>,
    /// Events dropped since the last marker the subscriber actually received.
    dropped: u64,
}

impl EventFeed {
    fn new(events: SyncSender<SessionEvent>, inflight: Arc<AtomicUsize>) -> Self {
        Self {
            events,
            inflight,
            dropped: 0,
        }
    }

    /// Offers one **log** event to the subscriber, returning `false` once the
    /// receiver is gone (the caller then clears the slot, as with
    /// [`LogFeed::offer`]).
    ///
    /// Log events are refused past [`LOG_SUBSCRIPTION_CAP`] even though the
    /// channel is [`EVENT_FEED_RESERVED`] deeper, which is what keeps the
    /// terminal marker + [`SessionEvent::Exited`] pair deliverable however
    /// far behind the consumer has fallen. `inflight` can only ever
    /// **over**-count (this is the sole sender, and the receiver decrements
    /// only after taking an event), so the reservation cannot be eaten by a
    /// stale read.
    fn offer(&mut self, line: &str) -> bool {
        // The marker counts against the budget too — otherwise a burst that
        // ends on a marker+line pair could leave the ending one slot short.
        let wanted = if self.dropped > 0 { 2 } else { 1 };
        if self.inflight.load(Ordering::Relaxed) + wanted > LOG_SUBSCRIPTION_CAP {
            self.dropped = self.dropped.saturating_add(1);
            return true;
        }
        if self.dropped > 0 {
            let marker = SessionEvent::Log(dropped_event_marker(self.dropped, SLOW_CONSUMER));
            self.dropped = 0;
            if !self.send(marker) {
                return false;
            }
        }
        self.send(SessionEvent::Log(line.to_string()))
    }

    /// Announces, as the feed's very first event, a backlog too deep to seed.
    /// Only ever called on an empty channel, so it cannot itself be lost.
    fn note_backlog_skipped(&mut self, skipped: u64) {
        let _ = self.send(SessionEvent::Log(dropped_event_marker(
            skipped,
            OLDER_THAN_BUFFER,
        )));
    }

    /// Closes the feed with the session's terminal `state`, preceded by a
    /// marker for anything the subscriber missed. Consumes the feed: the
    /// sender is dropped on return, which is the disconnect the consumer sees
    /// after the [`SessionEvent::Exited`].
    fn finish(mut self, state: SessionState) {
        if self.dropped > 0 {
            let marker = SessionEvent::Log(dropped_event_marker(self.dropped, SLOW_CONSUMER));
            self.dropped = 0;
            if !self.send(marker) {
                return;
            }
        }
        self.send(SessionEvent::Exited { state });
    }

    /// Sends one event, keeping `inflight` in step. `false` means the
    /// receiver is gone (or — impossible by the reservation above, handled
    /// rather than asserted — the channel was full).
    fn send(&mut self, event: SessionEvent) -> bool {
        match self.events.try_send(event) {
            Ok(()) => {
                self.inflight.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(TrySendError::Full(_)) => {
                self.dropped = self.dropped.saturating_add(1);
                true
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }
}

/// The lost-event counterpart of [`dropped_marker`], in the same in-band,
/// never-silent shape.
fn dropped_event_marker(count: u64, reason: &str) -> String {
    format!("[frust] {count} event(s) dropped ({reason})")
}

/// Everything behind the session's `data` lock — see the module doc.
struct SessionData {
    state: SessionState,
    pid: Option<String>,
    android_package: Option<String>,
    ios_bundle_id: Option<String>,
    devtools_port: Option<u16>,
    devtools_handshake: Option<HandshakeInfo>,
    devtools_error: Option<String>,
    /// The last discovery line accepted, so a re-announcement with the same
    /// port/token does not open a second connection.
    discovered: Option<Discovery>,
    metrics_sampling: bool,
    logs: Ring<String>,
    /// The registered log subscriber, if any — at most one per session (a new
    /// [`Session::subscribe_logs`] replaces it).
    log_feed: Option<LogFeed>,
    /// The registered session-event subscriber, if any — same one-per-session
    /// rule, and independent of `log_feed` (the two seams coexist; a consumer
    /// takes whichever it needs).
    event_feed: Option<EventFeed>,
    frames: Ring<FrameStats>,
    cpu: Option<CpuSample>,
    mem: Option<MemSample>,
    net: Option<NetSample>,
    thermal: BTreeMap<String, ThermalSample>,
}

/// The teardown-only handles — see the module doc's locking contract.
#[derive(Default)]
pub(crate) struct Resources {
    /// The app's output stream (`cargo run`, `adb logcat`, `simctl launch`).
    pub(crate) stream: Option<StreamHandle>,
    /// An allocated `adb forward`, as `(serial, local_port)`, to remove.
    pub(crate) forward: Option<(String, u16)>,
    /// Every thread this session spawned, to join at teardown.
    pub(crate) threads: Vec<JoinHandle<()>>,
}

/// One supervised session. Shared as `Arc<Session>` between the engine and
/// the session's own threads.
pub(crate) struct Session {
    pub(crate) id: SessionId,
    pub(crate) target: RunTarget,
    pub(crate) mode: BuildMode,
    pub(crate) project_root: PathBuf,
    pub(crate) started_at: SystemTime,
    /// Set once teardown begins: the drive's own cancellation flag
    /// (`android_run::spawn_session` honors it at every phase boundary) and
    /// this engine's own thread wind-down signal, deliberately the same flag.
    pub(crate) stop: Arc<AtomicBool>,
    data: Mutex<SessionData>,
    changed: Condvar,
    resources: Mutex<Resources>,
    devtools: Mutex<Option<Arc<DevtoolsClient>>>,
}

impl Session {
    pub(crate) fn new(
        id: SessionId,
        target: RunTarget,
        mode: BuildMode,
        project_root: PathBuf,
    ) -> Self {
        Self {
            id,
            target,
            mode,
            project_root,
            started_at: SystemTime::now(),
            stop: Arc::new(AtomicBool::new(false)),
            data: Mutex::new(SessionData {
                state: SessionState::Launching,
                pid: None,
                android_package: None,
                ios_bundle_id: None,
                devtools_port: None,
                devtools_handshake: None,
                devtools_error: None,
                discovered: None,
                metrics_sampling: false,
                logs: Ring::new(LOG_RING_CAP),
                log_feed: None,
                event_feed: None,
                frames: Ring::new(FRAME_RING_CAP),
                cpu: None,
                mem: None,
                net: None,
                thermal: BTreeMap::new(),
            }),
            changed: Condvar::new(),
            resources: Mutex::new(Resources::default()),
            devtools: Mutex::new(None),
        }
    }

    /// Whether teardown has been requested. Every session thread checks this
    /// at its own wind-down point.
    pub(crate) fn stopping(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// Signals teardown to every session thread and to the drive pipeline's
    /// own phase-boundary cancellation check.
    pub(crate) fn request_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    fn with_data<R>(&self, f: impl FnOnce(&mut SessionData) -> R) -> R {
        let mut data = self.data.lock().unwrap_or_else(|p| p.into_inner());
        let result = f(&mut data);
        drop(data);
        // Notify unconditionally: a caller waiting on a predicate cannot know
        // which mutation would satisfy it, and a spurious wake only costs one
        // re-evaluation.
        self.changed.notify_all();
        result
    }

    fn snapshot_of(&self, data: &SessionData) -> SessionSnapshot {
        SessionSnapshot {
            id: self.id,
            target: self.target.clone(),
            mode: self.mode,
            project_root: self.project_root.clone(),
            state: data.state.clone(),
            started_at: self.started_at,
            pid: data.pid.clone(),
            android_package: data.android_package.clone(),
            ios_bundle_id: data.ios_bundle_id.clone(),
            devtools_port: data.devtools_port,
            devtools_handshake: data.devtools_handshake.clone(),
            devtools_error: data.devtools_error.clone(),
            log_lines: data.logs.len(),
            dropped_log_lines: data.logs.dropped(),
            frames: data.frames.len(),
            dropped_frames: data.frames.dropped(),
            metrics_sampling: data.metrics_sampling,
        }
    }

    pub(crate) fn snapshot(&self) -> SessionSnapshot {
        let data = self.data.lock().unwrap_or_else(|p| p.into_inner());
        self.snapshot_of(&data)
    }

    /// Blocks until `predicate` holds for this session's snapshot or
    /// `timeout` elapses, returning the snapshot either way (the caller
    /// re-checks the predicate on it if it needs to distinguish).
    ///
    /// Waits on the session's own `Condvar`, so this settles the moment the
    /// engine records the change rather than on a polling interval.
    pub(crate) fn wait_for(
        &self,
        predicate: &dyn Fn(&SessionSnapshot) -> bool,
        timeout: Duration,
    ) -> SessionSnapshot {
        let deadline = Instant::now() + timeout;
        let mut data = self.data.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            let snapshot = self.snapshot_of(&data);
            if predicate(&snapshot) {
                return snapshot;
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return snapshot;
            };
            let (next, _) = self
                .changed
                .wait_timeout(data, remaining)
                .unwrap_or_else(|p| p.into_inner());
            data = next;
        }
    }

    /// Advances a still-[`SessionState::Launching`] session to
    /// [`SessionState::Running`]. Can never downgrade: a session whose
    /// devtools connected (or which already ended) keeps the state it has,
    /// since the connect thread races the launch thread's own transition.
    pub(crate) fn set_running(&self) {
        self.with_data(|data| {
            if data.state == SessionState::Launching {
                data.state = SessionState::Running;
            }
        });
    }

    /// Whether the session has reached a terminal state — the cheap read the
    /// engine's retention sweep takes, rather than a whole
    /// [`SessionSnapshot`] clone per session.
    pub(crate) fn is_terminal(&self) -> bool {
        self.data
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .state
            .is_terminal()
    }

    /// Records the state only while the session is still live — a session
    /// already torn down (or already terminal) keeps the state it ended with,
    /// so a late thread cannot resurrect it.
    ///
    /// Reaching a terminal state also ends both feeds: no further line will
    /// ever be ingested, so the subscriber is owed a closed channel rather
    /// than a feed that stays open until the session is evicted. The event
    /// feed is closed *through* its [`SessionEvent::Exited`] — this
    /// transition is the only place that event is produced, which is why a
    /// registered feed can never miss it (registration itself is refused for
    /// an already-terminal session).
    pub(crate) fn set_state_if_live(&self, state: SessionState) {
        self.with_data(|data| {
            if !data.state.is_terminal() {
                data.state = state;
                if data.state.is_terminal() {
                    data.log_feed = None;
                    if let Some(feed) = data.event_feed.take() {
                        feed.finish(data.state.clone());
                    }
                }
            }
        });
    }

    /// Retains one log line and offers it to both subscribers, if any.
    ///
    /// The offers are inside the ring's own critical section deliberately: it
    /// is what makes [`subscribe_logs`](Self::subscribe_logs)'s (and
    /// [`subscribe_events`](Self::subscribe_events)'s) seed and this live feed
    /// meet exactly, with no line dropped or repeated at the seam. Neither
    /// offer ever blocks (see [`LogFeed`], [`EventFeed`]), so a subscriber
    /// cannot stall the session's launch/drain thread.
    pub(crate) fn push_log(&self, line: String) {
        self.with_data(|data| {
            if let Some(feed) = data.log_feed.as_mut()
                && !feed.offer(&line)
            {
                // The subscriber is gone; the slot is cleared so the next
                // ingest does not retry a dead channel.
                data.log_feed = None;
            }
            if let Some(feed) = data.event_feed.as_mut()
                && !feed.offer(&line)
            {
                data.event_feed = None;
            }
            data.logs.push(line);
        });
    }

    /// Opens a [`LogSubscription`] over this session, replacing any previous
    /// one (whose receiver then sees the channel close).
    ///
    /// Seeded with the retained log tail and registered for live delivery in
    /// **one** `data` critical section, which is the whole point: an ingest
    /// can only run before the seed is taken or after the feed is registered,
    /// never between the two.
    ///
    /// A backlog deeper than the channel is seeded with its newest lines and
    /// preceded by one marker naming what was skipped — the same in-band,
    /// never-silent loss contract the overflow path uses.
    pub(crate) fn subscribe_logs(&self) -> LogSubscription {
        let (sender, lines) = mpsc::sync_channel(LOG_SUBSCRIPTION_CAP);
        self.with_data(|data| {
            let mut feed = LogFeed::new(sender);
            let held = data.logs.len();
            let seed = if held > LOG_SUBSCRIPTION_CAP {
                // One slot goes to the marker, so the seed is the newest
                // `cap - 1` lines and the channel opens exactly full.
                let kept = LOG_SUBSCRIPTION_CAP - 1;
                feed.note_backlog_skipped((held - kept) as u64);
                data.logs.tail(Some(kept))
            } else {
                data.logs.tail(None)
            };
            for line in seed {
                feed.offer(&line);
            }
            // A session that has already ended ingests nothing more: the
            // subscriber keeps the seed it just got and sees the channel close
            // straight away, rather than a feed that never speaks again.
            if !data.state.is_terminal() {
                data.log_feed = Some(feed);
            }
        });
        LogSubscription { lines }
    }

    /// Opens a [`SessionEventFeed`] over this session, replacing any previous
    /// one (whose receiver then sees the channel close).
    ///
    /// The same one-critical-section seed-then-register as
    /// [`subscribe_logs`](Self::subscribe_logs), with the same too-deep-backlog
    /// marker — the difference is the ending. A session that has **already**
    /// ended keeps its seed and is handed its [`SessionEvent::Exited`] right
    /// away rather than being registered for a transition that will never come
    /// again; a live one gets it from
    /// [`set_state_if_live`](Self::set_state_if_live).
    pub(crate) fn subscribe_events(&self) -> SessionEventFeed {
        let (sender, events) = mpsc::sync_channel(LOG_SUBSCRIPTION_CAP + EVENT_FEED_RESERVED);
        let inflight = Arc::new(AtomicUsize::new(0));
        self.with_data(|data| {
            let mut feed = EventFeed::new(sender, Arc::clone(&inflight));
            let held = data.logs.len();
            let seed = if held > LOG_SUBSCRIPTION_CAP {
                // One slot goes to the marker, so the seed is the newest
                // `cap - 1` lines and the log budget opens exactly full.
                let kept = LOG_SUBSCRIPTION_CAP - 1;
                feed.note_backlog_skipped((held - kept) as u64);
                data.logs.tail(Some(kept))
            } else {
                data.logs.tail(None)
            };
            for line in seed {
                feed.offer(&line);
            }
            if data.state.is_terminal() {
                feed.finish(data.state.clone());
            } else {
                data.event_feed = Some(feed);
            }
        });
        SessionEventFeed { events, inflight }
    }

    /// Ends any log subscription — teardown, where the session may already
    /// have been terminal (and so never passed through the state transition
    /// that closes the feed on its own).
    ///
    /// The event feed needs no counterpart here: it is only ever registered on
    /// a live session, and the terminal transition teardown itself performs is
    /// what closes it (with the [`SessionEvent::Exited`] the subscriber is
    /// owed) — dropping it here instead would swallow that event.
    pub(crate) fn close_log_subscription(&self) {
        self.with_data(|data| data.log_feed = None);
    }

    pub(crate) fn push_frame(&self, frame: FrameStats) {
        self.with_data(|data| data.frames.push(frame));
    }

    pub(crate) fn record_metric(&self, sample: MetricsSample) {
        self.with_data(|data| match sample {
            MetricsSample::Cpu(cpu) => data.cpu = Some(cpu),
            MetricsSample::Mem(mem) => data.mem = Some(mem),
            MetricsSample::Net(net) => data.net = Some(net),
            MetricsSample::Thermal(thermal) => {
                data.thermal.insert(thermal.zone_label.clone(), thermal);
            }
        });
    }

    pub(crate) fn logs(&self, tail: Option<usize>) -> Vec<String> {
        let data = self.data.lock().unwrap_or_else(|p| p.into_inner());
        data.logs.tail(tail)
    }

    pub(crate) fn frames(&self) -> Vec<FrameStats> {
        let data = self.data.lock().unwrap_or_else(|p| p.into_inner());
        data.frames.tail(None)
    }

    pub(crate) fn latest_metrics(&self) -> LatestMetrics {
        let data = self.data.lock().unwrap_or_else(|p| p.into_inner());
        LatestMetrics {
            cpu: data.cpu,
            mem: data.mem,
            net: data.net,
            thermal: data.thermal.values().cloned().collect(),
        }
    }

    /// Accepts a discovery line, returning `true` only when it is new (a
    /// first announcement, or a re-announcement on a different port/token) —
    /// the caller then opens the devtools connection. A live announcement
    /// supersedes any earlier "service did not start" reason.
    pub(crate) fn note_discovery(&self, discovery: Discovery) -> bool {
        self.with_data(|data| {
            if data.discovered.as_ref() == Some(&discovery) {
                return false;
            }
            data.discovered = Some(discovery);
            data.devtools_error = None;
            true
        })
    }

    /// Records the app's own "service did not start" reason, verbatim.
    pub(crate) fn note_devtools_failure(&self, reason: String) {
        self.with_data(|data| data.devtools_error = Some(reason));
    }

    /// Records this engine's own connect/handshake failure.
    pub(crate) fn set_devtools_error(&self, reason: String) {
        self.with_data(|data| data.devtools_error = Some(reason));
    }

    /// Stores the connected client and advances the session to
    /// [`SessionState::DevtoolsConnected`].
    pub(crate) fn set_connected(
        &self,
        client: Arc<DevtoolsClient>,
        local_port: u16,
        handshake: HandshakeInfo,
    ) {
        *self.devtools.lock().unwrap_or_else(|p| p.into_inner()) = Some(client);
        self.with_data(|data| {
            data.devtools_port = Some(local_port);
            data.devtools_handshake = Some(handshake);
            data.devtools_error = None;
            if !data.state.is_terminal() {
                data.state = SessionState::DevtoolsConnected;
            }
        });
    }

    /// The connected devtools client, if any. Cloned out so the caller issues
    /// its (blocking, per-call-timeout-bounded) request with no lock held.
    pub(crate) fn devtools_client(&self) -> Option<Arc<DevtoolsClient>> {
        self.devtools
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Drops the client, closing its socket — which is also what ends the
    /// frame-stats subscription its reader thread feeds.
    pub(crate) fn drop_devtools_client(&self) {
        let client = self
            .devtools
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        drop(client);
    }

    /// Records the pid parsed from the session's own output. Returns the
    /// `(serial, pid, package)` triple once every piece a metrics sampler
    /// needs has arrived and none has been started yet — the caller then
    /// starts one.
    pub(crate) fn note_pid(&self, pid: String) -> Option<(String, String, String)> {
        self.with_data(|data| {
            data.pid = Some(pid);
            self.metrics_identity(data)
        })
    }

    /// Records the installed Android package parsed from the session's own
    /// output; same return contract as [`note_pid`](Self::note_pid).
    pub(crate) fn note_android_package(&self, package: String) -> Option<(String, String, String)> {
        self.with_data(|data| {
            data.android_package = Some(package);
            self.metrics_identity(data)
        })
    }

    /// Records the bundle id an iOS Simulator session launched, straight from
    /// `frust_drive::ios_run::spawn_session`'s own return — teardown reads it
    /// back to `simctl terminate` the app rather than re-deriving it from the
    /// `Launching <bundle_id>…` log line.
    pub(crate) fn note_ios_bundle_id(&self, bundle_id: String) {
        self.with_data(|data| data.ios_bundle_id = Some(bundle_id));
    }

    /// `(serial, pid, package)` once all three are known and no sampler is
    /// running yet — marking the sampler started so only the first caller
    /// gets the triple.
    fn metrics_identity(&self, data: &mut SessionData) -> Option<(String, String, String)> {
        if data.metrics_sampling {
            return None;
        }
        let serial = self.target.android_serial()?;
        let pid = data.pid.clone()?;
        let package = data.android_package.clone()?;
        data.metrics_sampling = true;
        Some((serial.to_string(), pid, package))
    }

    /// Clears the sampling flag — teardown only, so a stopped session does
    /// not keep reporting that it is sampling.
    pub(crate) fn clear_metrics_sampling(&self) {
        self.with_data(|data| data.metrics_sampling = false);
    }

    /// Registers the app's output stream for teardown to kill — **unless the
    /// session is already stopping**, in which case the handle comes straight
    /// back as `Some(stream)` and the caller owns it: nothing would ever kill
    /// a handle stored after teardown emptied the set, and a `cargo run` /
    /// `adb logcat` child left behind that way has no second kill path.
    ///
    /// The check is inside the `resources` critical section and teardown
    /// requests the stop strictly *before* taking the same lock, so the two
    /// are totally ordered — there is no window in which both sides believe
    /// the other owns the handle.
    #[must_use = "a returned stream was not stored — the caller must kill it"]
    pub(crate) fn set_stream(&self, stream: StreamHandle) -> Option<StreamHandle> {
        let mut resources = self.resources.lock().unwrap_or_else(|p| p.into_inner());
        if self.stopping() {
            return Some(stream);
        }
        resources.stream = Some(stream);
        None
    }

    /// Registers an allocated `adb forward` for teardown to remove, with the
    /// same hand-it-back contract as [`set_stream`](Self::set_stream): a
    /// `Some((serial, local_port))` return means the forward was **not**
    /// stored and the caller must remove it itself, or the host keeps the
    /// mapping for the rest of the server's life.
    #[must_use = "a returned forward was not stored — the caller must remove it"]
    pub(crate) fn set_forward(&self, serial: String, local_port: u16) -> Option<(String, u16)> {
        let mut resources = self.resources.lock().unwrap_or_else(|p| p.into_inner());
        if self.stopping() {
            return Some((serial, local_port));
        }
        resources.forward = Some((serial, local_port));
        None
    }

    /// Registers a session thread for teardown to join, or — once teardown has
    /// begun — **detaches** it: a thread registered after teardown took the
    /// list would never be joined anyway, and it already sees the stop flag,
    /// so it winds down on its own and touches nothing but its own session.
    pub(crate) fn add_thread(&self, handle: JoinHandle<()>) {
        let mut resources = self.resources.lock().unwrap_or_else(|p| p.into_inner());
        if self.stopping() {
            drop(handle);
            return;
        }
        resources.threads.push(handle);
    }

    /// Waits for the app's output stream to be reaped and reports whether it
    /// exited successfully. `None` when teardown already took the handle.
    pub(crate) fn wait_stream(&self) -> Option<bool> {
        let mut resources = self.resources.lock().unwrap_or_else(|p| p.into_inner());
        resources.stream.as_mut().map(StreamHandle::wait)
    }

    /// Takes every teardown handle out in one short critical section, so the
    /// caller can kill/join them with **no** session lock held (the
    /// adb-join-under-a-lock freeze this engine inherits as a lesson from
    /// `frust-tui`'s supervisor).
    ///
    /// **Call [`request_stop`](Self::request_stop) first.** The stop flag is
    /// what makes the emptied set stay empty: every registrar re-checks it
    /// under this same lock, so anything registered from here on is handed
    /// back to its own registrar rather than silently landing in a set nobody
    /// will ever tear down.
    pub(crate) fn take_resources(&self) -> Resources {
        let mut resources = self.resources.lock().unwrap_or_else(|p| p.into_inner());
        std::mem::take(&mut *resources)
    }
}

#[cfg(test)]
mod tests {
    use frust_drive::process::{FakeProcessRunner, ProcessRunner};

    use super::*;

    /// The scripted invocation the fake runner hands a hanging stream back for.
    const FAKE_STREAM_KEY: &str = "app --run";

    fn session() -> Session {
        Session::new(
            SessionId(1),
            RunTarget::Desktop,
            BuildMode::Debug,
            PathBuf::from("/tmp/frust-mcp-session-test"),
        )
    }

    /// A live handle over a scripted process that never exits on its own —
    /// exactly what a late registration would otherwise leak.
    fn hanging_stream() -> StreamHandle {
        let runner =
            FakeProcessRunner::new().with_hanging_stream(FAKE_STREAM_KEY, Vec::<String>::new());
        runner
            .spawn_streaming("app", &["--run"], None, &[])
            .expect("the scripted stream spawns")
    }

    #[test]
    fn a_stream_registered_before_teardown_is_taken_by_it() {
        let session = session();
        assert!(session.set_stream(hanging_stream()).is_none());
        let mut resources = session.take_resources();
        let mut stream = resources.stream.take().expect("teardown owns the stream");
        stream.kill();
    }

    #[test]
    fn a_stream_registered_after_teardown_comes_back_to_its_registrar() {
        let session = session();
        session.request_stop();
        let mut orphaned = session
            .set_stream(hanging_stream())
            .expect("a stopped session refuses to store a stream");
        // Nothing landed in the set teardown already emptied…
        assert!(session.take_resources().stream.is_none());
        // …so the caller is the only one who can end the process, and can.
        orphaned.kill();
        assert!(!orphaned.wait());
    }

    #[test]
    fn a_forward_registered_after_teardown_comes_back_to_its_registrar() {
        let session = session();
        assert!(
            session
                .set_forward("emulator-5554".to_string(), 41_234)
                .is_none()
        );
        session.request_stop();
        let _ = session.take_resources();

        assert_eq!(
            session.set_forward("emulator-5554".to_string(), 41_235),
            Some(("emulator-5554".to_string(), 41_235))
        );
        assert!(session.take_resources().forward.is_none());
    }

    #[test]
    fn a_thread_registered_after_teardown_is_detached_not_queued() {
        let session = session();
        session.request_stop();
        session.add_thread(std::thread::spawn(|| {}));
        assert!(
            session.take_resources().threads.is_empty(),
            "a thread queued after teardown would never be joined"
        );
    }
}
