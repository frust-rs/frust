//! TEA engine: model ([`AppState`]), messages ([`Message`]), the pure
//! transition ([`update`]), and the [`Engine`] that owns the model plus the
//! unified message channel background tasks feed.
//!
//! Layering: this module never touches the terminal, ratatui, or any
//! render type — it is the testable core. `crate::ui` renders `&AppState` and
//! emits `Message`s but never mutates the model; `crate::runner` owns the
//! terminal lifecycle and drives the loop.

mod add_plugin;
mod bootstrap;
mod build_launcher;
mod context_menu;
mod create_wizard;
mod devtools;
mod doctor;
mod logstyle;
mod message;
mod modal;
pub mod palette;
mod perf;
mod persist;
mod run_config;
mod session_view;
mod state;
mod toast;
mod update;

pub use add_plugin::{
    AddPluginAdvance, AddPluginDialog, AddPluginStep, AddReport, FeatureToggle, PluginEntry,
};
pub use bootstrap::{BootstrapNode, BootstrapState, BootstrapWizard};
pub use build_launcher::{ArtifactKind, BuildFocus, BuildLauncher, BuildSpec, BuildTargetSpec};
pub use context_menu::{ContextMenu, MenuEntry};
pub use create_wizard::{ArchCard, CreateWizard, WizardAdvance, WizardStep};
pub use devtools::{
    ConnEvent, ConnState, CpuPoint, DevtoolsLaunch, DevtoolsPhase, DevtoolsState, DevtoolsTab,
    FRAME_RING_CAP, INSPECTOR_AUTO_EXPAND_DEPTH, InspectorEvent, InspectorFocus, InspectorRow,
    InspectorTab, METRICS_RING_CAP, MetricsIdentity, MetricsState, NetRatePoint, NetTotals,
    PERF_WINDOW, PerfFocus, PerfFrame, PerfPhases, PerfSource, PerfStats, PerformanceTab, RssPoint,
    SamplingState, ThermalPoint, network_honesty_note, perf_stats, perf_window, select_perf_source,
};
pub use doctor::{DoctorCheck, DoctorState};
pub use logstyle::{
    LEVEL_FILTER_SEGMENTS, LevelFilter, LineMeta, LineRole, LogLevel, LogSource, PanicBlock,
    SOURCE_TAG_WIDTH, classify_level, classify_source, hms_at,
};
pub use message::{ContextTarget, DragKind, Message, RegionId};
pub use modal::ActiveModal;
pub use palette::{Palette, PaletteCommand};
pub use perf::{FrameSummary, PerfLine, PerfPanel, RawFrame, StartupSummary, parse_perf_line};
pub use persist::{
    Settings, load_recent_projects, load_settings, merge_recent_and_detected,
    record_recent_project, save_mouse_capture, save_sidebar_width,
};
pub use run_config::{DeviceRow, RunConfig, RunFocus, RunTarget};
pub use session_view::{LineSelection, LogBuffer, Scroll, SessionView, line_matches, strip_ansi};
pub use state::{
    AppState, SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH, Screen, SearchState,
    clamp_sidebar_width,
};
pub use toast::{Toast, ToastKind, Toasts};
pub use update::{DevtoolsTarget, Effect, MetricsTarget, Outcome, update};

use frust_dap::DapClientRegistry;
use frust_mcp::{ClientRegistry, SharedBackend};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::supervise::{DapServerHandle, DapStatus, McpServerHandle, McpStatus};

/// The port the embedded MCP server binds when the workbench starts one —
/// `frust-mcp`'s own default, so an agent configured for Frust's conventional
/// MCP address finds the embedded server there. `0` lets the OS assign an
/// ephemeral port instead (what the tests use).
pub const DEFAULT_MCP_PORT: u16 = frust_mcp::DEFAULT_MCP_PORT;

/// The port the embedded DAP server binds when the workbench starts one —
/// `frust-dap`'s own default, so a generated editor launch configuration and
/// the workbench agree without either being told. `0` lets the OS assign an
/// ephemeral port instead (what the tests use).
pub const DEFAULT_DAP_PORT: u16 = frust_dap::DEFAULT_DAP_PORT;

