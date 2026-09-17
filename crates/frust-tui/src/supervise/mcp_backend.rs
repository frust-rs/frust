//! The workbench's own [`SessionBackend`]: one session world, two front ends.
//!
//! [`TuiSessionBackend`] implements `frust-mcp`'s `SessionBackend` over
//! **this** workbench's supervise layer, so an embedded MCP server drives the
//! exact sessions the user is looking at — not a second, headless engine
//! running the same app twice.
//!
//! # How a blocking trait method gets a synchronous answer
//!
//! `SessionBackend` is sync by charter (`frust_mcp::backend`'s module doc),
//! but everything it asks about lives behind the TEA event loop: `AppState`
//! is owned by [`crate::engine::Engine`] and the [`Supervisor`] by
//! [`crate::runner`]. So each method posts one [`McpCommand`] into the
//! engine's message channel and blocks on a [`Reply`] the command carries;
//! the runner serves it with [`serve_command`] and sends the answer back.
//!
//! **Why that cannot deadlock.** The event loop is driven by the thread that
//! owns the runtime (`frust-cli` runs `Runtime::block_on(frust_tui::run())`),
//! never by a runtime worker; `serve_embedded`'s tasks run on workers and the
//! blocking pool. A backend method therefore always blocks a *different*
//! thread from the one that answers it. The one shape that would deadlock —
//! calling a backend method from inside the event loop — does not exist here
//! and must never be added. Every wait is additionally bounded by
//! [`REPLY_DEADLINE`], so even a wedged workbench degrades to a typed error
//! rather than a hung agent.
//!
//! # What this backend deliberately cannot do
//!
//! The workbench is not a headless engine, and several of its shapes have no
//! honest mapping onto the MCP session vocabulary. Each is reported as an
//! [`EmbeddedError`] or an explicit absence — **never** a zeroed or invented
//! value:
//!
//! - **`devtools_client`** — the connection lives *inside*
//!   [`crate::supervise::DevtoolsBridge`]'s own thread, which owns the
//!   blocking socket for its whole life; there is no `Arc<DevtoolsClient>` to
//!   hand out. Reported as `None`, with [`DEVTOOLS_OWNED_BY_WORKBENCH`]
//!   carried on **every** snapshot's `devtools_error` so the driving and
//!   diagnosis tools say why rather than implying the app merely has not
//!   connected yet.
//! - **Sessions the workbench launched some other way** — an ad-hoc
//!   build/clean/toolchain-fix session has no target and no build mode, so it
//!   is not an MCP session at all and is omitted from [`SessionBackend::sessions`].
//!   A physical-iOS session is omitted for the same reason: `RunTarget` has
//!   no variant for it, and reporting one as `ios-sim:<udid>` would be a lie.
//! - **`restart_app` on a session with no launch record** — nothing here can
//!   reconstruct a spec it never saw ([`EmbeddedError::Unsupported`]).
//! - **`run_app` onto a (project, target) the workbench is already running**
//!   — refused as [`EmbeddedError::AlreadyRunning`], naming the live session
//!   so an agent can `stop_app` or `restart_app` it. The guard reads the
//!   workbench's *own* sessions, not only MCP-launched ones (one session
//!   world), and runs before the cap check and before any bookkeeping.
//!   `restart_app` excludes the session it restarts — it stops it first — so
//!   a 1-for-1 relaunch is never refused by its own predecessor.
//! - **`run_app`/`restart_app` once [`MCP_RECORD_CAP`] MCP-launched sessions
//!   are already live** — refused as [`EmbeddedError::TooManySessions`],
//!   with **no bookkeeping**: no record, no `RegisterSession`, no ad-hoc tab.
//!   A refusal that still registered a session would grow `AppState::sessions`
//!   faster than a successful launch does, defeating the cap entirely.
//! - **An evicted MCP record's tab still exists.** [`McpSessionRecords`]
//!   bounds its retained launch records at [`MCP_RECORD_CAP`] the same way
//!   `frust-mcp`'s own `TERMINAL_SESSION_CAP` bounds its sessions —
//!   oldest-terminal evicted first, a live session's record never touched —
//!   but `AppState::sessions` has no eviction of its own: no tab-close
//!   mechanism exists in the workbench to build one on. An agent driving
//!   many short-lived launches over hours will eventually see a session's
//!   *tab* survive after its *record* is gone; `restart_app` on that id then
//!   reports [`EmbeddedError::NoSuchSession`] — the same typed refusal a
//!   truly unknown id gets, not a crash, but a real divergence this doc
//!   records rather than hides.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use frust_devtools_protocol::{FrameStats, Method, RpcError, serde_json};
use frust_drive::build_info::{BuildInfo, BuildMode};
use frust_drive::devices::{Device, Kind, Platform, default_discoverers, discover_all};
use frust_drive::devtools_client::{DevtoolsClient, DevtoolsRpcError};
use frust_drive::process::ProcessRunner;
use frust_mcp::SessionBackend;
use frust_mcp::engine::{
    LatestMetrics, RunTarget, SessionEventFeed, SessionId as McpSessionId, SessionSnapshot,
    SessionState as McpSessionState, TERMINAL_SESSION_CAP,
};
use tokio::sync::mpsc::UnboundedSender;

use super::session_feeds::{PendingWidgetTrees, SessionSubscribers};
use super::{
    DeviceTarget, DevtoolsBridge, SessionEvent, SessionEventKind, SessionId, SessionSpec,
    SessionState, Supervisor,
};
use crate::engine::{
    AppState, ConnState, DevtoolsLaunch, Message, SamplingState, SessionTarget, SessionView,
};

/// How long a backend method waits for the event loop to answer before
/// reporting [`EmbeddedError::WorkbenchUnreachable`].
///
/// Generous against anything the loop actually does between two messages
/// (`update` is pure and every effect it enacts is a spawn, not a wait), and
/// short enough that an agent gets a typed answer rather than a hang if the
/// workbench is tearing down mid-request.
const REPLY_DEADLINE: Duration = Duration::from_secs(5);

/// What every session's `devtools_error` reports in embedded mode: the tools
/// cannot borrow the workbench's devtools connection (see the module doc).
pub const DEVTOOLS_OWNED_BY_WORKBENCH: &str = "the workbench owns this session's devtools connection and cannot share it with the \
     embedded MCP server; inspection and input tools are unavailable in embedded mode";

/// A session id no session is ever assigned — what [`SessionBackend::run_app`]
/// reports when the workbench event loop is already gone (the process is on
/// its way out), so the answer resolves to "no such session" rather than to
/// somebody else's app.
///
/// `crate::runner` mints its ad-hoc session ids *downward* from
/// [`MAX_ADHOC_SESSION_ID`] and the [`Supervisor`] counts upward from 0, so
/// this value is unallocatable by construction.
pub const UNRESOLVED_SESSION: McpSessionId = McpSessionId(u64::MAX);

/// The first (highest) id `crate::runner` may mint for an ad-hoc session —
/// one below [`UNRESOLVED_SESSION`], which is reserved.
pub const MAX_ADHOC_SESSION_ID: u64 = u64::MAX - 1;

/// Cap on retained MCP-launched session records ([`McpSessionRecords`]),
/// live and terminal together.
///
/// Reused directly from [`frust_mcp::engine::TERMINAL_SESSION_CAP`] rather
/// than a second magic number — the reasoning is identical (an agent driving
/// a server for hours must not grow a session map without bound) even though
/// this is a different registry: the workbench's own launch records, not
/// `frust-mcp`'s in-process `SessionEngine`.
pub const MCP_RECORD_CAP: usize = TERMINAL_SESSION_CAP;

// ── Errors ──────────────────────────────────────────────────────────────────

/// Why a backend call could not be served over the workbench.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum EmbeddedError {
    /// The event loop did not answer (it has exited, or is wedged past
    /// [`REPLY_DEADLINE`]).
    #[error(
        "the frust workbench did not answer within {:?} — it may be shutting down",
        REPLY_DEADLINE
    )]
    WorkbenchUnreachable,
    /// The workbench knows no session with this id (or it is one of the
    /// shapes the module doc lists as not-an-MCP-session).
    #[error("no such session in the workbench: {0}")]
    NoSuchSession(u64),
    /// The operation has no honest mapping onto the workbench's supervise
    /// layer. `what` names the operation, `why` the reason, both for an
    /// agent to read.
    #[error("{what} is unsupported while the MCP server is embedded in the workbench: {why}")]
    Unsupported {
        /// The operation that was refused (`restart_app`, …).
        what: &'static str,
        /// Why it cannot be served here.
        why: &'static str,
    },
    /// The workbench already has [`MCP_RECORD_CAP`] MCP-launched sessions
    /// live. Refused **before any bookkeeping** — no record, no
    /// `RegisterSession`, no ad-hoc tab — so a refusal never itself grows
    /// `AppState::sessions`; stop or wait for one to finish, then retry.
    #[error(
        "the workbench already has {cap} MCP-launched sessions live — stop one before \
         launching another"
    )]
    TooManySessions {
        /// The cap that was hit ([`MCP_RECORD_CAP`]).
        cap: usize,
    },
    /// The workbench is already running this project on this target. Refused
    /// **before any bookkeeping**, exactly like [`Self::TooManySessions`] —
    /// one live session per (project, device), whichever front end launched
    /// it. The message names the session an agent can act on, since the
    /// blocker may well be one the *user* started by hand.
    #[error(
        "session {session} is already running {target} — stop it (stop_app) or restart it \
         (restart_app)"
    )]
    AlreadyRunning {
        /// The live session occupying the target.
        session: u64,
        /// The target it occupies, named the way the workbench names it
        /// (`desktop`, or the device's own name).
        target: String,
    },
}

/// Why a `widget_tree` pull could not be answered with a tree.
///
/// Two shapes, because a consumer classifies them differently
/// (`frust_drive::devtools_client::is_not_supported` is what `frust-dap`'s
/// adapter branches on): [`Self::Unavailable`] means *this backend has no
/// tree to give for this session* — the same answer
/// [`SessionBackend::fetch_widget_tree`]'s own default refusal gives — while
/// [`Self::Failed`] means the pull was attempted and the devtools side
/// rejected or dropped it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeRefusal {
    /// No such session, or no devtools connection to pull through. Rendered
    /// as a `DevtoolsRpcError` carrying
    /// [`RpcError::NOT_SUPPORTED`](frust_devtools_protocol::RpcError::NOT_SUPPORTED),
    /// so a caller's existing not-supported check classifies it exactly like
    /// an app that declared no `widget_tree` capability.
    Unavailable(String),
    /// The pull itself failed — the bridge's own error text.
    ///
    /// The bridge renders every devtools failure to a `String` before it
    /// reaches the engine ([`crate::engine::InspectorEvent::Failed`]), so the
    /// original error's *code* is not recoverable here: an `unauthorized`
    /// pull arrives as prose, and a caller sees a plain failure rather than a
    /// classified one. That is a real narrowing, recorded rather than papered
    /// over by guessing a code back from the text.
    Failed(String),
}

