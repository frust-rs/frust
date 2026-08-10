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
    SOURCE_TAG_WIDTH, classify_level, classify_source,
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

use frust_mcp::{ClientRegistry, SharedBackend};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::supervise::{McpServerHandle, McpStatus};

/// The port the embedded MCP server binds when the workbench starts one —
/// `frust-mcp`'s own default, so an agent configured for a standalone
/// `frust mcp` finds the embedded server at the same address. `0` lets the OS
/// assign an ephemeral port instead (what the tests use).
pub const DEFAULT_MCP_PORT: u16 = frust_mcp::DEFAULT_MCP_PORT;

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
}

impl Engine {
    /// Wrap a model in an engine, creating its message channel.
    pub fn new(state: AppState) -> Self {
        let (tx, rx) = unbounded_channel();
        Self {
            state,
            tx,
            rx: Some(rx),
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
    pub fn start_mcp(&mut self, backend: SharedBackend, port: u16) -> bool {
        if self.state.mcp.is_some() {
            return false;
        }
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
                let _ = listening_tx.send(Message::McpListening(addr.port()));
            }
        });
        let stopped_tx = self.tx.clone();
        tokio::spawn(async move {
            let error = match serving.await {
                Ok(Ok(())) => None,
                Ok(Err(err)) => Some(format!("{err:#}")),
                Err(err) => Some(format!("the MCP server task failed: {err}")),
            };
            let _ = stopped_tx.send(Message::McpStopped(error));
        });

        self.state.mcp = Some(McpServerHandle::starting(cancel, registry));
        true
    }

    /// Stop the embedded MCP server, if one is running; returns whether there
    /// was one.
    ///
    /// The handle is dropped here and the cancellation is what actually
    /// closes the listener — the server's own [`Message::McpStopped`] follows
    /// once its task has wound down, and finds nothing left to clear. A
    /// caller that must know the port is free again (a restart on the *same*
    /// fixed port) waits for that message.
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
