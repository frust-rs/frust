//! TEA engine: model ([`AppState`]), messages ([`Message`]), the pure
//! transition ([`update`]), and the [`Engine`] that owns the model plus the
//! unified message channel background tasks feed.
//!
//! Layering (D2): this module never touches the terminal, ratatui, or any
//! render type — it is the testable core. `crate::ui` renders `&AppState` and
//! emits `Message`s but never mutates the model; `crate::runner` owns the
//! terminal lifecycle and drives the loop.

mod add_plugin;
mod bootstrap;
mod build_launcher;
mod context_menu;
mod create_wizard;
mod doctor;
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
pub use doctor::{DoctorCheck, DoctorState};
pub use message::{ContextTarget, DragKind, Message, RegionId};
pub use modal::ActiveModal;
pub use palette::{Palette, PaletteCommand};
pub use perf::{FrameSummary, PerfLine, PerfPanel, RawFrame, StartupSummary, parse_perf_line};
pub use persist::{
    Settings, load_recent_projects, load_settings, merge_recent_and_detected,
    record_recent_project, save_follow_tail_default, save_mouse_capture, save_sidebar_width,
};
pub use run_config::{DeviceRow, RunConfig, RunFocus, RunTarget};
pub use session_view::{
    LineSelection, LogBuffer, LogLevel, Scroll, SessionView, detect_level, line_matches, strip_ansi,
};
pub use state::{
    AppState, SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH, Screen, SearchState,
    clamp_sidebar_width,
};
pub use toast::{Toast, ToastKind, Toasts};
pub use update::{Effect, Outcome, update};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

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