impl TreeRefusal {
    /// Render the refusal as the error the trait method returns.
    fn into_error(self, id: McpSessionId) -> anyhow::Error {
        match self {
            Self::Unavailable(message) => anyhow::Error::new(DevtoolsRpcError {
                method: Method::WidgetTree.to_string(),
                code: RpcError::NOT_SUPPORTED,
                message,
            }),
            Self::Failed(message) => {
                anyhow::anyhow!("the widget-tree pull for session {id} failed: {message}")
            }
        }
    }
}

// ── The reply channel a command carries ─────────────────────────────────────

/// The one-shot answer channel an [`McpCommand`] carries back to the blocking
/// caller.
///
/// A `std` [`sync_channel`] rather than a `tokio::sync::oneshot`: the waiting
/// side is a `SessionBackend` method that may be called from a runtime worker
/// *or* from the blocking pool, and `oneshot`'s `blocking_recv` panics in the
/// former. A `std` receiver blocks whichever thread it is on and bounds the
/// wait itself.
///
/// `Clone`/`Debug`/`PartialEq` are hand-written so [`Message`] keeps its
/// derives: equality is handle identity (two `Reply`s are equal iff they name
/// the same channel), which is exactly what a message comparison in a test
/// wants.
pub struct Reply<T>(Arc<Mutex<Option<SyncSender<T>>>>);

impl<T> Reply<T> {
    /// A reply handle plus the receiver its answer arrives on.
    pub(crate) fn channel() -> (Self, Receiver<T>) {
        // Capacity 1: the answer is sent exactly once and never blocks the
        // event loop, whether or not the caller is still waiting.
        let (tx, rx) = sync_channel(1);
        (Self(Arc::new(Mutex::new(Some(tx)))), rx)
    }

    /// Answer the command. A second call, or a caller that already gave up,
    /// is a no-op.
    pub fn send(&self, value: T) {
        let sender = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(sender) = sender {
            let _ = sender.send(value);
        }
    }
}

impl<T> Clone for Reply<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> std::fmt::Debug for Reply<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Reply")
    }
}

impl<T> PartialEq for Reply<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl<T> Eq for Reply<T> {}

// ── The command vocabulary ──────────────────────────────────────────────────

/// One question (or lifecycle request) an embedded MCP server asks of the
/// workbench, carried on [`Message::Mcp`] and served by [`serve_command`].
///
/// The ids here are **MCP** ids ([`McpSessionId`]); they carry the same
/// number as the workbench's own [`SessionId`], which is what makes an id an
/// agent saw in `list_sessions` the same id the user's tab shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpCommand {
    /// Every session the MCP vocabulary can describe, oldest id first.
    Sessions(Reply<Vec<SessionSnapshot>>),
    /// One session's snapshot.
    Session {
        /// The session asked about.
        id: McpSessionId,
        /// Where the answer goes.
        reply: Reply<Option<SessionSnapshot>>,
    },
    /// A session's retained log lines (the most recent `tail`, or all).
    Logs {
        /// The session asked about.
        id: McpSessionId,
        /// How many of the newest lines to return; `None` for all retained.
        tail: Option<usize>,
        /// Where the answer goes.
        reply: Reply<Option<Vec<String>>>,
    },
    /// A session's retained devtools frame-stats samples, oldest first.
    FrameRing {
        /// The session asked about.
        id: McpSessionId,
        /// Where the answer goes.
        reply: Reply<Option<Vec<FrameStats>>>,
    },
    /// The latest system-metrics sample of each kind for a session.
    LatestMetrics {
        /// The session asked about.
        id: McpSessionId,
        /// Where the answer goes.
        reply: Reply<Option<LatestMetrics>>,
    },
    /// Launch a session in the workbench's active project, exactly as the
    /// run-config modal would.
    RunApp {
        /// Where it runs (already translated out of MCP's `RunTarget`).
        target: DeviceTarget,
        /// The build mode (`debug`/`profile`).
        mode: BuildMode,
        /// Where the new session's id (or an at-cap refusal, see
        /// [`MCP_RECORD_CAP`]) goes.
        reply: Reply<Result<McpSessionId, EmbeddedError>>,
    },
    /// Stop a session (the supervisor's group-kill).
    StopApp {
        /// The session to stop.
        id: McpSessionId,
        /// Where the outcome goes.
        reply: Reply<Result<(), EmbeddedError>>,
    },
    /// Stop a session and relaunch the same spec as a new one.
    RestartApp {
        /// The session to restart.
        id: McpSessionId,
        /// Where the new session's id (or the refusal) goes.
        reply: Reply<Result<McpSessionId, EmbeddedError>>,
    },
    /// Open a live feed of a session's log lines and its end — what a DAP
    /// client's output/exit pumps read instead of re-polling
    /// [`McpCommand::Logs`] and diffing.
    ///
    /// The workbench answers with the receiving half and keeps the sending
    /// half in [`SessionSubscribers`], which `crate::runner` feeds from every
    /// session transition thereafter.
    SubscribeSessionEvents {
        /// The session to follow.
        id: McpSessionId,
        /// Where the feed goes (`None` for a session this backend cannot
        /// describe).
        reply: Reply<Option<SessionEventFeed>>,
    },
    /// Where a launch issued right now would build: the workbench's currently
    /// open project, or `None` when none is open.
    ///
    /// Asked per launch rather than remembered by the consumer — the user can
    /// switch projects at any time, and a DAP client is told (and builds in)
    /// the project the workbench is on when it launches, not the one it was on
    /// when the server started.
    ProjectRoot {
        /// Where the answer goes.
        reply: Reply<Option<std::path::PathBuf>>,
    },
    /// Pull one `widget_tree` dump for a session, through the **workbench's
    /// own** devtools connection.
    ///
    /// Answered from the bridge's completion path rather than inline (see
    /// [`PendingWidgetTrees`]): the caller is blocked on
    /// [`REPLY_DEADLINE`] meanwhile, and the bridge's own request timeout is
    /// shorter, so a live pull lands inside the caller's wait.
    WidgetTree {
        /// The session whose tree to pull.
        id: McpSessionId,
        /// Where the tree (or the typed refusal) goes.
        reply: Reply<Result<serde_json::Value, TreeRefusal>>,
    },
}

// ── Launch records ──────────────────────────────────────────────────────────

/// What one MCP-describable session was launched from.
///
/// The workbench's own [`SessionView`] keeps no spec — it is a view-model of
/// output, not of a launch — so the runner records this beside it for every
/// session started through [`Supervisor::start`]. It is what makes
/// `restart_app` possible and what supplies a snapshot's `target`, `mode`,
/// and `started_at`.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    /// The spec the session was started from (and would be restarted from).
    pub spec: SessionSpec,
    /// When the launch was requested.
    pub started_at: SystemTime,
    /// Set when the launch never got off the ground (the process could not
    /// be spawned): the session exists only to carry the error, and reports
    /// MCP's `failed` state rather than a plain unsuccessful exit.
    pub launch_error: Option<String>,
}

/// Every session's [`SessionRecord`], owned by `crate::runner` alongside the
/// [`Supervisor`].
///
/// Bounded at [`MCP_RECORD_CAP`], oldest-terminal-evicted-first, live never
/// evicted — see [`Self::retain_bounded`]. That bound is enforced only on
/// the two paths a launch reaches through this module (`run_app`'s
/// [`start_session`]/[`failed_launch`]): `crate::runner::launch_sessions`
/// (the run-config modal's own launches) also inserts here — an agent must
/// see the sessions a *human* started too, not only its own — but does not
/// call [`Self::retain_bounded`], so a workbench driven purely by its human
/// user for a very long session can still grow this map unboundedly. That
/// path is out of this fix's scope (it needs no MCP server running at all,
/// so `MCP_RECORD_CAP`'s reasoning — an *agent* driving for hours — does not
/// apply the same way), but it is the same map, so the gap is recorded here
/// rather than left implicit.
#[derive(Debug, Default)]
pub struct McpSessionRecords {
    by_id: HashMap<SessionId, SessionRecord>,
}

impl McpSessionRecords {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a just-started session's launch.
    pub fn insert(&mut self, id: SessionId, spec: SessionSpec) {
        self.by_id.insert(
            id,
            SessionRecord {
                spec,
                started_at: SystemTime::now(),
                launch_error: None,
            },
        );
    }

    /// Record a launch that failed before a session could start, against the
    /// ad-hoc id its error is reported under.
    pub fn insert_failed(&mut self, id: SessionId, spec: SessionSpec, error: String) {
        self.by_id.insert(
            id,
            SessionRecord {
                spec,
                started_at: SystemTime::now(),
                launch_error: Some(error),
            },
        );
    }

    /// One session's record, if it has one.
    pub fn get(&self, id: SessionId) -> Option<&SessionRecord> {
        self.by_id.get(&id)
    }

    /// Evict oldest-terminal-first past [`MCP_RECORD_CAP`], mirroring
    /// `frust_mcp::engine`'s own `insert_retaining` shape.
    ///
    /// A record alone carries no live [`McpSessionState`] — only joining
    /// against `state`'s session views through [`session_state`] can say
    /// whether its session has actually ended (see this type's doc). A
    /// record this pass cannot find a view for (the just-launched one, whose
    /// `RegisterSession` the caller posted but the loop has not applied yet
    /// — see [`serve_command`]'s ordering note) is treated as live, never
    /// terminal: unprovable terminal-ness must never be evicted.
    fn retain_bounded(&mut self, state: &AppState) {
        let mut terminal: Vec<(SessionId, SystemTime)> = self
            .by_id
            .iter()
            .filter(|(id, record)| record_is_terminal(state, **id, record))
            .map(|(id, record)| (*id, record.started_at))
            .collect();
        terminal.sort_by_key(|(_, started_at)| *started_at);
        let excess = terminal.len().saturating_sub(MCP_RECORD_CAP);
        for (id, _) in terminal.into_iter().take(excess) {
            self.by_id.remove(&id);
        }
    }

    /// How many MCP-launched sessions are live right now (the same
    /// terminal-ness join [`Self::retain_bounded`] uses, inverted),
    /// optionally excluding one session — `restart_app` excludes the session
    /// it is about to stop and relaunch, so a 1-for-1 restart already at the
    /// cap never refuses itself.
    fn live_count(&self, state: &AppState, exclude: Option<SessionId>) -> usize {
        self.by_id
            .iter()
            .filter(|(id, _)| Some(**id) != exclude)
            .filter(|(id, record)| !record_is_terminal(state, **id, record))
            .count()
    }
}