/// Owns the model and the single mpsc channel every asynchronous producer
/// (session supervisors, preflight tasks) sends into.
///
/// The channel is unbounded and cloneable: [`Engine::sender`] hands a producer
/// a `Sender`, and the event loop drains the matching receiver
/// ([`Engine::take_receiver`]) in its `select!`. Mutation flows through exactly
/// one method — [`Engine::handle`] — which forwards to the pure [`update`].
pub struct Engine {
    /// The application model.
    pub state: AppState,
    tx: UnboundedSender<Message>,
    rx: Option<UnboundedReceiver<Message>>,
    /// The tag the next [`Self::start_mcp`] stamps its server and that
    /// server's reports with (see [`McpServerHandle::generation`]).
    next_mcp_generation: u64,
    /// The same counter for [`Self::start_dap`] — a separate sequence, since
    /// the two servers are superseded independently.
    next_dap_generation: u64,
}

impl Engine {
    /// Wrap a model in an engine, creating its message channel.
    pub fn new(state: AppState) -> Self {
        let (tx, rx) = unbounded_channel();
        Self {
            state,
            tx,
            rx: Some(rx),
            next_mcp_generation: 0,
            next_dap_generation: 0,
        }
    }

    /// A cloneable handle a background task posts `Message`s through.
    pub fn sender(&self) -> UnboundedSender<Message> {
        self.tx.clone()
    }

    /// Take the receiver for the event loop. Callable once; the loop selects on
    /// it. Panics if called twice (a wiring bug, not a runtime condition).
    pub fn take_receiver(&mut self) -> UnboundedReceiver<Message> {
        self.rx
            .take()
            .expect("Engine::take_receiver called more than once")
    }

    /// The single mutation point: apply one message to the model.
    pub fn handle(&mut self, msg: Message) -> Outcome {
        update(&mut self.state, msg)
    }

    /// Start an embedded MCP server over `backend`, bound to
    /// `127.0.0.1:port` (`0` = OS-assigned).
    ///
    /// Returns `false` — and starts nothing — when one is already running.
    /// The server is spawned on the **current** tokio runtime, so this must
    /// be called from inside it (the workbench's event loop always is).
    ///
    /// This and [`Self::stop_mcp`] are the only two mutations that do not go
    /// through [`Self::handle`]: the server handle is a live resource, not a
    /// value the pure [`update`] could construct. Everything the server then
    /// reports about itself — the port it bound, the fact that it stopped —
    /// comes back as an ordinary [`Message`] and *is* applied through
    /// `update`.
    ///
    /// Each start mints a fresh **generation** and stamps it on the handle and
    /// on both of the server's reports, so a previous server still winding
    /// down cannot have its late report applied to this one (see
    /// [`McpServerHandle::generation`]).
    pub fn start_mcp(&mut self, backend: SharedBackend, port: u16) -> bool {
        if self.state.mcp.is_some() {
            return false;
        }
        let generation = self.next_mcp_generation;
        self.next_mcp_generation += 1;
        let cancel = CancellationToken::new();
        let registry = ClientRegistry::new();
        let (ready_tx, ready_rx) = oneshot::channel();

        let serving = tokio::spawn(frust_mcp::serve_embedded(
            backend,
            registry.clone(),
            port,
            Some(ready_tx),
            cancel.clone(),
        ));
        // Two tasks rather than one: the ready signal has to be observed
        // *while* the server runs, and the server future only resolves once
        // it has stopped.
        let listening_tx = self.tx.clone();
        tokio::spawn(async move {
            if let Ok(addr) = ready_rx.await {
                let _ = listening_tx.send(Message::McpListening(generation, addr.port()));
            }
        });
        let stopped_tx = self.tx.clone();
        tokio::spawn(async move {
            let error = match serving.await {
                Ok(Ok(())) => None,
                Ok(Err(err)) => Some(format!("{err:#}")),
                Err(err) => Some(format!("the MCP server task failed: {err}")),
            };
            let _ = stopped_tx.send(Message::McpStopped(generation, error));
        });

        self.state.mcp = Some(McpServerHandle::starting(generation, cancel, registry));
        true
    }

    /// Stop the embedded MCP server, if one is running; returns whether there
    /// was one.
    ///
    /// The handle is dropped here and the cancellation is what actually
    /// closes the listener — the server's own [`Message::McpStopped`] follows
    /// once its task has wound down, and finds nothing left to clear (or a
    /// *newer* server it does not name, which its generation tag makes it
    /// leave alone). A caller that must know the port is free again (a
    /// restart on the *same* fixed port) waits for that message.
    pub fn stop_mcp(&mut self) -> bool {
        match self.state.mcp.take() {
            Some(handle) => {
                handle.cancel();
                true
            }
            None => false,
        }
    }