/// Whether `id`'s record is provably terminal, joining against `state`'s
/// session views through [`session_state`] — see
/// [`McpSessionRecords::retain_bounded`]. `false` (treated as live) when
/// `state` does not yet contain a view for `id`, never assumed.
fn record_is_terminal(state: &AppState, id: SessionId, record: &SessionRecord) -> bool {
    state
        .sessions
        .iter()
        .find(|view| view.id == id)
        .is_some_and(|view| session_state(view, record).is_terminal())
}

// ── The running server's handle ─────────────────────────────────────────────

/// The embedded MCP server the workbench is currently running.
///
/// Held on [`AppState`] (the model the render layer reads) but created and
/// destroyed only by [`crate::engine::Engine::start_mcp`]/`stop_mcp` — it is
/// a live resource handle rather than a value, so it is the one piece of
/// state the pure `update` does not construct. Cloning the model clones the
/// handle: both clones name the same server.
#[derive(Clone)]
pub struct McpServerHandle {
    /// Which server instance this is (`Engine`'s monotonic counter). A
    /// stopping server's tasks outlive the handle — axum's graceful shutdown
    /// runs on after `stop_mcp` has dropped it — so their late reports carry
    /// this number and `update` ignores any that does not name the
    /// *currently installed* server. Without it, a stop report from the
    /// previous server would clear a newly started one's handle, and since
    /// dropping a `CancellationToken` does **not** cancel it, that server
    /// would go on listening with nothing left able to stop it.
    generation: u64,
    /// What stops the server (`frust_mcp::serve_embedded`'s shutdown seam).
    cancel: tokio_util::sync::CancellationToken,
    /// The connected-client registry the server registers each MCP session
    /// in — the status badge's live source.
    registry: frust_mcp::ClientRegistry,
    /// `None` until the listener reports the address it actually bound (the
    /// only way to learn an OS-assigned port from `bind_port = 0`).
    bound_port: Option<u16>,
}

impl McpServerHandle {
    /// A handle for a server that has been spawned but has not reported its
    /// bound address yet, tagged with the `generation`
    /// [`crate::engine::Engine::start_mcp`] minted for it.
    pub(crate) fn starting(
        generation: u64,
        cancel: tokio_util::sync::CancellationToken,
        registry: frust_mcp::ClientRegistry,
    ) -> Self {
        Self {
            generation,
            cancel,
            registry,
            bound_port: None,
        }
    }

    /// Which server instance this handle names — the tag every report from
    /// that instance carries, so a superseded server's late report can be
    /// told apart from the running one's.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Record the port the listener actually bound. Returns whether this
    /// changed anything (so the caller knows whether to redraw).
    pub(crate) fn set_bound_port(&mut self, port: u16) -> bool {
        let changed = self.bound_port != Some(port);
        self.bound_port = Some(port);
        changed
    }

    /// Ask the server to shut down. Idempotent.
    pub(crate) fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Every client connected to this server right now, oldest first — the
    /// MCP panel's rows (workbook §B13). Read through
    /// [`crate::engine::AppState::mcp_clients`] rather than off the handle.
    pub fn clients(&self) -> Vec<frust_mcp::ClientEntry> {
        self.registry.snapshot()
    }

    /// What the status badge shows right now.
    pub fn status(&self) -> McpStatus {
        match self.bound_port {
            Some(port) => McpStatus::Listening {
                port,
                clients: self.registry.count(),
            },
            None => McpStatus::Starting,
        }
    }
}

impl std::fmt::Debug for McpServerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `ClientRegistry` is not `Debug`; the live count is the useful part.
        f.debug_struct("McpServerHandle")
            .field("generation", &self.generation)
            .field("bound_port", &self.bound_port)
            .field("clients", &self.registry.count())
            .finish()
    }
}

/// What the workbench's embedded MCP server is doing — the whole surface the
/// UI needs (and the only one it should read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpStatus {
    /// No server is running.
    Stopped,
    /// Spawned; the listener has not reported its bound address yet.
    Starting,
    /// Listening on `127.0.0.1:port` with `clients` MCP clients connected.
    Listening {
        /// The bound loopback port.
        port: u16,
        /// How many MCP clients are connected right now.
        clients: usize,
    },
}

// ── The backend ─────────────────────────────────────────────────────────────

/// `frust-mcp`'s [`SessionBackend`], served by the running workbench.
///
/// Cheap to clone into an `Arc` and hand to `frust_mcp::serve_embedded`; it
/// holds only the engine's message sender and the shared process runner.
pub struct TuiSessionBackend {
    tx: UnboundedSender<Message>,
    runner: Arc<dyn ProcessRunner + Send + Sync>,
}

impl TuiSessionBackend {
    /// A backend posting into `tx` (the engine's channel — see
    /// [`crate::engine::Engine::sender`]) and shelling out through `runner`
    /// (the same [`ProcessRunner`] the supervisor uses).
    pub fn new(tx: UnboundedSender<Message>, runner: Arc<dyn ProcessRunner + Send + Sync>) -> Self {
        Self { tx, runner }
    }

    /// Post one command and block on its answer (see the module doc's
    /// deadlock note). `Err` means the workbench never answered.
    fn ask<T>(&self, build: impl FnOnce(Reply<T>) -> McpCommand) -> Result<T, EmbeddedError> {
        let (reply, rx) = Reply::channel();
        if self.tx.send(Message::Mcp(build(reply))).is_err() {
            return Err(EmbeddedError::WorkbenchUnreachable);
        }
        match rx.recv_timeout(REPLY_DEADLINE) {
            Ok(value) => Ok(value),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                Err(EmbeddedError::WorkbenchUnreachable)
            }
        }
    }
}

impl SessionBackend for TuiSessionBackend {
    fn list_devices(&self) -> (Vec<Device>, Vec<String>) {
        // The one method that needs nothing from the event loop: discovery is
        // a `frust-drive` call over the shared runner, exactly as the
        // workbench's own device panel refreshes it.
        discover_all(self.runner.as_ref(), &default_discoverers())
    }

    fn sessions(&self) -> Vec<SessionSnapshot> {
        // An unreachable workbench has no sessions left to enumerate — the
        // loop that owned them is gone. Reporting an empty set is the fact,
        // not a placeholder.
        self.ask(McpCommand::Sessions).unwrap_or_default()
    }

    fn session(&self, id: McpSessionId) -> Option<SessionSnapshot> {
        self.ask(|reply| McpCommand::Session { id, reply })
            .ok()
            .flatten()
    }

    fn run_app(&self, target: RunTarget, mode: BuildMode) -> McpSessionId {
        let target = device_target_for(target);
        self.ask(|reply| McpCommand::RunApp {
            target,
            mode,
            reply,
        })
        .and_then(|inner| inner)
        // The trait has no failure channel here; a workbench that never
        // answered — or one that refused at `MCP_RECORD_CAP` — started
        // nothing, so report the id that resolves to no session at all
        // rather than one that might resolve to another.
        .unwrap_or(UNRESOLVED_SESSION)
    }

    fn stop_app(&self, id: McpSessionId) -> anyhow::Result<()> {
        self.ask(|reply| McpCommand::StopApp { id, reply })??;
        Ok(())
    }

    fn restart_app(&self, id: McpSessionId) -> anyhow::Result<McpSessionId> {
        Ok(self.ask(|reply| McpCommand::RestartApp { id, reply })??)
    }

    fn logs(&self, id: McpSessionId, tail: Option<usize>) -> Option<Vec<String>> {
        self.ask(|reply| McpCommand::Logs { id, tail, reply })
            .ok()
            .flatten()
    }

    fn frame_ring(&self, id: McpSessionId) -> Option<Vec<FrameStats>> {
        self.ask(|reply| McpCommand::FrameRing { id, reply })
            .ok()
            .flatten()
    }

    fn latest_metrics(&self, id: McpSessionId) -> Option<LatestMetrics> {
        self.ask(|reply| McpCommand::LatestMetrics { id, reply })
            .ok()
            .flatten()
    }

    fn devtools_client(&self, _id: McpSessionId) -> Option<Arc<DevtoolsClient>> {
        // Never shareable from here — see the module doc. The reason travels
        // on every snapshot's `devtools_error`, so the tools explain it.
        None
    }

    fn subscribe_session_events(&self, id: McpSessionId) -> Option<SessionEventFeed> {
        // An unreachable workbench yields `None` — the same answer an unknown
        // id gets, and the honest one: there is no session world left to
        // follow.
        self.ask(|reply| McpCommand::SubscribeSessionEvents { id, reply })
            .ok()
            .flatten()
    }

    fn fetch_widget_tree(&self, id: McpSessionId) -> anyhow::Result<serde_json::Value> {
        // Unlike `devtools_client`, this *is* answerable in embedded mode: the
        // workbench cannot lend out its socket, but it can pull through it and
        // hand back the result (see `PendingWidgetTrees`).
        match self.ask(|reply| McpCommand::WidgetTree { id, reply }) {
            Ok(Ok(tree)) => Ok(tree),
            Ok(Err(refusal)) => Err(refusal.into_error(id)),
            Err(unreachable) => Err(anyhow::Error::new(unreachable)),
        }
    }

    fn project_root(&self) -> Option<std::path::PathBuf> {
        // Read live, every time: the user can switch projects while a server
        // is running, and the answer must be the project the *next* launch
        // builds in. An unreachable workbench has no open project to name.
        self.ask(|reply| McpCommand::ProjectRoot { reply })
            .ok()
            .flatten()
    }

    fn runner(&self) -> Arc<dyn ProcessRunner + Send + Sync> {
        Arc::clone(&self.runner)
    }
}

// ── Serving a command, on the event loop ────────────────────────────────────

/// Everything serving one [`McpCommand`] needs from `crate::runner`'s scope.
///
/// Public because it is the embedding seam: a host that runs its own event
/// loop over this engine (the workbench itself, or a test standing in for it)
/// serves MCP commands through [`serve_command`] with exactly these handles.
pub struct McpServeCtx<'a> {
    /// The model, read-only — this path never mutates state directly; a
    /// launch registers itself through [`Message::RegisterSession`] like
    /// every other one.
    pub state: &'a AppState,
    /// The session supervisor (the launch/stop path).
    pub supervisor: &'a mut Supervisor,
    /// The launch records backing snapshots and `restart_app`.
    pub records: &'a mut McpSessionRecords,
    /// The engine channel registrations and log lines are posted through.
    pub tx: &'a UnboundedSender<Message>,
    /// The runner's ad-hoc id counter (see [`MAX_ADHOC_SESSION_ID`]), used
    /// only for a launch that failed to start.
    pub next_adhoc_id: &'a mut u64,
    /// The open session-event feeds a
    /// [`McpCommand::SubscribeSessionEvents`] registers into.
    pub subscribers: &'a mut SessionSubscribers,
    /// The per-session devtools connection threads — the only way to reach a
    /// running app's `widget_tree` from here.
    pub devtools: &'a mut DevtoolsBridge,
    /// The widget-tree pulls waiting on a bridge report.
    pub pending_trees: &'a mut PendingWidgetTrees,
}

/// Serve one command from the event loop and answer its [`Reply`].
///
/// Never blocks: every answer is computed from `state`/`records` or is a
/// request to the supervisor that returns as soon as the kill/launch is
/// issued. A `stop_app` is therefore a *request* — the session's terminal
/// state follows asynchronously through the normal event path, exactly as it
/// does for a user-pressed stop.
///
/// Ordering: a launch posts its [`Message::RegisterSession`] *before* the
/// reply is sent, and the caller's next command travels the same channel
/// behind it, so an agent that calls `session(id)` straight after `run_app`
/// always finds the session registered.
pub fn serve_command(cmd: McpCommand, ctx: &mut McpServeCtx<'_>) {
    match cmd {
        McpCommand::Sessions(reply) => reply.send(snapshots(ctx.state, ctx.records)),
        McpCommand::Session { id, reply } => {
            reply.send(snapshot_of(ctx.state, ctx.records, id));
        }
        McpCommand::Logs { id, tail, reply } => {
            reply.send(mcp_view(ctx.state, ctx.records, id).map(|view| log_tail(view, tail)));
        }
        McpCommand::FrameRing { id, reply } => {
            reply.send(
                mcp_view(ctx.state, ctx.records, id)
                    .map(|view| view.devtools.frames.iter().cloned().collect()),
            );
        }
        McpCommand::LatestMetrics { id, reply } => {
            reply.send(mcp_view(ctx.state, ctx.records, id).map(latest_metrics));
        }
        McpCommand::RunApp {
            target,
            mode,
            reply,
        } => reply.send(run_app(ctx, target, mode)),
        McpCommand::StopApp { id, reply } => {
            let Some(session) = mcp_view(ctx.state, ctx.records, id).map(|view| view.id) else {
                reply.send(Err(EmbeddedError::NoSuchSession(id.0)));
                return;
            };
            ctx.supervisor.stop(session);
            reply.send(Ok(()));
        }
        McpCommand::RestartApp { id, reply } => reply.send(restart_app(ctx, id)),
        McpCommand::SubscribeSessionEvents { id, reply } => {
            reply.send(subscribe_session_events(ctx, id));
        }
        McpCommand::ProjectRoot { reply } => reply.send(ctx.state.project_root.clone()),
        McpCommand::WidgetTree { id, reply } => serve_widget_tree(ctx, id, reply),
    }
}

/// Open a live feed over one session.
///
/// `None` for a session this backend cannot describe at all — the same
/// omission [`snapshots`] makes, so a consumer never gets a feed for an id
/// `list_sessions` never showed it.
///
/// A session that has **already** ended is handed its retained lines followed
/// immediately by its `Exited`, rather than being registered for a transition
/// that will never come again — the rule that keeps a late subscriber from
/// waiting forever on a dead app.
fn subscribe_session_events(
    ctx: &mut McpServeCtx<'_>,
    id: McpSessionId,
) -> Option<SessionEventFeed> {
    let view = mcp_view(ctx.state, ctx.records, id)?;
    let record = ctx.records.get(view.id)?;
    let state = session_state(view, record);
    let terminal = state.is_terminal().then_some(state);
    Some(ctx.subscribers.subscribe(view, terminal))
}

/// Ask the workbench's own devtools connection for a fresh `widget_tree`.
///
/// Nothing is answered inline: the pull runs on the session's
/// [`DevtoolsBridge`] thread and its result comes back as a
/// [`Message::DevtoolsInspector`], which `crate::runner` routes into
/// [`PendingWidgetTrees::resolve`]. The caller is blocked on
/// [`REPLY_DEADLINE`] throughout, and the bridge's own per-request timeout is
/// shorter than that, so a connected session answers inside the wait — with a
/// **freshly pulled** tree, never a cached one.
///
/// The two refusals are answered immediately, because neither can improve by
/// waiting: an id this backend cannot describe, and a session whose devtools
/// connection is not up.
fn serve_widget_tree(
    ctx: &mut McpServeCtx<'_>,
    id: McpSessionId,
    reply: Reply<Result<serde_json::Value, TreeRefusal>>,
) {
    let Some(view) = mcp_view(ctx.state, ctx.records, id) else {
        reply.send(Err(TreeRefusal::Unavailable(format!(
            "no such session in the workbench: {id}"
        ))));
        return;
    };
    if !matches!(view.devtools.conn, ConnState::Connected { .. }) {
        reply.send(Err(TreeRefusal::Unavailable(no_devtools_reason(view))));
        return;
    }
    let session = view.id;
    ctx.pending_trees.push(session, reply);
    ctx.devtools.fetch_tree(session, ctx.tx);
}

/// Why session `view` has no widget tree to give right now — the workbench's
/// own connection state, phrased for a consumer that is not looking at the
/// terminal.
fn no_devtools_reason(view: &SessionView) -> String {
    let detail = match &view.devtools.conn {
        ConnState::Connected { .. } => None,
        ConnState::Failed { error } => Some(error.clone()),
        ConnState::Idle | ConnState::Connecting => view.devtools.start_error.clone(),
    };
    let base = format!(
        "session {} has no devtools connection in the workbench, so there is no widget tree \
         to read; run the app with devtools enabled and let it announce its service",
        view.id.0
    );
    match detail {
        Some(detail) => format!("{base}. The workbench reports: {detail}"),
        None => base,
    }
}

/// Launch one session for the workbench's active project.
///
/// Refuses **before any bookkeeping** — no spec built, no supervisor call,
/// no record, no `RegisterSession` — in two cases: the workbench already has
/// a live session for this project on this target
/// ([`EmbeddedError::AlreadyRunning`]), or [`MCP_RECORD_CAP`] MCP-launched
/// sessions are already live ([`EmbeddedError::TooManySessions`]). A refusal
/// that still registered a session would grow `AppState::sessions` faster
/// than a successful launch does.
///
/// Otherwise, a launch that cannot start (no project open, or a spawn
/// failure) still produces a session: it is registered under an ad-hoc id
/// carrying the error as its one log line and MCP's `failed` state,
/// mirroring `frust-mcp`'s own engine — an agent reads *why*, instead of
/// getting an id that names nothing.
fn run_app(
    ctx: &mut McpServeCtx<'_>,
    target: DeviceTarget,
    mode: BuildMode,
) -> Result<McpSessionId, EmbeddedError> {
    // The duplicate guard runs first, and against the *workbench's* sessions
    // rather than only MCP-launched ones: the session already on this target
    // is as likely to be one the user started by hand, and launching a second
    // one would replace its app behind its own still-streaming tab.
    if let Some(project_root) = ctx.state.project_root.as_deref() {
        let identity = SessionTarget::of(&target);
        if let Some(live) = ctx.state.live_session_for(project_root, &identity) {
            return Err(EmbeddedError::AlreadyRunning {
                session: live.0,
                target: identity.label(),
            });
        }
    }
    if ctx.records.live_count(ctx.state, None) >= MCP_RECORD_CAP {
        return Err(EmbeddedError::TooManySessions {
            cap: MCP_RECORD_CAP,
        });
    }
    let label = target_label(&target);
    let Some(project_root) = ctx.state.project_root.clone() else {
        return Ok(failed_launch(
            ctx,
            None,
            label,
            "no project is open in the workbench — open one first (the project switcher), \
             then run_app again"
                .to_string(),
        ));
    };
    let spec = SessionSpec {
        project_root,
        target,
        build: BuildInfo {
            mode,
            flavor: None,
            defines: std::collections::HashMap::new(),
            build_name: None,
            build_number: None,
        },
    };
    Ok(start_session(ctx, spec))
}

/// Start `spec` through the supervisor, register the session, record its
/// launch, and report its id — the shared body of `run_app` and
/// `restart_app`.
fn start_session(ctx: &mut McpServeCtx<'_>, spec: SessionSpec) -> McpSessionId {
    let label = target_label(&spec.target);
    match ctx.supervisor.start(&spec) {
        Ok(id) => {
            let _ = ctx.tx.send(Message::RegisterSession {
                id,
                project_root: spec.project_root.clone(),
                target_label: label,
                devtools: devtools_launch(&spec),
                // An MCP/DAP launch owns its target exactly like a
                // user-driven one — one session world, one guard.
                target: Some(SessionTarget::of(&spec.target)),
            });
            ctx.records.insert(id, spec);
            ctx.records.retain_bounded(ctx.state);
            McpSessionId(id.0)
        }
        Err(err) => failed_launch(ctx, Some(spec), label, format!("{err:#}")),
    }
}

/// Register a session that exists only to report why a launch never started.
fn failed_launch(
    ctx: &mut McpServeCtx<'_>,
    spec: Option<SessionSpec>,
    label: String,
    error: String,
) -> McpSessionId {
    let id = SessionId(*ctx.next_adhoc_id);
    *ctx.next_adhoc_id = ctx.next_adhoc_id.saturating_sub(1);
    // With no spec there is no project to group the tab under; the same
    // synthetic root an ad-hoc toolchain session uses keeps it distinct.
    let spec = spec.unwrap_or_else(|| SessionSpec {
        project_root: std::path::PathBuf::from("mcp"),
        target: DeviceTarget::Desktop,
        build: BuildInfo {
            mode: BuildMode::Debug,
            flavor: None,
            defines: std::collections::HashMap::new(),
            build_name: None,
            build_number: None,
        },
    });
    let _ = ctx.tx.send(Message::RegisterSession {
        id,
        project_root: spec.project_root.clone(),
        target_label: label,
        devtools: DevtoolsLaunch::unavailable(),
        // A launch that never started occupies no target: this tab exists
        // only to carry the error, and is `Exited` a message later anyway.
        target: None,
    });
    let _ = ctx.tx.send(Message::Session(SessionEvent {
        id,
        kind: SessionEventKind::Lines(vec![format!("error: {error}")]),
    }));
    let _ = ctx.tx.send(Message::Session(SessionEvent {
        id,
        kind: SessionEventKind::State(SessionState::Exited(false)),
    }));
    ctx.records.insert_failed(id, spec, error);
    ctx.records.retain_bounded(ctx.state);
    McpSessionId(id.0)
}