    /// What the embedded MCP server is doing — [`AppState::mcp_status`],
    /// reachable from an `Engine` handle.
    pub fn mcp_status(&self) -> McpStatus {
        self.state.mcp_status()
    }

    /// Start an embedded DAP server over `backend`, bound to
    /// `127.0.0.1:port` (`0` = OS-assigned), rooted at the workbench's active
    /// project.
    ///
    /// [`Self::start_mcp`]'s counterpart in every respect — same
    /// live-resource ownership, same generation minting, same two watcher
    /// tasks, same current-runtime requirement — with one addition: the
    /// server needs a **project root**, and the workbench's is the only one it
    /// will ever build from (a DAP client's own `projectRoot` is refused,
    /// since the listener is unauthenticated loopback — see
    /// `frust_dap::serve_embedded`). With no project open there is nothing to
    /// root a server at, so nothing is started and the reason is retained on
    /// [`AppState::dap_error`] for the UI to show, rather than a server
    /// binding a port it could never serve a launch from.
    ///
    /// Returns `false` — and starts nothing — when one is already running, or
    /// when no project is open.
    pub fn start_dap(&mut self, backend: SharedBackend, port: u16) -> bool {
        if self.state.dap.is_some() {
            return false;
        }
        let Some(project_root) = self.state.project_root.clone() else {
            self.state.dap_error = Some(
                "no project is open in the workbench — open one before starting the DAP server, \
                 since every debug launch builds from the workbench's own project root"
                    .to_string(),
            );
            return false;
        };
        let generation = self.next_dap_generation;
        self.next_dap_generation += 1;
        let cancel = CancellationToken::new();
        let registry = DapClientRegistry::new();
        let (ready_tx, ready_rx) = oneshot::channel();

        let serving = tokio::spawn(frust_dap::serve_embedded(
            backend,
            registry.clone(),
            project_root,
            port,
            Some(ready_tx),
            cancel.clone(),
        ));
        let listening_tx = self.tx.clone();
        tokio::spawn(async move {
            if let Ok(port) = ready_rx.await {
                let _ = listening_tx.send(Message::DapListening(generation, port));
            }
        });
        let stopped_tx = self.tx.clone();
        tokio::spawn(async move {
            let error = match serving.await {
                Ok(Ok(())) => None,
                Ok(Err(err)) => Some(format!("{err}")),
                Err(err) => Some(format!("the DAP server task failed: {err}")),
            };
            let _ = stopped_tx.send(Message::DapStopped(generation, error));
        });

        self.state.dap = Some(DapServerHandle::starting(generation, cancel, registry));
        true
    }

    /// Stop the embedded DAP server, if one is running; returns whether there
    /// was one. [`Self::stop_mcp`]'s counterpart, with the same
    /// drop-then-cancel discipline and the same late-report gating.
    pub fn stop_dap(&mut self) -> bool {
        match self.state.dap.take() {
            Some(handle) => {
                handle.cancel();
                true
            }
            None => false,
        }
    }