/// Stop a session and relaunch its own spec as a new one.
///
/// Refused at [`MCP_RECORD_CAP`] like `run_app`, but excluding the session
/// being restarted from the live count: a 1-for-1 restart nets no growth in
/// live sessions, so it must not be refused just because the cap is already
/// exactly met. The duplicate guard is excluded the same way and for the same
/// reason — the session it would trip over is the one being replaced — while
/// any *other* live session on that target still refuses it.
fn restart_app(ctx: &mut McpServeCtx<'_>, id: McpSessionId) -> Result<McpSessionId, EmbeddedError> {
    let Some(view) = mcp_view(ctx.state, ctx.records, id) else {
        return Err(EmbeddedError::NoSuchSession(id.0));
    };
    let session = view.id;
    let Some(record) = ctx.records.get(session) else {
        return Err(EmbeddedError::NoSuchSession(id.0));
    };
    if record.launch_error.is_some() {
        return Err(EmbeddedError::Unsupported {
            what: "restart_app",
            why: "this session never launched — it exists only to report the error. \
                  Fix the cause and call run_app again",
        });
    }
    // The duplicate guard, excluding the session being restarted — it is
    // stopped below, so it must not refuse its own relaunch. Any *other* live
    // session on that target still does.
    let identity = SessionTarget::of(&record.spec.target);
    if let Some(live) =
        ctx.state
            .live_session_for_excluding(&record.spec.project_root, &identity, Some(session))
    {
        return Err(EmbeddedError::AlreadyRunning {
            session: live.0,
            target: identity.label(),
        });
    }
    if ctx.records.live_count(ctx.state, Some(session)) >= MCP_RECORD_CAP {
        return Err(EmbeddedError::TooManySessions {
            cap: MCP_RECORD_CAP,
        });
    }
    let spec = record.spec.clone();
    ctx.supervisor.stop(session);
    Ok(start_session(ctx, spec))
}

// ── Snapshot mapping ────────────────────────────────────────────────────────

/// Every MCP-describable session, oldest id first.
fn snapshots(state: &AppState, records: &McpSessionRecords) -> Vec<SessionSnapshot> {
    let mut all: Vec<SessionSnapshot> = state
        .sessions
        .iter()
        .filter_map(|view| {
            records
                .get(view.id)
                .and_then(|record| snapshot(view, record))
        })
        .collect();
    all.sort_by_key(|snapshot| snapshot.id.0);
    all
}

fn snapshot_of(
    state: &AppState,
    records: &McpSessionRecords,
    id: McpSessionId,
) -> Option<SessionSnapshot> {
    let view = mcp_view(state, records, id)?;
    snapshot(view, records.get(view.id)?)
}

/// The workbench's view for an MCP id, **only** when it is one this backend
/// can describe (see the module doc's omissions).
fn mcp_view<'a>(
    state: &'a AppState,
    records: &McpSessionRecords,
    id: McpSessionId,
) -> Option<&'a SessionView> {
    let session = SessionId(id.0);
    let record = records.get(session)?;
    run_target_for(&record.spec.target)?;
    state.sessions.iter().find(|view| view.id == session)
}

/// The MCP `RunTarget` for a workbench target, or `None` for a shape MCP
/// cannot name (a physical iOS device — reporting it as a Simulator would be
/// a lie).
fn run_target_for(target: &DeviceTarget) -> Option<RunTarget> {
    match target {
        DeviceTarget::Desktop => Some(RunTarget::Desktop),
        DeviceTarget::Device(device) => match (device.platform, device.kind) {
            (Platform::Android, _) => Some(RunTarget::Android(device.clone())),
            (Platform::Ios, Kind::PhysicalDevice) => None,
            (Platform::Ios, _) => Some(RunTarget::IosSimulator(device.clone())),
        },
    }
}

/// The workbench target for an MCP one — the inverse of [`run_target_for`].
/// The supervisor dispatches on the device's own platform/kind, so a
/// Simulator and a physical device travel the same variant here.
fn device_target_for(target: RunTarget) -> DeviceTarget {
    match target {
        RunTarget::Desktop => DeviceTarget::Desktop,
        RunTarget::Android(device) | RunTarget::IosSimulator(device) => {
            DeviceTarget::Device(device)
        }
    }
}

/// The short tab label a launch registers under (`desktop`, or the device
/// name) — the same shape `crate::runner`'s own launches use.
fn target_label(target: &DeviceTarget) -> String {
    match target {
        DeviceTarget::Desktop => "desktop".to_string(),
        DeviceTarget::Device(device) => device.name.clone(),
    }
}

/// What a launched session's config says about reaching a devtools service —
/// the same derivation `crate::runner::devtools_launch` makes, kept here so a
/// session an agent launched behaves exactly like one the user launched.
fn devtools_launch(spec: &SessionSpec) -> DevtoolsLaunch {
    let android_serial = match &spec.target {
        DeviceTarget::Device(device) if device.platform == Platform::Android => {
            Some(device.id.clone())
        }
        DeviceTarget::Device(_) | DeviceTarget::Desktop => None,
    };
    DevtoolsLaunch::from_launch(spec.build.mode, android_serial)
}

/// One session's snapshot in MCP's vocabulary, or `None` for a session MCP
/// cannot name at all (see [`run_target_for`]).
///
/// Absences are deliberate, and each one is a fact rather than a gap filled
/// with a zero:
///
/// - `pid`/`android_package`/`ios_bundle_id` — the workbench's supervise
///   layer never parses them out of a session's output (only its DevTools
///   metrics identity does, and not into this shape).
/// - `devtools_handshake` — the bridge reports the app name and capabilities
///   but not the protocol/frust versions `HandshakeInfo` requires, so there
///   is no complete value to report.
/// - `dropped_frames` — the DevTools frame ring drops oldest without
///   counting; `0` here means "not counted", not "none dropped".
fn snapshot(view: &SessionView, record: &SessionRecord) -> Option<SessionSnapshot> {
    Some(SessionSnapshot {
        id: McpSessionId(view.id.0),
        target: run_target_for(&record.spec.target)?,
        mode: record.spec.build.mode,
        project_root: view.project_root.clone(),
        state: session_state(view, record),
        started_at: record.started_at,
        pid: None,
        android_package: None,
        ios_bundle_id: None,
        devtools_port: devtools_port(view),
        devtools_handshake: None,
        devtools_error: devtools_error(view),
        log_lines: view.log.len(),
        dropped_log_lines: view.dropped,
        frames: view.devtools.frames.len(),
        dropped_frames: 0,
        metrics_sampling: matches!(view.devtools.metrics.sampling, SamplingState::On),
    })
}

/// The MCP lifecycle position of a workbench session.
///
/// The workbench's pre-`Running` phases (`Configuring`/`Building`/
/// `Installing`) are all MCP's `launching`; a `Killed` session reports
/// `exited` with `success: false`, the same as `frust-mcp`'s own engine
/// reports a session it stopped.
fn session_state(view: &SessionView, record: &SessionRecord) -> McpSessionState {
    if let Some(reason) = &record.launch_error {
        return McpSessionState::Failed {
            reason: reason.clone(),
        };
    }
    live_session_state(view)
}

/// [`session_state`] against the launch records rather than one record —
/// what `crate::runner` closes a session-event feed with when a session
/// reaches a terminal state.
///
/// A session with no record at all (an ad-hoc build/clean/toolchain tab) still
/// has an honest lifecycle state; it just has no recorded launch failure to
/// override it with.
pub fn mcp_session_state(records: &McpSessionRecords, view: &SessionView) -> McpSessionState {
    match records.get(view.id) {
        Some(record) => session_state(view, record),
        None => live_session_state(view),
    }
}

/// The lifecycle mapping proper, with no launch-record override applied.
fn live_session_state(view: &SessionView) -> McpSessionState {
    match &view.state {
        SessionState::Configuring | SessionState::Building | SessionState::Installing => {
            McpSessionState::Launching
        }
        SessionState::Running => {
            if matches!(view.devtools.conn, ConnState::Connected { .. }) {
                McpSessionState::DevtoolsConnected
            } else {
                McpSessionState::Running
            }
        }
        SessionState::Exited(success) => McpSessionState::Exited { success: *success },
        SessionState::Killed => McpSessionState::Exited { success: false },
    }
}

/// The **local** loopback port the snapshot's field means.
///
/// Only reportable for a non-Android session, where the announced port is
/// already a host port. An Android session reaches its service through an
/// ephemeral `adb forward` the bridge thread owns and never publishes, so
/// reporting the *device* port under a field documented as the local one
/// would send an agent to the wrong socket.
fn devtools_port(view: &SessionView) -> Option<u16> {
    if view.devtools.launch.android_serial.is_some() {
        return None;
    }
    view.devtools.discovered.as_ref().map(|d| d.port)
}

/// Why devtools is unavailable *to the MCP tools* for this session.
///
/// **Always populated in embedded mode**, because the answer is always the
/// same: the tools cannot borrow the workbench's connection
/// ([`DEVTOOLS_OWNED_BY_WORKBENCH`]), whether or not that connection is up.
/// Reporting only "the app is not connected" would imply the tool would start
/// working once it connects, which in embedded mode it never will. Whatever
/// the workbench itself has to say — a failed connect, or the app's own
/// "service did not start" line — is appended rather than dropped.
fn devtools_error(view: &SessionView) -> Option<String> {
    let workbench_reason = match &view.devtools.conn {
        ConnState::Connected { .. } => None,
        ConnState::Failed { error } => Some(error.clone()),
        ConnState::Idle | ConnState::Connecting => view.devtools.start_error.clone(),
    };
    Some(match workbench_reason {
        Some(reason) => format!("{DEVTOOLS_OWNED_BY_WORKBENCH}. The workbench reports: {reason}"),
        None => DEVTOOLS_OWNED_BY_WORKBENCH.to_string(),
    })
}

/// A session's retained log lines: the newest `tail`, or all of them.
fn log_tail(view: &SessionView, tail: Option<usize>) -> Vec<String> {
    let lines: Vec<String> = view.log.iter().map(|(_, line)| line.to_string()).collect();
    match tail {
        Some(n) if n < lines.len() => lines[lines.len() - n..].to_vec(),
        _ => lines,
    }
}