    /// What the embedded DAP server is doing — [`AppState::dap_status`],
    /// reachable from an `Engine` handle.
    pub fn dap_status(&self) -> DapStatus {
        self.state.dap_status()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use frust_drive::process::FakeProcessRunner;

    use super::*;
    use crate::supervise::TuiSessionBackend;

    /// A failure deadline for a real listener coming up — never a pacing
    /// device.
    const DEADLINE: Duration = Duration::from_secs(20);

    #[test]
    fn handle_routes_through_update() {
        let mut engine = Engine::new(AppState::default());
        let out = engine.handle(Message::Quit);
        assert!(engine.state.should_quit);
        assert!(!out.redraw);
    }

    #[tokio::test]
    async fn channel_delivers_to_receiver() {
        let mut engine = Engine::new(AppState::default());
        let mut rx = engine.take_receiver();
        engine.sender().send(Message::Tick).unwrap();
        assert_eq!(rx.recv().await, Some(Message::Tick));
    }

    // ── The embedded DAP server ─────────────────────────────────────────────

    fn workbench_with_a_project() -> Engine {
        Engine::new(AppState {
            project_root: Some(PathBuf::from("/tmp/frust-tui-dap-lifecycle")),
            ..AppState::default()
        })
    }

    fn test_backend(engine: &Engine) -> SharedBackend {
        Arc::new(TuiSessionBackend::new(
            engine.sender(),
            Arc::new(FakeProcessRunner::new()),
        ))
    }

    /// The whole lifecycle over a **real** loopback listener: a start binds
    /// and reports its port, a second start is refused, the report is applied
    /// only for the generation that owns it, and a stop actually stops.
    #[tokio::test]
    async fn a_started_dap_server_reports_the_port_it_bound() {
        let mut engine = workbench_with_a_project();
        let mut rx = engine.take_receiver();
        let backend = test_backend(&engine);

        assert!(engine.start_dap(Arc::clone(&backend), 0), "it started");
        assert_eq!(engine.dap_status(), DapStatus::Starting);
        assert!(
            !engine.start_dap(backend, 0),
            "a second start while one is running is refused, not stacked"
        );

        let report = tokio::time::timeout(DEADLINE, rx.recv())
            .await
            .expect("the listener came up")
            .expect("the engine channel is open");
        let Message::DapListening(generation, port) = report else {
            panic!("expected a DapListening report, got {report:?}");
        };
        assert_eq!(generation, 0, "the first server is generation 0");
        engine.handle(Message::DapListening(generation, port));
        assert_eq!(
            engine.dap_status(),
            DapStatus::Listening { port, clients: 0 }
        );

        assert!(engine.stop_dap(), "there was a server to stop");
        assert_eq!(engine.dap_status(), DapStatus::Stopped);
    }

    /// The race the generation tag exists for, on the DAP side: a superseded
    /// server's late reports must not restamp — or, far worse, clear — its
    /// successor's handle, since dropping a `CancellationToken` does not
    /// cancel it and the successor would be left listening with nothing able
    /// to stop it.
    #[tokio::test]
    async fn a_superseded_dap_servers_late_report_never_touches_its_successor() {
        let mut engine = workbench_with_a_project();
        let mut rx = engine.take_receiver();
        let backend = test_backend(&engine);

        // Server A, then stopped; server B started while A winds down.
        assert!(engine.start_dap(Arc::clone(&backend), 0));
        engine.stop_dap();
        assert!(engine.start_dap(backend, 0), "B takes the free slot");

        // Drain until B's own report arrives; A's may or may not precede it,
        // and either way `update` must only apply B's.
        let port = loop {
            let report = tokio::time::timeout(DEADLINE, rx.recv())
                .await
                .expect("a report arrived")
                .expect("the engine channel is open");
            if let Message::DapListening(generation, port) = report {
                engine.handle(Message::DapListening(generation, port));
                if generation == 1 {
                    break port;
                }
            }
        };
        assert_eq!(
            engine.dap_status(),
            DapStatus::Listening { port, clients: 0 },
            "only B's own report stamped B's handle"
        );

        // A's stale reports change nothing at all.
        assert!(!engine.handle(Message::DapListening(0, 9999)).redraw);
        assert!(!engine.handle(Message::DapStopped(0, None)).redraw);
        assert!(
            !engine
                .handle(Message::DapStopped(0, Some("address in use".to_string())))
                .redraw
        );
        assert_eq!(
            engine.dap_status(),
            DapStatus::Listening { port, clients: 0 }
        );
        assert_eq!(engine.state.dap_error, None);
        assert!(engine.state.toasts.items.is_empty());

        // …and B is still the handle a stop reaches.
        assert!(engine.stop_dap());
    }

    /// With no project open there is nothing to root a debug launch at, so
    /// nothing is started and the reason is retained rather than a server
    /// binding a port it could never serve from.
    #[tokio::test]
    async fn starting_without_a_project_is_refused_with_a_retained_reason() {
        let mut engine = Engine::new(AppState::default());
        let backend = test_backend(&engine);
        assert!(!engine.start_dap(backend, 0));
        assert_eq!(engine.dap_status(), DapStatus::Stopped);
        assert!(
            engine
                .state
                .dap_error
                .as_deref()
                .is_some_and(|reason| reason.contains("no project is open")),
            "the refusal must say why: {:?}",
            engine.state.dap_error
        );
    }
}