/// The latest sample of each metrics kind the workbench retains.
///
/// `net` is deliberately absent: the workbench keeps a *derived* rate ring
/// and since-sampling-started totals, never the raw cumulative counters
/// `NetSample` means — reporting either under that name would misstate what
/// the number is.
fn latest_metrics(view: &SessionView) -> LatestMetrics {
    let metrics = &view.devtools.metrics;
    LatestMetrics {
        cpu: metrics
            .cpu
            .back()
            .map(|point| frust_drive::metrics::CpuSample {
                percent: point.percent,
                at_ms: point.at_ms,
            }),
        mem: metrics
            .rss
            .back()
            .map(|point| frust_drive::metrics::MemSample {
                rss_bytes: point.rss_bytes,
                at_ms: point.at_ms,
            }),
        net: None,
        thermal: metrics
            .thermal
            .iter()
            .map(|(zone, point)| frust_drive::metrics::ThermalSample {
                zone_label: zone.clone(),
                millideg_c: point.millideg_c,
                at_ms: point.at_ms,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_drive::devices::Device;
    use frust_drive::process::FakeProcessRunner;
    use frust_mcp::engine::SessionEvent as McpSessionEvent;
    use std::path::PathBuf;
    use tokio::sync::mpsc::unbounded_channel;

    fn device(platform: Platform, kind: Kind) -> Device {
        Device {
            id: "device-id".to_string(),
            name: "A Device".to_string(),
            platform,
            kind,
            os_version: None,
            connection_state: None,
        }
    }

    fn spec(target: DeviceTarget) -> SessionSpec {
        SessionSpec {
            project_root: PathBuf::from("/tmp/frust-tui-mcp-unit"),
            target,
            build: BuildInfo {
                mode: BuildMode::Debug,
                flavor: None,
                defines: HashMap::new(),
                build_name: None,
                build_number: None,
            },
        }
    }

    fn state_with(view: SessionView) -> AppState {
        AppState {
            sessions: vec![view],
            ..AppState::default()
        }
    }

    fn state_with_many(sessions: Vec<SessionView>) -> AppState {
        AppState {
            sessions,
            ..AppState::default()
        }
    }

    fn view(id: u64) -> SessionView {
        SessionView::new(
            SessionId(id),
            PathBuf::from("/tmp/frust-tui-mcp-unit"),
            "desktop",
        )
    }

    /// A *live* session that occupies `target` — what the duplicate guard
    /// reads, as opposed to [`view`]'s targetless ad-hoc tab.
    fn app_view(id: u64, target: SessionTarget) -> SessionView {
        let mut view = SessionView::with_devtools(
            SessionId(id),
            PathBuf::from("/tmp/frust-tui-mcp-unit"),
            target.label(),
            DevtoolsLaunch::unavailable(),
            Some(target),
        );
        view.state = SessionState::Running;
        view
    }

    /// An `AppState` open on the unit-test project, holding `sessions`.
    fn open_state(sessions: Vec<SessionView>) -> AppState {
        AppState {
            project_root: Some(PathBuf::from("/tmp/frust-tui-mcp-unit")),
            sessions,
            ..AppState::default()
        }
    }

    fn backend() -> TuiSessionBackend {
        let (tx, rx) = unbounded_channel();
        // The receiver is dropped on purpose in the callers that want the
        // "nobody is serving" path; those that don't, keep it alive.
        drop(rx);
        TuiSessionBackend::new(tx, Arc::new(FakeProcessRunner::new()))
    }

    #[test]
    fn a_desktop_session_maps_onto_the_mcp_vocabulary() {
        let mut records = McpSessionRecords::new();
        records.insert(SessionId(0), spec(DeviceTarget::Desktop));
        let mut view = view(0);
        view.state = SessionState::Building;
        view.push_line_at("hello".to_string(), "00:00:00");
        let state = state_with(view);

        let snapshots = snapshots(&state, &records);
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[0];
        assert_eq!(snapshot.id, McpSessionId(0));
        assert!(matches!(snapshot.target, RunTarget::Desktop));
        assert_eq!(snapshot.mode, BuildMode::Debug);
        // Every pre-`Running` workbench phase is MCP's `launching`.
        assert_eq!(snapshot.state, McpSessionState::Launching);
        assert_eq!(snapshot.log_lines, 1);
        assert!(
            snapshot
                .devtools_error
                .as_deref()
                .is_some_and(|reason| reason.contains("workbench owns this session")),
            "embedded mode always says why devtools tools are unavailable"
        );
    }

    #[test]
    fn a_killed_session_reports_an_unsuccessful_exit() {
        let mut records = McpSessionRecords::new();
        records.insert(SessionId(0), spec(DeviceTarget::Desktop));
        let mut view = view(0);
        view.state = SessionState::Killed;
        let state = state_with(view);

        assert_eq!(
            snapshot_of(&state, &records, McpSessionId(0)).map(|s| s.state),
            Some(McpSessionState::Exited { success: false })
        );
    }

    #[test]
    fn a_session_with_no_launch_record_is_not_an_mcp_session() {
        // An ad-hoc build/clean/toolchain session: a real workbench tab, but
        // nothing MCP's vocabulary can describe.
        let state = state_with(view(7));
        let records = McpSessionRecords::new();
        assert!(snapshots(&state, &records).is_empty());
        assert!(snapshot_of(&state, &records, McpSessionId(7)).is_none());
    }

    #[test]
    fn a_physical_ios_session_is_omitted_rather_than_reported_as_a_simulator() {
        let mut records = McpSessionRecords::new();
        records.insert(
            SessionId(0),
            spec(DeviceTarget::Device(device(
                Platform::Ios,
                Kind::PhysicalDevice,
            ))),
        );
        let state = state_with(view(0));
        assert!(
            snapshots(&state, &records).is_empty(),
            "`RunTarget` cannot name a physical iOS device; reporting one as a Simulator \
             would be a lie"
        );

        // A Simulator, by contrast, maps exactly.
        let mut records = McpSessionRecords::new();
        records.insert(
            SessionId(0),
            spec(DeviceTarget::Device(device(Platform::Ios, Kind::Simulator))),
        );
        assert!(matches!(
            snapshots(&state, &records).first().map(|s| &s.target),
            Some(RunTarget::IosSimulator(_))
        ));
    }

    #[test]
    fn a_failed_launch_reports_mcps_failed_state_with_the_reason() {
        let mut records = McpSessionRecords::new();
        records.insert_failed(
            SessionId(0),
            spec(DeviceTarget::Desktop),
            "no project is open".to_string(),
        );
        let state = state_with(view(0));
        assert_eq!(
            snapshot_of(&state, &records, McpSessionId(0)).map(|s| s.state),
            Some(McpSessionState::Failed {
                reason: "no project is open".to_string()
            })
        );
    }

    #[test]
    fn logs_return_the_requested_tail_newest_last() {
        let mut view = view(0);
        for line in ["one", "two", "three"] {
            view.push_line_at(line.to_string(), "00:00:00");
        }
        assert_eq!(log_tail(&view, None), vec!["one", "two", "three"]);
        assert_eq!(log_tail(&view, Some(2)), vec!["two", "three"]);
        assert_eq!(log_tail(&view, Some(99)), vec!["one", "two", "three"]);
    }

    #[test]
    fn latest_metrics_omits_the_network_counters_it_does_not_hold() {
        let mut view = view(0);
        view.devtools
            .metrics
            .apply(frust_drive::metrics::MetricsSample::Cpu(
                frust_drive::metrics::CpuSample {
                    percent: 12.5,
                    at_ms: 40,
                },
            ));
        let latest = latest_metrics(&view);
        assert_eq!(latest.cpu.map(|cpu| cpu.percent), Some(12.5));
        assert!(
            latest.net.is_none(),
            "the workbench keeps a derived rate + since-start totals, never the raw \
             cumulative counters `NetSample` means"
        );
    }

    #[test]
    fn a_backend_with_no_workbench_listening_reports_a_typed_error() {
        let backend = backend();
        // Every answer-shaped method degrades honestly rather than inventing
        // a value…
        assert!(backend.sessions().is_empty());
        assert!(backend.session(McpSessionId(0)).is_none());
        assert!(backend.logs(McpSessionId(0), None).is_none());
        assert!(backend.frame_ring(McpSessionId(0)).is_none());
        assert!(backend.latest_metrics(McpSessionId(0)).is_none());
        // …and the id `run_app` reports can never name another session.
        assert_eq!(
            backend.run_app(RunTarget::Desktop, BuildMode::Debug),
            UNRESOLVED_SESSION
        );
        // …while the fallible ones say what went wrong.
        let error = backend.stop_app(McpSessionId(0)).unwrap_err().to_string();
        assert!(
            error.contains("did not answer"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn the_devtools_client_is_never_shared_out_of_the_bridge() {
        assert!(backend().devtools_client(McpSessionId(0)).is_none());
    }

    #[test]
    fn restarting_a_session_that_never_launched_is_refused_as_unsupported() {
        let (tx, _rx) = unbounded_channel();
        let (mut supervisor, _events) = Supervisor::new(Arc::new(FakeProcessRunner::new()));
        let mut subscribers = SessionSubscribers::new();
        let mut devtools = DevtoolsBridge::new(Arc::new(FakeProcessRunner::new()));
        let mut pending_trees = PendingWidgetTrees::new();
        let mut records = McpSessionRecords::new();
        records.insert_failed(
            SessionId(0),
            spec(DeviceTarget::Desktop),
            "boom".to_string(),
        );
        let state = state_with(view(0));
        let mut next_adhoc_id = MAX_ADHOC_SESSION_ID;
        let mut ctx = McpServeCtx {
            state: &state,
            supervisor: &mut supervisor,
            records: &mut records,
            tx: &tx,
            next_adhoc_id: &mut next_adhoc_id,
            subscribers: &mut subscribers,
            devtools: &mut devtools,
            pending_trees: &mut pending_trees,
        };

        assert_eq!(
            restart_app(&mut ctx, McpSessionId(0)),
            Err(EmbeddedError::Unsupported {
                what: "restart_app",
                why: "this session never launched — it exists only to report the error. \
                      Fix the cause and call run_app again",
            })
        );
        assert_eq!(
            restart_app(&mut ctx, McpSessionId(9)),
            Err(EmbeddedError::NoSuchSession(9))
        );
    }

    /// Builds a terminal (`Exited`) record + matching view directly into
    /// `records`/`views`, at `started_at = UNIX_EPOCH + seq` seconds — the
    /// deterministic age ordering [`McpSessionRecords::retain_bounded`]'s
    /// oldest-first eviction is checked against, rather than relying on
    /// `SystemTime::now()`'s resolution across a tight loop.
    fn push_terminal(records: &mut McpSessionRecords, views: &mut Vec<SessionView>, seq: u64) {
        let mut v = view(seq);
        v.state = SessionState::Exited(true);
        views.push(v);
        records.by_id.insert(
            SessionId(seq),
            SessionRecord {
                spec: spec(DeviceTarget::Desktop),
                started_at: std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(seq),
                launch_error: None,
            },
        );
    }

    #[test]
    fn filling_past_the_cap_evicts_oldest_terminal_first_and_never_touches_a_live_record() {
        let mut records = McpSessionRecords::new();
        let mut views = Vec::new();

        // The live session's record is the single *oldest* timestamp of
        // all — if eviction were pure age-sort with no liveness check, this
        // is exactly the one it would remove first. Proving it survives
        // proves the liveness gate, not just the sort.
        let live_id = 0u64;
        let mut live_view = view(live_id);
        live_view.state = SessionState::Running;
        views.push(live_view);
        records.by_id.insert(
            SessionId(live_id),
            SessionRecord {
                spec: spec(DeviceTarget::Desktop),
                started_at: std::time::SystemTime::UNIX_EPOCH,
                launch_error: None,
            },
        );

        let terminal_count = MCP_RECORD_CAP + 3;
        for seq in 1..=terminal_count as u64 {
            push_terminal(&mut records, &mut views, seq);
        }

        let state = state_with_many(views);
        records.retain_bounded(&state);

        assert_eq!(
            records.by_id.len(),
            MCP_RECORD_CAP + 1,
            "bounded to the cap of terminal records, plus the one live record"
        );
        for evicted in 1..=3u64 {
            assert!(
                records.get(SessionId(evicted)).is_none(),
                "session {evicted} is one of the three oldest terminal records and \
                 should have been evicted"
            );
        }
        assert!(
            records.get(SessionId(4)).is_some(),
            "the 4th-oldest terminal record is within the cap and must survive"
        );
        assert!(
            records.get(SessionId(live_id)).is_some(),
            "a live session's record must never be evicted, however old its timestamp"
        );
    }

    #[test]
    fn live_count_excludes_the_given_session() {
        let mut records = McpSessionRecords::new();
        let mut views = Vec::new();
        for seq in 0..3u64 {
            let mut v = view(seq);
            v.state = SessionState::Running;
            views.push(v);
            records.insert(SessionId(seq), spec(DeviceTarget::Desktop));
        }
        let state = state_with_many(views);

        assert_eq!(records.live_count(&state, None), 3);
        assert_eq!(
            records.live_count(&state, Some(SessionId(1))),
            2,
            "the excluded session must not count against its own restart"
        );
    }

    #[test]
    fn run_app_refuses_bookkeeping_free_once_the_cap_of_live_sessions_is_reached() {
        let (tx, mut rx) = unbounded_channel();
        let (mut supervisor, _events) = Supervisor::new(Arc::new(FakeProcessRunner::new()));
        let mut subscribers = SessionSubscribers::new();
        let mut devtools = DevtoolsBridge::new(Arc::new(FakeProcessRunner::new()));
        let mut pending_trees = PendingWidgetTrees::new();
        let mut records = McpSessionRecords::new();
        let mut views = Vec::new();
        for seq in 0..MCP_RECORD_CAP as u64 {
            let mut v = view(seq);
            v.state = SessionState::Running;
            views.push(v);
            records.insert(SessionId(seq), spec(DeviceTarget::Desktop));
        }
        let state = AppState {
            project_root: Some(PathBuf::from("/tmp/frust-tui-mcp-unit")),
            sessions: views,
            ..AppState::default()
        };
        let mut next_adhoc_id = MAX_ADHOC_SESSION_ID;
        let mut ctx = McpServeCtx {
            state: &state,
            supervisor: &mut supervisor,
            records: &mut records,
            tx: &tx,
            next_adhoc_id: &mut next_adhoc_id,
            subscribers: &mut subscribers,
            devtools: &mut devtools,
            pending_trees: &mut pending_trees,
        };

        let result = run_app(&mut ctx, DeviceTarget::Desktop, BuildMode::Debug);

        assert_eq!(
            result,
            Err(EmbeddedError::TooManySessions {
                cap: MCP_RECORD_CAP
            })
        );
        // No bookkeeping: the refusal must not have grown the record map…
        assert_eq!(
            records.by_id.len(),
            MCP_RECORD_CAP,
            "a refusal must not itself insert a record"
        );
        // …nor posted anything (a `RegisterSession` above all) onto the
        // engine channel — a refusal that still registered a tab would grow
        // `AppState::sessions` faster than a successful launch does.
        assert!(
            rx.try_recv().is_err(),
            "the refusal must post no message onto the engine channel at all"
        );
    }

    #[test]
    fn run_app_refuses_a_target_the_workbench_is_already_running() {
        let (tx, mut rx) = unbounded_channel();
        let (mut supervisor, _events) = Supervisor::new(Arc::new(FakeProcessRunner::new()));
        let mut subscribers = SessionSubscribers::new();
        let mut devtools = DevtoolsBridge::new(Arc::new(FakeProcessRunner::new()));
        let mut pending_trees = PendingWidgetTrees::new();
        let mut records = McpSessionRecords::new();
        // The blocker is a session the *user* started: no MCP record at all,
        // so nothing but the workbench's own model can see it.
        let state = open_state(vec![app_view(4, SessionTarget::Desktop)]);
        let mut next_adhoc_id = MAX_ADHOC_SESSION_ID;
        let mut ctx = McpServeCtx {
            state: &state,
            supervisor: &mut supervisor,
            records: &mut records,
            tx: &tx,
            next_adhoc_id: &mut next_adhoc_id,
            subscribers: &mut subscribers,
            devtools: &mut devtools,
            pending_trees: &mut pending_trees,
        };

        let result = run_app(&mut ctx, DeviceTarget::Desktop, BuildMode::Debug);

        assert_eq!(
            result,
            Err(EmbeddedError::AlreadyRunning {
                session: 4,
                target: "desktop".to_string(),
            })
        );
        let message = result.unwrap_err().to_string();
        assert!(
            message.contains("stop it (stop_app)") && message.contains("restart it (restart_app)"),
            "the refusal must name the way out: {message}"
        );
        // Refused before any bookkeeping, exactly like the cap refusal.
        assert!(records.by_id.is_empty(), "a refusal inserts no record");
        assert!(
            rx.try_recv().is_err(),
            "a refusal posts no message onto the engine channel"
        );
    }

    #[test]
    fn run_app_allows_a_target_no_live_session_occupies() {
        let (tx, _rx) = unbounded_channel();
        let (mut supervisor, _events) = Supervisor::new(Arc::new(FakeProcessRunner::new()));
        let mut subscribers = SessionSubscribers::new();
        let mut devtools = DevtoolsBridge::new(Arc::new(FakeProcessRunner::new()));
        let mut pending_trees = PendingWidgetTrees::new();
        let mut records = McpSessionRecords::new();
        let state = open_state(vec![app_view(4, SessionTarget::Desktop)]);
        let mut next_adhoc_id = MAX_ADHOC_SESSION_ID;
        let mut ctx = McpServeCtx {
            state: &state,
            supervisor: &mut supervisor,
            records: &mut records,
            tx: &tx,
            next_adhoc_id: &mut next_adhoc_id,
            subscribers: &mut subscribers,
            devtools: &mut devtools,
            pending_trees: &mut pending_trees,
        };

        // A device is a different place from the desktop preview the live
        // session occupies. (The launch itself still fails past the guard —
        // `FakeProcessRunner` has no script for it — which is
        // `start_session`'s concern, not this one.)
        let result = run_app(
            &mut ctx,
            DeviceTarget::Device(device(Platform::Android, Kind::Emulator)),
            BuildMode::Debug,
        );

        assert!(
            !matches!(result, Err(EmbeddedError::AlreadyRunning { .. })),
            "only the same (project, target) pair is exclusive: {result:?}"
        );
    }

    #[test]
    fn restart_app_is_never_refused_by_the_session_it_restarts() {
        let (tx, _rx) = unbounded_channel();
        let (mut supervisor, _events) = Supervisor::new(Arc::new(FakeProcessRunner::new()));
        let mut subscribers = SessionSubscribers::new();
        let mut devtools = DevtoolsBridge::new(Arc::new(FakeProcessRunner::new()));
        let mut pending_trees = PendingWidgetTrees::new();
        let mut records = McpSessionRecords::new();
        records.insert(SessionId(0), spec(DeviceTarget::Desktop));
        let state = open_state(vec![app_view(0, SessionTarget::Desktop)]);
        let mut next_adhoc_id = MAX_ADHOC_SESSION_ID;
        let mut ctx = McpServeCtx {
            state: &state,
            supervisor: &mut supervisor,
            records: &mut records,
            tx: &tx,
            next_adhoc_id: &mut next_adhoc_id,
            subscribers: &mut subscribers,
            devtools: &mut devtools,
            pending_trees: &mut pending_trees,
        };

        let result = restart_app(&mut ctx, McpSessionId(0));

        assert!(
            !matches!(result, Err(EmbeddedError::AlreadyRunning { .. })),
            "a restart stops the session it replaces, so it cannot block itself: {result:?}"
        );
    }

    #[test]
    fn restart_app_is_refused_when_another_session_holds_the_target() {
        let (tx, _rx) = unbounded_channel();
        let (mut supervisor, _events) = Supervisor::new(Arc::new(FakeProcessRunner::new()));
        let mut subscribers = SessionSubscribers::new();
        let mut devtools = DevtoolsBridge::new(Arc::new(FakeProcessRunner::new()));
        let mut pending_trees = PendingWidgetTrees::new();
        let mut records = McpSessionRecords::new();
        records.insert(SessionId(0), spec(DeviceTarget::Desktop));
        // A second, user-started session took the same target meanwhile.
        let state = open_state(vec![
            app_view(0, SessionTarget::Desktop),
            app_view(1, SessionTarget::Desktop),
        ]);
        let mut next_adhoc_id = MAX_ADHOC_SESSION_ID;
        let mut ctx = McpServeCtx {
            state: &state,
            supervisor: &mut supervisor,
            records: &mut records,
            tx: &tx,
            next_adhoc_id: &mut next_adhoc_id,
            subscribers: &mut subscribers,
            devtools: &mut devtools,
            pending_trees: &mut pending_trees,
        };

        assert_eq!(
            restart_app(&mut ctx, McpSessionId(0)),
            Err(EmbeddedError::AlreadyRunning {
                session: 1,
                target: "desktop".to_string(),
            }),
            "the exemption covers the restarted session only"
        );
    }

    #[test]
    fn restart_app_excludes_the_target_session_from_its_own_cap_check() {
        let (tx, _rx) = unbounded_channel();
        let (mut supervisor, _events) = Supervisor::new(Arc::new(FakeProcessRunner::new()));
        let mut subscribers = SessionSubscribers::new();
        let mut devtools = DevtoolsBridge::new(Arc::new(FakeProcessRunner::new()));
        let mut pending_trees = PendingWidgetTrees::new();
        let mut records = McpSessionRecords::new();
        let mut views = Vec::new();
        for seq in 0..MCP_RECORD_CAP as u64 {
            let mut v = view(seq);
            v.state = SessionState::Running;
            views.push(v);
            records.insert(SessionId(seq), spec(DeviceTarget::Desktop));
        }
        let state = AppState {
            project_root: Some(PathBuf::from("/tmp/frust-tui-mcp-unit")),
            sessions: views,
            ..AppState::default()
        };
        let mut next_adhoc_id = MAX_ADHOC_SESSION_ID;
        let mut ctx = McpServeCtx {
            state: &state,
            supervisor: &mut supervisor,
            records: &mut records,
            tx: &tx,
            next_adhoc_id: &mut next_adhoc_id,
            subscribers: &mut subscribers,
            devtools: &mut devtools,
            pending_trees: &mut pending_trees,
        };

        // Restarting one of the cap's own live sessions is a 1-for-1 swap —
        // net zero live sessions — and must not be refused as
        // `TooManySessions` just because the cap is already exactly met.
        // (`FakeProcessRunner` has no script for the desktop invocation, so
        // the launch itself still fails past the cap check — that failure is
        // `start_session`'s own concern, not this one.)
        let result = restart_app(&mut ctx, McpSessionId(0));
        assert_ne!(
            result,
            Err(EmbeddedError::TooManySessions {
                cap: MCP_RECORD_CAP
            }),
            "excluding the session being restarted must drop it below the cap: {result:?}"
        );
    }

    #[test]
    fn restart_app_on_an_evicted_record_reports_no_such_session() {
        let mut records = McpSessionRecords::new();
        let mut views = Vec::new();
        let terminal_count = MCP_RECORD_CAP + 1;
        for seq in 0..terminal_count as u64 {
            push_terminal(&mut records, &mut views, seq);
        }
        let state = state_with_many(views);
        records.retain_bounded(&state);
        assert!(
            records.get(SessionId(0)).is_none(),
            "sanity: session 0 is the single oldest terminal record and was evicted"
        );

        let (tx, _rx) = unbounded_channel();
        let (mut supervisor, _events) = Supervisor::new(Arc::new(FakeProcessRunner::new()));
        let mut subscribers = SessionSubscribers::new();
        let mut devtools = DevtoolsBridge::new(Arc::new(FakeProcessRunner::new()));
        let mut pending_trees = PendingWidgetTrees::new();
        let mut next_adhoc_id = MAX_ADHOC_SESSION_ID;
        let mut ctx = McpServeCtx {
            state: &state,
            supervisor: &mut supervisor,
            records: &mut records,
            tx: &tx,
            next_adhoc_id: &mut next_adhoc_id,
            subscribers: &mut subscribers,
            devtools: &mut devtools,
            pending_trees: &mut pending_trees,
        };

        assert_eq!(
            restart_app(&mut ctx, McpSessionId(0)),
            Err(EmbeddedError::NoSuchSession(0)),
            "an evicted record's tab may still exist in `state`, but restart_app \
             reports the same typed refusal a truly unknown id gets"
        );
    }

    // ── The two deferred-answer commands ────────────────────────────────────

    /// Everything `serve_command` needs, owned by the caller so a test can
    /// keep driving the same registries across several commands — the shape
    /// `crate::runner`'s loop holds for the workbench's whole life.
    struct Harness {
        supervisor: Supervisor,
        records: McpSessionRecords,
        tx: UnboundedSender<Message>,
        _rx: tokio::sync::mpsc::UnboundedReceiver<Message>,
        next_adhoc_id: u64,
        subscribers: SessionSubscribers,
        devtools: DevtoolsBridge,
        pending_trees: PendingWidgetTrees,
    }

    impl Harness {
        fn new() -> Self {
            let (tx, _rx) = unbounded_channel();
            let (supervisor, _events) = Supervisor::new(Arc::new(FakeProcessRunner::new()));
            // The supervisor's own event receiver is dropped: no session is
            // ever really started through this harness.
            Self {
                supervisor,
                records: McpSessionRecords::new(),
                tx,
                _rx,
                next_adhoc_id: MAX_ADHOC_SESSION_ID,
                subscribers: SessionSubscribers::new(),
                devtools: DevtoolsBridge::new(Arc::new(FakeProcessRunner::new())),
                pending_trees: PendingWidgetTrees::new(),
            }
        }

        fn serve(&mut self, state: &AppState, cmd: McpCommand) {
            serve_command(
                cmd,
                &mut McpServeCtx {
                    state,
                    supervisor: &mut self.supervisor,
                    records: &mut self.records,
                    tx: &self.tx,
                    next_adhoc_id: &mut self.next_adhoc_id,
                    subscribers: &mut self.subscribers,
                    devtools: &mut self.devtools,
                    pending_trees: &mut self.pending_trees,
                },
            );
        }
    }

    /// A generous failure deadline for a reply that is sent synchronously —
    /// never a pacing device.
    const REPLY: Duration = Duration::from_secs(5);

    /// The whole `subscribe_session_events` contract over the workbench, in
    /// the order a DAP client's output pump sees it: the retained lines it
    /// missed, then every line the session produces afterwards, then the
    /// session's end — and then nothing, because the feed closes.
    #[test]
    fn a_session_event_subscription_is_seeded_then_fed_live_then_closed_by_the_exit() {
        let mut harness = Harness::new();
        harness
            .records
            .insert(SessionId(0), spec(DeviceTarget::Desktop));
        let mut view = view(0);
        view.state = SessionState::Running;
        view.push_line_at("already retained".to_string(), "00:00:00");
        let mut state = state_with(view);

        let (reply, rx) = Reply::channel();
        harness.serve(
            &state,
            McpCommand::SubscribeSessionEvents {
                id: McpSessionId(0),
                reply,
            },
        );
        let feed = rx
            .recv_timeout(REPLY)
            .expect("the workbench answered")
            .expect("a describable session gets a feed");
        assert_eq!(
            feed.recv_timeout(REPLY),
            Ok(McpSessionEvent::Log("already retained".to_string())),
            "the seed is the retained ring, exactly as the log pane holds it"
        );

        // Live delivery, driven the way `crate::runner::dispatch` drives it:
        // cursor before the transition, replay after.
        let cursor = SessionSubscribers::cursor(&state, SessionId(0)).expect("the view exists");
        state.sessions[0].push_line_at("live".to_string(), "00:00:01");
        let records = &harness.records;
        harness
            .subscribers
            .replay(&state, cursor, |view| mcp_session_state(records, view));
        assert_eq!(
            feed.recv_timeout(REPLY),
            Ok(McpSessionEvent::Log("live".to_string()))
        );

        // …and the ending.
        let cursor = SessionSubscribers::cursor(&state, SessionId(0)).expect("the view exists");
        state.sessions[0].state = SessionState::Exited(true);
        let records = &harness.records;
        harness
            .subscribers
            .replay(&state, cursor, |view| mcp_session_state(records, view));
        assert_eq!(
            feed.recv_timeout(REPLY),
            Ok(McpSessionEvent::Exited {
                state: McpSessionState::Exited { success: true }
            })
        );
        assert!(
            feed.recv_timeout(REPLY).is_err(),
            "the feed closes after the exit it owes — a pump reading it stops, \
             rather than blocking on a session that is over"
        );
    }

    /// A discovery line is redacted on its way into the ring, and the feed is
    /// fed *from* the ring — so a DAP client's Debug Console can no more read
    /// a devtools handshake token back than the log pane can.
    #[test]
    fn a_feed_never_carries_the_devtools_handshake_token() {
        let mut harness = Harness::new();
        harness
            .records
            .insert(SessionId(0), spec(DeviceTarget::Desktop));
        let mut view = view(0);
        view.state = SessionState::Running;
        let mut state = state_with(view);

        let (reply, rx) = Reply::channel();
        harness.serve(
            &state,
            McpCommand::SubscribeSessionEvents {
                id: McpSessionId(0),
                reply,
            },
        );
        let feed = rx.recv_timeout(REPLY).expect("answered").expect("a feed");

        let cursor = SessionSubscribers::cursor(&state, SessionId(0)).expect("the view exists");
        state.sessions[0].push_line_at(
            "frust-devtools listening on 53214 token cafe1234".to_string(),
            "00:00:00",
        );
        let records = &harness.records;
        harness
            .subscribers
            .replay(&state, cursor, |view| mcp_session_state(records, view));

        let McpSessionEvent::Log(line) = feed.recv_timeout(REPLY).expect("the line arrived") else {
            panic!("expected a log line");
        };
        assert!(
            !line.contains("cafe1234"),
            "the feed must carry the redacted ring copy: {line}"
        );
        assert!(line.contains("<redacted>"), "unexpected line: {line}");
    }

    /// A session this backend cannot describe has no feed — the same omission
    /// `list_sessions` makes, so a consumer never follows an id it was never
    /// shown.
    #[test]
    fn subscribing_to_an_undescribable_session_yields_no_feed() {
        let mut harness = Harness::new();
        let state = state_with(view(7));
        let (reply, rx) = Reply::channel();
        harness.serve(
            &state,
            McpCommand::SubscribeSessionEvents {
                id: McpSessionId(7),
                reply,
            },
        );
        assert!(
            rx.recv_timeout(REPLY).expect("answered").is_none(),
            "an ad-hoc tab is not an MCP session, so there is nothing to follow"
        );
    }

    /// The widget-tree refusal with no devtools connection: answered
    /// immediately (waiting could not improve it) and typed in the devtools
    /// layer's own vocabulary, so a consumer's `is_not_supported` check
    /// classifies it exactly like an app that declared no `widget_tree`
    /// capability.
    #[test]
    fn a_widget_tree_pull_without_a_devtools_connection_is_refused_as_not_supported() {
        let mut harness = Harness::new();
        harness
            .records
            .insert(SessionId(0), spec(DeviceTarget::Desktop));
        let mut view = view(0);
        view.state = SessionState::Running;
        let state = state_with(view);

        let (reply, rx) = Reply::channel();
        harness.serve(
            &state,
            McpCommand::WidgetTree {
                id: McpSessionId(0),
                reply,
            },
        );
        let refusal = rx
            .recv_timeout(REPLY)
            .expect("answered without waiting for a bridge that will never report")
            .expect_err("no connection, no tree");
        let TreeRefusal::Unavailable(reason) = &refusal else {
            panic!("expected an Unavailable refusal, got {refusal:?}");
        };
        assert!(
            reason.contains("no devtools connection"),
            "unhelpful refusal: {reason}"
        );
        assert!(
            harness.pending_trees.is_empty(),
            "an immediate refusal must not leave a waiter behind"
        );

        let error = refusal.into_error(McpSessionId(0));
        assert!(
            frust_drive::devtools_client::is_not_supported(&error),
            "the refusal must classify as not-supported: {error:#}"
        );
    }

    /// An id the backend cannot describe at all gets the same typed family,
    /// naming the session rather than implying the app merely has not
    /// connected yet.
    #[test]
    fn a_widget_tree_pull_for_an_unknown_session_is_refused_too() {
        let mut harness = Harness::new();
        let state = state_with(view(7));
        let (reply, rx) = Reply::channel();
        harness.serve(
            &state,
            McpCommand::WidgetTree {
                id: McpSessionId(7),
                reply,
            },
        );
        let refusal = rx
            .recv_timeout(REPLY)
            .expect("answered")
            .expect_err("no tree");
        assert_eq!(
            refusal,
            TreeRefusal::Unavailable("no such session in the workbench: 7".to_string())
        );
    }

    /// A backend with no workbench listening degrades the two new methods the
    /// same honest way the older ones degrade: an absent feed, and a typed
    /// error rather than a hang.
    #[test]
    fn the_deferred_methods_degrade_honestly_with_no_workbench_listening() {
        let backend = backend();
        assert!(backend.subscribe_session_events(McpSessionId(0)).is_none());
        let error = backend
            .fetch_widget_tree(McpSessionId(0))
            .expect_err("nothing is serving");
        assert!(
            format!("{error:#}").contains("did not answer"),
            "unexpected error: {error:#}"
        );
    }
}
