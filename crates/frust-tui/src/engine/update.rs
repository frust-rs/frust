//! The pure TEA transition function.
//!
//! [`update`] is the *single mutation point*: it takes the model and one
//! message and returns an [`Outcome`] — whether the frame is now dirty, plus an
//! optional [`Effect`] the runner performs (the only I/O the pure core cannot
//! do itself: killing a session through the supervisor, or writing the
//! clipboard). It performs no I/O and reads no clock, so every transition is
//! unit-testable without a terminal (see the tests below).

use std::path::{Path, PathBuf};

use frust_dap::ide_config::{ParentIde, WriteMode};

use super::add_plugin::{AddPluginAdvance, AddPluginDialog};
use super::bootstrap::BootstrapWizard;
use super::build_launcher::{BuildLauncher, BuildSpec};
use super::context_menu::ContextMenu;
use super::create_wizard::{CreateWizard, WizardAdvance};
use super::dap_settings::{DapFocus, DapIdeReport, DapSetting, IdeConfigRequest};
use super::devtools::{ConnEvent, ConnState, DevtoolsPhase, DevtoolsTab, InspectorTab, PerfFrame};
use super::message::{ContextTarget, DragKind, Message};
use super::run_config::{DeviceRow, RunConfig};
use super::session_view::{SessionTarget, SessionView, truncate_with_ellipsis};
use super::state::{AppState, Screen};
use super::toast::ToastKind;
use crate::supervise::{DeviceTarget, SessionEvent, SessionEventKind, SessionId, SessionSpec};

/// How much of a copied log line the "Copied: …" notice previews before
/// eliding the rest — counted in Unicode scalar values, not bytes (see
/// [`truncate_with_ellipsis`]).
const COPY_LINE_PREVIEW_CHARS: usize = 60;

/// A side effect the (terminal/supervisor-owning) runner performs after a
/// transition — the pure core requests it, the runner enacts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Stop a session: route to `Supervisor::stop` (the group-kill).
    StopSession(SessionId),
    /// Stop `id` and relaunch its retained spec as a new session — the
    /// keyboard restart's enactment (`Message::RestartSession`), mirroring
    /// `crate::supervise::mcp_backend::restart_app`'s own stop-then-relaunch
    /// contract (see that fn's doc for the shared guard/cap discipline the
    /// two callers keep aligned). The runner looks the spec up by `id` in its
    /// own launch records; `update()` has already applied every guard this
    /// effect needs before requesting it.
    RestartSession(SessionId),
    /// Start (`on`) or stop session `id`'s source watcher — the enactment of
    /// a "Watch: restart on save" flip ([`Message::ToggleWatch`],
    /// [`Message::EnableWatch`]). The runner keys its
    /// `crate::supervise::SourceWatchers` by `id`, watching the launch
    /// record's project root; every settled change burst comes back as
    /// [`Message::WatchTriggered`].
    WatchSet {
        /// The session whose watcher starts/stops.
        id: SessionId,
        /// Start (`true`) or stop (`false`).
        on: bool,
    },
    /// Copy text to the system clipboard (the runner emits an OSC 52 sequence).
    Copy(String),
    /// Discover devices off-thread (`frust-drive`'s `DeviceDiscovery` set),
    /// posting the result back as [`Message::DevicesLoaded`].
    RefreshDevices,
    /// Launch one supervised session per spec (the run-config modal's checked
    /// targets), registering each returned id back into the model.
    LaunchSessions(Vec<SessionSpec>),
    /// [`Self::LaunchSessions`] for desktop specs launched with the
    /// run-config modal's watch checkbox ticked: the runner launches them
    /// exactly the same way, then posts [`Message::EnableWatch`] for each
    /// session that started, after its registration.
    LaunchWatchedSessions(Vec<SessionSpec>),
    /// Persist `path` as the most-recently-opened project (`toml_edit`
    /// format-preserving save to `~/.config/frust/tui.toml`) — the runner
    /// performs the actual file I/O; the pure engine only requests it.
    RecordRecentProject(PathBuf),
    /// Historically probed (off-thread) whether the `../clean-signals-rs`
    /// sibling checkout was present, posting the result back as
    /// [`Message::CleanSignalsProbed`] to gate the create wizard's
    /// clean-signals arch card / the Add Plugin dialog's facade-tier cards.
    /// `clean-signals` is now git+rev-pinned to its public repo, so no card
    /// is sibling-gated any more and the runner replies immediately with
    /// `true` rather than touching disk — kept as the priming effect fired
    /// on opening either the wizard or the dialog, and as the generic
    /// mechanism a future sibling-dependent registry entry would reuse.
    ProbeCleanSignals,
    /// Scaffold a new project off-thread via `frust_drive::scaffold::generate`,
    /// posting [`Message::ScaffoldSucceeded`]/[`Message::ScaffoldFailed`] back.
    /// The runner resolves `directory` against the process cwd and fills the
    /// template context (org/description/frust path+version).
    ScaffoldProject {
        /// The target directory as typed in the wizard (cwd-relative or
        /// absolute).
        directory: String,
        /// The validated project (crate) name.
        project_name: String,
        /// The selected architecture tag (`None` = default template).
        arch: Option<String>,
    },
    /// Run `frust-drive`'s validator set off-thread, posting the results back
    /// as [`Message::DoctorResults`] — the titlebar chip's live source.
    RunDoctor,
    /// Drive the resolved build off-thread through `frust-drive`'s
    /// `android_build`/`ios_build` pipelines, reporting progress/completion
    /// as a supervised-style session (reusing the tab/log-view machinery via
    /// `Message::RegisterSession`/`Message::Session`).
    LaunchBuild(BuildSpec),
    /// Run `cargo clean` + remove the generated Android/iOS build
    /// directories for `project_root` off-thread, reported the same way as
    /// [`Effect::LaunchBuild`].
    RunClean(PathBuf),
    /// Run `frust-drive`'s component-level toolchain report off-thread
    /// ([`frust_drive::doctor::build_report`]), posting the result back as
    /// [`Message::BootstrapReport`] — the titlebar chip's rollup source and the
    /// bootstrap wizard's data.
    RunBootstrapReport,
    /// Run a bootstrap wizard's guided fix command off-thread as a supervised
    /// session (streamed into a log tab, reusing the ad-hoc-session machinery
    /// like [`Effect::LaunchBuild`]), then re-run the preflight report so the
    /// chip/wizard reflect the fixed component (the fresh-machine flow). Only
    /// ever carries an `auto_runnable` fix — one command, never chained.
    RunBootstrapCommand {
        /// The program to spawn (`rustup`, `cargo`, …).
        program: String,
        /// Its arguments.
        args: Vec<String>,
        /// A short label for the session tab.
        label: String,
    },
    /// Enable (`true`) or disable (`false`) crossterm mouse capture at the
    /// terminal level — the runner enacts the `EnableMouseCapture`/
    /// `DisableMouseCapture` sequence; the pure engine only requests it. The
    /// runner also persists this as the mouse-capture preference alongside
    /// the terminal-level toggle (settings persistence).
    SetMouseCapture(bool),
    /// Persist the sidebar's current width — the runner's enactment of a
    /// just-completed `SidebarSplitter` drag (settings persistence,
    /// fulfilling `DragEnd`'s previously-deferred note).
    SaveSidebarWidth(u16),
    /// Apply a registry plugin's contributions to `project_root` off-thread via
    /// [`frust_drive::plugin::add_plugin`], posting
    /// [`Message::AddPluginSucceeded`]/[`Message::AddPluginFailed`] back — the
    /// Add Plugin dialog's apply step.
    AddPlugin {
        /// The generated project root the edits are applied to.
        project_root: PathBuf,
        /// The registry plugin id (`"secure-storage"`, …).
        id: String,
        /// The checked optional-feature ids.
        features: Vec<String>,
    },
    /// Open (or replace) the devtools connection for one session: the runner
    /// hands this to [`crate::supervise::DevtoolsBridge`], which does the
    /// `adb forward` (Android), the TCP connect, the token handshake, and the
    /// frame-stats subscription on its own thread — never on the event loop.
    /// Progress comes back as [`Message::DevtoolsConn`].
    DevtoolsConnect(DevtoolsTarget),
    /// Tear the devtools connection for one session down (session ended, or
    /// a reconnect is about to replace it), removing any `adb forward` it
    /// allocated.
    DevtoolsDisconnect(SessionId),
    /// Pull a fresh `widget_tree` snapshot for one session's Inspector tab —
    /// the runner hands it to that session's bridge thread, which serves it
    /// between frame-pump windows and reports back as
    /// [`Message::DevtoolsInspector`].
    DevtoolsFetchTree {
        /// The session whose bridge serves the request.
        session: SessionId,
    },
    /// Pull one node's `widget_props` for the same tab — §B12's second,
    /// per-selection call, served and reported exactly like
    /// [`Effect::DevtoolsFetchTree`].
    DevtoolsFetchProps {
        /// The session whose bridge serves the request.
        session: SessionId,
        /// The widget-tree node id to describe.
        id: u64,
    },
    /// Start a session's System/Network metrics sampler (workbook §B12) —
    /// the runner hands this to [`crate::supervise::MetricsBridge`], which
    /// spawns `frust_drive::metrics::MetricsSampler` on its own thread and
    /// drains it into coalesced [`Message::DevtoolsMetrics`] batches. Only
    /// ever emitted for an Android session whose identity has resolved (see
    /// [`super::MetricsIdentity`]'s doc for why desktop/iOS never reach
    /// this).
    MetricsStart(MetricsTarget),
    /// Stop a session's metrics sampler (the session ended) — see
    /// `crate::supervise::metrics_bridge`'s retention note (mirrors
    /// [`Effect::DevtoolsDisconnect`]'s session-end-only teardown).
    MetricsStop(SessionId),
    /// Start the embedded MCP server (workbook §B13) — the runner builds the
    /// [`crate::supervise::TuiSessionBackend`] over its own supervisor and
    /// calls [`super::Engine::start_mcp`], because the server handle is a
    /// live resource the pure core cannot construct. Everything the server
    /// then reports about itself (the bound port, a bind failure) arrives
    /// back as an ordinary [`Message`].
    StartMcpServer,
    /// Stop the embedded MCP server — the runner's
    /// [`super::Engine::stop_mcp`]. A no-op when none is running.
    StopMcpServer,
    /// Start the embedded DAP server on `port` (`0` = OS-assigned) — the
    /// runner's [`super::Engine::start_dap`], over the **same**
    /// [`crate::supervise::TuiSessionBackend`] the MCP server is handed, so an
    /// editor and an agent drive one session world. The same live-resource
    /// reasoning as [`Effect::StartMcpServer`]: only the runner can build the
    /// handle.
    StartDapServer {
        /// The loopback port to bind (`frust_dap::DEFAULT_DAP_PORT` unless a
        /// caller says otherwise).
        port: u16,
    },
    /// Stop the embedded DAP server — the runner's
    /// [`super::Engine::stop_dap`]. A no-op when none is running.
    StopDapServer,
    /// Write one just-changed `[dap]` preference to `~/.config/frust/tui.toml`
    /// (`crate::engine::save_dap_setting`) — the same
    /// "the pure core decides, the runner writes" split as
    /// [`Effect::SaveSidebarWidth`].
    SaveDapSetting(super::dap_settings::DapSetting),
    /// Generate (or refresh) an IDE's DAP client config off-thread
    /// (`frust_dap::ide_config::generate_ide_config` reads and writes real
    /// files, so it never runs on the transition path), reporting the outcome
    /// back as [`Message::DapIdeConfig`].
    GenerateIdeConfig(super::dap_settings::IdeConfigRequest),
    /// Enact every effect in order — the escape hatch for a transition that
    /// must kick off more than one independent side effect at once (opening
    /// DevTools can both (re)open its frame-stats connection *and* start
    /// metrics sampling in the same keypress). Every other transition still
    /// returns at most one effect; see [`batch`].
    Batch(Vec<Effect>),
}

/// Everything a session's metrics sampler needs to reach the running app's
/// OS process — resolved by the pure engine from its Android identity
/// ([`super::MetricsIdentity`]), enacted by the runner
/// ([`crate::supervise::MetricsBridge`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsTarget {
    /// The session whose sampler this is (and whose `DevtoolsMetrics`
    /// messages it reports through).
    pub session: SessionId,
    /// The `adb` serial the sampler shells out against.
    pub serial: String,
    /// The running app's pid on that device.
    pub pid: String,
    /// The running app's package — Android's `dumpsys meminfo` probe is
    /// keyed by package, not pid.
    pub pkg: String,
}

/// Everything the bridge needs to reach one session's devtools service —
/// resolved by the pure engine from the session's discovery line and its
/// launch metadata, enacted by the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevtoolsTarget {
    /// The session whose bridge this is (and whose `DevtoolsConn` messages
    /// it reports through).
    pub session: SessionId,
    /// The port the app announced. On Android this is a *device* port, which
    /// the bridge maps to a host one with `adb forward`.
    pub port: u16,
    /// The handshake token from the same discovery line (`None` for a
    /// service running with auth off, or a build predating the token).
    pub token: Option<String>,
    /// The `adb` serial when the session runs on an Android device — see
    /// [`super::DevtoolsLaunch::android_serial`].
    pub android_serial: Option<String>,
}

/// What the loop must do after a transition.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// `true` when the visible state changed and the next frame must be drawn.
    /// The event loop ORs these across a turn and skips `terminal.draw` when it
    /// stays `false` (dirty-frame skip — helps CPU and screen readers).
    pub redraw: bool,
    /// A side effect for the runner to perform (stop a session, copy text).
    pub effect: Option<Effect>,
}

impl Outcome {
    /// No redraw, no effect.
    fn idle() -> Self {
        Self::default()
    }

    /// Redraw, no effect.
    fn redraw() -> Self {
        Self {
            redraw: true,
            effect: None,
        }
    }

    /// A redraw iff `dirty`.
    fn dirty(dirty: bool) -> Self {
        Self {
            redraw: dirty,
            effect: None,
        }
    }

    /// An effect with no redraw (the visible change follows from a later
    /// message — e.g. the supervisor's `Killed` event).
    fn effect(effect: Effect) -> Self {
        Self {
            redraw: false,
            effect: Some(effect),
        }
    }
}

/// Apply `msg` to `state`, returning whether a redraw is warranted and any
/// effect the runner must perform.
pub fn update(state: &mut AppState, msg: Message) -> Outcome {
    match msg {
        Message::Quit => {
            state.should_quit = true;
            // No point drawing a frame we're about to tear down.
            Outcome::idle()
        }
        // A tick ages the toast stack and advances `animation_frame`, the
        // clock every `crate::ui::anim` helper (spinner/shimmer) reads. It
        // dirties a frame when a toast actually expires OR any session is
        // still in a transient build/install phase (the tab spinner has a
        // new frame to paint even though nothing else about the session
        // changed). With no live toast and no transient session,
        // `animating()` is false and the runner never delivers a tick here —
        // the dirty-frame skip.
        Message::Tick => {
            state.animation_frame = state.animation_frame.wrapping_add(1);
            let toast_expired = state.toasts.tick();
            let session_animating = state
                .sessions
                .iter()
                .any(|s| super::state::is_transient(&s.state));
            // The open MCP panel repaints on every tick: its client list is
            // read live off the registry at render time, so nothing else
            // would ever dirty the frame when a client connects or drops
            // (see `AppState::animating`). The DAP settings dialog shows the
            // same kind of live count and follows the same rule.
            Outcome::dirty(
                toast_expired
                    || session_animating
                    || state.mcp_panel_open
                    || state.dap_settings_open,
            )
        }
        Message::Resize(_, _) => Outcome::redraw(),
        Message::HoverChanged(next) => {
            let hover_changed = state.hover != next;
            state.hover = next;
            // Hovering a context-menu row moves its highlight (mouse parity for
            // the keyboard arrows) — see `context_menu`.
            let mut menu_changed = false;
            if let (Some(menu), Some(super::message::RegionId::ContextMenuItem(i))) =
                (state.context_menu.as_mut(), next)
                && i < menu.entries.len()
                && menu.cursor != i
            {
                menu.cursor = i;
                menu_changed = true;
            }
            Outcome::dirty(hover_changed || menu_changed)
        }
        Message::CreatePressed => {
            if state.create_pressed {
                Outcome::idle()
            } else {
                state.create_pressed = true;
                Outcome::redraw()
            }
        }
        Message::CreateActivate => {
            state.create_pressed = false;
            open_create_wizard(state)
        }
        Message::CreateCancel => {
            if state.create_pressed {
                state.create_pressed = false;
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }

        Message::Session(ev) => on_session_event(state, ev),
        Message::RegisterSession {
            id,
            project_root,
            target_label,
            devtools,
            target,
        } => {
            if state.session_index(id).is_some() {
                return Outcome::idle();
            }
            // Every session starts following its own tail; there is no
            // persisted global default. `Scroll::Anchored` must always name
            // an existing absolute line index, and a freshly registered
            // session has no lines yet — so `Follow` is the only valid
            // starting state.
            // A keyboard restart's relaunch takes focus (the replaced tab is
            // on its way out); the pending focus is consumed by this
            // registration either way — see `AppState::focus_next_registered`.
            let restart_relaunch = state.take_restart_focus(&project_root, target.as_ref());
            let view = SessionView::with_devtools(id, project_root, target_label, devtools, target);
            state.sessions.push(view);
            // Auto-select the first session that appears.
            if state.active_session.is_none() || restart_relaunch {
                state.active_session = Some(state.sessions.len() - 1);
            }
            Outcome::redraw()
        }
        Message::NextTab => cycle_tab(state, 1),
        Message::PrevTab => cycle_tab(state, -1),
        Message::SelectTab(i) => {
            if i < state.sessions.len() && state.active_session != Some(i) {
                state.active_session = Some(i);
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }
        Message::StopSession => match state.active_session_mut() {
            Some(session) => {
                // An explicit stop ends the watch loop too (the runner's
                // `StopSession` enactment stops the watcher): otherwise the
                // next save would relaunch what the user just stopped.
                session.watch = false;
                Outcome::effect(Effect::StopSession(session.id))
            }
            None => Outcome::idle(),
        },
        Message::RestartSession => match state.active_session {
            Some(idx) => restart_session_at(state, idx),
            None => Outcome::idle(),
        },
        Message::ToggleWatch => toggle_watch(state),
        Message::WatchTriggered { session } => watch_triggered(state, session),
        Message::EnableWatch { session } => enable_watch(state, session),
        Message::WatchFailed { session, reason } => {
            if let Some(idx) = state.session_index(session) {
                state.sessions[idx].watch = false;
            }
            state
                .toasts
                .push(ToastKind::Warn, format!("Watch unavailable: {reason}"));
            Outcome::redraw()
        }
        Message::CloseTab(idx) => close_tab(state, idx),
        Message::CloseActiveTab => match state.active_session {
            Some(idx) => close_tab(state, idx),
            None => Outcome::idle(),
        },
        Message::ToggleFollow => {
            let Some(session) = state.active_session_mut() else {
                return Outcome::idle();
            };
            session.toggle_follow();
            Outcome::redraw()
        }
        Message::ToggleWrap => {
            state.wrap = !state.wrap;
            Outcome::redraw()
        }
        Message::LogScrollUp(n) => with_active_filtered(state, |s, f| s.scroll_up(n, f)),
        Message::LogScrollDown(n) => with_active_filtered(state, |s, f| s.scroll_down(n, f)),
        Message::LogScrollToTop => with_active(state, |s| s.scroll_to_top()),
        Message::LogScrollToBottom => with_active(state, |s| s.scroll_to_bottom()),

        Message::SearchOpen => {
            state.search.open = true;
            state.search.query = state.search.filter.clone().unwrap_or_default();
            Outcome::redraw()
        }
        Message::SearchInput(c) => {
            if state.search.open {
                state.search.query.push(c);
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }
        Message::SearchBackspace => {
            if state.search.open {
                state.search.query.pop();
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }
        Message::SearchCommit => {
            if !state.search.open {
                return Outcome::idle();
            }
            state.search.open = false;
            let q = std::mem::take(&mut state.search.query);
            state.search.filter = (!q.is_empty()).then_some(q);
            // Jump to the latest match so the committed filter shows fresh output.
            if let Some(s) = state.active_session_mut() {
                s.scroll_to_bottom();
            }
            Outcome::redraw()
        }
        Message::SearchCancel => {
            if state.search.open {
                state.search.open = false;
                state.search.query.clear();
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }

        // Refused (idle, no toast) while the active session's tab is showing
        // DevTools: that pane owns the whole key namespace (`translate_key`'s
        // early return on `devtools.open`), so a `v` press can never reach
        // here — but the palette's "Select lines…" row re-dispatches this
        // same `Message` regardless of which pane is in front, and the
        // palette is the one surface that stays reachable over DevTools
        // (see `palette::commands`'s "Select lines…" entry). Gating here
        // rather than disabling the palette row keeps the choke point single
        // and covers every future caller of this message alike.
        Message::SelectEnter => with_active_changed(state, |s, f| {
            if s.devtools.open {
                return false;
            }
            s.enter_select_mode(f)
        }),
        Message::SelectExit => with_active_changed(state, |s, f| s.exit_select_mode(f)),
        Message::SelectMove(n) => with_active_changed(state, |s, f| s.move_cursor(n, f)),
        Message::SelectPage(dir) => with_active_changed(state, |s, f| s.move_cursor_page(dir, f)),
        Message::SelectHome => with_active_changed(state, |s, f| s.cursor_home(f)),
        Message::SelectEnd => with_active_changed(state, |s, f| s.cursor_end(f)),
        // The runner translates a row click without knowing the mode; the
        // mode semantics live here: the first click of a mode session
        // re-anchors, every later one drags the range end to the clicked row.
        Message::LogRowClicked(abs) => with_active_changed(state, |s, _| {
            if !s.select_mode {
                false
            } else if s.clicked_since_enter() {
                s.cursor_to(abs)
            } else {
                s.anchor_at(abs)
            }
        }),
        Message::ToggleFold(block_start) => with_active(state, |s| {
            s.toggle_fold(block_start);
        }),
        Message::ToggleNearestFold => with_active(state, |s| {
            s.toggle_nearest_fold();
        }),
        Message::CycleLevelFilter(delta) => with_active(state, |s| s.cycle_level_filter(delta)),
        Message::SetLevelFilter(filter) => with_active(state, |s| s.set_level_filter(filter)),
        // `y` is the whole copy gesture: it copies the range *and* leaves the
        // mode (follow-tail restored under `exit_select_mode`'s rule), so the
        // highlight never lingers over a log the user has finished with.
        Message::CopySelection => {
            let filter = state.search.filter.clone();
            let Some(session) = state.active_session_mut() else {
                return Outcome::idle();
            };
            let Some(text) = session.selected_text(filter.as_deref()) else {
                return Outcome::idle();
            };
            let lines = session.selected_visible_count(filter.as_deref());
            session.exit_select_mode(filter.as_deref());
            let word = if lines == 1 { "line" } else { "lines" };
            state
                .toasts
                .push(ToastKind::Info, format!("Copied: {lines} {word}"));
            Outcome {
                redraw: true,
                effect: Some(Effect::Copy(text)),
            }
        }
        // The log view's right-click "Copy line". The text comes from the
        // same redacted ring the view renders (never a raw line), and an
        // already-evicted index says so instead of copying nothing.
        Message::CopyLine(abs) => match state.active_session().and_then(|s| s.line_text(abs)) {
            Some(text) => {
                let preview = truncate_with_ellipsis(&text, COPY_LINE_PREVIEW_CHARS);
                state
                    .toasts
                    .push(ToastKind::Info, format!("Copied: {preview}"));
                Outcome {
                    redraw: true,
                    effect: Some(Effect::Copy(text)),
                }
            }
            None => {
                state
                    .toasts
                    .push(ToastKind::Warn, "Line no longer available");
                Outcome::redraw()
            }
        },

        // ── Devices panel + run-config modal ────────────────────────────────
        Message::RefreshDevices => {
            state.devices_refreshing = true;
            Outcome {
                redraw: true,
                effect: Some(Effect::RefreshDevices),
            }
        }
        Message::DevicesLoaded(devices) => {
            // Preserve the panel multi-select across a refresh for any device
            // still present (matched by id); new devices arrive unselected.
            let previously: std::collections::HashSet<String> = state
                .devices
                .iter()
                .filter(|d| d.selected)
                .map(|d| d.device.id.clone())
                .collect();
            state.devices = devices
                .into_iter()
                .map(|device| {
                    let selected = previously.contains(&device.id);
                    DeviceRow { device, selected }
                })
                .collect();
            state.devices_refreshing = false;
            state.clamp_device_cursor();
            // The device list is replaced wholesale. Any open context menu
            // targeting a DeviceRow becomes stale since the entry's SelectDeviceAt
            // is positional; close it. A menu on a SessionTab is untouched.
            if matches!(
                &state.context_menu,
                Some(m) if matches!(m.target, ContextTarget::DeviceRow(_))
            ) {
                state.context_menu = None;
            }
            Outcome::redraw()
        }
        Message::DeviceCursorUp => move_device_cursor(state, -1),
        Message::DeviceCursorDown => move_device_cursor(state, 1),
        Message::ToggleDeviceSelect => {
            let i = state.device_cursor;
            match state.devices.get_mut(i) {
                Some(row) => {
                    row.selected = !row.selected;
                    Outcome::redraw()
                }
                None => Outcome::idle(),
            }
        }
        Message::SelectDeviceAt(i) => match state.devices.get_mut(i) {
            Some(row) => {
                row.selected = !row.selected;
                state.device_cursor = i;
                Outcome::redraw()
            }
            None => Outcome::idle(),
        },
        Message::OpenRunConfig => match run_config_project(state) {
            Some(project_root) => {
                state.run_config = Some(RunConfig::new(project_root, &state.devices));
                Outcome::redraw()
            }
            None => Outcome::idle(),
        },
        Message::CloseRunConfig => {
            if state.run_config.take().is_some() {
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }
        Message::RunConfigFocusNext => with_modal(state, RunConfig::focus_next),
        Message::RunConfigFocusPrev => with_modal(state, RunConfig::focus_prev),
        Message::RunConfigToggleTarget => with_modal(state, RunConfig::toggle_focused_target),
        Message::RunConfigToggleTargetAt(i) => with_modal(state, |m| m.toggle_target(i)),
        Message::RunConfigToggleWatch => with_modal(state, RunConfig::toggle_watch),
        Message::RunConfigCycleMode(delta) => with_modal(state, |m| {
            m.cycle_mode(delta);
            m.focus = super::run_config::RunFocus::Mode;
        }),
        Message::RunConfigFocus(focus) => with_modal(state, |m| m.focus = focus),
        Message::RunConfigInput(c) => with_modal(state, |m| m.input_char(c)),
        Message::RunConfigBackspace => with_modal(state, RunConfig::backspace),
        Message::RunConfigLaunch => match &state.run_config {
            Some(modal) if modal.any_selected() => {
                let specs = modal.launch_specs();
                let watch = modal.watch;
                state.run_config = None;
                // The modal closes either way: a refusal is explained by its
                // own toast, not by leaving the dialog up.
                let specs = drop_already_running(state, specs);
                Outcome {
                    redraw: true,
                    effect: launch_effect(state, specs, watch),
                }
            }
            // Modal open but nothing checked, or no modal: nothing to launch.
            _ => Outcome::idle(),
        },

        // ── Project switcher + recent-projects persistence ──────────────────
        Message::ToggleProjectSwitcher => {
            if state.project_switcher_open {
                state.project_switcher_open = false;
            } else {
                state.project_switcher_open = true;
                state.project_switcher_cursor = state.active_project_index();
            }
            Outcome::redraw()
        }
        Message::CloseProjectSwitcher => {
            if state.project_switcher_open {
                state.project_switcher_open = false;
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }
        Message::ProjectSwitcherCursorUp => move_project_switcher_cursor(state, -1),
        Message::ProjectSwitcherCursorDown => move_project_switcher_cursor(state, 1),
        Message::SwitchProject(index) => switch_project(state, index),

        // ── Create-project wizard ───────────────────────────────────────────
        Message::OpenCreateWizard => {
            if state.create_wizard.is_some() {
                Outcome::idle()
            } else {
                open_create_wizard(state)
            }
        }
        Message::CloseCreateWizard => {
            if state.create_wizard.take().is_some() {
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }
        Message::CreateWizardInput(c) => with_wizard(state, |w| w.input_char(c)),
        Message::CreateWizardBackspace => with_wizard(state, CreateWizard::backspace),
        Message::CreateWizardArchMove(delta) => with_wizard(state, |w| w.arch_move(delta)),
        Message::CreateWizardSelectArchAt(i) => with_wizard(state, |w| w.select_arch_at(i)),
        Message::CreateWizardBack => match state.create_wizard.as_mut() {
            Some(wizard) => {
                if wizard.back() {
                    state.create_wizard = None;
                }
                Outcome::redraw()
            }
            None => Outcome::idle(),
        },
        Message::CreateWizardAdvance => match state.create_wizard.as_mut() {
            Some(wizard) => match wizard.advance() {
                WizardAdvance::Scaffold { arch } => Outcome {
                    redraw: true,
                    effect: Some(Effect::ScaffoldProject {
                        directory: wizard.directory.trim().to_string(),
                        project_name: wizard.name.clone(),
                        arch,
                    }),
                },
                WizardAdvance::Stepped | WizardAdvance::Blocked => Outcome::redraw(),
            },
            None => Outcome::idle(),
        },
        Message::CleanSignalsProbed(available) => {
            // The one sibling probe gates both the create wizard's clean-signals
            // arch card and the Add Plugin dialog's facade-tier plugin card.
            let mut dirty = false;
            if let Some(w) = state.create_wizard.as_mut() {
                w.set_clean_signals_available(available);
                dirty = true;
            }
            if let Some(d) = state.add_plugin.as_mut() {
                d.set_sibling_available(available);
                dirty = true;
            }
            Outcome::dirty(dirty)
        }
        Message::ScaffoldSucceeded { project_root } => {
            state.create_wizard = None;
            let name = project_name_of(&project_root);
            state
                .toasts
                .push(ToastKind::Success, format!("Created project {name}"));
            open_project(state, project_root)
        }
        Message::ScaffoldFailed(message) => with_wizard(state, |w| w.fail(message)),

        // ── Add plugin dialog ────────────────────────────────────────────────
        Message::OpenAddPlugin => open_add_plugin(state),
        Message::CloseAddPlugin => {
            if state.add_plugin.take().is_some() {
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }
        Message::AddPluginSelectMove(delta) => with_add_plugin(state, |d| d.select_move(delta)),
        Message::AddPluginSelectAt(i) => with_add_plugin(state, |d| d.select_at(i)),
        Message::AddPluginFeatureMove(delta) => with_add_plugin(state, |d| d.feature_move(delta)),
        Message::AddPluginToggleFeature => with_add_plugin(state, AddPluginDialog::toggle_feature),
        Message::AddPluginToggleFeatureAt(i) => with_add_plugin(state, |d| d.toggle_feature_at(i)),
        Message::AddPluginBack => match state.add_plugin.as_mut() {
            Some(dialog) => {
                if dialog.back() {
                    state.add_plugin = None;
                }
                Outcome::redraw()
            }
            None => Outcome::idle(),
        },
        Message::AddPluginAdvance => match state.add_plugin.as_mut() {
            Some(dialog) => match dialog.advance() {
                AddPluginAdvance::Apply { id, features } => Outcome {
                    redraw: true,
                    effect: Some(Effect::AddPlugin {
                        project_root: dialog.project_root.clone(),
                        id,
                        features,
                    }),
                },
                AddPluginAdvance::Done => {
                    state.add_plugin = None;
                    Outcome::redraw()
                }
                AddPluginAdvance::Stepped | AddPluginAdvance::Blocked => Outcome::redraw(),
            },
            None => Outcome::idle(),
        },
        Message::AddPluginSucceeded(report) => match state.add_plugin.as_mut() {
            Some(dialog) => {
                let text = add_plugin_toast_text(&report);
                dialog.succeed(report);
                state.toasts.push(ToastKind::Success, text);
                Outcome::redraw()
            }
            // The dialog was closed before the apply finished — the edits still
            // landed, so surface a toast rather than silently dropping it.
            None => {
                let text = add_plugin_toast_text(&report);
                state.toasts.push(ToastKind::Success, text);
                Outcome::redraw()
            }
        },
        Message::AddPluginFailed(message) => with_add_plugin(state, |d| d.fail(message)),

        // ── Doctor panel + titlebar chip ────────────────────────────────────
        Message::RunDoctor => {
            state.doctor.refreshing = true;
            Outcome {
                redraw: true,
                effect: Some(Effect::RunDoctor),
            }
        }
        Message::DoctorResults(results) => {
            state.doctor.results = results;
            state.doctor.refreshing = false;
            // Surface a doctor failure as an error toast, but only when no modal
            // is already showing (the panel/wizard carries the detail there).
            let has_fail = state
                .doctor
                .results
                .iter()
                .any(|c| c.status == frust_drive::doctor::Status::Fail);
            let quiet = state.active_modal().is_none();
            if has_fail && quiet {
                state
                    .toasts
                    .push(ToastKind::Error, "Doctor found problems · press i");
            }
            Outcome::redraw()
        }
        Message::OpenDoctorPanel => {
            if state.doctor_panel_open {
                Outcome::idle()
            } else {
                state.doctor_panel_open = true;
                Outcome::redraw()
            }
        }
        Message::CloseDoctorPanel => {
            if state.doctor_panel_open {
                state.doctor_panel_open = false;
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }
        Message::OpenToolchainFromDoctor => {
            // Toolchain setup via the Doctor panel: close it, then run
            // the same transition `OpenBootstrapWizard` does — always a
            // redraw, since closing the panel alone already is one.
            state.doctor_panel_open = false;
            open_bootstrap_wizard(state)
        }

        // ── Bootstrap wizard + titlebar toolchain chip ──────────────────────
        Message::RunBootstrapReport => {
            state.bootstrap.refreshing = true;
            Outcome {
                redraw: true,
                effect: Some(Effect::RunBootstrapReport),
            }
        }
        Message::BootstrapReport(report) => on_bootstrap_report(state, report),
        Message::OpenBootstrapWizard => open_bootstrap_wizard(state),
        Message::CloseBootstrapWizard => {
            if state.bootstrap_wizard.take().is_some() {
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }
        Message::BootstrapNavUp => with_bootstrap(state, |w| {
            w.nav(-1);
        }),
        Message::BootstrapNavDown => with_bootstrap(state, |w| {
            w.nav(1);
        }),
        Message::BootstrapSelectStep(i) => match state.bootstrap_wizard.as_mut() {
            Some(w) => {
                // Clicking a `Platforms` header toggles it; any other row just
                // selects it (mouse parity for the keyboard nav + Space).
                let was_header = matches!(
                    w.nodes().get(i),
                    Some(super::BootstrapNode::PlatformsHeader)
                );
                w.select_step(i);
                if was_header {
                    w.toggle_expand();
                }
                Outcome::redraw()
            }
            None => Outcome::idle(),
        },
        Message::BootstrapToggleExpand => match state.bootstrap_wizard.as_mut() {
            Some(w) => {
                if matches!(w.current_node(), super::BootstrapNode::PlatformsHeader) {
                    w.toggle_expand();
                    Outcome::redraw()
                } else {
                    Outcome::idle()
                }
            }
            None => Outcome::idle(),
        },
        Message::BootstrapFixUp => with_bootstrap(state, |w| {
            w.nav_fix(-1);
        }),
        Message::BootstrapFixDown => with_bootstrap(state, |w| {
            w.nav_fix(1);
        }),
        Message::BootstrapSelectFix(i) => with_bootstrap(state, |w| w.select_fix(i)),
        Message::BootstrapRunFix => match state.bootstrap_wizard.as_ref() {
            Some(w) => match w.runnable_selected_fix() {
                Some(fix) => {
                    let effect = Effect::RunBootstrapCommand {
                        program: fix.program.clone(),
                        args: fix.args.clone(),
                        label: fix.display.clone(),
                    };
                    // Running a fix closes the wizard so its streamed session
                    // log tab (which the modal would otherwise cover) is
                    // visible; the re-preflight after it exits refreshes the
                    // chip (see `crate::runner`).
                    state.bootstrap_wizard = None;
                    Outcome {
                        redraw: true,
                        effect: Some(effect),
                    }
                }
                // A guidance-only fix has nothing to run.
                None => Outcome::idle(),
            },
            None => Outcome::idle(),
        },
        Message::BootstrapCopyFix => {
            let text = state
                .bootstrap_wizard
                .as_ref()
                .and_then(|w| w.selected_fix().map(fix_copy_text));
            match text {
                Some(t) => {
                    state
                        .toasts
                        .push(ToastKind::Success, "Copied command to clipboard");
                    Outcome {
                        redraw: true,
                        effect: Some(Effect::Copy(t)),
                    }
                }
                None => Outcome::idle(),
            }
        }

        // ── Build launcher ──────────────────────────────────────────────────
        Message::OpenBuildLauncher => match run_config_project(state) {
            Some(project_root) if state.build_launcher.is_none() => {
                state.build_launcher = Some(BuildLauncher::new(project_root));
                Outcome::redraw()
            }
            _ => Outcome::idle(),
        },
        Message::CloseBuildLauncher => {
            if state.build_launcher.take().is_some() {
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }
        Message::BuildFocusNext => with_build_launcher(state, BuildLauncher::focus_next),
        Message::BuildFocusPrev => with_build_launcher(state, BuildLauncher::focus_prev),
        Message::BuildCycleKind(delta) => with_build_launcher(state, |l| l.cycle_kind(delta)),
        Message::BuildCycleMode(delta) => with_build_launcher(state, |l| l.cycle_mode(delta)),
        Message::BuildToggleSplitPerAbi => {
            with_build_launcher(state, BuildLauncher::toggle_split_per_abi)
        }
        Message::BuildToggleSimulator => {
            with_build_launcher(state, BuildLauncher::toggle_simulator)
        }
        Message::BuildToggleNoCodesign => {
            with_build_launcher(state, BuildLauncher::toggle_no_codesign)
        }
        Message::BuildFocus(focus) => with_build_launcher(state, |l| l.focus = focus),
        Message::BuildInput(c) => with_build_launcher(state, |l| l.input_char(c)),
        Message::BuildBackspace => with_build_launcher(state, BuildLauncher::backspace),
        Message::BuildLaunch => match state.build_launcher.take() {
            Some(launcher) => {
                let spec = launcher.build_spec();
                Outcome {
                    redraw: true,
                    effect: Some(Effect::LaunchBuild(spec)),
                }
            }
            None => Outcome::idle(),
        },

        // ── Clean confirm dialog ────────────────────────────────────────────
        Message::OpenCleanConfirm => match run_config_project(state) {
            Some(project_root) if state.clean_confirm.is_none() => {
                state.clean_confirm = Some(project_root);
                Outcome::redraw()
            }
            _ => Outcome::idle(),
        },
        Message::CloseCleanConfirm => {
            if state.clean_confirm.take().is_some() {
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }
        Message::ConfirmClean => match state.clean_confirm.take() {
            Some(root) => Outcome {
                redraw: true,
                effect: Some(Effect::RunClean(root)),
            },
            None => Outcome::idle(),
        },

        // ── Quit confirm dialog ──────────────────────────────────────────────
        Message::RequestQuit => {
            if state.live_session_count() == 0 {
                state.should_quit = true;
                Outcome::idle()
            } else {
                state.quit_confirm = true;
                Outcome::redraw()
            }
        }
        Message::ConfirmQuit => {
            state.quit_confirm = false;
            state.should_quit = true;
            // No point drawing a frame we're about to tear down.
            Outcome::idle()
        }
        Message::CloseQuitConfirm => {
            if state.quit_confirm {
                state.quit_confirm = false;
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }

        // ── Build artifact copy-path ────────────────────────────────────────
        Message::CopyBuiltArtifacts => {
            let joined = state
                .active_session()
                .map(|s| s.built_artifact_paths())
                .filter(|p| !p.is_empty())
                .map(|p| p.join("\n"));
            match joined {
                Some(text) => {
                    state
                        .toasts
                        .push(ToastKind::Success, "Copied artifact path(s)");
                    Outcome {
                        redraw: true,
                        effect: Some(Effect::Copy(text)),
                    }
                }
                None => Outcome::idle(),
            }
        }

        // ── Command palette ──────────────────────────────────────────────────
        Message::OpenPalette => {
            if state.palette.is_some() {
                Outcome::idle()
            } else {
                state.palette = Some(super::palette::Palette::new());
                Outcome::redraw()
            }
        }
        Message::ClosePalette => {
            if state.palette.take().is_some() {
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }
        Message::PaletteInput(c) => with_palette(state, |p| p.input_char(c)),
        Message::PaletteBackspace => with_palette(state, super::palette::Palette::backspace),
        Message::PaletteCursorUp => move_palette_cursor(state, -1),
        Message::PaletteCursorDown => move_palette_cursor(state, 1),
        Message::PaletteExecute => execute_palette(state, None),
        Message::PaletteExecuteAt(i) => execute_palette(state, Some(i)),

        Message::RunOnAllDevices => run_on_all_devices(state),

        // ── Drag-to-resize + scrollbar thumb ────────────────────────────────
        Message::DragStart(kind) => {
            state.active_drag = Some(kind);
            // No visual change yet; the immediately-following DragMove (the
            // runner sends one on press) applies the initial position.
            Outcome::idle()
        }
        Message::DragMove(x, y) => match state.active_drag {
            Some(DragKind::SidebarSplitter { body_left }) => {
                let next = super::state::clamp_sidebar_width(x.saturating_sub(body_left));
                if next != state.sidebar_width {
                    state.sidebar_width = next;
                    Outcome::redraw()
                } else {
                    Outcome::idle()
                }
            }
            Some(DragKind::LogScrollbar {
                track_top,
                track_height,
            }) => {
                let frac = track_fraction(y, track_top, track_height);
                let filter = state.search.filter.clone();
                match state.active_session_mut() {
                    Some(s) => Outcome::dirty(s.scroll_to_fraction(frac, filter.as_deref())),
                    None => Outcome::idle(),
                }
            }
            None => Outcome::idle(),
        },
        Message::DragEnd => {
            // A just-completed sidebar-splitter drag persists the new width
            // (settings persistence, fulfilling the note this arm used to
            // carry); a scrollbar-thumb drag (or no drag at all) has nothing
            // to persist.
            let effect = match state.active_drag {
                Some(DragKind::SidebarSplitter { .. }) => {
                    Some(Effect::SaveSidebarWidth(state.sidebar_width))
                }
                _ => None,
            };
            state.active_drag = None;
            Outcome {
                redraw: false,
                effect,
            }
        }

        // ── Context menus ───────────────────────────────────────────────────
        Message::OpenContextMenu { x, y, target } => open_context_menu(state, x, y, target),
        Message::CloseContextMenu => {
            if state.context_menu.take().is_some() {
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }
        Message::ContextMenuCursorUp => with_context_menu(state, ContextMenu::cursor_up),
        Message::ContextMenuCursorDown => with_context_menu(state, ContextMenu::cursor_down),
        Message::ContextMenuActivate => activate_context_menu(state, None),
        Message::ContextMenuActivateAt(i) => activate_context_menu(state, Some(i)),

        // ── Mouse-capture toggle ────────────────────────────────────────────
        Message::ToggleMouseCapture => {
            state.mouse_capture = !state.mouse_capture;
            let msg = if state.mouse_capture {
                "Mouse capture on"
            } else {
                "Mouse capture off · terminal selection restored"
            };
            state.toasts.push(ToastKind::Info, msg);
            Outcome {
                redraw: true,
                effect: Some(Effect::SetMouseCapture(state.mouse_capture)),
            }
        }

        // ── Perf sparkline panel ────────────────────────────────────────────
        Message::TogglePerfPanel => with_active(state, |s| s.perf.toggle()),

        // ── DevTools mode (workbook §B12) ────────────────────────────────────
        Message::DevtoolsToggle => {
            let Some(idx) = state.active_session else {
                return Outcome::idle();
            };
            state.sessions[idx].devtools.open = !state.sessions[idx].devtools.open;
            // Opening is the trigger for the first connect: the discovery
            // line may have scrolled past long before the user asked to look.
            // It is also the trigger for the metrics sampler — cheap either
            // way, so it starts on open regardless of which tab is active
            // rather than waiting for a first System/Network visit (see
            // `metrics_start`'s doc).
            let effect = if state.sessions[idx].devtools.open {
                let mut effects = Vec::new();
                match devtools_connect(state, idx) {
                    Some(effect) => effects.push(effect),
                    // Re-opening onto a retained connection connects
                    // nothing, so an Inspector tab left selected gets its
                    // snapshot here instead (the two are mutually
                    // exclusive: a fresh connect leaves the phase
                    // `Connecting`, which pulls nothing).
                    None => effects.extend(devtools_inspector_enter(&mut state.sessions[idx])),
                }
                effects.extend(metrics_start(state, idx));
                batch(effects)
            } else {
                None
            };
            Outcome {
                redraw: true,
                effect,
            }
        }
        Message::DevtoolsClose => {
            let Some(session) = state.active_session_mut() else {
                return Outcome::idle();
            };
            if !session.devtools.open {
                return Outcome::idle();
            }
            session.devtools.open = false;
            Outcome::redraw()
        }
        Message::DevtoolsTab(index) => with_active_devtools_tab(state, |d| {
            d.select_tab(index);
        }),
        Message::DevtoolsTabCycle(delta) => with_active_devtools_tab(state, |d| {
            d.cycle_tab(delta);
        }),
        Message::DevtoolsRetry => {
            let Some(idx) = state.active_session else {
                return Outcome::idle();
            };
            // A terminal session's service died with the process — retrying
            // would just re-fail against a port nobody is listening on.
            if state.sessions[idx].state.is_terminal() {
                return Outcome::idle();
            }
            match devtools_connect(state, idx) {
                Some(effect) => Outcome {
                    redraw: true,
                    effect: Some(effect),
                },
                None => Outcome::idle(),
            }
        }
        Message::DevtoolsConn(id, event) => {
            let Some(idx) = state.session_index(id) else {
                return Outcome::idle();
            };
            let is_active = state.active_session == Some(idx);
            // A handshake completing is the other moment the Inspector can
            // pull its first snapshot: the tab may have been selected (and
            // shown empty) all through the connect. A *frame batch* is not —
            // it carries no connection-state change, and running the entry
            // check on every batch is what let a failing pull re-fire at
            // frame-batch rate (the `wants_tree` latch is the second half of
            // that fix).
            let handshake = matches!(event, ConnEvent::Connected { .. });
            let mut changed = state.sessions[idx].devtools.apply(event);
            // The drawn window just slid: a pinned frame that has aged out of
            // it drops back to the live tail rather than being re-pointed at
            // whatever frame took its place.
            changed |= devtools_perf_retain_selection(&mut state.sessions[idx]);
            let effect = handshake
                .then(|| devtools_inspector_entry(&mut state.sessions[idx]))
                .flatten();
            // Only the visible session's DevTools surface can be dirtied by a
            // report; a background session's ring keeps filling silently
            // (the dirty-frame skip).
            Outcome {
                redraw: changed && is_active && state.sessions[idx].devtools.open,
                effect,
            }
        }
        Message::DevtoolsMetrics(id, samples) => {
            let Some(idx) = state.session_index(id) else {
                return Outcome::idle();
            };
            let is_active = state.active_session == Some(idx);
            let mut changed = false;
            for sample in samples {
                changed |= state.sessions[idx].devtools.metrics.apply(sample);
            }
            // Same dirty-frame-skip gating as `DevtoolsConn`'s frame
            // batches: only the visible, DevTools-open session's System/
            // Network surface is worth a repaint.
            Outcome::dirty(changed && is_active && state.sessions[idx].devtools.open)
        }

        // ── Performance tab (workbook §B12) ──────────────────────────────────
        Message::DevtoolsPerfScrub(delta) => with_active(state, |s| {
            let window = devtools_perf_window(s);
            s.devtools.performance.scrub(delta, &window);
        }),
        Message::DevtoolsPerfSelectFrame(n) => with_active(state, |s| {
            let window = devtools_perf_window(s);
            s.devtools.performance.select_frame(n, &window);
        }),
        Message::DevtoolsPerfClearSelection => with_active(state, |s| {
            s.devtools.performance.clear_selection();
        }),
        Message::DevtoolsPerfFocusCycle => with_active(state, |s| {
            s.devtools.performance.cycle_focus();
        }),

        // ── Inspector tab (workbook §B12) ────────────────────────────────────
        Message::DevtoolsInspectorSelect(delta) => with_inspector(state, |i| i.select(delta)),
        Message::DevtoolsInspectorSelectRow(index) => {
            with_inspector(state, |i| i.select_row(index))
        }
        Message::DevtoolsInspectorExpand => with_inspector(state, InspectorTab::expand),
        Message::DevtoolsInspectorCollapse => with_inspector(state, InspectorTab::collapse),
        Message::DevtoolsInspectorToggleNode(id) => with_inspector(state, |i| i.toggle_node(id)),
        Message::DevtoolsInspectorFocusCycle => with_active(state, |s| {
            s.devtools.inspector.cycle_focus();
        }),
        Message::DevtoolsInspectorRefresh => {
            let Some(session) = state.active_session_mut() else {
                return Outcome::idle();
            };
            // `r` only means "refresh" where there is a service to ask; on
            // the failed screen the same key is the retry (§B12's mutually
            // exclusive contexts), routed as `Message::DevtoolsRetry`.
            if session.devtools.phase() != DevtoolsPhase::Connected
                || !session.devtools.inspector.begin_tree_fetch()
            {
                return Outcome::idle();
            }
            Outcome {
                redraw: true,
                effect: Some(Effect::DevtoolsFetchTree {
                    session: session.id,
                }),
            }
        }
        Message::DevtoolsInspector(id, event) => {
            let Some(idx) = state.session_index(id) else {
                return Outcome::idle();
            };
            let is_active = state.active_session == Some(idx);
            let session = &mut state.sessions[idx];
            let changed = session.devtools.inspector.apply(event);
            // A fresh snapshot drops the (now stale) props cache, so whatever
            // the reconciled selection landed on needs its own pull — the
            // same second call a selection move makes.
            let effect = devtools_props_fetch(session);
            Outcome {
                redraw: changed && is_active && session.devtools.open,
                effect,
            }
        }

        // ── Responsive breakpoints ──────────────────────────────────────────
        Message::ToggleSidebarOverlay => {
            state.sidebar_overlay_open = !state.sidebar_overlay_open;
            Outcome::redraw()
        }

        // ── Help overlay ─────────────────────────────────────────────────────
        Message::OpenHelpOverlay => {
            if state.help_open {
                Outcome::idle()
            } else {
                state.help_open = true;
                Outcome::redraw()
            }
        }
        Message::CloseHelpOverlay => {
            if state.help_open {
                state.help_open = false;
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }

        // ── Embedded MCP server ──────────────────────────────────────────────
        // The command itself is served by `crate::runner`, which holds the
        // `Supervisor` this pure core cannot reach — it never arrives here in
        // a wired workbench. Reaching this arm therefore means nothing is
        // serving MCP: dropping the command drops its reply channel, which is
        // exactly the "the workbench did not answer" signal the backend
        // reports as a typed error (`crate::supervise::EmbeddedError`).
        Message::Mcp(_) => Outcome::idle(),
        // Both reports below are gated on the generation naming the server
        // *currently installed* on the model. A stopping server's tasks
        // outlive `stop_mcp`'s handle drop (graceful shutdown runs on), so a
        // start in that window leaves two servers reporting; applying the
        // older one's report to the newer one's handle would stamp it with
        // the wrong port, or — far worse — clear it, and since dropping a
        // `CancellationToken` does not cancel it, that server would be left
        // listening with nothing able to stop it.
        Message::McpListening(generation, port) => match state.mcp.as_mut() {
            Some(handle) if handle.generation() == generation => {
                Outcome::dirty(handle.set_bound_port(port))
            }
            // The server was stopped between spawning and binding (its own
            // shutdown follows), or has already been superseded.
            Some(_) | None => Outcome::idle(),
        },
        Message::McpStopped(generation, error) => {
            match state.mcp.as_ref().map(|handle| handle.generation()) {
                // A superseded server winding down: the handle here is
                // somebody else's, and neither clearing it nor reporting its
                // predecessor's fate would be true of the running server.
                Some(installed) if installed != generation => Outcome::idle(),
                installed => {
                    let was_running = installed.is_some();
                    state.mcp = None;
                    match error {
                        Some(error) => {
                            // Two surfaces, deliberately: the toast catches the
                            // eye of someone looking elsewhere, and `mcp_error`
                            // keeps the reason on the sidebar row / panel
                            // afterwards, so a failed start never reads as a
                            // silent no-op (§B13).
                            state
                                .toasts
                                .push(ToastKind::Error, format!("MCP server stopped: {error}"));
                            state.mcp_error = Some(error);
                            Outcome::redraw()
                        }
                        None => Outcome::dirty(was_running),
                    }
                }
            }
        }
        Message::ToggleMcpServer => {
            if state.mcp.is_some() {
                // The handle is dropped by the runner's `stop_mcp`, not here:
                // cancelling a live server is exactly the impure work the
                // pure core pushes out as an effect.
                Outcome {
                    redraw: true,
                    effect: Some(Effect::StopMcpServer),
                }
            } else {
                // A retry clears the previous failure first, so the panel
                // shows "starting…" rather than the stale reason beside it.
                state.mcp_error = None;
                Outcome {
                    redraw: true,
                    effect: Some(Effect::StartMcpServer),
                }
            }
        }
        Message::OpenMcpPanel => {
            if state.mcp_panel_open {
                Outcome::idle()
            } else {
                state.mcp_panel_open = true;
                Outcome::redraw()
            }
        }
        Message::CloseMcpPanel => {
            if state.mcp_panel_open {
                state.mcp_panel_open = false;
                Outcome::redraw()
            } else {
                Outcome::idle()
            }
        }

        // The embedded DAP server's two reports, gated on the generation
        // naming the server *currently installed* — the identical race, and
        // the identical consequence of losing it, as the MCP pair above.
        //
        // A fresh listener is also one of the two auto-configuration triggers
        // (the other is an app launch — see `launch_effect`), but only when a
        // session is active: the config goes into *that* session's project,
        // named with the port actually bound (a server started on `0` learns
        // its port here and nowhere else). With no session, a bind writes
        // nothing — the workbench's active project at startup is just the
        // first `frust.toml` the walk found, not a project anyone asked to
        // debug.
        Message::DapListening(generation, port) => {
            let names_installed = state
                .dap
                .as_ref()
                .is_some_and(|handle| handle.generation() == generation);
            if !names_installed {
                // Stopped between spawning and binding, or already superseded.
                return Outcome::idle();
            }
            let changed = state
                .dap
                .as_mut()
                .is_some_and(|handle| handle.set_bound_port(port));
            // Gated on `changed`, so auto-configuration runs once per bind:
            // a repeat report for a port already recorded is the same server
            // saying the same thing, not a second listener to reconfigure for.
            let effect = if changed {
                state
                    .active_session()
                    .map(|session| session.project_root.clone())
                    .and_then(|root| auto_ide_config(state, &root))
            } else {
                None
            };
            Outcome {
                redraw: changed,
                effect,
            }
        }
        Message::DapStopped(generation, error) => {
            match state.dap.as_ref().map(|handle| handle.generation()) {
                Some(installed) if installed != generation => Outcome::idle(),
                installed => {
                    let was_running = installed.is_some();
                    state.dap = None;
                    match error {
                        Some(error) => {
                            state
                                .toasts
                                .push(ToastKind::Error, format!("DAP server stopped: {error}"));
                            state.dap_error = Some(error);
                            Outcome::redraw()
                        }
                        None => Outcome::dirty(was_running),
                    }
                }
            }
        }

        // ── DAP settings dialog + the preferences behind it ──────────────────
        Message::ToggleDapServer => {
            if state.dap.is_some() {
                // The handle is dropped by the runner's `stop_dap`, never
                // here — the same live-resource split as the MCP toggle.
                Outcome {
                    redraw: true,
                    effect: Some(Effect::StopDapServer),
                }
            } else {
                state.dap_error = None;
                // Starting is one of the two answers to the first-run notice,
                // so the notice has served its purpose either way.
                state.dap_settings.intro_port = None;
                Outcome {
                    redraw: true,
                    effect: Some(Effect::StartDapServer {
                        port: state.dap_settings.port,
                    }),
                }
            }
        }
        Message::DapAutoStart => {
            let settings = &state.dap_settings;
            let wanted = super::dap_settings::should_auto_start(
                settings.enabled,
                settings.auto_start_in_ide,
                settings.detected_ide,
            );
            if !wanted || state.dap.is_some() {
                Outcome::idle()
            } else if settings.intro_seen {
                // Every launch after the first: exactly the silent auto-start
                // the preferences ask for.
                Outcome::effect(Effect::StartDapServer {
                    port: settings.port,
                })
            } else {
                // The first auto-start this install would ever have performed:
                // say what it is about to open before opening it. The server
                // is *not* started here — only the dialog's own Start action
                // does that on this one run. The notice is spent immediately
                // (persisted before anything else can happen), so quitting
                // without acting never brings it back.
                let port = settings.port;
                update(state, Message::OpenDapSettings);
                state.dap_settings.intro_seen = true;
                state.dap_settings.intro_port = Some(port);
                Outcome {
                    redraw: true,
                    effect: Some(Effect::SaveDapSetting(DapSetting::IntroSeen(true))),
                }
            }
        }
        Message::OpenDapSettings => {
            if state.dap_settings_open {
                Outcome::idle()
            } else {
                state.dap_settings.reopen();
                state.dap_settings_open = true;
                Outcome::redraw()
            }
        }
        Message::CloseDapSettings => {
            if state.dap_settings_open {
                state.dap_settings_open = false;
                // Dismissing is the other answer to the first-run notice.
                state.dap_settings.intro_port = None;
                // Closing commits whatever is in the port field, so a typed
                // value never quietly evaporates (and an invalid one never
                // quietly sticks — see `commit_dap_port`).
                let effect = commit_dap_port(state);
                Outcome {
                    redraw: true,
                    effect,
                }
            } else {
                Outcome::idle()
            }
        }
        Message::DapSettingsFocusNext => {
            state.dap_settings.focus_next();
            Outcome::redraw()
        }
        Message::DapSettingsFocusPrev => {
            state.dap_settings.focus_prev();
            Outcome::redraw()
        }
        Message::DapSettingsFocus(focus) => {
            let changed = state.dap_settings.focus != focus;
            state.dap_settings.focus = focus;
            Outcome::dirty(changed)
        }
        Message::DapSettingsInput(c) => {
            state.dap_settings.input_char(c);
            Outcome::redraw()
        }
        Message::DapSettingsBackspace => {
            state.dap_settings.backspace();
            Outcome::redraw()
        }
        Message::DapSettingsActivate => match state.dap_settings.focus {
            DapFocus::Server => update(state, Message::ToggleDapServer),
            DapFocus::Port => {
                let effect = commit_dap_port(state);
                Outcome {
                    redraw: true,
                    effect,
                }
            }
            DapFocus::AutoStart => update(state, Message::DapSettingsToggleAutoStart),
            DapFocus::AutoConfigure => update(state, Message::DapSettingsToggleAutoConfigure),
            DapFocus::Ide => update(state, Message::DapSettingsCycleIde(1)),
            DapFocus::Generate => update(state, Message::DapSettingsGenerate),
        },
        Message::DapSettingsCycleIde(delta) => {
            state.dap_settings.cycle_ide(delta);
            state.dap_settings.focus = DapFocus::Ide;
            Outcome {
                redraw: true,
                effect: Some(Effect::SaveDapSetting(DapSetting::IdeOverride(
                    state.dap_settings.ide_override,
                ))),
            }
        }
        Message::DapSettingsToggleAutoStart => {
            let on = !state.dap_settings.auto_start_in_ide;
            state.dap_settings.auto_start_in_ide = on;
            state.dap_settings.focus = DapFocus::AutoStart;
            Outcome {
                redraw: true,
                effect: Some(Effect::SaveDapSetting(DapSetting::AutoStartInIde(on))),
            }
        }
        Message::DapSettingsToggleAutoConfigure => {
            let on = !state.dap_settings.auto_configure_ide;
            state.dap_settings.auto_configure_ide = on;
            state.dap_settings.focus = DapFocus::AutoConfigure;
            Outcome {
                redraw: true,
                effect: Some(Effect::SaveDapSetting(DapSetting::AutoConfigureIde(on))),
            }
        }
        Message::DapSettingsGenerate => {
            // A running listener's *bound* port is what an editor must be
            // pointed at; the configured port is only a proposal until then.
            let port = match state.dap_status() {
                crate::supervise::DapStatus::Listening { port, .. } => port,
                _ => state.dap_settings.port,
            };
            let project_root = state.project_root.clone();
            request_ide_config(state, port, project_root, WriteMode::Refresh)
        }
        Message::DapIdeConfig(report) => {
            // An automatic write's report arrives tagged with its project and
            // IDE: a repeating outcome (stale port, failure) toasts once per
            // pair per run. The explicit path's report arrives bare and
            // always toasts. Either way the dialog keeps the latest outcome.
            let (report, toast) = match report {
                DapIdeReport::Auto {
                    project_root,
                    ide,
                    report,
                } => {
                    let toast = state
                        .dap_settings
                        .admit_auto_toast(&project_root, ide, &report);
                    (*report, toast)
                }
                report => (report, true),
            };
            if toast {
                let kind = if report.is_failure() {
                    ToastKind::Error
                } else if report.is_stale_port() {
                    ToastKind::Warn
                } else {
                    ToastKind::Info
                };
                state
                    .toasts
                    .push(kind, format!("DAP · {}", report.summary()));
            }
            state.dap_settings.last_ide_config = Some(report);
            Outcome::redraw()
        }
        Message::Notify { level, text } => {
            state.toasts.push(level, text);
            Outcome::redraw()
        }
    }
}

/// Commit the DAP settings dialog's port field, returning the persistence
/// effect a real change earns.
///
/// A rejected value is surfaced as a warn toast rather than silently dropped
/// (the field itself is restored to the port still in effect), and a change
/// made while a server is listening says so instead of restarting it —
/// a port change applies to the *next* start, never to the running listener.
fn commit_dap_port(state: &mut AppState) -> Option<Effect> {
    match state.dap_settings.commit_port() {
        super::dap_settings::PortCommit::Changed(port) => {
            if let crate::supervise::DapStatus::Listening { port: bound, .. } = state.dap_status()
                && bound != port
            {
                state.toasts.push(
                    ToastKind::Info,
                    format!("DAP port {port} takes effect on the next start"),
                );
            }
            Some(Effect::SaveDapSetting(DapSetting::Port(port)))
        }
        super::dap_settings::PortCommit::Rejected(text) => {
            state.toasts.push(
                ToastKind::Warn,
                format!(
                    "'{text}' is not a valid port — keeping {}",
                    state.dap_settings.port
                ),
            );
            None
        }
        super::dap_settings::PortCommit::Unchanged => None,
    }
}

/// The IDE a config would be generated for, or the refusal explaining why
/// there is none: no IDE detected or chosen, or an IDE with no DAP config
/// format at all.
fn config_ide(state: &AppState) -> Result<ParentIde, DapIdeReport> {
    let Some(ide) = state.dap_settings.effective_ide() else {
        return Err(DapIdeReport::NoIde);
    };
    if !ide.supports_dap_config() {
        return Err(DapIdeReport::Unsupported(ide));
    }
    Ok(ide)
}

/// Request an IDE DAP-config generation for `port` into `project_root` with
/// `mode`, or record why there is nothing to generate — the dialog's explicit
/// "generate now" path.
///
/// The generation itself reads and writes real files, so it can only be an
/// [`Effect`] — but every *refusal* is decided here, in the pure core, and
/// stored where the dialog shows it: an absent IDE, an IDE with no DAP config
/// format at all, and a workbench with no project open are each reported
/// rather than silently doing nothing. (The automatic path,
/// [`auto_ide_config`], refuses silently instead.)
fn request_ide_config(
    state: &mut AppState,
    port: u16,
    project_root: Option<PathBuf>,
    mode: WriteMode,
) -> Outcome {
    fn report(state: &mut AppState, report: DapIdeReport) -> Outcome {
        state.dap_settings.last_ide_config = Some(report);
        Outcome::redraw()
    }

    let ide = match config_ide(state) {
        Ok(ide) => ide,
        Err(refusal) => return report(state, refusal),
    };
    let Some(project_root) = project_root else {
        return report(
            state,
            DapIdeReport::Failed(
                "no project open — there is nothing to write a launch config into".to_string(),
            ),
        );
    };
    Outcome::effect(Effect::GenerateIdeConfig(IdeConfigRequest {
        ide,
        port,
        project_root,
        mode,
    }))
}

/// The automatic IDE-config write for `project_root`, if one is due: only
/// with `auto_configure_ide` on and the DAP server currently listening (its
/// bound port is what the config names), and always
/// [`WriteMode::IfAbsent`] — an existing frust entry is never rewritten.
///
/// Refusals (no IDE, an IDE with no DAP config format) are silent here: the
/// automatic path runs on every launch, and neither stores a
/// [`DapIdeReport`] nor toasts — only the dialog's explicit
/// [`request_ide_config`] reports them.
fn auto_ide_config(state: &AppState, project_root: &Path) -> Option<Effect> {
    if !state.dap_settings.auto_configure_ide {
        return None;
    }
    let crate::supervise::DapStatus::Listening { port, .. } = state.dap_status() else {
        return None;
    };
    let ide = config_ide(state).ok()?;
    Some(Effect::GenerateIdeConfig(IdeConfigRequest {
        ide,
        port,
        project_root: project_root.to_path_buf(),
        mode: WriteMode::IfAbsent,
    }))
}

/// The effect for launching `specs` — every workbench launch path
/// (the run-config modal, run-on-all-devices) goes through here, so an app
/// launch is where the editor's DAP client config gets written: one
/// [`Effect::LaunchSessions`], followed (via [`Effect::Batch`]) by one
/// [`auto_ide_config`] write per *distinct* project root among the specs,
/// when one is due. `None` for no specs. With `watch` set (the run-config
/// modal's checkbox), the desktop specs go out as
/// [`Effect::LaunchWatchedSessions`] instead and the device specs stay in a
/// plain [`Effect::LaunchSessions`] — watch is desktop-only.
///
/// Sessions launched through the MCP/DAP backend
/// (`crate::supervise::mcp_backend`) never pass through here, by design: the
/// DAP client that launched them already had a config to connect with.
fn launch_effect(state: &AppState, specs: Vec<SessionSpec>, watch: bool) -> Option<Effect> {
    if specs.is_empty() {
        return None;
    }
    let mut roots: Vec<&Path> = Vec::new();
    for spec in &specs {
        let root = spec.project_root.as_path();
        // `Path` equality is already component-wise.
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    let configs: Vec<Effect> = roots
        .into_iter()
        .filter_map(|root| auto_ide_config(state, root))
        .collect();
    let mut effects = Vec::with_capacity(2 + configs.len());
    if watch {
        let (desktop, devices): (Vec<SessionSpec>, Vec<SessionSpec>) = specs
            .into_iter()
            .partition(|spec| matches!(spec.target, DeviceTarget::Desktop));
        if !desktop.is_empty() {
            effects.push(Effect::LaunchWatchedSessions(desktop));
        }
        if !devices.is_empty() {
            effects.push(Effect::LaunchSessions(devices));
        }
    } else {
        effects.push(Effect::LaunchSessions(specs));
    }
    effects.extend(configs);
    batch(effects)
}

/// The scrollbar-track fraction (`0.0`..=`1.0`, top→bottom) for a pointer at
/// absolute row `y` over a track spanning `track_top .. track_top +
/// track_height`. A degenerate (≤1-row) track maps everything to the top.
fn track_fraction(y: u16, track_top: u16, track_height: u16) -> f32 {
    if track_height <= 1 {
        return 0.0;
    }
    let rel = y.saturating_sub(track_top).min(track_height - 1);
    rel as f32 / (track_height - 1) as f32
}

/// Open a context menu for `target`, focusing the target row first (mouse
/// parity with a left click) so a follow-up entry acts on the row the user
/// pointed at. An empty entry set (nothing to offer) opens no menu but still
/// commits the focus change.
fn open_context_menu(state: &mut AppState, x: u16, y: u16, target: ContextTarget) -> Outcome {
    match target {
        ContextTarget::SessionTab(i) if i < state.sessions.len() => {
            state.active_session = Some(i);
        }
        ContextTarget::DeviceRow(i) if i < state.devices.len() => {
            state.device_cursor = i;
        }
        _ => {}
    }
    let entries = super::context_menu::entries_for(state, target);
    if entries.is_empty() {
        return Outcome::redraw();
    }
    state.context_menu = Some(ContextMenu {
        x,
        y,
        target,
        entries,
        cursor: 0,
    });
    Outcome::redraw()
}

/// Apply `f` to the open context menu (if any), redrawing only when it reports
/// a change; idle when closed.
fn with_context_menu(state: &mut AppState, f: impl FnOnce(&mut ContextMenu) -> bool) -> Outcome {
    match state.context_menu.as_mut() {
        Some(menu) => Outcome::dirty(f(menu)),
        None => Outcome::idle(),
    }
}

/// Activate a context-menu entry: the row at `index` (a click) or the
/// highlighted row (`Enter`). Closes the menu and **re-dispatches the entry's
/// existing `Message`** through `update` — the menu never duplicates command
/// logic (the palette discipline). A disabled entry is a no-op (the menu stays
/// open); an out-of-range index is ignored.
fn activate_context_menu(state: &mut AppState, index: Option<usize>) -> Outcome {
    let Some(menu) = state.context_menu.as_ref() else {
        return Outcome::idle();
    };
    let idx = index.unwrap_or(menu.cursor);
    let Some(entry) = menu.entries.get(idx) else {
        return Outcome::idle();
    };
    if !entry.enabled {
        return Outcome::idle();
    }
    let message = entry.message.clone();
    state.context_menu = None;
    let mut out = update(state, message);
    out.redraw = true;
    out
}

/// Open the create wizard and request the off-thread clean-signals sibling
/// probe that gates its arch card.
fn open_create_wizard(state: &mut AppState) -> Outcome {
    state.create_wizard = Some(CreateWizard::new());
    Outcome {
        redraw: true,
        effect: Some(Effect::ProbeCleanSignals),
    }
}

/// Open `root` as the active project in place (a freshly scaffolded project, or
/// a switch): register it in `projects` keeping the local/previous boundary
/// invariant (see `AppState::insert_project`), make it active, show the
/// workbench, focus its first session (if any), and request the runner persist
/// it as most-recently-opened. `root` is run through `host_path::simplify`
/// first — the wizard/open-result entry point paths enter state through, so a
/// Windows verbatim scaffold path never reaches `projects`/`project_root`.
fn open_project(state: &mut AppState, root: PathBuf) -> Outcome {
    let root = frust_drive::host_path::simplify(&root);
    state.insert_project(root.clone());
    state.project_root = Some(root.clone());
    state.screen = Screen::Workbench;
    state.active_session = state.sessions.iter().position(|s| s.project_root == root);
    state.clamp_project_switcher_cursor();
    Outcome {
        redraw: true,
        effect: Some(Effect::RecordRecentProject(root)),
    }
}

/// Open the Add Plugin dialog for the active project and request the off-thread
/// sibling probe that gates a facade-tier plugin card (shared with the create
/// wizard's clean-signals gating). A no-op — with a warn toast — when no
/// project is open (the dialog edits a *generated* project; there's nothing to
/// add to otherwise).
fn open_add_plugin(state: &mut AppState) -> Outcome {
    if state.add_plugin.is_some() {
        return Outcome::idle();
    }
    let Some(root) = state
        .project_root
        .clone()
        .or_else(|| state.projects.first().cloned())
    else {
        state
            .toasts
            .push(ToastKind::Warn, "Open a project first to add a plugin");
        return Outcome::redraw();
    };
    state.add_plugin = Some(AddPluginDialog::new(root));
    Outcome {
        redraw: true,
        effect: Some(Effect::ProbeCleanSignals),
    }
}

/// The `AddPluginSucceeded` toast text: `"Added <id> · <applied> applied,
/// <already> already present[, <n> applied at build]"` — built from
/// [`frust_drive::plugin::AddReport::outcome_counts`]'s three-bucket shape,
/// the same counting rule `crate::ui::views::add_plugin`'s
/// `report_summary_line` uses, so a desktop-lane [`AddOutcome::AppliedAtBuild`]
/// item is never mislabeled "already present" here either. The trailing
/// clause is only appended when the report actually carries one.
fn add_plugin_toast_text(report: &frust_drive::plugin::AddReport) -> String {
    let (applied, already_present, applied_at_build) = report.outcome_counts();
    let mut text = format!(
        "Added {} · {applied} applied, {already_present} already present",
        report.plugin_id
    );
    if applied_at_build > 0 {
        text.push_str(&format!(", {applied_at_build} applied at build"));
    }
    text
}

/// Apply `f` to the open Add Plugin dialog (if any) and redraw; idle when
/// closed.
fn with_add_plugin(state: &mut AppState, f: impl FnOnce(&mut AddPluginDialog)) -> Outcome {
    match state.add_plugin.as_mut() {
        Some(dialog) => {
            f(dialog);
            Outcome::redraw()
        }
        None => Outcome::idle(),
    }
}

/// Apply `f` to the open create wizard (if any) and redraw; idle when closed.
fn with_wizard(state: &mut AppState, f: impl FnOnce(&mut CreateWizard)) -> Outcome {
    match state.create_wizard.as_mut() {
        Some(wizard) => {
            f(wizard);
            Outcome::redraw()
        }
        None => Outcome::idle(),
    }
}

/// Move the switcher's highlighted row by `delta`, clamped to `projects`;
/// redraw only on a real move. A no-op while the switcher is closed or
/// `projects` is empty.
fn move_project_switcher_cursor(state: &mut AppState, delta: isize) -> Outcome {
    if !state.project_switcher_open || state.projects.is_empty() {
        return Outcome::idle();
    }
    let n = state.projects.len();
    let cur = state.project_switcher_cursor.min(n - 1) as isize;
    let next = (cur + delta).clamp(0, n as isize - 1) as usize;
    if next == state.project_switcher_cursor {
        Outcome::idle()
    } else {
        state.project_switcher_cursor = next;
        Outcome::redraw()
    }
}

/// Switch the active project to `state.projects[index]`: closes the
/// switcher, re-focuses `active_session` to the first session under the new
/// project (`None` — the dashboard — if it has none, so the active project
/// really does drive which sessions group is focused), and requests the
/// runner persist it as the most-recently-opened project. An out-of-range
/// index (a stale click after the list changed) is a no-op.
fn switch_project(state: &mut AppState, index: usize) -> Outcome {
    let Some(target) = state.projects.get(index).cloned() else {
        return Outcome::idle();
    };
    state.project_root = Some(target.clone());
    state.project_switcher_open = false;
    state.active_session = state.sessions.iter().position(|s| s.project_root == target);
    Outcome {
        redraw: true,
        effect: Some(Effect::RecordRecentProject(target)),
    }
}

/// The project the run-config modal launches under: the active project, else
/// the first detected project. `None` on the welcome screen (no project).
fn run_config_project(state: &AppState) -> Option<std::path::PathBuf> {
    state
        .project_root
        .clone()
        .or_else(|| state.projects.first().cloned())
}

/// Move the devices-panel cursor by `delta`, clamped to the list; redraw only
/// on a real move.
fn move_device_cursor(state: &mut AppState, delta: isize) -> Outcome {
    let n = state.devices.len();
    if n == 0 {
        return Outcome::idle();
    }
    let cur = state.device_cursor.min(n - 1) as isize;
    let next = (cur + delta).clamp(0, n as isize - 1) as usize;
    if next == state.device_cursor {
        Outcome::idle()
    } else {
        state.device_cursor = next;
        Outcome::redraw()
    }
}

/// Apply `f` to the open run-config modal (if any) and redraw; idle when the
/// modal is closed.
fn with_modal(state: &mut AppState, f: impl FnOnce(&mut RunConfig)) -> Outcome {
    match state.run_config.as_mut() {
        Some(modal) => {
            f(modal);
            Outcome::redraw()
        }
        None => Outcome::idle(),
    }
}

/// Apply `f` to the open build-launcher modal (if any) and redraw; idle when
/// closed — mirrors [`with_modal`].
fn with_build_launcher(state: &mut AppState, f: impl FnOnce(&mut BuildLauncher)) -> Outcome {
    match state.build_launcher.as_mut() {
        Some(launcher) => {
            f(launcher);
            Outcome::redraw()
        }
        None => Outcome::idle(),
    }
}

/// Apply `f` to the open bootstrap wizard (if any) and redraw; idle when
/// closed — mirrors [`with_modal`].
fn with_bootstrap(state: &mut AppState, f: impl FnOnce(&mut BootstrapWizard)) -> Outcome {
    match state.bootstrap_wizard.as_mut() {
        Some(wizard) => {
            f(wizard);
            Outcome::redraw()
        }
        None => Outcome::idle(),
    }
}

/// Apply `f` to the open command palette (if any) and redraw; idle when closed
/// — mirrors [`with_modal`].
fn with_palette(state: &mut AppState, f: impl FnOnce(&mut super::palette::Palette)) -> Outcome {
    match state.palette.as_mut() {
        Some(palette) => {
            f(palette);
            Outcome::redraw()
        }
        None => Outcome::idle(),
    }
}

/// Move the palette selection by `delta`, clamped to the current ranked-result
/// count; redraw only on a real move.
fn move_palette_cursor(state: &mut AppState, delta: isize) -> Outcome {
    let n = super::palette::ranked(state).len();
    let Some(palette) = state.palette.as_mut() else {
        return Outcome::idle();
    };
    if n == 0 {
        palette.cursor = 0;
        return Outcome::idle();
    }
    let cur = palette.cursor.min(n - 1) as isize;
    let next = (cur + delta).clamp(0, n as isize - 1) as usize;
    if next == palette.cursor {
        Outcome::idle()
    } else {
        palette.cursor = next;
        Outcome::redraw()
    }
}

/// Execute a palette command: the row at `index` (a click) or the selected row
/// (`Enter`). Closes the palette and **re-dispatches the command's existing
/// `Message`** through `update` — the palette never duplicates command logic. A
/// disabled command is a no-op (the palette stays open). An out-of-range index
/// (a stale click after the ranking changed) is ignored.
fn execute_palette(state: &mut AppState, index: Option<usize>) -> Outcome {
    let ranked = super::palette::ranked(state);
    let idx = index.unwrap_or_else(|| state.palette.as_ref().map_or(0, |p| p.cursor));
    let Some(cmd) = ranked.get(idx) else {
        return Outcome::idle();
    };
    if !cmd.enabled {
        // A disabled command can't run; leave the palette open (its row shows
        // the reason) rather than silently closing.
        return Outcome::idle();
    }
    let message = cmd.message.clone();
    state.palette = None;
    // Route the command's own message through the same transition every other
    // input path uses; force a redraw since closing the palette is itself a
    // visible change even when the underlying message reports none.
    let mut out = update(state, message);
    out.redraw = true;
    out
}

/// Launch one supervised session per discovered device (debug) for the active
/// project — the palette's "run on all devices" action. Reuses [`RunConfig`]'s
/// spec building (every device checked, desktop off) so it never re-implements
/// launch logic. A no-op with no project or no devices.
fn run_on_all_devices(state: &mut AppState) -> Outcome {
    let Some(project) = run_config_project(state) else {
        return Outcome::idle();
    };
    if state.devices.is_empty() {
        return Outcome::idle();
    }
    let mut config = RunConfig::new(project, &state.devices);
    for target in &mut config.targets {
        target.selected = matches!(target.target, DeviceTarget::Device(_));
    }
    let specs = config.launch_specs();
    if specs.is_empty() {
        return Outcome::idle();
    }
    let specs = drop_already_running(state, specs);
    if specs.is_empty() {
        // Every device already runs this project; the toasts say so.
        return Outcome::redraw();
    }
    Outcome {
        redraw: true,
        effect: launch_effect(state, specs, false),
    }
}

/// Keep only the specs whose (project, target) has no live session yet,
/// pushing one `Warn` toast per refused spec — the one-live-session-per-
/// (project, device) guard on the workbench's own launch paths (its MCP/DAP
/// counterpart is `crate::supervise::mcp_backend`'s typed refusal).
///
/// Refusing here rather than in the runner keeps the decision in the pure
/// core, where it is testable and where the toast explaining it is written:
/// a spec that survives is one nothing is running yet, so the effect the
/// runner enacts never needs a second opinion. Launching the same app onto
/// the same device twice is never what the user meant — the second install
/// replaces the first app's process behind its own still-streaming session
/// tab, leaving a tab that logs nothing and a stop that stops the wrong
/// thing.
fn drop_already_running(state: &mut AppState, specs: Vec<SessionSpec>) -> Vec<SessionSpec> {
    let mut launchable = Vec::with_capacity(specs.len());
    for spec in specs {
        let target = SessionTarget::of(&spec.target);
        if state
            .live_session_for(&spec.project_root, &target)
            .is_some()
        {
            state.toasts.push(
                ToastKind::Warn,
                format!(
                    "{}: already running here — stop it first (x)",
                    target.label()
                ),
            );
        } else {
            launchable.push(spec);
        }
    }
    launchable
}

/// A project's short display name (its final path component), for a toast.
fn project_name_of(root: &std::path::Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned())
}

/// Cache a freshly-arrived component-level report: it feeds the titlebar chip's
/// rollup and, if the wizard is open, refreshes its snapshot in place. On the
/// first report of a launch it also drives the fresh-machine auto-open — the
/// wizard pops once (never re-nags) when the core toolchain is `Missing` and no
/// other modal is already up.
fn on_bootstrap_report(state: &mut AppState, report: frust_drive::doctor::DoctorReport) -> Outcome {
    use frust_drive::doctor::ComponentStatus;

    state.bootstrap.refreshing = false;
    if let Some(wizard) = state.bootstrap_wizard.as_mut() {
        wizard.set_report(report.clone());
    }
    let first = !state.bootstrap.auto_shown;
    state.bootstrap.auto_shown = true;
    let missing_core = report.rollup() == ComponentStatus::Missing;
    state.bootstrap.report = Some(report);

    // Fresh-machine auto-open: only on the first report, only for a blocking
    // (Missing) core, and only when nothing else is already open.
    let should_auto_open =
        first && missing_core && state.bootstrap_wizard.is_none() && state.active_modal().is_none();
    if let Some(report) = state.bootstrap.report.clone().filter(|_| should_auto_open) {
        state.bootstrap_wizard = Some(BootstrapWizard::from_report(report));
    }
    Outcome::redraw()
}

/// Open the bootstrap wizard from the cached report, requesting a preflight
/// first when none is cached yet (the wizard opens over an empty report showing
/// "running preflight…", then refreshes in place when the report lands).
fn open_bootstrap_wizard(state: &mut AppState) -> Outcome {
    if state.bootstrap_wizard.is_some() {
        return Outcome::idle();
    }
    let report = state
        .bootstrap
        .report
        .clone()
        .unwrap_or_else(|| frust_drive::doctor::DoctorReport { areas: Vec::new() });
    let need_preflight = state.bootstrap.report.is_none() && !state.bootstrap.refreshing;
    state.bootstrap_wizard = Some(BootstrapWizard::from_report(report));
    if need_preflight {
        state.bootstrap.refreshing = true;
        Outcome {
            redraw: true,
            effect: Some(Effect::RunBootstrapReport),
        }
    } else {
        Outcome::redraw()
    }
}

/// The clipboard text for a fix command: the runnable command line
/// (`program args…`) for an auto-runnable fix, else its doc link, else its
/// display string.
fn fix_copy_text(fix: &frust_drive::doctor::FixCommand) -> String {
    if fix.auto_runnable && !fix.program.is_empty() {
        let mut parts = vec![fix.program.clone()];
        parts.extend(fix.args.iter().cloned());
        parts.join(" ")
    } else if let Some(link) = &fix.doc_link {
        link.clone()
    } else {
        fix.display.clone()
    }
}

/// Route one supervisor event into the matching session's view-model.
///
/// Redraw policy honors the dirty-frame skip: a **line batch** for a
/// background (non-active) session is buffered without a repaint; a batch for
/// the *active* followed session, and *any* state change (the sidebar/tab
/// status glyph) or a new drop count, redraw. A **phase label**
/// (`crate::supervise::progress`, workbook §B10's transient status line)
/// mirrors the drop-count gating — dirty only on the active tab and only when
/// the label actually changed, never an unconditional redraw, so a
/// backgrounded session's build chatter doesn't force a repaint. An event for
/// an unknown id is dropped (the session must be registered first — see
/// [`Message::RegisterSession`]).
fn on_session_event(state: &mut AppState, ev: SessionEvent) -> Outcome {
    let Some(idx) = state.session_index(ev.id) else {
        return Outcome::idle();
    };
    let is_active = state.active_session == Some(idx);
    // A toast to raise once the session borrow ends (a terminal transition).
    let mut toast: Option<(ToastKind, String)> = None;
    // Deferred devtools work, likewise: a fresh discovery line to act on, or
    // a connection to tear down because the session itself ended.
    let mut discovered = false;
    let mut session_ended = false;
    // Same shape, for the System/Network metrics sampler: its Android
    // identity just resolved (pkg+pid both known), or the session ending
    // means a running sampler has to stop.
    let mut metrics_ready = false;
    let mut metrics_ended = false;
    let outcome = {
        let session = &mut state.sessions[idx];
        match ev.kind {
            SessionEventKind::Lines(lines) => {
                let following = session.is_following();
                for line in lines {
                    // Every line is scanned for a devtools discovery
                    // announcement (workbook §B12) and, independently, for
                    // the drive's own `Launching {pkg}…`/`Streaming logs
                    // (pid …)` lines the metrics sampler's Android identity
                    // resolves from.
                    metrics_ready |= session.devtools.metrics.ingest_line(&line);
                    discovered |= session.push_line(line);
                }
                // A discovery line changes what the DevTools surface shows
                // even when the log view itself is frozen off the tail.
                Outcome::dirty(is_active && (following || discovered))
            }
            SessionEventKind::State(s) => {
                session.state = s;
                if session.state.is_terminal() {
                    session_ended = session.devtools.on_session_end();
                    metrics_ended = session.devtools.metrics.on_session_end();
                }
                // Clear a stale phase label the moment the state leaves the
                // transient (`Building`/`Installing`) window — the sole
                // clearing point (`crate::supervise::progress`'s module
                // docs), independent of `Phase` event arrival order.
                if !super::state::is_transient(&session.state) {
                    session.current_phase = None;
                }
                toast = terminal_toast(session);
                Outcome::redraw()
            }
            SessionEventKind::Phase(p) => {
                let changed = session.current_phase.as_ref() != Some(&p);
                session.current_phase = Some(p);
                Outcome::dirty(is_active && changed)
            }
            SessionEventKind::Dropped(n) => {
                // The supervisor's bounded-channel overflow counter (cumulative,
                // drop-newest). Store it so the UI can flag a session whose log
                // is missing lines; a change is worth a repaint on the active
                // tab.
                let changed = session.dropped != n;
                session.dropped = n;
                Outcome::dirty(is_active && changed)
            }
        }
    };
    if let Some((kind, text)) = toast {
        state.toasts.push(kind, text);
    }
    // Connection retention (workbook §B12): once DevTools has been opened for
    // a session the connection is kept for the session's whole life — the
    // frame ring keeps filling behind a hidden tab, which is what makes
    // re-opening DevTools instant instead of a fresh handshake. It is closed
    // in exactly two places: the session ending (below) and an explicit
    // reconnect replacing it. The metrics sampler mirrors the same
    // retention policy (`crate::supervise::metrics_bridge`'s module doc):
    // once started it runs for the session's whole life too, stopping only
    // here. `session_ended`/`discovered` and `metrics_ended`/`metrics_ready`
    // are each set by exactly one of the match arms above (`State` vs.
    // `Lines`), so at most one pair is ever non-trivial per call.
    let mut effects = Vec::new();
    if session_ended {
        effects.push(Effect::DevtoolsDisconnect(ev.id));
    }
    if metrics_ended {
        effects.push(Effect::MetricsStop(ev.id));
    }
    if discovered {
        effects.extend(on_devtools_discovery(state, idx));
    }
    if metrics_ready {
        effects.extend(metrics_start(state, idx));
    }
    let effect = batch(effects);
    let mut outcome = Outcome {
        redraw: outcome.redraw || effect.is_some(),
        effect: outcome.effect.or(effect),
    };
    // A tab `close_tab` marked `close_on_exit` while it was still live
    // removes itself the instant its terminal state lands — after the
    // bookkeeping/effects above, so its devtools/metrics bridges still get
    // their disconnect first.
    if state.sessions[idx].state.is_terminal() && state.sessions[idx].close_on_exit {
        remove_session(state, idx);
        outcome.redraw = true;
    }
    outcome
}

/// Close the tab at `idx`. A session already in a terminal state is removed
/// immediately; a live one is marked `close_on_exit` and stopped exactly like
/// [`Message::StopSession`] — the removal itself happens once its terminal
/// [`SessionEvent`] lands in [`on_session_event`].
fn close_tab(state: &mut AppState, idx: usize) -> Outcome {
    let Some(session) = state.sessions.get(idx) else {
        return Outcome::idle();
    };
    if session.state.is_terminal() {
        remove_session(state, idx);
        Outcome::redraw()
    } else {
        let id = session.id;
        state.sessions[idx].close_on_exit = true;
        Outcome::effect(Effect::StopSession(id))
    }
}

/// Restart the session at `idx` — the one restart path shared by the active
/// tab (`R` with an app session active, and the palette's "Restart session"
/// row) and a watched session's settled save-burst
/// ([`Message::WatchTriggered`], via [`watch_triggered`]). The keyboard twin
/// of `crate::supervise::mcp_backend::restart_app`/DAP `frustRestart`; see
/// that fn's doc for the shared contract this mirrors (same duplicate guard,
/// excluding the session being restarted; stop; relaunch the retained spec)
/// so the two stay aligned. MCP additionally applies its record cap; this
/// path has none — a restart is a net-zero swap, and the keyboard's own `r`
/// launches are uncapped too.
///
/// An out-of-range `idx` idles. An ad-hoc session (no `SessionTarget`)
/// refuses with a toast — only an app launch has a spec worth relaunching.
/// Otherwise the one-live-session guard runs excluding the session being
/// restarted (`AppState::live_session_for_excluding`, exactly as
/// `restart_app` excludes it from its own duplicate check) and refuses with
/// the same toast wording [`drop_already_running`] uses on a hit. Once it
/// clears, the tab is marked for removal-once-terminal exactly as
/// [`close_tab`] does for a live session, except an already-terminal tab is
/// removed immediately *and* the effect still fires — a crashed session must
/// still relaunch, unlike closing a tab, which has nothing left to do once
/// the tab is gone. When the restarted tab was the active one, the
/// relaunch's tab is focused when it registers
/// (`AppState::focus_next_registered`); a watched background tab's relaunch
/// does not steal focus.
///
/// The replaced tab's `watch` flag is cleared: the runner stops its watcher
/// in the same enactment and re-enables watch on the *relaunch* instead
/// ([`Message::EnableWatch`]), so the dying tab never shows the indicator.
fn restart_session_at(state: &mut AppState, idx: usize) -> Outcome {
    let Some(session) = state.sessions.get(idx) else {
        return Outcome::idle();
    };
    let id = session.id;
    let Some(target) = session.target.clone() else {
        state.toasts.push(
            ToastKind::Warn,
            "only an app session can be restarted".to_string(),
        );
        return Outcome::idle();
    };
    let project_root = session.project_root.clone();

    if state
        .live_session_for_excluding(&project_root, &target, Some(id))
        .is_some()
    {
        state.toasts.push(
            ToastKind::Warn,
            format!(
                "{}: already running here — stop it first (x)",
                target.label()
            ),
        );
        return Outcome::idle();
    }

    let refocus = state.active_session == Some(idx);
    state.sessions[idx].watch = false;
    if state.sessions[idx].state.is_terminal() {
        remove_session(state, idx);
    } else {
        state.sessions[idx].close_on_exit = true;
    }
    if refocus {
        state.focus_next_registered = Some((project_root, target));
    }
    Outcome::effect(Effect::RestartSession(id))
}

/// The refusal toast for "Watch: restart on save" on anything but a desktop
/// app session — `frust run --watch`'s own reason (`frust-cli`'s `run` docs).
const WATCH_DESKTOP_ONLY: &str = "Watch is desktop-only: the watch loop has no device-side \
     kill/rebuild/relaunch story yet";

/// Flip "Watch: restart on save" on the active session
/// ([`Message::ToggleWatch`]). No active session idles; a session whose
/// target is not the desktop preview (a device, or an ad-hoc session with no
/// target) refuses with [`WATCH_DESKTOP_ONLY`] and no effect. Otherwise the
/// flag flips and the runner is told to start/stop the watcher.
fn toggle_watch(state: &mut AppState) -> Outcome {
    let Some(session) = state.active_session_mut() else {
        return Outcome::idle();
    };
    if session.target != Some(SessionTarget::Desktop) {
        state
            .toasts
            .push(ToastKind::Warn, WATCH_DESKTOP_ONLY.to_string());
        return Outcome::redraw();
    }
    session.watch = !session.watch;
    let (id, on) = (session.id, session.watch);
    let text = if on {
        "Watching src/ + Cargo.toml — a save restarts this session"
    } else {
        "Watch off"
    };
    state.toasts.push(ToastKind::Info, text.to_string());
    Outcome::effect(Effect::WatchSet { id, on })
}

/// A watched session's sources settled after a change
/// ([`Message::WatchTriggered`]): restart it through [`restart_session_at`]
/// — unless it is gone, has watch off, or already has a restart pending
/// (`close_on_exit` set by an earlier restart or close that has not landed
/// yet). That last guard is what keeps one save-burst to one relaunch: a
/// trigger already in flight when the first restart was requested finds the
/// replaced tab marked and does nothing.
fn watch_triggered(state: &mut AppState, session: SessionId) -> Outcome {
    let Some(idx) = state.session_index(session) else {
        return Outcome::idle();
    };
    let view = &state.sessions[idx];
    if !view.watch || view.close_on_exit {
        return Outcome::idle();
    }
    restart_session_at(state, idx)
}

/// Turn watch on for a freshly launched session the runner says should carry
/// it ([`Message::EnableWatch`] — a watched session's relaunch, or a
/// watch-checked desktop launch). Only a desktop app session takes it;
/// anything else, an unknown id, or a session already watching idles.
fn enable_watch(state: &mut AppState, session: SessionId) -> Outcome {
    let Some(idx) = state.session_index(session) else {
        return Outcome::idle();
    };
    let view = &mut state.sessions[idx];
    if view.watch || view.target != Some(SessionTarget::Desktop) {
        return Outcome::idle();
    }
    view.watch = true;
    Outcome::effect(Effect::WatchSet {
        id: session,
        on: true,
    })
}

/// Remove `sessions[idx]`, repairing `active_session` and closing a context
/// menu that targeted the removed tab.
///
/// Index-repair rule: if the removed tab was active, the tab that slides into
/// its slot becomes active (the "next tab" — same index, now the following
/// session) when one exists, else the previous tab, else no tab at all. If
/// the removed tab was *before* the active one, the active index shifts down
/// by one to keep pointing at the same session; if it was after, the active
/// index is untouched.
fn remove_session(state: &mut AppState, idx: usize) {
    state.sessions.remove(idx);
    state.active_session = match state.active_session {
        Some(active) if active == idx => {
            if idx < state.sessions.len() {
                Some(idx)
            } else if idx > 0 {
                Some(idx - 1)
            } else {
                None
            }
        }
        Some(active) if active > idx => Some(active - 1),
        other => other,
    };
    // Any open context menu is invalidated by a session removal: a positional
    // tab index shifts or becomes stale, and a LogView menu was built against
    // the then-active session, which may have changed. Closing is the safe
    // repair — the user reopens on the context they meant.
    state.context_menu = None;
}

/// A fresh devtools discovery line landed for the session at `idx`. While
/// DevTools is open for that session, (re)open the connection: a
/// re-announcement means the app restarted, so whatever the bridge is holding
/// points at a dead port and must be replaced rather than kept.
fn on_devtools_discovery(state: &mut AppState, idx: usize) -> Option<Effect> {
    if !state.sessions.get(idx)?.devtools.open {
        return None;
    }
    state.sessions[idx].devtools.conn = ConnState::Idle;
    devtools_connect(state, idx)
}

/// Start a devtools connection for the session at `idx` when one is warranted
/// — its build can host the service, a discovery line has landed, and nothing
/// is already connected or in flight. Marks the state `Connecting` and hands
/// the runner the target to reach; returns `None` (and changes nothing) when
/// a connect isn't warranted.
fn devtools_connect(state: &mut AppState, idx: usize) -> Option<Effect> {
    let session = state.sessions.get_mut(idx)?;
    if !session.devtools.wants_connect() {
        return None;
    }
    let discovery = session.devtools.discovered.clone()?;
    session.devtools.begin_connect();
    Some(Effect::DevtoolsConnect(DevtoolsTarget {
        session: session.id,
        port: discovery.port,
        token: discovery.token,
        android_serial: session.devtools.launch.android_serial.clone(),
    }))
}

/// Start the System/Network metrics sampler for the session at `idx` when
/// warranted: DevTools is open for it, its Android identity has resolved,
/// and nothing is already running (see [`super::MetricsState::wants_start`]).
/// Fires on DevTools open (regardless of which tab is showing — cheap, and
/// it means switching to System/Network later never pays a cold start) and,
/// symmetrically, the moment a still-open session's identity resolves from
/// its own log lines (see `on_session_event`'s `Lines` arm). Marks the
/// state `On` and hands the runner the target to reach; `None` (no state
/// change) when a start isn't warranted right now.
fn metrics_start(state: &mut AppState, idx: usize) -> Option<Effect> {
    let session = state.sessions.get_mut(idx)?;
    if !session.devtools.open || !session.devtools.metrics.wants_start() {
        return None;
    }
    let (serial, pid, pkg) = session.devtools.metrics.target()?;
    session.devtools.metrics.begin_sampling();
    Some(Effect::MetricsStart(MetricsTarget {
        session: session.id,
        serial,
        pid,
        pkg,
    }))
}

/// Fold a transition's independent effects into `Outcome::effect`'s single
/// slot: zero effects is `None`, exactly one collapses to that effect
/// directly (so every existing single-effect call site/assertion is
/// unaffected), and two or more become one [`Effect::Batch`] the runner
/// unpacks in order.
fn batch(mut effects: Vec<Effect>) -> Option<Effect> {
    match effects.len() {
        0 => None,
        1 => effects.pop(),
        _ => Some(Effect::Batch(effects)),
    }
}

/// The toast (if any) for a session that just reached a terminal state: a
/// success toast on a clean exit (noting built artifacts, when present), an
/// error toast on a failed exit or a kill. A still-live session raises none.
fn terminal_toast(session: &SessionView) -> Option<(ToastKind, String)> {
    use crate::supervise::SessionState;
    let label = session.target_label.clone();
    match session.state {
        SessionState::Exited(true) => {
            let arts = session.built_artifact_paths().len();
            let text = if arts > 0 {
                format!("{label}: built {arts} artifact(s) · c copies path")
            } else {
                format!("{label}: finished")
            };
            Some((ToastKind::Success, text))
        }
        SessionState::Exited(false) => Some((ToastKind::Error, format!("{label}: failed"))),
        SessionState::Killed => Some((ToastKind::Warn, format!("{label}: stopped"))),
        _ => None,
    }
}

/// Move the active tab by `delta` (wrapping), redrawing on a real change.
fn cycle_tab(state: &mut AppState, delta: isize) -> Outcome {
    let n = state.sessions.len();
    if n == 0 {
        return Outcome::idle();
    }
    let cur = state.active_session.unwrap_or(0) as isize;
    let next = (cur + delta).rem_euclid(n as isize) as usize;
    if state.active_session == Some(next) {
        Outcome::idle()
    } else {
        state.active_session = Some(next);
        Outcome::redraw()
    }
}

/// The Performance tab's current scrub-able window — whichever source
/// [`crate::engine::perf_window`] picks for `session`, capped at
/// [`crate::engine::PERF_WINDOW`]. Shared by the scrub/select handlers above
/// so a pin is always resolved against the data actually on screen, not a
/// stale figure.
fn devtools_perf_window(session: &SessionView) -> Vec<PerfFrame> {
    let (_, window) = crate::engine::perf_window(
        &session.devtools.conn,
        &session.devtools.frames,
        &session.perf,
    );
    window
}

/// Drop a pinned Performance frame that has aged out of the drawn window
/// (see [`crate::engine::PerformanceTab::retain_in_window`]). Cheap in the
/// common case: an un-pinned tab never builds the window at all.
fn devtools_perf_retain_selection(session: &mut SessionView) -> bool {
    if !session.devtools.performance.has_selection() {
        return false;
    }
    let window = devtools_perf_window(session);
    session.devtools.performance.retain_in_window(&window)
}

/// [`with_active`] for the DevTools tab-selection arms: apply `f` to the
/// active session's [`crate::engine::DevtoolsState`], then let the newly
/// selected tab claim its own first fetch (only the Inspector has one).
fn with_active_devtools_tab(
    state: &mut AppState,
    f: impl FnOnce(&mut crate::engine::DevtoolsState),
) -> Outcome {
    let Some(session) = state.active_session_mut() else {
        return Outcome::idle();
    };
    f(&mut session.devtools);
    Outcome {
        redraw: true,
        effect: devtools_inspector_enter(session),
    }
}

/// Apply `f` to the active session's Inspector tab and pair it with §B12's
/// per-selection `widget_props` pull: any transition that can move the
/// selection (a move, a row click, an expand/collapse that re-anchors it)
/// requests the newly-selected node's props unless they are already cached or
/// in flight. `f` reports whether the visible state changed.
fn with_inspector(state: &mut AppState, f: impl FnOnce(&mut InspectorTab) -> bool) -> Outcome {
    let Some(session) = state.active_session_mut() else {
        return Outcome::idle();
    };
    let changed = f(&mut session.devtools.inspector);
    let effect = devtools_props_fetch(session);
    Outcome {
        redraw: changed || effect.is_some(),
        effect,
    }
}

/// The `widget_props` pull the current Inspector selection needs, if any —
/// gated on a live connection, since a request has nowhere to go otherwise.
fn devtools_props_fetch(session: &mut SessionView) -> Option<Effect> {
    if session.devtools.phase() != DevtoolsPhase::Connected {
        return None;
    }
    let id = session.devtools.inspector.begin_props_fetch()?;
    Some(Effect::DevtoolsFetchProps {
        session: session.id,
        id,
    })
}

/// A deliberate *entry* into the Inspector tab (`3`, `[`/`]`, a tab-pill
/// click, or re-opening DevTools onto it): re-arm the once-only automatic
/// pull, then take it. Re-arming is what lets an entry retry after an earlier
/// automatic pull failed — the entry is a user action, bounded by input rate,
/// unlike the message-driven check in the `DevtoolsConn` arm. Re-arming a tab
/// that already has a snapshot changes nothing
/// ([`InspectorTab::wants_tree`] stays false on `loaded`).
fn devtools_inspector_enter(session: &mut SessionView) -> Option<Effect> {
    if session.devtools.active_tab == DevtoolsTab::Inspector {
        session.devtools.inspector.rearm_auto_pull();
    }
    devtools_inspector_entry(session)
}

/// The automatic first `widget_tree` pull: an Inspector tab with no snapshot
/// yet, a live connection, and no automatic attempt already spent fires
/// exactly one ([`InspectorTab::wants_tree`]'s latch). A *failed* pull does
/// not re-open that latch — only [`devtools_inspector_enter`] or the explicit
/// `r` refresh re-triggers one — so a persistently failing pull cannot storm
/// the bridge from the message path that calls this.
fn devtools_inspector_entry(session: &mut SessionView) -> Option<Effect> {
    if session.devtools.active_tab != DevtoolsTab::Inspector
        || session.devtools.phase() != DevtoolsPhase::Connected
        || !session.devtools.inspector.wants_tree()
    {
        return None;
    }
    session.devtools.inspector.begin_tree_fetch();
    Some(Effect::DevtoolsFetchTree {
        session: session.id,
    })
}

fn with_active(state: &mut AppState, f: impl FnOnce(&mut SessionView)) -> Outcome {
    match state.active_session_mut() {
        Some(s) => {
            f(s);
            Outcome::redraw()
        }
        None => Outcome::idle(),
    }
}

/// [`with_active`] for the log-scroll arms, which also need the committed
/// free-text search filter: it is *global* state (`AppState::search`), not
/// per-session, so the session's own visible-sequence math
/// ([`SessionView::visible_indices`]) can only see it when routing passes it
/// in. Cloned (rather than borrowed) because the session borrow is mutable —
/// one short query string per scroll event.
fn with_active_filtered(
    state: &mut AppState,
    f: impl FnOnce(&mut SessionView, Option<&str>),
) -> Outcome {
    let filter = state.search.filter.clone();
    match state.active_session_mut() {
        Some(s) => {
            f(s, filter.as_deref());
            Outcome::redraw()
        }
        None => Outcome::idle(),
    }
}

/// [`with_active_filtered`] for the transitions that report whether they
/// actually changed anything (the line-selection-mode moves, each of which is
/// a no-op at a clamp or outside the mode) — a no-op costs no redraw.
fn with_active_changed(
    state: &mut AppState,
    f: impl FnOnce(&mut SessionView, Option<&str>) -> bool,
) -> Outcome {
    let filter = state.search.filter.clone();
    match state.active_session_mut() {
        Some(s) => Outcome::dirty(f(s, filter.as_deref())),
        None => Outcome::idle(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::devtools::{
        ConnEvent, DevtoolsLaunch, DevtoolsPhase, DevtoolsTab, InspectorEvent, PerfFocus,
    };
    use crate::engine::message::RegionId;
    use crate::engine::session_view::Scroll;
    use crate::engine::state::Screen;
    use crate::supervise::SessionState;
    use std::path::PathBuf;

    fn welcome() -> AppState {
        AppState::default()
    }

    #[test]
    fn quit_sets_flag_without_redraw() {
        let mut s = welcome();
        let out = update(&mut s, Message::Quit);
        assert!(s.should_quit);
        assert!(!out.redraw, "a quit does not need a paint");
    }

    #[test]
    fn tick_is_a_dirty_frame_skip_when_idle() {
        let mut s = welcome();
        let out = update(&mut s, Message::Tick);
        assert!(!out.redraw, "no animation → tick skips the draw");
    }

    #[test]
    fn tick_always_advances_animation_frame() {
        let mut s = welcome();
        assert_eq!(s.animation_frame, 0);
        update(&mut s, Message::Tick);
        assert_eq!(s.animation_frame, 1);
        update(&mut s, Message::Tick);
        assert_eq!(s.animation_frame, 2);
    }

    #[test]
    fn tick_redraws_while_a_session_is_transient() {
        let mut s = welcome();
        let id = register(&mut s, 1, "/tmp/proj", "desktop");
        assert_eq!(
            s.session_index(id).map(|i| &s.sessions[i].state),
            Some(&SessionState::Configuring)
        );
        let out = update(&mut s, Message::Tick);
        assert!(
            out.redraw,
            "a transient session gives the tick spinner something to paint"
        );
    }

    #[test]
    fn tick_skips_redraw_once_session_is_running() {
        let mut s = welcome();
        let id = register(&mut s, 1, "/tmp/proj", "desktop");
        let idx = s.session_index(id).unwrap();
        s.sessions[idx].state = SessionState::Running;
        let out = update(&mut s, Message::Tick);
        assert!(
            !out.redraw,
            "a stably-streaming session is not transient → dirty-frame skip"
        );
    }

    #[test]
    fn hover_change_redraws_once_then_dedups() {
        let mut s = welcome();
        assert_eq!(s.hover, None);
        let first = update(&mut s, Message::HoverChanged(Some(RegionId::CreateButton)));
        assert!(first.redraw);
        assert_eq!(s.hover, Some(RegionId::CreateButton));
        let again = update(&mut s, Message::HoverChanged(Some(RegionId::CreateButton)));
        assert!(!again.redraw, "unchanged hover is a dirty-frame skip");
        let cleared = update(&mut s, Message::HoverChanged(None));
        assert!(cleared.redraw);
        assert_eq!(s.hover, None);
    }

    #[test]
    fn press_then_activate_opens_the_wizard_and_clears_pressed() {
        let mut s = welcome();
        assert!(update(&mut s, Message::CreatePressed).redraw);
        assert!(s.create_pressed);
        assert!(!update(&mut s, Message::CreatePressed).redraw);
        let out = update(&mut s, Message::CreateActivate);
        assert!(out.redraw);
        assert!(!s.create_pressed);
        // Activating the Create button opens the wizard and primes the probe.
        assert!(s.create_wizard.is_some());
        assert_eq!(out.effect, Some(Effect::ProbeCleanSignals));
    }

    #[test]
    fn cancel_clears_pressed_only_when_set() {
        let mut s = welcome();
        assert!(!update(&mut s, Message::CreateCancel).redraw);
        s.create_pressed = true;
        assert!(update(&mut s, Message::CreateCancel).redraw);
        assert!(!s.create_pressed);
    }

    #[test]
    fn detect_picks_screen_from_marker() {
        let s = AppState::default();
        assert_eq!(s.screen, Screen::Welcome);
    }

    // ── Session wiring ──────────────────────────────────────────────────────

    fn register(state: &mut AppState, id: u64, project: &str, label: &str) -> SessionId {
        register_with(
            state,
            id,
            project,
            label,
            DevtoolsLaunch::unavailable(),
            None,
        )
    }

    /// [`register`] for a session that occupies a run target — what the
    /// one-live-session guard reads.
    fn register_on(
        state: &mut AppState,
        id: u64,
        project: &str,
        target: SessionTarget,
    ) -> SessionId {
        let label = target.label();
        register_with(
            state,
            id,
            project,
            &label,
            DevtoolsLaunch::unavailable(),
            Some(target),
        )
    }

    /// [`register`] for a real app session — `launch` decides whether it
    /// could host a devtools service (workbook §B12), and `target` is where
    /// it runs (`None` for an ad-hoc build/clean session).
    fn register_with(
        state: &mut AppState,
        id: u64,
        project: &str,
        label: &str,
        launch: DevtoolsLaunch,
        target: Option<SessionTarget>,
    ) -> SessionId {
        let id = SessionId(id);
        update(
            state,
            Message::RegisterSession {
                id,
                project_root: PathBuf::from(project),
                target_label: label.to_string(),
                devtools: launch,
                target,
            },
        );
        id
    }

    fn line(id: SessionId, s: &str) -> Message {
        Message::Session(SessionEvent {
            id,
            kind: SessionEventKind::Lines(vec![s.to_string()]),
        })
    }

    #[test]
    fn register_adds_session_and_auto_selects_first() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/huddle", "desktop");
        assert_eq!(st.sessions.len(), 1);
        assert_eq!(st.active_session, Some(0));
        // A duplicate register is a no-op.
        let out = update(
            &mut st,
            Message::RegisterSession {
                id: a,
                project_root: PathBuf::from("/tmp/huddle"),
                target_label: "desktop".into(),
                devtools: DevtoolsLaunch::unavailable(),
                target: None,
            },
        );
        assert!(!out.redraw);
        assert_eq!(st.sessions.len(), 1);
    }

    #[test]
    fn active_session_line_redraws_background_line_does_not() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        let b = register(&mut st, 1, "/tmp/b", "desktop");
        // `a` is active (auto-selected first).
        assert_eq!(st.active_session, Some(0));
        // A line for the active, following session repaints.
        assert!(update(&mut st, line(a, "hello")).redraw);
        // A line for the background session is buffered but skips the draw.
        assert!(!update(&mut st, line(b, "quiet")).redraw);
        // Yet it was retained.
        let bi = st.session_index(b).unwrap();
        assert_eq!(st.sessions[bi].log.len(), 1);
    }

    #[test]
    fn a_state_change_always_redraws_the_glyph() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        let out = update(
            &mut st,
            Message::Session(SessionEvent {
                id: a,
                kind: SessionEventKind::State(SessionState::Running),
            }),
        );
        assert!(out.redraw);
        assert_eq!(st.sessions[0].state, SessionState::Running);
    }

    // ── Build-phase progress (workbook §B10) ────────────────────────────────

    fn phase_event(id: SessionId, phase: crate::supervise::PhaseLabel) -> Message {
        Message::Session(SessionEvent {
            id,
            kind: SessionEventKind::Phase(phase),
        })
    }

    fn compiling(crate_name: &str) -> crate::supervise::PhaseLabel {
        crate::supervise::PhaseLabel::Compiling {
            crate_name: crate_name.to_string(),
            progress: None,
        }
    }

    #[test]
    fn a_phase_event_on_the_active_session_redraws_and_stores_the_label() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        st.sessions[0].state = SessionState::Building;
        let out = update(&mut st, phase_event(a, compiling("frust-core")));
        assert!(out.redraw);
        assert_eq!(
            st.active_session().unwrap().current_phase,
            Some(compiling("frust-core"))
        );
    }

    /// The redraw-gating half of criterion 4: a phase update for a
    /// *background* session neither redraws nor is dropped — it's stored, so
    /// switching to that tab later shows the latest label, but the frame
    /// stays clean while it's not on screen (mirrors
    /// `active_session_line_redraws_background_line_does_not`).
    #[test]
    fn a_phase_event_on_a_background_session_is_buffered_without_a_redraw() {
        let mut st = welcome();
        register(&mut st, 0, "/tmp/a", "desktop");
        let b = register(&mut st, 1, "/tmp/b", "desktop");
        st.sessions[1].state = SessionState::Building;
        assert_eq!(st.active_session, Some(0));
        let out = update(&mut st, phase_event(b, compiling("frust-widgets")));
        assert!(
            !out.redraw,
            "a background session's phase update must not force a redraw"
        );
        assert_eq!(
            st.sessions[1].current_phase,
            Some(compiling("frust-widgets")),
            "the label is still retained for when that tab becomes active"
        );
    }

    /// A repeated identical phase (the same crate re-reported, or a
    /// duplicate delivery) never re-dirties the active tab — only an actual
    /// change does, the same "changed, not merely present" gate `Dropped`
    /// uses.
    #[test]
    fn an_unchanged_phase_on_the_active_session_does_not_redraw() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        st.sessions[0].state = SessionState::Building;
        assert!(update(&mut st, phase_event(a, compiling("frust-core"))).redraw);
        let out = update(&mut st, phase_event(a, compiling("frust-core")));
        assert!(
            !out.redraw,
            "an identical phase must not re-dirty the frame"
        );
    }

    /// The clearing invariant: a `State` event that leaves the transient
    /// window wipes any phase label the session was carrying, regardless of
    /// whether a matching `Phase(None)`-shaped clear was ever sent (there is
    /// no such variant — `State` is the sole clearing point, see
    /// `crate::supervise::progress`'s module docs).
    #[test]
    fn a_state_transition_to_running_clears_a_stale_phase_label() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        st.sessions[0].state = SessionState::Building;
        update(&mut st, phase_event(a, compiling("frust-core")));
        assert!(st.active_session().unwrap().current_phase.is_some());

        update(
            &mut st,
            Message::Session(SessionEvent {
                id: a,
                kind: SessionEventKind::State(SessionState::Running),
            }),
        );
        assert_eq!(
            st.active_session().unwrap().current_phase,
            None,
            "reaching Running must clear any phase label the tab was showing"
        );
    }

    /// Same clearing invariant on the failure path: a `Building` session's
    /// phase label is wiped the moment the state reports `Exited(false)`
    /// (workbook §B10's failed row — spinner stopped, no lingering shimmer
    /// text from the build that just failed).
    #[test]
    fn a_failed_build_clears_its_phase_label_too() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        st.sessions[0].state = SessionState::Building;
        update(&mut st, phase_event(a, compiling("frust-core")));

        update(
            &mut st,
            Message::Session(SessionEvent {
                id: a,
                kind: SessionEventKind::State(SessionState::Exited(false)),
            }),
        );
        assert_eq!(st.active_session().unwrap().current_phase, None);
        assert_eq!(
            st.active_session().unwrap().state,
            SessionState::Exited(false)
        );
    }

    #[test]
    fn tab_cycling_wraps_and_select_jumps() {
        let mut st = welcome();
        register(&mut st, 0, "/tmp/a", "desktop");
        register(&mut st, 1, "/tmp/b", "desktop");
        register(&mut st, 2, "/tmp/c", "desktop");
        assert_eq!(st.active_session, Some(0));
        assert!(update(&mut st, Message::NextTab).redraw);
        assert_eq!(st.active_session, Some(1));
        update(&mut st, Message::NextTab);
        update(&mut st, Message::NextTab); // wraps 2 -> 0
        assert_eq!(st.active_session, Some(0));
        assert!(update(&mut st, Message::PrevTab).redraw); // wraps 0 -> 2
        assert_eq!(st.active_session, Some(2));
        assert!(update(&mut st, Message::SelectTab(1)).redraw);
        assert_eq!(st.active_session, Some(1));
        // Out-of-range select is a no-op.
        assert!(!update(&mut st, Message::SelectTab(9)).redraw);
    }

    #[test]
    fn stop_routes_the_active_session_id_as_an_effect() {
        let mut st = welcome();
        let a = register(&mut st, 7, "/tmp/a", "desktop");
        let out = update(&mut st, Message::StopSession);
        assert_eq!(out.effect, Some(Effect::StopSession(a)));
        assert!(!out.redraw, "the Killed event will redraw, not the request");
        // With no session, stop is a no-op.
        let mut empty = welcome();
        assert_eq!(update(&mut empty, Message::StopSession).effect, None);
    }

    #[test]
    fn close_tab_on_a_terminal_session_removes_it_and_repairs_the_active_index() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        let b = register(&mut st, 1, "/tmp/b", "desktop");
        let c = register(&mut st, 2, "/tmp/c", "desktop");
        for id in [a, b, c] {
            update(&mut st, state_event(id, SessionState::Exited(true)));
        }

        // Closing the active tab (first) activates the tab that slides into
        // its slot — the next tab.
        st.active_session = Some(0);
        let out = update(&mut st, Message::CloseTab(0));
        assert!(out.redraw);
        assert_eq!(st.sessions.len(), 2);
        assert_eq!(st.sessions[0].id, b);
        assert_eq!(st.active_session, Some(0), "b slides into slot 0");

        // Closing the active (now last) tab with no next tab falls back to
        // the previous one.
        st.active_session = Some(1);
        update(&mut st, Message::CloseTab(1));
        assert_eq!(st.sessions.len(), 1);
        assert_eq!(st.sessions[0].id, b);
        assert_eq!(st.active_session, Some(0));

        // Closing the only remaining tab leaves no active tab, and the
        // workbench falls back to its no-session path.
        update(&mut st, Message::CloseTab(0));
        assert!(st.sessions.is_empty());
        assert_eq!(st.active_session, None);
    }

    #[test]
    fn close_tab_on_a_non_active_terminal_session_only_shifts_a_left_neighbor() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        let b = register(&mut st, 1, "/tmp/b", "desktop");
        let c = register(&mut st, 2, "/tmp/c", "desktop");
        for id in [a, b, c] {
            update(&mut st, state_event(id, SessionState::Exited(true)));
        }
        st.active_session = Some(1); // b is active

        // Closing a tab to the *right* of the active one leaves the active
        // index untouched.
        update(&mut st, Message::CloseTab(2));
        assert_eq!(st.active_session, Some(1));
        assert_eq!(st.sessions[st.active_session.unwrap()].id, b);

        // Closing a tab to the *left* of the active one shifts the active
        // index down by one, still pointing at the same session.
        update(&mut st, Message::CloseTab(0));
        assert_eq!(st.active_session, Some(0));
        assert_eq!(st.sessions[st.active_session.unwrap()].id, b);
    }

    #[test]
    fn close_tab_on_a_live_session_stops_it_and_removes_it_once_terminal() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        st.sessions[0].state = SessionState::Running;

        let out = update(&mut st, Message::CloseTab(0));
        assert_eq!(out.effect, Some(Effect::StopSession(a)));
        assert!(
            !out.redraw,
            "the removal happens once the terminal event lands, not here"
        );
        assert_eq!(st.sessions.len(), 1, "a live session is not removed yet");
        assert!(st.sessions[0].close_on_exit);

        // The terminal event now removes it exactly once.
        let out = update(&mut st, state_event(a, SessionState::Killed));
        assert!(out.redraw);
        assert!(st.sessions.is_empty());
        assert_eq!(st.active_session, None);

        // A terminal event for the id it just removed is ignored — no panic,
        // no second removal.
        let out = update(&mut st, state_event(a, SessionState::Killed));
        assert!(!out.redraw);
        assert!(st.sessions.is_empty());
    }

    #[test]
    fn restart_routes_the_active_session_id_and_marks_the_tab_for_removal_once_terminal() {
        let mut st = workbench_with_project();
        let a = register_on(&mut st, 0, "/tmp/huddle", SessionTarget::Desktop);
        st.sessions[0].state = SessionState::Running;

        let out = update(&mut st, Message::RestartSession);
        assert_eq!(out.effect, Some(Effect::RestartSession(a)));
        assert_eq!(st.sessions.len(), 1, "the old tab is not removed yet");
        assert!(st.sessions[0].close_on_exit);

        // The terminal event now removes the old tab exactly once — the
        // restart's relaunch is a brand new session, registered separately.
        let out = update(&mut st, state_event(a, SessionState::Killed));
        assert!(out.redraw);
        assert!(st.sessions.is_empty());

        // With no active session, restart is a no-op.
        let mut empty = welcome();
        assert_eq!(update(&mut empty, Message::RestartSession).effect, None);
    }

    #[test]
    fn restart_of_an_already_terminal_tab_removes_it_immediately_and_still_emits_the_effect() {
        let mut st = workbench_with_project();
        let a = register_on(&mut st, 0, "/tmp/huddle", SessionTarget::Desktop);
        update(&mut st, state_event(a, SessionState::Exited(false)));

        let out = update(&mut st, Message::RestartSession);
        assert_eq!(
            out.effect,
            Some(Effect::RestartSession(a)),
            "a crashed session must still relaunch"
        );
        assert!(
            st.sessions.is_empty(),
            "an already-terminal tab is removed on the spot, like close_tab"
        );
    }

    #[test]
    fn restart_with_another_live_session_on_the_same_target_is_refused_with_a_toast() {
        let mut st = workbench_with_project();
        let a = register_on(&mut st, 0, "/tmp/huddle", SessionTarget::Desktop);
        // A second session on the *other* device does not block the first —
        // only an exact (project, target) hit does. Simulate the collision by
        // registering a second desktop session for the same project directly
        // (bypassing the launch guard, exactly like `register_on` does for
        // every other guard test in this module).
        register_on(&mut st, 1, "/tmp/huddle", SessionTarget::Desktop);
        st.active_session = Some(st.session_index(a).unwrap());

        let out = update(&mut st, Message::RestartSession);
        assert_eq!(out.effect, None);
        assert_eq!(
            warn_texts(&st),
            vec!["desktop: already running here — stop it first (x)"]
        );
    }

    #[test]
    fn restart_of_a_targetless_session_is_refused() {
        let mut st = welcome();
        register(&mut st, 0, "/tmp/huddle", "build apk");

        let out = update(&mut st, Message::RestartSession);
        assert_eq!(out.effect, None);
        assert_eq!(
            warn_texts(&st),
            vec!["only an app session can be restarted"]
        );
    }

    #[test]
    fn a_restart_relaunch_becomes_the_active_tab_and_stays_active_once_the_old_tab_goes() {
        let mut st = workbench_with_project();
        let a = register(&mut st, 0, "/tmp/huddle", "build apk");
        let old = register_on(&mut st, 1, "/tmp/huddle", SessionTarget::Desktop);
        let b = register(&mut st, 2, "/tmp/huddle", "clean");
        st.sessions[1].state = SessionState::Running;
        st.active_session = Some(st.session_index(old).unwrap());

        let out = update(&mut st, Message::RestartSession);
        assert_eq!(out.effect, Some(Effect::RestartSession(old)));

        let new = register_on(&mut st, 3, "/tmp/huddle", SessionTarget::Desktop);
        assert_eq!(
            st.active_session().map(|s| s.id),
            Some(new),
            "the relaunch's tab takes focus when it registers"
        );
        assert_eq!(
            st.focus_next_registered, None,
            "the pending focus is consumed"
        );

        // The replaced tab's terminal event removes it; `new` stays active.
        update(&mut st, state_event(old, SessionState::Killed));
        assert_eq!(
            st.sessions.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![a, b, new]
        );
        assert_eq!(st.active_session().map(|s| s.id), Some(new));
    }

    #[test]
    fn a_pending_restart_focus_only_applies_to_the_very_next_matching_registration() {
        let mut st = workbench_with_project();
        let old = register_on(&mut st, 0, "/tmp/huddle", SessionTarget::Desktop);
        st.sessions[0].state = SessionState::Running;
        update(&mut st, Message::RestartSession);

        // The relaunch never registered (e.g. its launch failed); an
        // unrelated registration arrives next. It is not focused, and it
        // consumes the pending focus.
        let unrelated = register(&mut st, 7, "/tmp/huddle", "build apk");
        assert_eq!(st.active_session().map(|s| s.id), Some(old));
        assert_ne!(st.active_session().map(|s| s.id), Some(unrelated));
        assert_eq!(st.focus_next_registered, None);

        // A later desktop launch is not focused by the stale restart.
        update(&mut st, state_event(old, SessionState::Killed));
        let first = register(&mut st, 8, "/tmp/huddle", "clean");
        st.active_session = Some(st.session_index(first).unwrap());
        register_on(&mut st, 9, "/tmp/huddle", SessionTarget::Desktop);
        assert_eq!(st.active_session().map(|s| s.id), Some(first));
    }

    // ── Watch: restart on save ──────────────────────────────────────────────

    /// Every effect `out` carries, flattening one level of `Effect::Batch`.
    fn effects_of(out: &Outcome) -> Vec<Effect> {
        match &out.effect {
            Some(Effect::Batch(effects)) => effects.clone(),
            Some(effect) => vec![effect.clone()],
            None => vec![],
        }
    }

    #[test]
    fn toggle_watch_on_a_device_session_refuses_with_the_cli_wording() {
        let mut st = workbench_with_project();
        register_on(&mut st, 0, "/tmp/huddle", pixel_7_target());

        let out = update(&mut st, Message::ToggleWatch);
        assert_eq!(out.effect, None);
        assert!(!st.sessions[0].watch);
        assert_eq!(warn_texts(&st), vec![WATCH_DESKTOP_ONLY]);
        assert!(
            WATCH_DESKTOP_ONLY
                .contains("the watch loop has no device-side kill/rebuild/relaunch story yet"),
            "reuses `frust run --watch`'s wording"
        );

        // An ad-hoc (targetless) session refuses the same way.
        let mut st = welcome();
        register(&mut st, 0, "/tmp/huddle", "build apk");
        assert_eq!(update(&mut st, Message::ToggleWatch).effect, None);
        assert!(!st.sessions[0].watch);

        // No session at all: nothing to do, nothing said.
        let mut empty = welcome();
        let out = update(&mut empty, Message::ToggleWatch);
        assert_eq!(out.effect, None);
        assert!(warn_texts(&empty).is_empty());
    }

    #[test]
    fn toggle_watch_on_a_desktop_session_flips_the_flag_and_emits_watch_set() {
        let mut st = workbench_with_project();
        let a = register_on(&mut st, 0, "/tmp/huddle", SessionTarget::Desktop);

        let out = update(&mut st, Message::ToggleWatch);
        assert_eq!(out.effect, Some(Effect::WatchSet { id: a, on: true }));
        assert!(st.sessions[0].watch);

        let out = update(&mut st, Message::ToggleWatch);
        assert_eq!(out.effect, Some(Effect::WatchSet { id: a, on: false }));
        assert!(!st.sessions[0].watch);
    }

    #[test]
    fn watch_triggered_for_a_session_with_watch_off_is_a_no_op() {
        let mut st = workbench_with_project();
        let a = register_on(&mut st, 0, "/tmp/huddle", SessionTarget::Desktop);
        st.sessions[0].state = SessionState::Running;

        let out = update(&mut st, Message::WatchTriggered { session: a });
        assert_eq!(out.effect, None);
        assert!(!st.sessions[0].close_on_exit);

        // An unknown session id is ignored too.
        let out = update(
            &mut st,
            Message::WatchTriggered {
                session: SessionId(99),
            },
        );
        assert_eq!(out.effect, None);
    }

    #[test]
    fn watch_triggered_restarts_that_session_once_per_burst() {
        let mut st = workbench_with_project();
        let a = register_on(&mut st, 0, "/tmp/huddle", SessionTarget::Desktop);
        st.sessions[0].state = SessionState::Running;
        update(&mut st, Message::ToggleWatch);

        let out = update(&mut st, Message::WatchTriggered { session: a });
        assert_eq!(out.effect, Some(Effect::RestartSession(a)));
        assert!(
            st.sessions[0].close_on_exit,
            "the replaced tab is removed once terminal"
        );
        assert!(
            !st.sessions[0].watch,
            "the dying tab hands the flag to its relaunch"
        );

        // A second trigger already in flight for the same burst finds the
        // restart pending and does nothing — one relaunch, never two.
        let out = update(&mut st, Message::WatchTriggered { session: a });
        assert_eq!(out.effect, None);

        // The terminal event removes the replaced tab exactly once.
        update(&mut st, state_event(a, SessionState::Killed));
        assert!(st.sessions.is_empty());
    }

    #[test]
    fn a_pending_close_blocks_a_watch_restart_even_with_the_flag_still_on() {
        let mut st = workbench_with_project();
        let a = register_on(&mut st, 0, "/tmp/huddle", SessionTarget::Desktop);
        st.sessions[0].state = SessionState::Running;
        st.sessions[0].watch = true;
        st.sessions[0].close_on_exit = true;

        let out = update(&mut st, Message::WatchTriggered { session: a });
        assert_eq!(out.effect, None);
    }

    #[test]
    fn a_crashed_watched_session_is_relaunched_by_the_next_save() {
        let mut st = workbench_with_project();
        let a = register_on(&mut st, 0, "/tmp/huddle", SessionTarget::Desktop);
        update(&mut st, Message::ToggleWatch);
        // A compile error: the session exits non-zero; watch stays on.
        update(&mut st, state_event(a, SessionState::Exited(false)));
        assert!(st.sessions[0].watch);

        let out = update(&mut st, Message::WatchTriggered { session: a });
        assert_eq!(out.effect, Some(Effect::RestartSession(a)));
        assert!(st.sessions.is_empty(), "the terminal tab goes at once");
    }

    #[test]
    fn a_watch_restart_of_a_background_tab_does_not_steal_focus() {
        let mut st = workbench_with_project();
        let watched = register_on(&mut st, 0, "/tmp/huddle", SessionTarget::Desktop);
        let other = register(&mut st, 1, "/tmp/huddle", "build apk");
        st.sessions[0].state = SessionState::Running;
        update(&mut st, Message::ToggleWatch);
        st.active_session = Some(st.session_index(other).unwrap());

        let out = update(&mut st, Message::WatchTriggered { session: watched });
        assert_eq!(out.effect, Some(Effect::RestartSession(watched)));
        assert_eq!(st.focus_next_registered, None);
    }

    #[test]
    fn the_relaunch_of_a_watched_session_carries_watch_on() {
        let mut st = workbench_with_project();
        let old = register_on(&mut st, 0, "/tmp/huddle", SessionTarget::Desktop);
        st.sessions[0].state = SessionState::Running;
        update(&mut st, Message::ToggleWatch);
        update(&mut st, Message::WatchTriggered { session: old });

        // The runner relaunches the spec (a synthetic registration here) and,
        // because `old` had a watcher, posts `EnableWatch` for the new id
        // right after it.
        let new = register_on(&mut st, 1, "/tmp/huddle", SessionTarget::Desktop);
        assert!(
            !st.sessions[1].watch,
            "a registration alone starts unwatched"
        );
        let out = update(&mut st, Message::EnableWatch { session: new });
        assert_eq!(out.effect, Some(Effect::WatchSet { id: new, on: true }));
        let view = &st.sessions[st.session_index(new).unwrap()];
        assert!(view.watch);
        assert_eq!(st.active_session().map(|s| s.id), Some(new));

        // Idempotent: a repeated enable starts nothing twice.
        assert_eq!(
            update(&mut st, Message::EnableWatch { session: new }).effect,
            None
        );
    }

    #[test]
    fn enable_watch_is_refused_for_a_device_session() {
        let mut st = workbench_with_project();
        let a = register_on(&mut st, 0, "/tmp/huddle", pixel_7_target());
        assert_eq!(
            update(&mut st, Message::EnableWatch { session: a }).effect,
            None
        );
        assert!(!st.sessions[0].watch);
    }

    #[test]
    fn stopping_a_watched_session_turns_watch_off() {
        let mut st = workbench_with_project();
        let a = register_on(&mut st, 0, "/tmp/huddle", SessionTarget::Desktop);
        st.sessions[0].state = SessionState::Running;
        update(&mut st, Message::ToggleWatch);

        let out = update(&mut st, Message::StopSession);
        assert_eq!(out.effect, Some(Effect::StopSession(a)));
        assert!(!st.sessions[0].watch);
    }

    #[test]
    fn a_failed_watcher_clears_the_flag_and_says_why() {
        let mut st = workbench_with_project();
        let a = register_on(&mut st, 0, "/tmp/huddle", SessionTarget::Desktop);
        update(&mut st, Message::ToggleWatch);

        update(
            &mut st,
            Message::WatchFailed {
                session: a,
                reason: "no inotify".to_string(),
            },
        );
        assert!(!st.sessions[0].watch);
        assert_eq!(warn_texts(&st), vec!["Watch unavailable: no inotify"]);
    }

    #[test]
    fn a_watch_checked_launch_routes_only_the_desktop_spec_through_the_watched_launch() {
        let mut st = workbench_with_project();
        run_config_on_devices(&mut st, vec![pixel_7()]);
        update(&mut st, Message::RunConfigToggleTargetAt(0)); // desktop on
        update(&mut st, Message::RunConfigToggleWatch);
        assert!(st.run_config.as_ref().unwrap().watch);

        let out = update(&mut st, Message::RunConfigLaunch);
        let effects = effects_of(&out);
        let watched: Vec<&SessionSpec> = effects
            .iter()
            .filter_map(|e| match e {
                Effect::LaunchWatchedSessions(specs) => Some(specs),
                _ => None,
            })
            .flatten()
            .collect();
        let plain: Vec<&SessionSpec> = effects
            .iter()
            .filter_map(|e| match e {
                Effect::LaunchSessions(specs) => Some(specs),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(watched.len(), 1);
        assert_eq!(watched[0].target, DeviceTarget::Desktop);
        assert_eq!(plain.len(), 1);
        assert_eq!(SessionTarget::of(&plain[0].target), pixel_7_target());

        // Unchecked (the default), the same launch stays one plain effect.
        let mut st = workbench_with_project();
        update(&mut st, Message::OpenRunConfig);
        let out = update(&mut st, Message::RunConfigLaunch);
        assert!(
            effects_of(&out)
                .iter()
                .all(|e| !matches!(e, Effect::LaunchWatchedSessions(_)))
        );
    }

    #[test]
    fn close_active_tab_resolves_to_the_active_index() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        update(&mut st, state_event(a, SessionState::Exited(true)));

        let out = update(&mut st, Message::CloseActiveTab);
        assert!(out.redraw);
        assert!(st.sessions.is_empty());

        // With no active session, closing the active tab is a no-op.
        let mut empty = welcome();
        let out = update(&mut empty, Message::CloseActiveTab);
        assert!(!out.redraw);
        assert_eq!(out.effect, None);
    }

    #[test]
    fn digit_select_after_removal_targets_the_shifted_tab() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        let b = register(&mut st, 1, "/tmp/b", "desktop");
        let c = register(&mut st, 2, "/tmp/c", "desktop");
        for id in [a, b, c] {
            update(&mut st, state_event(id, SessionState::Exited(true)));
        }

        // Remove `a` (index 0); `b` and `c` shift down to 0 and 1.
        update(&mut st, Message::CloseTab(0));
        assert_eq!(st.sessions[0].id, b);
        assert_eq!(st.sessions[1].id, c);

        // `SelectTab(1)` (what pressing `2` resolves to) now targets `c`, not
        // the stale pre-removal tab that index used to name.
        update(&mut st, Message::SelectTab(1));
        assert_eq!(st.active_session, Some(1));
        assert_eq!(st.sessions[st.active_session.unwrap()].id, c);
    }

    #[test]
    fn closing_a_tab_clears_a_context_menu_that_targeted_it() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        update(&mut st, state_event(a, SessionState::Exited(true)));
        update(
            &mut st,
            Message::OpenContextMenu {
                x: 0,
                y: 0,
                target: ContextTarget::SessionTab(0),
            },
        );
        assert!(st.context_menu.is_some());

        update(&mut st, Message::CloseTab(0));
        assert!(
            st.context_menu.is_none(),
            "the menu targeted the tab that just closed"
        );
    }

    #[test]
    fn closing_a_live_session_invalidates_a_higher_indexed_context_menu() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        let b = register(&mut st, 1, "/tmp/b", "desktop");
        let c = register(&mut st, 2, "/tmp/c", "desktop");
        st.sessions[0].state = SessionState::Running;

        // Close tab 0 (live); removal is deferred until its terminal event lands.
        let out = update(&mut st, Message::CloseTab(0));
        assert!(out.effect.is_some());
        assert_eq!(st.sessions.len(), 3);
        assert!(st.sessions[0].close_on_exit);

        // Open a context menu on tab 2 (which will become index 1 after the
        // deferred removal).
        update(
            &mut st,
            Message::OpenContextMenu {
                x: 0,
                y: 0,
                target: ContextTarget::SessionTab(2),
            },
        );
        assert!(st.context_menu.is_some());
        assert!(matches!(
            st.context_menu.as_ref().unwrap().target,
            ContextTarget::SessionTab(2)
        ));

        // Tab 0's terminal event lands and removes it.
        update(&mut st, state_event(a, SessionState::Killed));
        assert_eq!(st.sessions.len(), 2);
        assert_eq!(st.sessions[0].id, b);
        assert_eq!(st.sessions[1].id, c);
        // The menu targeting the old tab 2 should have been cleared because its
        // index >= the removed index.
        assert!(
            st.context_menu.is_none(),
            "menu targeting old tab 2 should clear when tab 0 is removed"
        );

        // A fresh menu on the tab that was at index 2 (now 1) should yield the
        // correct CloseTab(1) message. The label may be "Stop & close" or "Close tab"
        // depending on session state.
        update(
            &mut st,
            Message::OpenContextMenu {
                x: 0,
                y: 0,
                target: ContextTarget::SessionTab(1),
            },
        );
        assert!(st.context_menu.is_some());
        let entries = &st.context_menu.as_ref().unwrap().entries;
        let close_tab_entry = entries
            .iter()
            .find(|e| e.label == "Close tab" || e.label == "Stop & close")
            .expect("Close tab / Stop & close entry should exist");
        // Verify the entry has the correct message for the shifted index.
        assert!(
            matches!(&close_tab_entry.message, Message::CloseTab(1)),
            "close_tab_entry message should target index 1, not the old index 2"
        );
    }

    #[test]
    fn context_menu_on_lower_index_closes_when_any_tab_removed() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        let b = register(&mut st, 1, "/tmp/b", "desktop");
        let c = register(&mut st, 2, "/tmp/c", "desktop");
        for id in [a, b, c] {
            update(&mut st, state_event(id, SessionState::Exited(true)));
        }

        // Open a context menu on tab 0.
        update(
            &mut st,
            Message::OpenContextMenu {
                x: 0,
                y: 0,
                target: ContextTarget::SessionTab(0),
            },
        );
        assert!(st.context_menu.is_some());
        assert!(matches!(
            st.context_menu.as_ref().unwrap().target,
            ContextTarget::SessionTab(0)
        ));

        // Remove tab 2 via CloseTab.
        update(&mut st, Message::CloseTab(2));
        assert_eq!(st.sessions.len(), 2);
        assert_eq!(st.sessions[0].id, a);
        assert_eq!(st.sessions[1].id, b);

        // Menu targeting tab 0 closes even though the removed tab is after it.
        // Any session removal invalidates any open context menu.
        assert!(
            st.context_menu.is_none(),
            "menu on tab 0 should close when tab 2 is removed"
        );
    }

    #[test]
    fn context_menu_on_log_view_closes_on_tab_removal() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        let b = register(&mut st, 1, "/tmp/b", "desktop");
        for id in [a, b] {
            update(&mut st, state_event(id, SessionState::Exited(true)));
        }

        // Open a context menu on a LogView target.
        update(
            &mut st,
            Message::OpenContextMenu {
                x: 0,
                y: 0,
                target: ContextTarget::LogView { row: Some(42) },
            },
        );
        assert!(st.context_menu.is_some());
        assert!(matches!(
            st.context_menu.as_ref().unwrap().target,
            ContextTarget::LogView { .. }
        ));

        // Remove tab 0.
        update(&mut st, Message::CloseTab(0));
        assert_eq!(st.sessions.len(), 1);

        // Menu targeting LogView closes: the menu was built against the
        // then-active session, which may have changed or may not match
        // what the menu entries expect to operate on.
        assert!(
            st.context_menu.is_none(),
            "menu on LogView should close on tab removal"
        );
    }

    #[test]
    fn deferred_menu_on_active_session_closes_when_session_removed() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        let b = register(&mut st, 1, "/tmp/b", "desktop");
        st.sessions[0].state = SessionState::Running;
        st.active_session = Some(0);

        // Close tab 0 (live); removal is deferred until its terminal event lands.
        let out = update(&mut st, Message::CloseTab(0));
        assert!(out.effect.is_some());
        assert_eq!(st.sessions.len(), 2);
        assert!(st.sessions[0].close_on_exit);

        // Open a context menu on the active session's log view. The menu is built
        // against session 0, which is about to be removed.
        update(
            &mut st,
            Message::OpenContextMenu {
                x: 0,
                y: 0,
                target: ContextTarget::LogView { row: Some(10) },
            },
        );
        assert!(st.context_menu.is_some());
        assert!(matches!(
            st.context_menu.as_ref().unwrap().target,
            ContextTarget::LogView { .. }
        ));

        // Tab 0's terminal event lands and removes it.
        update(&mut st, state_event(a, SessionState::Killed));
        assert_eq!(st.sessions.len(), 1);
        assert_eq!(st.sessions[0].id, b);

        // The menu closes: even though it targets a LogView (not a SessionTab),
        // it was built against the active session, which is gone. A later
        // 'CopyLine' action cannot read from the wrong session.
        assert!(
            st.context_menu.is_none(),
            "menu on removed active session should close"
        );
    }

    #[test]
    fn context_menu_on_device_row_closes_when_devices_reload() {
        let mut st = workbench_with_project();
        update(
            &mut st,
            Message::DevicesLoaded(vec![
                dev("a", "A", Platform::Android, Kind::Emulator),
                dev("b", "B", Platform::Android, Kind::Emulator),
            ]),
        );

        // Open a context menu on device row 1.
        update(
            &mut st,
            Message::OpenContextMenu {
                x: 0,
                y: 0,
                target: ContextTarget::DeviceRow(1),
            },
        );
        assert!(st.context_menu.is_some());
        assert!(matches!(
            st.context_menu.as_ref().unwrap().target,
            ContextTarget::DeviceRow(1)
        ));

        // Devices reload with a reordered/shorter list: row 1's entry is stale.
        update(
            &mut st,
            Message::DevicesLoaded(vec![dev("b", "B", Platform::Android, Kind::Emulator)]),
        );

        assert!(
            st.context_menu.is_none(),
            "menu on a DeviceRow should close when the device list reloads"
        );
    }

    #[test]
    fn context_menu_on_session_tab_survives_devices_reload() {
        let mut st = welcome();
        register(&mut st, 0, "/tmp/a", "desktop");

        // Open a context menu on a session tab.
        update(
            &mut st,
            Message::OpenContextMenu {
                x: 0,
                y: 0,
                target: ContextTarget::SessionTab(0),
            },
        );
        assert!(st.context_menu.is_some());

        // A device list reload is unrelated to a SessionTab menu; it stays open
        // and still targets the same tab.
        update(
            &mut st,
            Message::DevicesLoaded(vec![dev("a", "A", Platform::Android, Kind::Emulator)]),
        );

        assert!(
            st.context_menu.is_some(),
            "menu on a SessionTab should survive a device list reload"
        );
        assert!(matches!(
            st.context_menu.as_ref().unwrap().target,
            ContextTarget::SessionTab(0)
        ));
    }

    #[test]
    fn scroll_and_follow_route_to_the_active_session() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        for i in 0..10 {
            update(&mut st, line(a, &format!("line {i}")));
        }
        assert!(st.active_session().unwrap().is_following());
        update(&mut st, Message::LogScrollUp(3));
        assert!(matches!(
            st.active_session().unwrap().scroll,
            Scroll::Anchored(_)
        ));
        update(&mut st, Message::ToggleFollow);
        // toggle from Anchored -> Follow
        assert!(st.active_session().unwrap().is_following());
    }

    #[test]
    fn scroll_routing_carries_the_committed_search_filter() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        for l in ["keep 0", "drop", "keep 1", "drop", "keep 2"] {
            update(&mut st, line(a, l));
        }
        // The free-text filter is global (`AppState::search`), so only the
        // routing here can hand it to the session's visible-sequence math —
        // without it a step would land on a hidden line.
        update(&mut st, Message::SearchOpen);
        for c in "keep".chars() {
            update(&mut st, Message::SearchInput(c));
        }
        update(&mut st, Message::SearchCommit);
        update(&mut st, Message::LogScrollUp(1));
        assert_eq!(st.active_session().unwrap().scroll, Scroll::Anchored(2));
        update(&mut st, Message::LogScrollDown(1));
        assert!(st.active_session().unwrap().is_following());
    }

    #[test]
    fn search_open_type_commit_filters_and_cancel_restores() {
        let mut st = welcome();
        register(&mut st, 0, "/tmp/a", "desktop");
        update(&mut st, Message::SearchOpen);
        assert!(st.search.open);
        update(&mut st, Message::SearchInput('e'));
        update(&mut st, Message::SearchInput('r'));
        update(&mut st, Message::SearchBackspace);
        update(&mut st, Message::SearchInput('r'));
        assert_eq!(st.search.query, "er");
        update(&mut st, Message::SearchCommit);
        assert!(!st.search.open);
        assert_eq!(st.search.filter.as_deref(), Some("er"));
        // Re-open then cancel leaves the committed filter intact.
        update(&mut st, Message::SearchOpen);
        update(&mut st, Message::SearchInput('x'));
        update(&mut st, Message::SearchCancel);
        assert!(!st.search.open);
        assert_eq!(st.search.filter.as_deref(), Some("er"));
    }

    // ── Devices panel + run-config modal ────────────────────────────────────

    use crate::engine::run_config::RunFocus;
    use frust_drive::build_info::BuildMode;
    use frust_drive::devices::{Device, Kind, Platform};

    fn dev(id: &str, name: &str, platform: Platform, kind: Kind) -> Device {
        Device {
            id: id.into(),
            name: name.into(),
            platform,
            kind,
            os_version: None,
            connection_state: None,
        }
    }

    fn workbench_with_project() -> AppState {
        let root = PathBuf::from("/tmp/huddle");
        AppState {
            screen: crate::engine::Screen::Workbench,
            project_root: Some(root.clone()),
            projects: vec![root],
            ..Default::default()
        }
    }

    #[test]
    fn refresh_requests_the_discovery_effect_and_flags_scanning() {
        let mut st = workbench_with_project();
        let out = update(&mut st, Message::RefreshDevices);
        assert!(st.devices_refreshing);
        assert_eq!(out.effect, Some(Effect::RefreshDevices));
        assert!(out.redraw);
    }

    #[test]
    fn devices_loaded_populates_clears_flag_and_preserves_selection() {
        let mut st = workbench_with_project();
        st.devices_refreshing = true;
        update(
            &mut st,
            Message::DevicesLoaded(vec![
                dev(
                    "emulator-5554",
                    "Pixel 7",
                    Platform::Android,
                    Kind::Emulator,
                ),
                dev("AAAA", "iPhone 15", Platform::Ios, Kind::Simulator),
            ]),
        );
        assert!(!st.devices_refreshing);
        assert_eq!(st.devices.len(), 2);
        // Select the emulator, then a refresh keeps it selected (matched by id).
        st.devices[0].selected = true;
        update(
            &mut st,
            Message::DevicesLoaded(vec![
                dev(
                    "emulator-5554",
                    "Pixel 7",
                    Platform::Android,
                    Kind::Emulator,
                ),
                dev("BBBB", "iPhone SE", Platform::Ios, Kind::PhysicalDevice),
            ]),
        );
        assert!(
            st.devices[0].selected,
            "surviving device keeps its selection"
        );
        assert!(!st.devices[1].selected, "new device arrives unselected");
    }

    #[test]
    fn device_cursor_moves_and_clamps() {
        let mut st = workbench_with_project();
        update(
            &mut st,
            Message::DevicesLoaded(vec![
                dev("a", "A", Platform::Android, Kind::Emulator),
                dev("b", "B", Platform::Android, Kind::Emulator),
            ]),
        );
        assert_eq!(st.device_cursor, 0);
        assert!(!update(&mut st, Message::DeviceCursorUp).redraw); // at top, no-op
        assert!(update(&mut st, Message::DeviceCursorDown).redraw);
        assert_eq!(st.device_cursor, 1);
        assert!(!update(&mut st, Message::DeviceCursorDown).redraw); // at bottom
        // Toggling select tracks the cursor.
        update(&mut st, Message::ToggleDeviceSelect);
        assert!(st.devices[1].selected);
        // A click on index 0 toggles it and moves the cursor.
        update(&mut st, Message::SelectDeviceAt(0));
        assert!(st.devices[0].selected);
        assert_eq!(st.device_cursor, 0);
    }

    #[test]
    fn open_run_config_primes_from_panel_selection() {
        let mut st = workbench_with_project();
        update(
            &mut st,
            Message::DevicesLoaded(vec![dev(
                "emulator-5554",
                "Pixel 7",
                Platform::Android,
                Kind::Emulator,
            )]),
        );
        update(&mut st, Message::ToggleDeviceSelect); // select Pixel 7
        assert!(update(&mut st, Message::OpenRunConfig).redraw);
        let modal = st.run_config.as_ref().expect("modal open");
        // desktop + Pixel 7, the device checked.
        assert_eq!(modal.targets.len(), 2);
        assert!(modal.targets[1].selected);
        // Esc closes it.
        assert!(update(&mut st, Message::CloseRunConfig).redraw);
        assert!(st.run_config.is_none());
    }

    #[test]
    fn open_run_config_is_a_noop_on_the_welcome_screen() {
        let mut st = welcome();
        assert!(!update(&mut st, Message::OpenRunConfig).redraw);
        assert!(st.run_config.is_none());
    }

    #[test]
    fn modal_edits_route_through_the_focused_control() {
        let mut st = workbench_with_project();
        update(&mut st, Message::OpenRunConfig); // desktop-only modal
        update(&mut st, Message::RunConfigCycleMode(1));
        assert_eq!(st.run_config.as_ref().unwrap().mode, BuildMode::Profile);
        // Cycling focuses the mode row.
        assert_eq!(st.run_config.as_ref().unwrap().focus, RunFocus::Mode);
        update(&mut st, Message::RunConfigFocus(RunFocus::Flavor));
        update(&mut st, Message::RunConfigInput('p'));
        update(&mut st, Message::RunConfigInput('q'));
        update(&mut st, Message::RunConfigBackspace);
        assert_eq!(st.run_config.as_ref().unwrap().flavor, "p");
    }

    /// The N-device multi-launch acceptance path (engine level): N selected
    /// targets → a launch effect carrying N specs → N registered sessions,
    /// grouped under the one project.
    #[test]
    fn multi_device_launch_produces_one_session_per_target() {
        let mut st = workbench_with_project();
        update(
            &mut st,
            Message::DevicesLoaded(vec![
                dev(
                    "emulator-5554",
                    "Pixel 7",
                    Platform::Android,
                    Kind::Emulator,
                ),
                dev(
                    "emulator-5556",
                    "Pixel 8",
                    Platform::Android,
                    Kind::Emulator,
                ),
                dev("AAAA", "iPhone 15", Platform::Ios, Kind::Simulator),
            ]),
        );
        // Select all three devices in the panel.
        for _ in 0..3 {
            update(&mut st, Message::ToggleDeviceSelect);
            update(&mut st, Message::DeviceCursorDown);
        }
        update(&mut st, Message::OpenRunConfig);
        // Also check desktop for a 4-way launch.
        update(&mut st, Message::RunConfigToggleTargetAt(0));

        let out = update(&mut st, Message::RunConfigLaunch);
        assert!(st.run_config.is_none(), "launch closes the modal");
        let Some(Effect::LaunchSessions(specs)) = out.effect else {
            panic!("expected a LaunchSessions effect, got {:?}", out.effect);
        };
        // desktop + 3 devices.
        assert_eq!(specs.len(), 4);
        assert!(
            specs
                .iter()
                .all(|s| s.project_root == std::path::Path::new("/tmp/huddle"))
        );

        // Simulate the runner registering each started session (the supervisor
        // hands back ids in order); the model then holds N grouped sessions.
        for (i, spec) in specs.iter().enumerate() {
            let label = match &spec.target {
                crate::supervise::DeviceTarget::Desktop => "desktop".to_string(),
                crate::supervise::DeviceTarget::Device(d) => d.name.clone(),
            };
            update(
                &mut st,
                Message::RegisterSession {
                    id: SessionId(i as u64),
                    project_root: spec.project_root.clone(),
                    target_label: label,
                    devtools: DevtoolsLaunch::unavailable(),
                    target: Some(SessionTarget::of(&spec.target)),
                },
            );
        }
        assert_eq!(st.sessions.len(), 4);
        // All under the one project → a single group.
        assert_eq!(st.sessions_grouped().len(), 1);
        assert_eq!(st.active_session, Some(0));
    }

    #[test]
    fn launch_with_nothing_checked_does_not_emit_an_effect() {
        let mut st = workbench_with_project();
        update(&mut st, Message::OpenRunConfig);
        // Uncheck the defaulted desktop target.
        update(&mut st, Message::RunConfigToggleTargetAt(0));
        let out = update(&mut st, Message::RunConfigLaunch);
        assert_eq!(out.effect, None);
        assert!(
            st.run_config.is_some(),
            "modal stays open with nothing to launch"
        );
    }

    // ── One live session per (project, target) ──────────────────────────────

    /// A `Pixel 7` emulator, as both the discovery panel and the guard see it.
    fn pixel_7() -> Device {
        dev(
            "emulator-5554",
            "Pixel 7",
            Platform::Android,
            Kind::Emulator,
        )
    }

    /// The identity of [`pixel_7`].
    fn pixel_7_target() -> SessionTarget {
        SessionTarget::of(&DeviceTarget::Device(pixel_7()))
    }

    /// Load `devices`, select them all in the panel, and open the run-config
    /// modal — which checks exactly the selected devices (row 0, desktop, is
    /// left unchecked once any device is selected).
    fn run_config_on_devices(st: &mut AppState, devices: Vec<Device>) {
        let count = devices.len();
        update(st, Message::DevicesLoaded(devices));
        for _ in 0..count {
            update(st, Message::ToggleDeviceSelect);
            update(st, Message::DeviceCursorDown);
        }
        update(st, Message::OpenRunConfig);
    }

    fn warn_texts(st: &AppState) -> Vec<&str> {
        st.toasts
            .items
            .iter()
            .filter(|t| t.kind == ToastKind::Warn)
            .map(|t| t.text.as_str())
            .collect()
    }

    #[test]
    fn launching_a_target_that_is_already_running_is_refused_with_a_toast() {
        let mut st = workbench_with_project();
        register_on(&mut st, 0, "/tmp/huddle", SessionTarget::Desktop);
        update(&mut st, Message::OpenRunConfig); // desktop checked by default

        let out = update(&mut st, Message::RunConfigLaunch);

        assert_eq!(
            out.effect, None,
            "a refused launch must not reach the supervisor at all"
        );
        assert!(
            st.run_config.is_none(),
            "the modal still closes — the toast is what explains the refusal"
        );
        assert_eq!(
            warn_texts(&st),
            vec!["desktop: already running here — stop it first (x)"]
        );
    }

    #[test]
    fn a_mixed_launch_starts_only_the_targets_that_are_free() {
        let mut st = workbench_with_project();
        register_on(&mut st, 0, "/tmp/huddle", pixel_7_target());
        run_config_on_devices(&mut st, vec![pixel_7()]);
        // Check desktop too, so the batch is {desktop (free), Pixel 7 (busy)}.
        update(&mut st, Message::RunConfigToggleTargetAt(0));

        let out = update(&mut st, Message::RunConfigLaunch);

        let Some(Effect::LaunchSessions(specs)) = out.effect else {
            panic!("the free target must still launch, got {:?}", out.effect);
        };
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].target, DeviceTarget::Desktop);
        assert_eq!(
            warn_texts(&st),
            vec!["Pixel 7: already running here — stop it first (x)"],
            "one toast, naming the device that was refused"
        );
    }

    #[test]
    fn a_terminal_session_does_not_block_relaunching_its_target() {
        let mut st = workbench_with_project();
        let id = register_on(&mut st, 0, "/tmp/huddle", SessionTarget::Desktop);
        update(
            &mut st,
            Message::Session(SessionEvent {
                id,
                kind: SessionEventKind::State(crate::supervise::SessionState::Exited(true)),
            }),
        );
        update(&mut st, Message::OpenRunConfig);

        let out = update(&mut st, Message::RunConfigLaunch);

        assert!(
            matches!(out.effect, Some(Effect::LaunchSessions(ref specs)) if specs.len() == 1),
            "an exited session occupies nothing, got {:?}",
            out.effect
        );
        assert!(warn_texts(&st).is_empty());
    }

    #[test]
    fn a_different_project_on_the_same_device_is_allowed() {
        let mut st = workbench_with_project();
        // Another project is already on the Pixel 7 — the pair is what is
        // exclusive, not the device.
        register_on(&mut st, 0, "/tmp/other-app", pixel_7_target());
        run_config_on_devices(&mut st, vec![pixel_7()]);

        let out = update(&mut st, Message::RunConfigLaunch);

        let Some(Effect::LaunchSessions(specs)) = out.effect else {
            panic!(
                "a different project must still launch, got {:?}",
                out.effect
            );
        };
        assert_eq!(specs.len(), 1);
        assert!(warn_texts(&st).is_empty());
    }

    #[test]
    fn an_ad_hoc_session_never_blocks_a_launch() {
        let mut st = workbench_with_project();
        // A build of the same project: live, same root, but no target.
        register(&mut st, 0, "/tmp/huddle", "build apk");
        update(&mut st, Message::OpenRunConfig);

        let out = update(&mut st, Message::RunConfigLaunch);

        assert!(
            matches!(out.effect, Some(Effect::LaunchSessions(ref specs)) if specs.len() == 1),
            "building a project must not stop it being run, got {:?}",
            out.effect
        );
        assert!(warn_texts(&st).is_empty());
    }

    #[test]
    fn run_on_all_devices_skips_the_devices_already_running_this_project() {
        let mut st = workbench_with_project();
        register_on(&mut st, 0, "/tmp/huddle", pixel_7_target());
        update(
            &mut st,
            Message::DevicesLoaded(vec![
                pixel_7(),
                dev(
                    "emulator-5556",
                    "Pixel 8",
                    Platform::Android,
                    Kind::Emulator,
                ),
            ]),
        );

        let out = update(&mut st, Message::RunOnAllDevices);

        let Some(Effect::LaunchSessions(specs)) = out.effect else {
            panic!("the free device must still launch, got {:?}", out.effect);
        };
        assert_eq!(specs.len(), 1);
        assert!(
            matches!(&specs[0].target, DeviceTarget::Device(d) if d.name == "Pixel 8"),
            "only the device with no live session launches"
        );
        assert_eq!(
            warn_texts(&st),
            vec!["Pixel 7: already running here — stop it first (x)"]
        );
    }

    #[test]
    fn run_on_all_devices_emits_no_effect_when_every_device_is_busy() {
        let mut st = workbench_with_project();
        register_on(&mut st, 0, "/tmp/huddle", pixel_7_target());
        update(&mut st, Message::DevicesLoaded(vec![pixel_7()]));

        let out = update(&mut st, Message::RunOnAllDevices);

        assert_eq!(out.effect, None);
        assert!(out.redraw, "the toast explaining it is itself a paint");
        assert_eq!(
            warn_texts(&st),
            vec!["Pixel 7: already running here — stop it first (x)"]
        );
    }

    // ── Line-selection mode (`v` … `y`) ─────────────────────────────────────

    /// A workbench with one session holding `n` seeded lines, following its
    /// tail — the starting point every mode flow below drives with messages.
    fn selection_state(n: usize) -> AppState {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        for i in 0..n {
            update(&mut st, line(a, &format!("line {i}")));
        }
        st
    }

    fn selected(st: &AppState) -> Option<(u64, u64)> {
        st.active_session()
            .and_then(|s| s.selection)
            .map(|sel| (sel.lo(), sel.hi()))
    }

    fn toast_texts(st: &AppState, kind: ToastKind) -> Vec<&str> {
        st.toasts
            .items
            .iter()
            .filter(|t| t.kind == kind)
            .map(|t| t.text.as_str())
            .collect()
    }

    #[test]
    fn select_enter_anchors_one_line_at_the_tail_and_pauses_follow() {
        let mut st = selection_state(5);
        assert!(st.active_session().unwrap().is_following());

        let out = update(&mut st, Message::SelectEnter);

        assert!(out.redraw);
        assert_eq!(out.effect, None);
        let session = st.active_session().unwrap();
        assert!(session.select_mode);
        assert_eq!(selected(&st), Some((4, 4)), "`v` selects the newest line");
        assert!(!session.is_following(), "the mode pauses follow-tail");
        assert_eq!(session.follow_before_select, Some(true));
    }

    /// DevTools owns the whole key namespace while it is open (workbook
    /// §B12); the palette's "Select lines…" row re-dispatches the same
    /// `Message::SelectEnter` a `v` press would, so the refusal has to live
    /// in `update` itself rather than in `translate_key`'s early return.
    #[test]
    fn select_enter_is_refused_while_devtools_owns_the_pane() {
        let mut st = selection_state(5);
        st.active_session_mut().unwrap().devtools.open = true;

        let out = update(&mut st, Message::SelectEnter);

        assert!(!out.redraw, "no toast, no highlight — a silent refusal");
        assert!(!st.active_session().unwrap().select_mode);
        assert_eq!(st.active_session().unwrap().selection, None);
    }

    #[test]
    fn select_move_down_extends_the_range_a_row_at_a_time() {
        let mut st = selection_state(8);
        update(&mut st, Message::SelectEnter);
        // From the tail the cursor can only walk back up the log.
        for _ in 0..3 {
            update(&mut st, Message::SelectMove(-1));
        }
        assert_eq!(selected(&st), Some((4, 7)), "four lines selected");
        // …and back down again, shrinking the range.
        update(&mut st, Message::SelectMove(1));
        assert_eq!(selected(&st), Some((5, 7)));
    }

    #[test]
    fn select_move_clamps_at_both_ends_and_home_end_jump_to_them() {
        let mut st = selection_state(6);
        update(&mut st, Message::SelectEnter);
        for _ in 0..20 {
            update(&mut st, Message::SelectMove(-1));
        }
        assert_eq!(selected(&st), Some((0, 5)), "clamped at the oldest line");
        let out = update(&mut st, Message::SelectMove(-1));
        assert!(!out.redraw, "a clamped move is not a repaint");

        update(&mut st, Message::SelectEnd);
        assert_eq!(selected(&st), Some((5, 5)), "cursor back at the tail");
        update(&mut st, Message::SelectHome);
        assert_eq!(selected(&st), Some((0, 5)));
        update(&mut st, Message::SelectPage(1));
        assert_eq!(selected(&st), Some((5, 5)), "a page covers this short log");
    }

    #[test]
    fn a_row_click_anchors_once_then_drags_the_range_end() {
        let mut st = selection_state(10);
        update(&mut st, Message::SelectEnter);

        // First click re-anchors at the clicked row…
        update(&mut st, Message::LogRowClicked(2));
        assert_eq!(selected(&st), Some((2, 2)));
        // …every later one only moves the range end.
        update(&mut st, Message::LogRowClicked(6));
        assert_eq!(selected(&st), Some((2, 6)));
        update(&mut st, Message::LogRowClicked(4));
        assert_eq!(selected(&st), Some((2, 4)), "the anchor stays put");
    }

    #[test]
    fn a_row_click_outside_the_mode_is_idle() {
        let mut st = selection_state(4);
        let out = update(&mut st, Message::LogRowClicked(1));
        assert!(!out.redraw);
        assert_eq!(out.effect, None);
        assert_eq!(selected(&st), None);
    }

    #[test]
    fn copy_selection_emits_the_joined_lines_toasts_and_leaves_the_mode() {
        let mut st = selection_state(5);
        update(&mut st, Message::SelectEnter); // anchors newest (line 4)
        update(&mut st, Message::SelectMove(-2)); // -> lines 2..=4

        let out = update(&mut st, Message::CopySelection);

        assert_eq!(
            out.effect,
            Some(Effect::Copy("line 2\nline 3\nline 4".to_string()))
        );
        assert_eq!(toast_texts(&st, ToastKind::Info), vec!["Copied: 3 lines"]);
        let session = st.active_session().unwrap();
        assert!(!session.select_mode, "`y` is the whole gesture");
        assert_eq!(session.selection, None);
        assert!(
            !session.is_following(),
            "the cursor was walked off the tail, so the view stays where it was"
        );
    }

    /// The WarnPlus counterexample: a Warn/Info/Warn triple with the level
    /// filter hiding the middle line. The cursor keys skip the hidden line
    /// (already covered in `session_view`), and `y` copies and counts only
    /// the two visible rows, even though the absolute range spans all three.
    #[test]
    fn copy_selection_counts_only_the_visible_sequence_across_a_level_filter() {
        use crate::engine::logstyle::LevelFilter;

        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        update(&mut st, line(a, "warn a"));
        update(&mut st, line(a, "info b"));
        update(&mut st, line(a, "warn c"));
        st.active_session_mut()
            .unwrap()
            .set_level_filter(LevelFilter::WarnPlus);

        update(&mut st, Message::SelectEnter); // anchors the newest visible: warn c
        update(&mut st, Message::SelectMove(-1)); // -> warn a, the hidden info skipped

        let out = update(&mut st, Message::CopySelection);

        assert_eq!(out.effect, Some(Effect::Copy("warn a\nwarn c".to_string())));
        assert_eq!(toast_texts(&st, ToastKind::Info), vec!["Copied: 2 lines"]);
    }

    #[test]
    fn copying_at_the_tail_restores_the_follow_it_paused() {
        let mut st = selection_state(5);
        update(&mut st, Message::SelectEnter);
        let out = update(&mut st, Message::CopySelection);
        assert_eq!(out.effect, Some(Effect::Copy("line 4".to_string())));
        assert_eq!(toast_texts(&st, ToastKind::Info), vec!["Copied: 1 line"]);
        assert!(st.active_session().unwrap().is_following());
    }

    #[test]
    fn select_exit_leaves_the_mode_without_copying() {
        let mut st = selection_state(5);
        update(&mut st, Message::SelectEnter);
        update(&mut st, Message::SelectMove(-1));

        let out = update(&mut st, Message::SelectExit);

        assert_eq!(out.effect, None, "`Esc` copies nothing");
        assert!(out.redraw);
        let session = st.active_session().unwrap();
        assert!(!session.select_mode);
        assert_eq!(session.selection, None);
        assert!(st.toasts.items.is_empty());
        assert!(
            !update(&mut st, Message::SelectExit).redraw,
            "leaving a mode that is already left is idle"
        );
    }

    #[test]
    fn the_mode_is_per_session_and_survives_a_tab_switch() {
        let mut st = selection_state(4);
        let b = register(&mut st, 1, "/tmp/a", "Pixel 7");
        update(&mut st, line(b, "other"));

        update(&mut st, Message::SelectTab(0));
        update(&mut st, Message::SelectEnter);
        update(&mut st, Message::SelectTab(1));
        assert!(
            !st.active_session().unwrap().select_mode,
            "the other tab is not in the mode"
        );
        update(&mut st, Message::SelectTab(0));
        assert!(
            st.active_session().unwrap().select_mode,
            "the tab that entered the mode is still in it"
        );
    }

    #[test]
    fn a_terminal_session_event_does_not_leave_the_mode() {
        let mut st = selection_state(3);
        let a = SessionId(0);
        update(&mut st, Message::SelectEnter);
        update(
            &mut st,
            Message::Session(SessionEvent {
                id: a,
                kind: SessionEventKind::State(SessionState::Exited(true)),
            }),
        );
        assert!(
            st.active_session().unwrap().select_mode,
            "the log is still there to select from"
        );
    }

    #[test]
    fn copy_line_copies_one_line_with_a_previewed_toast() {
        let mut st = selection_state(3);
        let long = "x".repeat(80);
        update(&mut st, line(SessionId(0), &long));

        let out = update(&mut st, Message::CopyLine(3));

        assert_eq!(out.effect, Some(Effect::Copy(long.clone())));
        let preview = toast_texts(&st, ToastKind::Info);
        assert_eq!(preview.len(), 1);
        assert_eq!(preview[0], format!("Copied: {}\u{2026}", "x".repeat(60)));
    }

    #[test]
    fn copy_line_on_an_evicted_line_warns_instead_of_copying() {
        let mut st = selection_state(3);
        let out = update(&mut st, Message::CopyLine(99));
        assert_eq!(out.effect, None);
        assert!(out.redraw);
        assert_eq!(
            toast_texts(&st, ToastKind::Warn),
            vec!["Line no longer available"]
        );
    }

    // ── Project switcher + recent-projects persistence ──────────────────────

    fn multi_project_workbench() -> AppState {
        let a = PathBuf::from("/tmp/a");
        let b = PathBuf::from("/tmp/b");
        let c = PathBuf::from("/tmp/c");
        AppState {
            screen: crate::engine::Screen::Workbench,
            project_root: Some(a.clone()),
            projects: vec![a, b, c],
            ..Default::default()
        }
    }

    #[test]
    fn toggle_opens_and_closes_seeding_the_cursor_at_the_active_project() {
        let mut st = multi_project_workbench();
        assert!(update(&mut st, Message::ToggleProjectSwitcher).redraw);
        assert!(st.project_switcher_open);
        assert_eq!(st.project_switcher_cursor, 0); // /tmp/a is active
        assert!(update(&mut st, Message::ToggleProjectSwitcher).redraw);
        assert!(!st.project_switcher_open);
    }

    #[test]
    fn close_is_a_noop_when_already_closed() {
        let mut st = multi_project_workbench();
        assert!(!update(&mut st, Message::CloseProjectSwitcher).redraw);
    }

    #[test]
    fn cursor_moves_clamp_and_only_apply_while_open() {
        let mut st = multi_project_workbench();
        // Closed: cursor messages are a no-op.
        assert!(!update(&mut st, Message::ProjectSwitcherCursorDown).redraw);
        update(&mut st, Message::ToggleProjectSwitcher); // cursor -> 0
        assert!(update(&mut st, Message::ProjectSwitcherCursorDown).redraw);
        assert_eq!(st.project_switcher_cursor, 1);
        update(&mut st, Message::ProjectSwitcherCursorDown);
        assert_eq!(st.project_switcher_cursor, 2);
        assert!(!update(&mut st, Message::ProjectSwitcherCursorDown).redraw); // at bottom
        assert!(update(&mut st, Message::ProjectSwitcherCursorUp).redraw);
        assert_eq!(st.project_switcher_cursor, 1);
    }

    #[test]
    fn switch_project_updates_active_root_closes_switcher_and_records_effect() {
        let mut st = multi_project_workbench();
        update(&mut st, Message::ToggleProjectSwitcher);
        let out = update(&mut st, Message::SwitchProject(1));
        assert!(out.redraw);
        assert_eq!(
            out.effect,
            Some(Effect::RecordRecentProject(PathBuf::from("/tmp/b")))
        );
        assert_eq!(st.project_root, Some(PathBuf::from("/tmp/b")));
        assert!(!st.project_switcher_open, "switching closes the dropdown");
    }

    #[test]
    fn switch_project_out_of_range_is_a_noop() {
        let mut st = multi_project_workbench();
        let out = update(&mut st, Message::SwitchProject(99));
        assert!(!out.redraw);
        assert_eq!(out.effect, None);
        assert_eq!(st.project_root, Some(PathBuf::from("/tmp/a")));
    }

    // ── Create-project wizard ───────────────────────────────────────────────

    use crate::engine::WizardStep;

    fn type_str(state: &mut AppState, s: &str) {
        for c in s.chars() {
            update(state, Message::CreateWizardInput(c));
        }
    }

    #[test]
    fn open_wizard_probes_and_close_dismisses() {
        let mut st = welcome();
        let out = update(&mut st, Message::OpenCreateWizard);
        assert!(st.create_wizard.is_some());
        assert_eq!(out.effect, Some(Effect::ProbeCleanSignals));
        // A second open while already open is a no-op.
        assert!(!update(&mut st, Message::OpenCreateWizard).redraw);
        // Close dismisses it.
        assert!(update(&mut st, Message::CloseCreateWizard).redraw);
        assert!(st.create_wizard.is_none());
    }

    #[test]
    fn esc_steps_back_then_closes_from_the_first_step() {
        let mut st = welcome();
        update(&mut st, Message::OpenCreateWizard);
        type_str(&mut st, "my_app");
        update(&mut st, Message::CreateWizardAdvance); // Name -> Directory
        assert_eq!(
            st.create_wizard.as_ref().unwrap().step,
            WizardStep::Directory
        );
        update(&mut st, Message::CreateWizardBack); // Directory -> Name
        assert_eq!(st.create_wizard.as_ref().unwrap().step, WizardStep::Name);
        update(&mut st, Message::CreateWizardBack); // Name -> close
        assert!(st.create_wizard.is_none());
    }

    #[test]
    fn probe_result_is_a_harmless_no_op_for_the_clean_signals_card() {
        // clean-signals is git+rev-pinned to its public repo, so
        // the card starts enabled and stays enabled regardless of the probe
        // result — the probe still fires (`Effect::ProbeCleanSignals`) but
        // no card depends on its outcome today.
        let mut st = welcome();
        update(&mut st, Message::OpenCreateWizard);
        let clean = |st: &AppState| {
            st.create_wizard
                .as_ref()
                .unwrap()
                .arches
                .iter()
                .find(|c| c.tag.as_deref() == Some("clean-signals"))
                .unwrap()
                .enabled
        };
        assert!(clean(&st));
        update(&mut st, Message::CleanSignalsProbed(true));
        assert!(clean(&st));
        // A probe that arrives after the wizard closed is a harmless no-op.
        update(&mut st, Message::CloseCreateWizard);
        assert!(!update(&mut st, Message::CleanSignalsProbed(false)).redraw);
    }

    /// Acceptance path (engine level): from an empty temp dir, the wizard
    /// drives to a real `scaffold::generate` (the runner's off-thread work,
    /// simulated inline here), and the resulting `ScaffoldSucceeded` opens the
    /// new project in the workbench.
    #[test]
    fn wizard_scaffolds_a_real_project_and_opens_it() {
        use frust_drive::scaffold::{self, TemplateContext};
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base =
            std::env::temp_dir().join(format!("frust-tui-wizard-e2e-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let dest = base.join("my_app");

        let mut st = welcome();
        update(&mut st, Message::OpenCreateWizard);
        update(&mut st, Message::CleanSignalsProbed(false));
        type_str(&mut st, "my_app");
        update(&mut st, Message::CreateWizardAdvance); // Name -> Directory
        update(&mut st, Message::CreateWizardAdvance); // Directory -> Arch
        assert_eq!(st.create_wizard.as_ref().unwrap().step, WizardStep::Arch);

        // The default card scaffolds; assert the effect carries the right args.
        let out = update(&mut st, Message::CreateWizardAdvance);
        let Some(Effect::ScaffoldProject {
            directory,
            project_name,
            arch,
        }) = out.effect
        else {
            panic!("expected a ScaffoldProject effect, got {:?}", out.effect);
        };
        assert_eq!(project_name, "my_app");
        assert_eq!(directory, "my_app");
        assert_eq!(arch, None);
        assert_eq!(
            st.create_wizard.as_ref().unwrap().step,
            WizardStep::Scaffolding
        );

        // Simulate the runner performing the real scaffold off-thread into the
        // temp dir (the template is embedded, so this is cheap).
        let ctx = TemplateContext {
            title_case_name: scaffold::title_case(&project_name),
            project_name: project_name.clone(),
            org: "dev.f0x".to_string(),
            description: "A new Frust application.".to_string(),
            frust_version: "0.1.0".to_string(),
            frust_path: "/path/to/frust".to_string(),
            deeplink_scheme: None,
            deeplink_host: None,
        };
        scaffold::generate(&dest, &ctx, None, false, arch.as_deref())
            .expect("real scaffold into the temp dir");
        assert!(
            dest.join("Cargo.toml").is_file(),
            "scaffold wrote the project"
        );

        // The runner posts the absolute root back; the wizard closes and the
        // project opens in the workbench.
        let root = frust_drive::host_path::canonicalize_simplified(&dest).unwrap();
        let out = update(
            &mut st,
            Message::ScaffoldSucceeded {
                project_root: root.clone(),
            },
        );
        assert!(st.create_wizard.is_none(), "success closes the wizard");
        assert_eq!(st.screen, Screen::Workbench);
        assert_eq!(st.project_root, Some(root.clone()));
        assert!(st.projects.contains(&root));
        assert_eq!(out.effect, Some(Effect::RecordRecentProject(root)));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn scaffold_failure_lands_on_the_error_step() {
        let mut st = welcome();
        update(&mut st, Message::OpenCreateWizard);
        type_str(&mut st, "my_app");
        update(&mut st, Message::CreateWizardAdvance);
        update(&mut st, Message::CreateWizardAdvance);
        update(&mut st, Message::CreateWizardAdvance); // -> Scaffolding
        update(
            &mut st,
            Message::ScaffoldFailed("destination not empty".to_string()),
        );
        let w = st.create_wizard.as_ref().unwrap();
        assert_eq!(w.step, WizardStep::Error);
        assert_eq!(w.error.as_deref(), Some("destination not empty"));
        // Enter retries from the arch step.
        update(&mut st, Message::CreateWizardAdvance);
        assert_eq!(st.create_wizard.as_ref().unwrap().step, WizardStep::Arch);
    }

    // ── Add plugin dialog ─────────────────────────────────────────────────

    use crate::engine::AddPluginStep;

    #[test]
    fn open_add_plugin_probes_when_a_project_is_open() {
        let mut st = workbench_with_project();
        let out = update(&mut st, Message::OpenAddPlugin);
        assert!(st.add_plugin.is_some());
        assert_eq!(out.effect, Some(Effect::ProbeCleanSignals));
        // A second open is a no-op.
        assert!(!update(&mut st, Message::OpenAddPlugin).redraw);
        // Close dismisses it.
        assert!(update(&mut st, Message::CloseAddPlugin).redraw);
        assert!(st.add_plugin.is_none());
    }

    #[test]
    fn open_add_plugin_with_no_project_toasts_the_reason() {
        let mut st = welcome();
        let out = update(&mut st, Message::OpenAddPlugin);
        assert!(st.add_plugin.is_none(), "no project → dialog does not open");
        assert!(out.redraw, "a warn toast is pushed");
        assert_eq!(out.effect, None);
        assert!(!st.toasts.items.is_empty());
    }

    #[test]
    fn add_plugin_probe_is_a_harmless_no_op_with_no_sibling_gated_entries() {
        // clean-signals-frust was the sole `requires_sibling` registry user
        // before clean-signals moved to a git+rev pin; no entry is
        // sibling-gated today, so every card starts (and stays) enabled
        // regardless of the probe result.
        let mut st = workbench_with_project();
        update(&mut st, Message::OpenAddPlugin);
        let entries = &st.add_plugin.as_ref().unwrap().entries;
        assert!(entries.iter().all(|e| !e.sibling_gated && e.enabled));
        update(&mut st, Message::CleanSignalsProbed(true));
        let entries = &st.add_plugin.as_ref().unwrap().entries;
        assert!(entries.iter().all(|e| !e.sibling_gated && e.enabled));
    }

    #[test]
    fn add_plugin_advance_emits_the_apply_effect_with_selected_features() {
        let mut st = workbench_with_project();
        update(&mut st, Message::OpenAddPlugin);
        // Select secure-storage.
        let idx = st
            .add_plugin
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .position(|e| e.id == "secure-storage")
            .unwrap();
        update(&mut st, Message::AddPluginSelectAt(idx));
        update(&mut st, Message::AddPluginAdvance); // Select -> Options
        assert_eq!(st.add_plugin.as_ref().unwrap().step, AddPluginStep::Options);
        // Check the biometric gate, then apply.
        update(&mut st, Message::AddPluginToggleFeature);
        let out = update(&mut st, Message::AddPluginAdvance);
        assert_eq!(
            out.effect,
            Some(Effect::AddPlugin {
                project_root: PathBuf::from("/tmp/huddle"),
                id: "secure-storage".to_string(),
                features: vec!["biometric-gate".to_string()],
            })
        );
        assert_eq!(
            st.add_plugin.as_ref().unwrap().step,
            AddPluginStep::Applying
        );
    }

    #[test]
    fn add_plugin_success_shows_report_and_toasts() {
        use frust_drive::plugin::{AddItem, AddOutcome, AddReport};

        let mut st = workbench_with_project();
        update(&mut st, Message::OpenAddPlugin);
        update(&mut st, Message::AddPluginAdvance); // into Options
        update(&mut st, Message::AddPluginAdvance); // into Applying (emits effect)
        let report = AddReport {
            plugin_id: "shared-preferences".to_string(),
            items: vec![AddItem {
                description: "Cargo.toml dependency `frust-shared-preferences`".to_string(),
                outcome: AddOutcome::Applied,
            }],
        };
        update(&mut st, Message::AddPluginSucceeded(report));
        assert_eq!(st.add_plugin.as_ref().unwrap().step, AddPluginStep::Report);
        assert!(!st.toasts.items.is_empty());
        assert_eq!(
            st.toasts.items.last().unwrap().text,
            "Added shared-preferences · 1 applied, 0 already present"
        );
        // Enter on the report closes the dialog.
        update(&mut st, Message::AddPluginAdvance);
        assert!(st.add_plugin.is_none());
    }

    /// A report carrying an [`AddOutcome::AppliedAtBuild`] item must toast
    /// "applied at build", never "already present" — the same truthful
    /// counting rule `crate::ui::views::add_plugin`'s `report_summary_line`
    /// uses, via the shared `AddReport::outcome_counts` (R0 toast cluster).
    #[test]
    fn add_plugin_success_toast_labels_applied_at_build_items_correctly() {
        use frust_drive::plugin::{AddItem, AddOutcome, AddReport};

        let mut st = workbench_with_project();
        update(&mut st, Message::OpenAddPlugin);
        update(&mut st, Message::AddPluginAdvance); // into Options
        update(&mut st, Message::AddPluginAdvance); // into Applying (emits effect)
        let report = AddReport {
            plugin_id: "desktop-test-plugin".to_string(),
            items: vec![
                AddItem {
                    description: "Cargo.toml dependency `frust-desktop-test-plugin`".to_string(),
                    outcome: AddOutcome::Applied,
                },
                AddItem {
                    description: "Info.plist key `NSSupportsSuddenTermination` (applied at `frust build macos`)"
                        .to_string(),
                    outcome: AddOutcome::AppliedAtBuild,
                },
            ],
        };
        update(&mut st, Message::AddPluginSucceeded(report));
        let text = &st.toasts.items.last().unwrap().text;
        assert_eq!(
            text,
            "Added desktop-test-plugin · 1 applied, 0 already present, 1 applied at build",
        );
        assert!(
            !text.contains("2 already present") && !text.contains("1 already present, 1 applied"),
            "an AppliedAtBuild item must never be lumped into 'already present': {text}"
        );
    }

    #[test]
    fn add_plugin_failure_lands_on_the_error_step() {
        let mut st = workbench_with_project();
        update(&mut st, Message::OpenAddPlugin);
        update(&mut st, Message::AddPluginAdvance);
        update(&mut st, Message::AddPluginAdvance);
        update(
            &mut st,
            Message::AddPluginFailed("no `frust` dependency".to_string()),
        );
        assert_eq!(st.add_plugin.as_ref().unwrap().step, AddPluginStep::Error);
        // Enter retries from the options step.
        update(&mut st, Message::AddPluginAdvance);
        assert_eq!(st.add_plugin.as_ref().unwrap().step, AddPluginStep::Options);
    }

    #[test]
    fn switch_project_focuses_the_new_projects_first_session_or_falls_back_to_none() {
        let mut st = multi_project_workbench();
        register(&mut st, 0, "/tmp/b", "desktop");
        register(&mut st, 1, "/tmp/b", "Pixel 7");
        // Registering auto-selected session 0 (project /tmp/b) as active even
        // though /tmp/a is still the nominal active project.
        assert_eq!(st.active_session, Some(0));

        // Switching to /tmp/a (no sessions there) drives the focused session
        // group to None — the dashboard, not a stale /tmp/b tab.
        update(&mut st, Message::SwitchProject(0));
        assert_eq!(st.active_session, None);

        // Switching to /tmp/b (which has sessions) focuses its first one.
        update(&mut st, Message::SwitchProject(1));
        assert_eq!(st.active_session, Some(0));
    }

    // ── Doctor panel + titlebar chip ────────────────────────────────────────

    use crate::engine::doctor::DoctorCheck;
    use frust_drive::doctor::Status;

    #[test]
    fn run_doctor_flags_refreshing_and_requests_the_effect() {
        let mut st = welcome();
        let out = update(&mut st, Message::RunDoctor);
        assert!(st.doctor.refreshing);
        assert_eq!(out.effect, Some(Effect::RunDoctor));
        assert!(out.redraw);
    }

    #[test]
    fn doctor_results_populate_and_clear_refreshing() {
        let mut st = welcome();
        update(&mut st, Message::RunDoctor);
        let results = vec![DoctorCheck {
            name: "Rust toolchain".to_string(),
            status: Status::Pass,
            messages: Vec::new(),
        }];
        update(&mut st, Message::DoctorResults(results.clone()));
        assert!(!st.doctor.refreshing);
        assert_eq!(st.doctor.results, results);
    }

    #[test]
    fn doctor_failure_toast_shows_correct_key() {
        let mut st = welcome();
        update(&mut st, Message::RunDoctor);
        let results = vec![DoctorCheck {
            name: "Rust toolchain".to_string(),
            status: Status::Fail,
            messages: vec!["Something is wrong".to_string()],
        }];
        update(&mut st, Message::DoctorResults(results));
        assert_eq!(st.toasts.items.len(), 1);
        let toast = &st.toasts.items[0];
        assert_eq!(toast.kind, ToastKind::Error);
        assert!(toast.text.contains("press i"), "{}", toast.text);
        assert!(!toast.text.contains("press d"), "{}", toast.text);
    }

    #[test]
    fn open_and_close_doctor_panel_toggles_and_dedupes() {
        let mut st = welcome();
        assert!(update(&mut st, Message::OpenDoctorPanel).redraw);
        assert!(st.doctor_panel_open);
        assert!(
            !update(&mut st, Message::OpenDoctorPanel).redraw,
            "already open"
        );
        assert!(update(&mut st, Message::CloseDoctorPanel).redraw);
        assert!(!st.doctor_panel_open);
        assert!(
            !update(&mut st, Message::CloseDoctorPanel).redraw,
            "already closed"
        );
    }

    /// Toolchain setup via the Doctor panel's `t` key: close the panel and
    /// open the bootstrap wizard in the same transition `OpenBootstrapWizard`
    /// performs.
    #[test]
    fn open_toolchain_from_doctor_closes_the_panel_and_opens_the_wizard() {
        let mut st = welcome();
        update(&mut st, Message::OpenDoctorPanel);
        assert!(st.doctor_panel_open);
        let out = update(&mut st, Message::OpenToolchainFromDoctor);
        assert!(out.redraw);
        assert!(!st.doctor_panel_open);
        assert!(st.bootstrap_wizard.is_some());
    }

    // ── Build launcher ──────────────────────────────────────────────────────

    use crate::engine::build_launcher::{ArtifactKind, BuildTargetSpec};

    #[test]
    fn open_build_launcher_primes_from_the_active_project() {
        let mut st = workbench_with_project();
        assert!(update(&mut st, Message::OpenBuildLauncher).redraw);
        let launcher = st.build_launcher.as_ref().expect("launcher open");
        assert_eq!(launcher.project_root, PathBuf::from("/tmp/huddle"));
        assert_eq!(launcher.kind, ArtifactKind::Apk);
        // A second open while already open is a no-op.
        assert!(!update(&mut st, Message::OpenBuildLauncher).redraw);
        assert!(update(&mut st, Message::CloseBuildLauncher).redraw);
        assert!(st.build_launcher.is_none());
    }

    #[test]
    fn open_build_launcher_is_a_noop_on_the_welcome_screen() {
        let mut st = welcome();
        assert!(!update(&mut st, Message::OpenBuildLauncher).redraw);
        assert!(st.build_launcher.is_none());
    }

    #[test]
    fn build_launcher_edits_route_through_the_focused_control() {
        let mut st = workbench_with_project();
        update(&mut st, Message::OpenBuildLauncher);
        update(&mut st, Message::BuildCycleKind(1));
        assert_eq!(
            st.build_launcher.as_ref().unwrap().kind,
            ArtifactKind::Appbundle
        );
        update(
            &mut st,
            Message::BuildFocus(crate::engine::BuildFocus::Flavor),
        );
        update(&mut st, Message::BuildInput('p'));
        update(&mut st, Message::BuildInput('q'));
        update(&mut st, Message::BuildBackspace);
        assert_eq!(st.build_launcher.as_ref().unwrap().flavor, "p");
    }

    #[test]
    fn build_launch_emits_the_resolved_spec_and_closes_the_modal() {
        let mut st = workbench_with_project();
        update(&mut st, Message::OpenBuildLauncher);
        let out = update(&mut st, Message::BuildLaunch);
        assert!(st.build_launcher.is_none(), "launch closes the modal");
        let Some(Effect::LaunchBuild(spec)) = out.effect else {
            panic!("expected a LaunchBuild effect, got {:?}", out.effect);
        };
        assert_eq!(spec.project_root, PathBuf::from("/tmp/huddle"));
        assert!(matches!(spec.target, BuildTargetSpec::Apk { .. }));
    }

    #[test]
    fn build_launch_with_no_modal_open_is_a_noop() {
        let mut st = workbench_with_project();
        assert_eq!(update(&mut st, Message::BuildLaunch).effect, None);
    }

    // ── Clean confirm dialog ────────────────────────────────────────────────

    #[test]
    fn open_clean_confirm_primes_from_the_active_project_and_confirm_emits_effect() {
        let mut st = workbench_with_project();
        assert!(update(&mut st, Message::OpenCleanConfirm).redraw);
        assert_eq!(st.clean_confirm, Some(PathBuf::from("/tmp/huddle")));
        let out = update(&mut st, Message::ConfirmClean);
        assert!(st.clean_confirm.is_none(), "confirming closes the dialog");
        assert_eq!(
            out.effect,
            Some(Effect::RunClean(PathBuf::from("/tmp/huddle")))
        );
    }

    #[test]
    fn close_clean_confirm_dismisses_without_an_effect() {
        let mut st = workbench_with_project();
        update(&mut st, Message::OpenCleanConfirm);
        let out = update(&mut st, Message::CloseCleanConfirm);
        assert!(out.redraw);
        assert!(st.clean_confirm.is_none());
        assert_eq!(update(&mut st, Message::ConfirmClean).effect, None);
    }

    // ── Quit confirm dialog ──────────────────────────────────────────────────

    #[test]
    fn request_quit_with_no_live_session_quits_immediately() {
        let mut st = workbench_with_project();
        assert_eq!(st.live_session_count(), 0);
        let out = update(&mut st, Message::RequestQuit);
        assert!(
            st.should_quit,
            "no live session — behaves exactly like Quit"
        );
        assert!(!st.quit_confirm);
        assert!(!out.redraw, "about to tear down, no point drawing a frame");
    }

    #[test]
    fn request_quit_with_a_live_session_opens_the_dialog_instead_of_quitting() {
        let mut st = workbench_with_project();
        register(&mut st, 1, "/tmp/huddle", "desktop");
        assert_eq!(st.live_session_count(), 1);
        let out = update(&mut st, Message::RequestQuit);
        assert!(!st.should_quit, "asks first rather than quitting outright");
        assert!(st.quit_confirm);
        assert!(out.redraw);
    }

    #[test]
    fn a_terminal_session_does_not_count_toward_request_quit() {
        let mut st = workbench_with_project();
        let id = register(&mut st, 1, "/tmp/huddle", "desktop");
        let idx = st.session_index(id).unwrap();
        st.sessions[idx].state = SessionState::Exited(true);
        assert_eq!(st.live_session_count(), 0);
        update(&mut st, Message::RequestQuit);
        assert!(
            st.should_quit,
            "an exited session never blocks a quit request"
        );
    }

    #[test]
    fn confirm_quit_quits_and_clears_the_dialog() {
        let mut st = workbench_with_project();
        register(&mut st, 1, "/tmp/huddle", "desktop");
        update(&mut st, Message::RequestQuit);
        assert!(st.quit_confirm);
        let out = update(&mut st, Message::ConfirmQuit);
        assert!(st.should_quit);
        assert!(!st.quit_confirm);
        assert!(!out.redraw);
    }

    #[test]
    fn close_quit_confirm_clears_the_flag_without_quitting() {
        let mut st = workbench_with_project();
        register(&mut st, 1, "/tmp/huddle", "desktop");
        update(&mut st, Message::RequestQuit);
        let out = update(&mut st, Message::CloseQuitConfirm);
        assert!(out.redraw);
        assert!(!st.quit_confirm);
        assert!(!st.should_quit);
    }

    // ── Build artifact copy-path ────────────────────────────────────────────

    #[test]
    fn copy_built_artifacts_emits_effect_only_when_the_active_session_built_something() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "build apk");
        assert_eq!(update(&mut st, Message::CopyBuiltArtifacts).effect, None);
        update(&mut st, line(a, "Built: /tmp/a/app.apk"));
        update(&mut st, line(a, "Built: /tmp/a/app2.apk"));
        let out = update(&mut st, Message::CopyBuiltArtifacts);
        assert_eq!(
            out.effect,
            Some(Effect::Copy("/tmp/a/app.apk\n/tmp/a/app2.apk".to_string()))
        );
    }

    // ── Bootstrap wizard + titlebar toolchain chip ──────────────────────────

    use crate::engine::bootstrap::tests::partial_report;
    use frust_drive::doctor::ComponentStatus;

    #[test]
    fn run_bootstrap_report_flags_refreshing_and_requests_the_effect() {
        let mut st = welcome();
        let out = update(&mut st, Message::RunBootstrapReport);
        assert!(st.bootstrap.refreshing);
        assert_eq!(out.effect, Some(Effect::RunBootstrapReport));
        assert!(out.redraw);
    }

    #[test]
    fn bootstrap_report_caches_rollup_and_refreshes_an_open_wizard() {
        let mut st = welcome();
        update(&mut st, Message::OpenBootstrapWizard); // opens empty, requests preflight
        assert!(st.bootstrap_wizard.is_some());
        update(&mut st, Message::BootstrapReport(partial_report()));
        assert!(!st.bootstrap.refreshing);
        assert_eq!(st.bootstrap.rollup(), Some(ComponentStatus::Partial));
        // The open wizard now projects the real report (core + Platforms +
        // 3 areas + Rollup = 6 nodes).
        assert_eq!(st.bootstrap_wizard.as_ref().unwrap().nodes().len(), 6);
    }

    #[test]
    fn open_from_a_cached_report_does_not_re_request_preflight() {
        let mut st = welcome();
        // Seed a cached report first.
        update(&mut st, Message::BootstrapReport(partial_report()));
        let out = update(&mut st, Message::OpenBootstrapWizard);
        assert!(st.bootstrap_wizard.is_some());
        assert_eq!(out.effect, None, "a cached report needs no re-run");
    }

    #[test]
    fn a_missing_core_auto_opens_the_wizard_once() {
        let mut st = welcome();
        let mut report = partial_report();
        report.areas[0].components[0].status = ComponentStatus::Missing;
        // First report with a Missing core auto-opens the wizard.
        update(&mut st, Message::BootstrapReport(report.clone()));
        assert!(st.bootstrap_wizard.is_some(), "fresh-machine auto-open");
        // Close it; a later re-preflight does NOT re-nag.
        update(&mut st, Message::CloseBootstrapWizard);
        update(&mut st, Message::BootstrapReport(report));
        assert!(
            st.bootstrap_wizard.is_none(),
            "auto-open fires at most once"
        );
    }

    #[test]
    fn a_green_core_never_auto_opens() {
        let mut st = welcome();
        update(&mut st, Message::BootstrapReport(partial_report()));
        assert!(st.bootstrap_wizard.is_none());
    }

    #[test]
    fn running_an_auto_runnable_fix_emits_a_command_effect_and_closes_the_wizard() {
        let mut st = welcome();
        update(&mut st, Message::BootstrapReport(partial_report()));
        update(&mut st, Message::OpenBootstrapWizard);
        // Navigate to the Android area (cursor 2) where the runnable
        // cargo-ndk fix lives; its fix cursor defaults to 0.
        update(&mut st, Message::BootstrapNavDown); // -> Platforms header (1)
        update(&mut st, Message::BootstrapNavDown); // -> Android (2)
        let out = update(&mut st, Message::BootstrapRunFix);
        assert!(
            st.bootstrap_wizard.is_none(),
            "running a fix closes the wizard"
        );
        assert_eq!(
            out.effect,
            Some(Effect::RunBootstrapCommand {
                program: "cargo".to_string(),
                args: vec!["install".to_string(), "cargo-ndk".to_string()],
                label: "cargo install cargo-ndk".to_string(),
            })
        );
    }

    #[test]
    fn running_a_guidance_only_fix_is_a_noop() {
        let mut st = welcome();
        update(&mut st, Message::BootstrapReport(partial_report()));
        update(&mut st, Message::OpenBootstrapWizard);
        update(&mut st, Message::BootstrapNavDown);
        update(&mut st, Message::BootstrapNavDown); // Android
        update(&mut st, Message::BootstrapFixDown); // -> the JDK guidance fix
        let out = update(&mut st, Message::BootstrapRunFix);
        assert_eq!(out.effect, None, "a guidance-only fix has nothing to run");
        assert!(st.bootstrap_wizard.is_some(), "and leaves the wizard open");
    }

    #[test]
    fn copy_fix_yields_the_command_line_or_doc_link() {
        let mut st = welcome();
        update(&mut st, Message::BootstrapReport(partial_report()));
        update(&mut st, Message::OpenBootstrapWizard);
        update(&mut st, Message::BootstrapNavDown);
        update(&mut st, Message::BootstrapNavDown); // Android, fix 0 = cargo-ndk
        let out = update(&mut st, Message::BootstrapCopyFix);
        assert_eq!(
            out.effect,
            Some(Effect::Copy("cargo install cargo-ndk".to_string()))
        );
        // The guidance fix copies its doc link instead.
        update(&mut st, Message::BootstrapFixDown);
        let out = update(&mut st, Message::BootstrapCopyFix);
        assert_eq!(
            out.effect,
            Some(Effect::Copy(
                "https://developer.android.com/studio".to_string()
            ))
        );
    }

    #[test]
    fn clicking_the_platforms_header_toggles_its_expansion() {
        let mut st = welcome();
        update(&mut st, Message::BootstrapReport(partial_report()));
        update(&mut st, Message::OpenBootstrapWizard);
        assert_eq!(st.bootstrap_wizard.as_ref().unwrap().nodes().len(), 6);
        // Header is node 1.
        update(&mut st, Message::BootstrapSelectStep(1));
        assert_eq!(
            st.bootstrap_wizard.as_ref().unwrap().nodes().len(),
            3,
            "clicking the header collapsed the platform leaves"
        );
    }

    // ── Drag-to-resize + scrollbar thumb ────────────────────────────────────

    use crate::engine::message::{ContextTarget, DragKind};

    #[test]
    fn sidebar_splitter_drag_resizes_and_clamps() {
        let mut st = workbench_with_project();
        assert_eq!(st.sidebar_width, crate::engine::SIDEBAR_DEFAULT_WIDTH);
        // Body origin at column 0; a DragStart records the active drag.
        update(
            &mut st,
            Message::DragStart(DragKind::SidebarSplitter { body_left: 0 }),
        );
        assert!(st.active_drag.is_some());
        // Dragging to column 40 sets the width to 40 (within bounds).
        assert!(update(&mut st, Message::DragMove(40, 10)).redraw);
        assert_eq!(st.sidebar_width, 40);
        // Dragging way out clamps to the max, not past it.
        update(&mut st, Message::DragMove(500, 10));
        assert_eq!(st.sidebar_width, crate::engine::SIDEBAR_MAX_WIDTH);
        // And below the min clamps up.
        update(&mut st, Message::DragMove(2, 10));
        assert_eq!(st.sidebar_width, crate::engine::SIDEBAR_MIN_WIDTH);
        // Release clears the active drag.
        update(&mut st, Message::DragEnd);
        assert!(st.active_drag.is_none());
        // A stray move with no active drag is a no-op.
        assert!(!update(&mut st, Message::DragMove(30, 10)).redraw);
    }

    #[test]
    fn scrollbar_thumb_drag_moves_the_log_anchor() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        for i in 0..100 {
            update(&mut st, line(a, &format!("line {i}")));
        }
        assert!(st.active_session().unwrap().is_following());
        update(
            &mut st,
            Message::DragStart(DragKind::LogScrollbar {
                track_top: 5,
                track_height: 21, // rows 5..=25
            }),
        );
        // Drag to the top of the track → oldest line anchored.
        update(&mut st, Message::DragMove(0, 5));
        assert!(matches!(
            st.active_session().unwrap().scroll,
            Scroll::Anchored(0)
        ));
        // Drag to the bottom → follow re-engaged.
        update(&mut st, Message::DragMove(0, 25));
        assert!(st.active_session().unwrap().is_following());
        update(&mut st, Message::DragEnd);
    }

    // ── Context menus ───────────────────────────────────────────────────────

    #[test]
    fn opening_a_session_tab_menu_focuses_the_tab_and_builds_entries() {
        let mut st = welcome();
        register(&mut st, 0, "/tmp/a", "desktop");
        register(&mut st, 1, "/tmp/b", "desktop");
        assert_eq!(st.active_session, Some(0));
        // Right-clicking tab 1 focuses it (mouse parity) and opens the menu.
        let out = update(
            &mut st,
            Message::OpenContextMenu {
                x: 4,
                y: 2,
                target: ContextTarget::SessionTab(1),
            },
        );
        assert!(out.redraw);
        assert_eq!(st.active_session, Some(1));
        let menu = st.context_menu.as_ref().expect("menu open");
        assert!(!menu.entries.is_empty());
    }

    #[test]
    fn context_menu_nav_activate_redispatches_and_closes() {
        let mut st = welcome();
        register(&mut st, 0, "/tmp/a", "desktop");
        // Log-view menu, clicked below the last row (no row under the
        // cursor): Copy line (disabled) / Copy selection (disabled, nothing
        // selected) / Select lines… / Toggle follow-tail / Search.
        update(
            &mut st,
            Message::OpenContextMenu {
                x: 10,
                y: 10,
                target: ContextTarget::LogView { row: None },
            },
        );
        // Move to "Toggle follow-tail" (index 3) and activate it.
        for _ in 0..3 {
            update(&mut st, Message::ContextMenuCursorDown);
        }
        let following_before = st.active_session().unwrap().is_following();
        let out = update(&mut st, Message::ContextMenuActivate);
        assert!(out.redraw);
        assert!(st.context_menu.is_none(), "activation closes the menu");
        assert_ne!(
            st.active_session().unwrap().is_following(),
            following_before,
            "the entry re-dispatched ToggleFollow"
        );
    }

    #[test]
    fn activating_a_disabled_entry_is_a_noop_and_keeps_the_menu_open() {
        let mut st = welcome();
        register(&mut st, 0, "/tmp/a", "desktop");
        update(
            &mut st,
            Message::OpenContextMenu {
                x: 10,
                y: 10,
                target: ContextTarget::LogView { row: None },
            },
        );
        // Index 0 is "Copy line", disabled with no row under the cursor.
        let out = update(&mut st, Message::ContextMenuActivateAt(0));
        assert!(!out.redraw);
        assert!(st.context_menu.is_some(), "a disabled entry doesn't close");
    }

    #[test]
    fn close_context_menu_clears_it() {
        let mut st = welcome();
        st.projects = vec![PathBuf::from("/tmp/a")];
        st.project_root = Some(PathBuf::from("/tmp/a"));
        update(
            &mut st,
            Message::OpenContextMenu {
                x: 1,
                y: 1,
                target: ContextTarget::ProjectRow(0),
            },
        );
        assert!(st.context_menu.is_some());
        assert!(update(&mut st, Message::CloseContextMenu).redraw);
        assert!(st.context_menu.is_none());
        // Closing an already-closed menu is idle.
        assert!(!update(&mut st, Message::CloseContextMenu).redraw);
    }

    #[test]
    fn empty_target_menu_does_not_open() {
        let mut st = welcome();
        // No sessions → a session-tab menu has no entries → no menu opens.
        let out = update(
            &mut st,
            Message::OpenContextMenu {
                x: 0,
                y: 0,
                target: ContextTarget::SessionTab(0),
            },
        );
        assert!(out.redraw);
        assert!(st.context_menu.is_none());
    }

    // ── Mouse-capture toggle ────────────────────────────────────────────────

    #[test]
    fn toggle_mouse_capture_flips_state_and_requests_the_effect() {
        let mut st = welcome();
        assert!(st.mouse_capture);
        let out = update(&mut st, Message::ToggleMouseCapture);
        assert!(!st.mouse_capture);
        assert_eq!(out.effect, Some(Effect::SetMouseCapture(false)));
        assert!(out.redraw);
        let out = update(&mut st, Message::ToggleMouseCapture);
        assert!(st.mouse_capture);
        assert_eq!(out.effect, Some(Effect::SetMouseCapture(true)));
    }

    // ── Settings persistence ─────────────────────────────────────────────────

    #[test]
    fn ending_a_sidebar_splitter_drag_requests_the_save_effect() {
        let mut st = workbench_with_project();
        update(
            &mut st,
            Message::DragStart(DragKind::SidebarSplitter { body_left: 0 }),
        );
        update(&mut st, Message::DragMove(40, 10));
        let out = update(&mut st, Message::DragEnd);
        assert_eq!(out.effect, Some(Effect::SaveSidebarWidth(40)));
        assert!(st.active_drag.is_none());
    }

    #[test]
    fn ending_a_scrollbar_drag_requests_no_save_effect() {
        let mut st = welcome();
        register(&mut st, 0, "/tmp/a", "desktop");
        update(
            &mut st,
            Message::DragStart(DragKind::LogScrollbar {
                track_top: 0,
                track_height: 10,
            }),
        );
        let out = update(&mut st, Message::DragEnd);
        assert_eq!(out.effect, None, "only a sidebar-width drag persists");
    }

    #[test]
    fn ending_a_drag_with_none_active_requests_no_effect() {
        let mut st = workbench_with_project();
        let out = update(&mut st, Message::DragEnd);
        assert_eq!(out.effect, None);
    }

    #[test]
    fn toggling_follow_is_purely_per_session_and_requests_no_persistence_effect() {
        let mut st = welcome();
        register(&mut st, 0, "/tmp/a", "desktop");
        assert!(st.active_session().unwrap().is_following());
        let out = update(&mut st, Message::ToggleFollow);
        assert!(!st.active_session().unwrap().is_following());
        assert_eq!(
            out.effect, None,
            "follow-tail is in-memory, per-session state now — there is no \
             persisted global default to save"
        );

        let out = update(&mut st, Message::ToggleFollow);
        assert!(st.active_session().unwrap().is_following());
        assert_eq!(out.effect, None);
    }

    #[test]
    fn toggling_follow_on_one_session_does_not_affect_another() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        register(&mut st, 1, "/tmp/b", "desktop");
        let a_idx = st.session_index(a).unwrap();
        // `a` is active first (auto-selected on register); disengage its follow.
        update(&mut st, Message::ToggleFollow);
        assert!(!st.sessions[a_idx].is_following());

        // Switch to the second session — untouched, still following.
        update(&mut st, Message::SelectTab(1));
        assert!(
            st.active_session().unwrap().is_following(),
            "session b was never toggled"
        );
        assert!(
            !st.sessions[a_idx].is_following(),
            "toggling the now-inactive session b must not re-engage a's follow"
        );
    }

    #[test]
    fn a_freshly_registered_session_always_starts_following() {
        let mut st = welcome();
        register(&mut st, 0, "/tmp/a", "desktop");
        assert!(
            st.active_session().unwrap().is_following(),
            "every session starts following its own tail — there is no \
             persisted global default to seed a different starting state"
        );
    }

    #[test]
    fn a_freshly_registered_session_tracks_the_tail_as_lines_arrive() {
        let mut st = welcome();
        let a = register(&mut st, 0, "/tmp/a", "desktop");
        for i in 0..500 {
            update(&mut st, line(a, &format!("line {i}")));
        }
        let session = st.active_session().unwrap();
        assert!(
            session.is_following(),
            "a session that was never scrolled stays in Follow, tracking the \
             tail as lines arrive rather than freezing at the first line"
        );
        assert_eq!(session.log.end_index(), 500);
    }

    // ── Perf sparkline panel ─────────────────────────────────────────────────

    #[test]
    fn toggle_perf_panel_flips_the_active_session_only() {
        let mut st = welcome();
        register(&mut st, 0, "/tmp/a", "desktop");
        assert!(!st.active_session().unwrap().perf.visible);
        let out = update(&mut st, Message::TogglePerfPanel);
        assert!(out.redraw);
        assert!(st.active_session().unwrap().perf.visible);
    }

    #[test]
    fn toggle_perf_panel_with_no_active_session_is_a_noop() {
        let mut st = welcome();
        let out = update(&mut st, Message::TogglePerfPanel);
        assert!(!out.redraw);
    }

    // ── DevTools mode (workbook §B12) ────────────────────────────────────────

    const DISCOVERY: &str = "frust-devtools listening on 53214 token cafe";

    fn debug_launch() -> DevtoolsLaunch {
        DevtoolsLaunch::from_launch(frust_drive::build_info::BuildMode::Debug, None)
    }

    /// A workbench with one running, devtools-capable desktop session.
    fn devtools_workbench() -> (AppState, SessionId) {
        let mut st = workbench_with_project();
        let id = register_with(
            &mut st,
            0,
            "/tmp/huddle",
            "desktop",
            debug_launch(),
            Some(SessionTarget::Desktop),
        );
        update(&mut st, state_event(id, SessionState::Running));
        (st, id)
    }

    fn state_event(id: SessionId, state: SessionState) -> Message {
        Message::Session(SessionEvent {
            id,
            kind: SessionEventKind::State(state),
        })
    }

    fn connect_target(effect: Option<Effect>) -> DevtoolsTarget {
        match effect {
            Some(Effect::DevtoolsConnect(target)) => target,
            other => panic!("expected a DevtoolsConnect effect, got {other:?}"),
        }
    }

    #[test]
    fn toggling_devtools_before_any_discovery_opens_the_mode_without_connecting() {
        let (mut st, _) = devtools_workbench();
        let out = update(&mut st, Message::DevtoolsToggle);
        assert!(out.redraw);
        assert!(st.active_session().unwrap().devtools.open);
        assert_eq!(out.effect, None, "nothing discovered yet to connect to");
        assert_eq!(
            st.active_session().unwrap().devtools.phase(),
            DevtoolsPhase::Discovering
        );

        // Toggling back off leaves the mode closed, no effect either way.
        let out = update(&mut st, Message::DevtoolsToggle);
        assert!(out.redraw);
        assert!(!st.active_session().unwrap().devtools.open);
        assert_eq!(out.effect, None);
    }

    #[test]
    fn a_discovery_line_while_devtools_is_open_starts_the_connection() {
        let (mut st, id) = devtools_workbench();
        update(&mut st, Message::DevtoolsToggle);
        let out = update(&mut st, line(id, DISCOVERY));
        let target = connect_target(out.effect);
        assert_eq!(target.session, id);
        assert_eq!(target.port, 53214);
        assert_eq!(target.token.as_deref(), Some("cafe"));
        assert_eq!(target.android_serial, None);
        assert_eq!(
            st.active_session().unwrap().devtools.phase(),
            DevtoolsPhase::Connecting
        );

        // A re-announcement (the app restarted within the session) replaces
        // the connection rather than being ignored — latest wins.
        let out = update(
            &mut st,
            line(id, "frust-devtools listening on 60001 token beef"),
        );
        let target = connect_target(out.effect);
        assert_eq!(target.port, 60001);
        assert_eq!(target.token.as_deref(), Some("beef"));
    }

    #[test]
    fn a_discovery_line_is_recorded_but_not_connected_while_devtools_is_closed() {
        let (mut st, id) = devtools_workbench();
        let out = update(&mut st, line(id, DISCOVERY));
        assert_eq!(out.effect, None, "nobody is looking yet");
        assert!(st.active_session().unwrap().devtools.discovered.is_some());

        // Opening is what connects, using the line that already scrolled past.
        let out = update(&mut st, Message::DevtoolsToggle);
        assert_eq!(connect_target(out.effect).port, 53214);
    }

    #[test]
    fn an_android_session_carries_its_serial_into_the_connect_target() {
        let mut st = workbench_with_project();
        let launch = DevtoolsLaunch::from_launch(
            frust_drive::build_info::BuildMode::Profile,
            Some("emulator-5554".to_string()),
        );
        let id = register_with(
            &mut st,
            0,
            "/tmp/huddle",
            "Pixel 8",
            launch,
            Some(SessionTarget::Device {
                id: "serial-8".to_string(),
                name: "Pixel 8".to_string(),
                platform: frust_drive::devices::Platform::Android,
            }),
        );
        update(&mut st, line(id, DISCOVERY));
        let out = update(&mut st, Message::DevtoolsToggle);
        assert_eq!(
            connect_target(out.effect).android_serial.as_deref(),
            Some("emulator-5554")
        );
    }

    #[test]
    fn a_release_session_never_connects_and_shows_the_unavailable_screen() {
        let mut st = workbench_with_project();
        let launch = DevtoolsLaunch::from_launch(frust_drive::build_info::BuildMode::Release, None);
        let id = register_with(
            &mut st,
            0,
            "/tmp/huddle",
            "desktop",
            launch,
            Some(SessionTarget::Desktop),
        );
        update(&mut st, line(id, DISCOVERY));
        let out = update(&mut st, Message::DevtoolsToggle);
        assert_eq!(out.effect, None);
        assert_eq!(
            st.active_session().unwrap().devtools.phase(),
            DevtoolsPhase::Unavailable
        );
    }

    #[test]
    fn tabs_switch_by_index_and_cycle() {
        let (mut st, _) = devtools_workbench();
        update(&mut st, Message::DevtoolsToggle);
        let out = update(&mut st, Message::DevtoolsTab(1));
        assert!(out.redraw);
        assert_eq!(
            st.active_session().unwrap().devtools.active_tab,
            DevtoolsTab::System
        );
        update(&mut st, Message::DevtoolsTabCycle(1));
        assert_eq!(
            st.active_session().unwrap().devtools.active_tab,
            DevtoolsTab::Inspector
        );
        update(&mut st, Message::DevtoolsTabCycle(-2));
        assert_eq!(
            st.active_session().unwrap().devtools.active_tab,
            DevtoolsTab::Performance
        );
        // Wrapping backward from the first tab lands on the last.
        update(&mut st, Message::DevtoolsTabCycle(-1));
        assert_eq!(
            st.active_session().unwrap().devtools.active_tab,
            DevtoolsTab::Network
        );
    }

    #[test]
    fn conn_events_drive_the_state_and_only_dirty_a_visible_surface() {
        let (mut st, id) = devtools_workbench();
        update(&mut st, Message::DevtoolsToggle);
        update(&mut st, line(id, DISCOVERY));

        let out = update(
            &mut st,
            Message::DevtoolsConn(
                id,
                ConnEvent::Connected {
                    app_name: "huddle".to_string(),
                    caps: Vec::new(),
                },
            ),
        );
        assert!(out.redraw);
        assert_eq!(
            st.active_session().unwrap().devtools.phase(),
            DevtoolsPhase::Connected
        );

        // With the mode closed again, a background report keeps filling the
        // ring but never dirties a frame (the dirty-frame skip).
        update(&mut st, Message::DevtoolsClose);
        let out = update(
            &mut st,
            Message::DevtoolsConn(id, ConnEvent::Frames(vec![frame_stats(1)])),
        );
        assert!(!out.redraw);
        assert_eq!(st.active_session().unwrap().devtools.frames.len(), 1);
    }

    #[test]
    fn the_frame_ring_caps_at_its_bound_across_batches() {
        let (mut st, id) = devtools_workbench();
        // Batches sized like the bridge's coalescing window forwards them.
        for batch in 0..10u64 {
            let frames = (0..40).map(|i| frame_stats(batch * 40 + i)).collect();
            update(
                &mut st,
                Message::DevtoolsConn(id, ConnEvent::Frames(frames)),
            );
        }
        let ring = &st.active_session().unwrap().devtools.frames;
        assert_eq!(ring.len(), crate::engine::FRAME_RING_CAP);
        assert_eq!(
            ring.back().unwrap().n,
            399,
            "the newest sample is always retained"
        );
    }

    #[test]
    fn retry_reconnects_from_the_failed_state_but_not_after_the_session_ended() {
        let (mut st, id) = devtools_workbench();
        update(&mut st, Message::DevtoolsToggle);
        update(&mut st, line(id, DISCOVERY));
        update(
            &mut st,
            Message::DevtoolsConn(id, ConnEvent::Failed("connection refused".into())),
        );
        assert_eq!(
            st.active_session().unwrap().devtools.phase(),
            DevtoolsPhase::Failed
        );

        let out = update(&mut st, Message::DevtoolsRetry);
        assert_eq!(connect_target(out.effect).port, 53214);
        assert_eq!(
            st.active_session().unwrap().devtools.phase(),
            DevtoolsPhase::Connecting
        );

        // Once the session itself has ended there is no service left to reach.
        update(&mut st, state_event(id, SessionState::Exited(true)));
        let out = update(&mut st, Message::DevtoolsRetry);
        assert_eq!(out.effect, None);
    }

    #[test]
    fn a_session_ending_tears_its_devtools_connection_down() {
        let (mut st, id) = devtools_workbench();
        update(&mut st, Message::DevtoolsToggle);
        update(&mut st, line(id, DISCOVERY));
        update(
            &mut st,
            Message::DevtoolsConn(
                id,
                ConnEvent::Connected {
                    app_name: "huddle".to_string(),
                    caps: Vec::new(),
                },
            ),
        );

        let out = update(&mut st, state_event(id, SessionState::Exited(true)));
        assert_eq!(out.effect, Some(Effect::DevtoolsDisconnect(id)));
        assert_eq!(
            st.active_session().unwrap().devtools.phase(),
            DevtoolsPhase::Failed,
            "the service went with the process"
        );
    }

    #[test]
    fn devtools_messages_are_noops_with_no_active_session() {
        let mut st = welcome();
        for msg in [
            Message::DevtoolsToggle,
            Message::DevtoolsClose,
            Message::DevtoolsTab(2),
            Message::DevtoolsTabCycle(1),
            Message::DevtoolsRetry,
        ] {
            let out = update(&mut st, msg.clone());
            assert!(!out.redraw, "{msg:?} should be a no-op");
            assert_eq!(out.effect, None);
        }
    }

    fn frame_stats(n: u64) -> frust_devtools_protocol::FrameStats {
        frust_devtools_protocol::FrameStats {
            n,
            total_us: 16_000,
            rebuild_us: 8_000,
            layout_us: 3_000,
            paint_us: 2_000,
            encode_us: 1_400,
            acquire_us: 600,
            submit_us: 1_000,
            skipped: false,
        }
    }

    // ── Performance tab (workbook §B12) ──────────────────────────────────────

    #[test]
    fn scrub_and_select_route_through_the_active_sessions_live_ring() {
        let (mut st, id) = devtools_workbench();
        update(&mut st, Message::DevtoolsToggle);
        let frames = (0..30).map(frame_stats).collect();
        update(
            &mut st,
            Message::DevtoolsConn(id, ConnEvent::Frames(frames)),
        );

        let out = update(&mut st, Message::DevtoolsPerfScrub(0));
        assert!(out.redraw);
        assert_eq!(
            pinned_n(&st),
            Some(29),
            "the first scrub starts at the tail of the 30-frame window"
        );

        update(&mut st, Message::DevtoolsPerfScrub(-1));
        assert_eq!(pinned_n(&st), Some(28));

        update(&mut st, Message::DevtoolsPerfSelectFrame(0));
        assert_eq!(pinned_n(&st), Some(0));
    }

    /// The active session's pinned Performance frame (`FrameStats::n`).
    fn pinned_n(state: &AppState) -> Option<u64> {
        state
            .active_session()
            .unwrap()
            .devtools
            .performance
            .selected_n
    }

    /// What the Performance tab would actually *render* as the pinned frame:
    /// the pin resolved against the window on screen, exactly as
    /// `ui::views::devtools::performance` does it. The header/breakdown read
    /// this, so it is the assertion that catches a pin whose identity walks.
    fn rendered_pin_n(state: &AppState) -> Option<u64> {
        let session = state.active_session().unwrap();
        let window = devtools_perf_window(session);
        let index = session.devtools.performance.resolve(&window)?;
        Some(window[index].n)
    }

    #[test]
    fn a_pinned_frame_survives_later_frame_batches_unchanged() {
        let (mut st, id) = devtools_workbench();
        update(&mut st, Message::DevtoolsToggle);
        // Enough frames that the drawn window is already *sliding* (it only
        // starts to once the ring passes `PERF_WINDOW`) — the condition the
        // walk needs.
        let win = crate::engine::PERF_WINDOW as u64;
        let frames = (0..win + 10).map(frame_stats).collect();
        update(
            &mut st,
            Message::DevtoolsConn(id, ConnEvent::Frames(frames)),
        );
        // Scrub back off the tail onto a specific frame and hold it.
        update(&mut st, Message::DevtoolsPerfScrub(0));
        update(&mut st, Message::DevtoolsPerfScrub(-29));
        let pinned = win + 10 - 1 - 29;
        assert_eq!(pinned_n(&st), Some(pinned));
        assert_eq!(rendered_pin_n(&st), Some(pinned));

        // Ten more frames arrive, each sliding the window by one. The pin
        // must still name the same frame — a positional pin would name a
        // frame ten later by the end of this loop, so the header and
        // breakdown would have walked off the spike with no key pressed.
        for n in (win + 10)..(win + 20) {
            update(
                &mut st,
                Message::DevtoolsConn(id, ConnEvent::Frames(vec![frame_stats(n)])),
            );
            assert_eq!(pinned_n(&st), Some(pinned), "after frame {n}");
            assert_eq!(
                rendered_pin_n(&st),
                Some(pinned),
                "the header/breakdown still show frame #{pinned} after frame {n}"
            );
        }
    }

    #[test]
    fn a_pin_that_slides_out_of_the_window_drops_back_to_live() {
        let (mut st, id) = devtools_workbench();
        update(&mut st, Message::DevtoolsToggle);
        let frames = (0..30).map(frame_stats).collect();
        update(
            &mut st,
            Message::DevtoolsConn(id, ConnEvent::Frames(frames)),
        );
        update(&mut st, Message::DevtoolsPerfSelectFrame(0));
        assert_eq!(pinned_n(&st), Some(0));

        // Push the window (`PERF_WINDOW` frames) past frame 0 entirely.
        let batch: Vec<_> = (30..(30 + crate::engine::PERF_WINDOW as u64))
            .map(frame_stats)
            .collect();
        update(&mut st, Message::DevtoolsConn(id, ConnEvent::Frames(batch)));
        assert_eq!(
            pinned_n(&st),
            None,
            "the pinned frame's data is gone from the window — back to live"
        );
    }

    #[test]
    fn a_chart_click_naming_a_departed_frame_pins_nothing() {
        let (mut st, id) = devtools_workbench();
        update(&mut st, Message::DevtoolsToggle);
        let frames = (0..30).map(frame_stats).collect();
        update(
            &mut st,
            Message::DevtoolsConn(id, ConnEvent::Frames(frames)),
        );
        update(&mut st, Message::DevtoolsPerfSelectFrame(25));

        // 100 more frames: the window is now frames 10..=129, so the pin
        // (25) is still retained but frame 0 has left.
        let batch: Vec<_> = (30..130).map(frame_stats).collect();
        update(&mut st, Message::DevtoolsConn(id, ConnEvent::Frames(batch)));
        assert_eq!(pinned_n(&st), Some(25));

        // A click registered against a column drawing frame 0, consumed
        // after the window moved past it (message-level: the payload is the
        // frame itself, so the race is decidable here).
        update(&mut st, Message::DevtoolsPerfSelectFrame(0));
        assert_eq!(
            pinned_n(&st),
            Some(25),
            "a stale click pins nothing rather than whatever slid into that column"
        );
    }

    #[test]
    fn focus_cycle_toggles_chart_and_breakdown() {
        let (mut st, _) = devtools_workbench();
        update(&mut st, Message::DevtoolsToggle);
        assert_eq!(
            st.active_session().unwrap().devtools.performance.focus,
            PerfFocus::Chart
        );
        let out = update(&mut st, Message::DevtoolsPerfFocusCycle);
        assert!(out.redraw);
        assert_eq!(
            st.active_session().unwrap().devtools.performance.focus,
            PerfFocus::Breakdown
        );
    }

    #[test]
    fn clear_selection_drops_back_to_the_live_tail() {
        let (mut st, id) = devtools_workbench();
        update(&mut st, Message::DevtoolsToggle);
        let frames = (0..5).map(frame_stats).collect();
        update(
            &mut st,
            Message::DevtoolsConn(id, ConnEvent::Frames(frames)),
        );
        update(&mut st, Message::DevtoolsPerfScrub(0));
        assert!(
            st.active_session()
                .unwrap()
                .devtools
                .performance
                .has_selection()
        );

        update(&mut st, Message::DevtoolsPerfClearSelection);
        assert!(
            !st.active_session()
                .unwrap()
                .devtools
                .performance
                .has_selection()
        );
    }

    #[test]
    fn performance_messages_are_noops_with_no_active_session() {
        let mut st = welcome();
        for msg in [
            Message::DevtoolsPerfScrub(1),
            Message::DevtoolsPerfSelectFrame(0),
            Message::DevtoolsPerfClearSelection,
            Message::DevtoolsPerfFocusCycle,
        ] {
            let out = update(&mut st, msg.clone());
            assert!(!out.redraw, "{msg:?} should be a no-op");
        }
    }

    // ── Inspector tab (workbook §B12) ────────────────────────────────────────

    fn widget_node(
        id: u64,
        type_name: &str,
        children: Vec<frust_devtools_protocol::WidgetNode>,
    ) -> frust_devtools_protocol::WidgetNode {
        frust_devtools_protocol::WidgetNode {
            id,
            type_name: type_name.to_string(),
            debug_label: None,
            bounds: None,
            children,
        }
    }

    /// `Column #1 > [Padding #2 > Text #3, Text #4]`.
    fn widget_tree() -> frust_devtools_protocol::WidgetTreeDump {
        frust_devtools_protocol::WidgetTreeDump {
            roots: vec![widget_node(
                1,
                "frust_widgets::flex::FlexWidget",
                vec![
                    widget_node(
                        2,
                        "frust_widgets::padding::PaddingWidget",
                        vec![widget_node(
                            3,
                            "frust_widgets::text::TextWidget",
                            Vec::new(),
                        )],
                    ),
                    widget_node(4, "frust_widgets::text::TextWidget", Vec::new()),
                ],
            )],
        }
    }

    /// A connected DevTools session sitting on the Inspector tab, with the
    /// automatic first pull already answered.
    fn inspector_workbench() -> (AppState, SessionId) {
        let (mut st, id) = devtools_workbench();
        update(&mut st, Message::DevtoolsToggle);
        update(
            &mut st,
            Message::DevtoolsConn(
                id,
                ConnEvent::Connected {
                    app_name: "huddle".to_string(),
                    caps: Vec::new(),
                },
            ),
        );
        let out = update(&mut st, Message::DevtoolsTab(2));
        assert_eq!(
            out.effect,
            Some(Effect::DevtoolsFetchTree { session: id }),
            "entering the Inspector tab pulls its first snapshot"
        );
        update(
            &mut st,
            Message::DevtoolsInspector(id, InspectorEvent::TreeArrived(widget_tree())),
        );
        (st, id)
    }

    #[test]
    fn entering_the_inspector_tab_pulls_one_snapshot_and_never_a_second() {
        let (mut st, id) = inspector_workbench();
        assert_eq!(
            st.active_session().unwrap().devtools.inspector.rows().len(),
            4,
            "Column > [Padding > Text, Text] — the whole tree is inside the \
             default expansion depth"
        );

        // Leaving and re-entering the tab does not re-pull — `r` is the
        // explicit refresh.
        update(&mut st, Message::DevtoolsTab(0));
        let out = update(&mut st, Message::DevtoolsTab(2));
        assert_eq!(out.effect, None);

        let out = update(&mut st, Message::DevtoolsInspectorRefresh);
        assert_eq!(out.effect, Some(Effect::DevtoolsFetchTree { session: id }));
        // …and a second `r` while that pull is in flight is a no-op.
        assert_eq!(
            update(&mut st, Message::DevtoolsInspectorRefresh).effect,
            None
        );
    }

    #[test]
    fn a_handshake_landing_on_the_inspector_tab_pulls_the_first_snapshot() {
        let (mut st, id) = devtools_workbench();
        update(&mut st, Message::DevtoolsToggle);
        update(&mut st, Message::DevtoolsTab(2));
        // Nothing to pull while the connection is still coming up.
        assert_eq!(
            update(&mut st, Message::DevtoolsInspectorRefresh).effect,
            None
        );

        let out = update(
            &mut st,
            Message::DevtoolsConn(
                id,
                ConnEvent::Connected {
                    app_name: "huddle".to_string(),
                    caps: Vec::new(),
                },
            ),
        );
        assert_eq!(out.effect, Some(Effect::DevtoolsFetchTree { session: id }));
    }

    /// A connected session on the Inspector tab whose first automatic pull
    /// has *failed* — the storm precondition.
    fn failed_first_pull() -> (AppState, SessionId) {
        let (mut st, id) = devtools_workbench();
        update(&mut st, Message::DevtoolsToggle);
        update(
            &mut st,
            Message::DevtoolsConn(
                id,
                ConnEvent::Connected {
                    app_name: "huddle".to_string(),
                    caps: Vec::new(),
                },
            ),
        );
        let out = update(&mut st, Message::DevtoolsTab(2));
        assert_eq!(out.effect, Some(Effect::DevtoolsFetchTree { session: id }));
        update(
            &mut st,
            Message::DevtoolsInspector(id, InspectorEvent::Failed("request timed out".to_string())),
        );
        (st, id)
    }

    #[test]
    fn a_failed_inspector_pull_is_never_re_fired_by_arriving_frame_batches() {
        let (mut st, id) = failed_first_pull();

        // Frame-stats batches keep arriving at the bridge's coalescing rate
        // while the Inspector sits open on a failed pull. Not one of them
        // may re-issue the request: pre-fix, every batch re-ran the tab-entry
        // check, whose gate had re-opened the moment the failure cleared
        // `tree_pending` — an unbounded request storm, each attempt blocking
        // the bridge pump for a request timeout.
        for n in 0..64 {
            let out = update(
                &mut st,
                Message::DevtoolsConn(id, ConnEvent::Frames(vec![frame_stats(n)])),
            );
            assert_eq!(out.effect, None, "frame batch {n} re-fired the pull");
        }
        // A repeated connection report is not a loophole either.
        for _ in 0..4 {
            let out = update(
                &mut st,
                Message::DevtoolsConn(
                    id,
                    ConnEvent::Connected {
                        app_name: "huddle".to_string(),
                        caps: Vec::new(),
                    },
                ),
            );
            assert_eq!(out.effect, None);
        }
    }

    #[test]
    fn an_explicit_refresh_or_a_tab_re_entry_retries_a_failed_pull() {
        let (mut st, id) = failed_first_pull();

        // `r` is the explicit re-trigger.
        let out = update(&mut st, Message::DevtoolsInspectorRefresh);
        assert_eq!(out.effect, Some(Effect::DevtoolsFetchTree { session: id }));
        update(
            &mut st,
            Message::DevtoolsInspector(id, InspectorEvent::Failed("request timed out".to_string())),
        );

        // So is leaving and re-entering the tab — one attempt per entry,
        // bounded by the user's own keypresses.
        update(&mut st, Message::DevtoolsTab(0));
        let out = update(&mut st, Message::DevtoolsTab(2));
        assert_eq!(out.effect, Some(Effect::DevtoolsFetchTree { session: id }));
        update(
            &mut st,
            Message::DevtoolsInspector(id, InspectorEvent::Failed("request timed out".to_string())),
        );

        // Closing and re-opening DevTools onto the tab is the third entry
        // route (§B12's re-open onto a retained connection).
        update(&mut st, Message::DevtoolsClose);
        let out = update(&mut st, Message::DevtoolsToggle);
        assert_eq!(out.effect, Some(Effect::DevtoolsFetchTree { session: id }));
    }

    #[test]
    fn moving_the_selection_requests_the_new_nodes_props_exactly_once() {
        let (mut st, id) = inspector_workbench();
        // The snapshot's own arrival asked for the root's props.
        let out = update(
            &mut st,
            Message::DevtoolsInspector(id, InspectorEvent::TreeArrived(widget_tree())),
        );
        assert_eq!(
            out.effect,
            Some(Effect::DevtoolsFetchProps { session: id, id: 1 })
        );
        update(
            &mut st,
            Message::DevtoolsInspector(
                id,
                InspectorEvent::PropsArrived(
                    1,
                    frust_devtools_protocol::WidgetProps {
                        id: 1,
                        entries: vec![("axis".to_string(), "Vertical".to_string())],
                    },
                ),
            ),
        );

        // Down onto Padding #2: a fresh pull.
        let out = update(&mut st, Message::DevtoolsInspectorSelect(1));
        assert!(out.redraw);
        assert_eq!(
            out.effect,
            Some(Effect::DevtoolsFetchProps { session: id, id: 2 })
        );
        // Straight back up onto the cached root: no pull at all.
        let out = update(&mut st, Message::DevtoolsInspectorSelect(-1));
        assert_eq!(out.effect, None, "the root's props are already cached");
        assert_eq!(
            st.active_session()
                .unwrap()
                .devtools
                .inspector
                .selected_props()
                .map(|p| p.id),
            Some(1)
        );
    }

    #[test]
    fn expand_and_collapse_route_to_the_active_sessions_tree() {
        let (mut st, id) = inspector_workbench();
        // Collapse the root: only it stays visible.
        update(&mut st, Message::DevtoolsInspectorCollapse);
        assert_eq!(
            st.active_session().unwrap().devtools.inspector.rows().len(),
            1
        );
        update(&mut st, Message::DevtoolsInspectorExpand);
        assert_eq!(
            st.active_session().unwrap().devtools.inspector.rows().len(),
            4
        );
        // The `▸`/`▾` click affordance addresses a node by id: collapsing
        // Padding #2 hides only its own child.
        update(&mut st, Message::DevtoolsInspectorToggleNode(2));
        assert_eq!(
            st.active_session().unwrap().devtools.inspector.rows().len(),
            3
        );

        // A failure from the bridge surfaces verbatim and frees the pull.
        update(
            &mut st,
            Message::DevtoolsInspector(id, InspectorEvent::Failed("connection closed".to_string())),
        );
        assert_eq!(
            st.active_session().unwrap().devtools.inspector.error(),
            Some("connection closed")
        );
    }

    #[test]
    fn inspector_messages_are_noops_with_no_active_session() {
        let mut st = welcome();
        for msg in [
            Message::DevtoolsInspectorSelect(1),
            Message::DevtoolsInspectorSelectRow(0),
            Message::DevtoolsInspectorExpand,
            Message::DevtoolsInspectorCollapse,
            Message::DevtoolsInspectorToggleNode(1),
            Message::DevtoolsInspectorFocusCycle,
            Message::DevtoolsInspectorRefresh,
        ] {
            let out = update(&mut st, msg.clone());
            assert!(!out.redraw, "{msg:?} should be a no-op");
            assert_eq!(out.effect, None, "{msg:?} should request nothing");
        }
    }

    // ── Responsive breakpoints ───────────────────────────────────────────────

    #[test]
    fn toggle_sidebar_overlay_flips_and_redraws() {
        let mut st = workbench_with_project();
        assert!(!st.sidebar_overlay_open);
        let out = update(&mut st, Message::ToggleSidebarOverlay);
        assert!(out.redraw);
        assert!(st.sidebar_overlay_open);
        update(&mut st, Message::ToggleSidebarOverlay);
        assert!(!st.sidebar_overlay_open);
    }

    // ── Help overlay ──────────────────────────────────────────────────────────

    #[test]
    fn open_and_close_help_overlay() {
        let mut st = welcome();
        assert!(!st.help_open);
        let out = update(&mut st, Message::OpenHelpOverlay);
        assert!(out.redraw);
        assert!(st.help_open);
        assert!(matches!(
            st.active_modal(),
            Some(crate::engine::ActiveModal::HelpOverlay)
        ));
        let out = update(&mut st, Message::CloseHelpOverlay);
        assert!(out.redraw);
        assert!(!st.help_open);
    }

    #[test]
    fn re_opening_an_already_open_help_overlay_is_a_noop() {
        let mut st = welcome();
        update(&mut st, Message::OpenHelpOverlay);
        let out = update(&mut st, Message::OpenHelpOverlay);
        assert!(!out.redraw);
    }

    // ── Embedded MCP server ───────────────────────────────────────────────────

    /// A handle for a server of `generation` that was "started" without
    /// spawning anything — enough to exercise the two reports the server
    /// sends back.
    fn mcp_handle(generation: u64) -> crate::supervise::McpServerHandle {
        crate::supervise::McpServerHandle::starting(
            generation,
            tokio_util::sync::CancellationToken::new(),
            frust_mcp::ClientRegistry::new(),
        )
    }

    #[test]
    fn the_bound_port_report_promotes_starting_to_listening() {
        let mut st = welcome();
        st.mcp = Some(mcp_handle(0));
        assert_eq!(st.mcp_status(), crate::supervise::McpStatus::Starting);

        let out = update(&mut st, Message::McpListening(0, 4848));
        assert!(out.redraw);
        assert_eq!(
            st.mcp_status(),
            crate::supervise::McpStatus::Listening {
                port: 4848,
                clients: 0
            }
        );
        // The same report again changes nothing.
        assert!(!update(&mut st, Message::McpListening(0, 4848)).redraw);
    }

    #[test]
    fn a_stopped_server_clears_the_handle_and_an_error_toasts() {
        let mut st = welcome();
        st.mcp = Some(mcp_handle(0));
        let out = update(&mut st, Message::McpStopped(0, None));
        assert!(out.redraw);
        assert_eq!(st.mcp_status(), crate::supervise::McpStatus::Stopped);
        assert!(st.toasts.items.is_empty(), "a clean stop is not an error");

        st.mcp = Some(mcp_handle(1));
        update(
            &mut st,
            Message::McpStopped(1, Some("address already in use".to_string())),
        );
        assert_eq!(st.mcp_status(), crate::supervise::McpStatus::Stopped);
        assert_eq!(st.toasts.items.len(), 1);
        assert!(st.toasts.items[0].text.contains("address already in use"));
        assert_eq!(
            st.mcp_error.as_deref(),
            Some("address already in use"),
            "the reason is retained for the sidebar row / panel, not just toasted"
        );
    }

    /// The §B13 toggle: one message, two directions, each pushed out as the
    /// effect only the runner can enact.
    #[test]
    fn the_toggle_starts_a_stopped_server_and_stops_a_running_one() {
        let mut st = welcome();
        let out = update(&mut st, Message::ToggleMcpServer);
        assert!(out.redraw);
        assert_eq!(out.effect, Some(Effect::StartMcpServer));
        assert!(
            st.mcp.is_none(),
            "the pure core never builds the live server handle itself"
        );

        st.mcp = Some(mcp_handle(0));
        let out = update(&mut st, Message::ToggleMcpServer);
        assert!(out.redraw);
        assert_eq!(out.effect, Some(Effect::StopMcpServer));
    }

    /// The race the generation tag exists for: the user
    /// stops server A and immediately starts B, while A is still inside
    /// graceful shutdown. A's late reports name a generation the model no
    /// longer holds and must not touch B — clearing B's handle would drop its
    /// only `CancellationToken`, and dropping one does **not** cancel it, so
    /// B would be left listening with nothing able to stop it and every later
    /// start failing on the address.
    #[test]
    fn a_superseded_servers_late_reports_never_touch_its_successor() {
        let mut st = welcome();
        // A (generation 0) was stopped — `stop_mcp` took its handle — and B
        // (generation 1) was started in the window before A's task wound down.
        st.mcp = Some(mcp_handle(1));
        update(&mut st, Message::McpListening(1, 4848));

        // A's stale bound-port report does not restamp B's port…
        let out = update(&mut st, Message::McpListening(0, 9999));
        assert!(!out.redraw);
        assert_eq!(
            st.mcp_status(),
            crate::supervise::McpStatus::Listening {
                port: 4848,
                clients: 0
            }
        );

        // …and A's stale stop report leaves B installed, listening, and
        // stoppable, with no error surfaced against it.
        let out = update(&mut st, Message::McpStopped(0, None));
        assert!(!out.redraw);
        let out = update(
            &mut st,
            Message::McpStopped(0, Some("address already in use".to_string())),
        );
        assert!(!out.redraw);
        assert_eq!(
            st.mcp_status(),
            crate::supervise::McpStatus::Listening {
                port: 4848,
                clients: 0
            }
        );
        assert!(st.toasts.items.is_empty());
        assert_eq!(st.mcp_error, None);
        assert_eq!(
            st.mcp.as_ref().map(|handle| handle.generation()),
            Some(1),
            "B's own handle — the one carrying its cancellation token — is still installed"
        );
        // Which is what keeps B stoppable: the toggle still reaches it, and
        // the handle the runner would cancel is B's.
        assert_eq!(
            update(&mut st, Message::ToggleMcpServer).effect,
            Some(Effect::StopMcpServer)
        );

        // B's *own* stop report, by contrast, is applied.
        assert!(update(&mut st, Message::McpStopped(1, None)).redraw);
        assert_eq!(st.mcp_status(), crate::supervise::McpStatus::Stopped);
    }

    /// A failed start stays visible until the next attempt — and the next
    /// attempt clears it, so a retry never renders "starting…" beside the
    /// previous run's reason.
    #[test]
    fn requesting_a_start_clears_the_previous_failure_reason() {
        let mut st = welcome();
        st.mcp = Some(mcp_handle(0));
        update(&mut st, Message::McpStopped(0, Some("boom".to_string())));
        assert_eq!(st.mcp_error.as_deref(), Some("boom"));

        update(&mut st, Message::ToggleMcpServer);
        assert_eq!(st.mcp_error, None);
    }

    // ── Embedded DAP server ───────────────────────────────────────────────────

    /// A handle for a DAP server of `generation` that was "started" without
    /// spawning anything — enough to exercise the two reports it sends back.
    fn dap_handle(generation: u64) -> crate::supervise::DapServerHandle {
        crate::supervise::DapServerHandle::starting(
            generation,
            tokio_util::sync::CancellationToken::new(),
            frust_dap::DapClientRegistry::new(),
        )
    }

    #[test]
    fn the_dap_bound_port_report_promotes_starting_to_listening() {
        let mut st = welcome();
        st.dap = Some(dap_handle(0));
        assert_eq!(st.dap_status(), crate::supervise::DapStatus::Starting);

        let out = update(&mut st, Message::DapListening(0, 4849));
        assert!(out.redraw);
        assert_eq!(
            st.dap_status(),
            crate::supervise::DapStatus::Listening {
                port: 4849,
                clients: 0
            }
        );
        assert!(!update(&mut st, Message::DapListening(0, 4849)).redraw);
    }

    /// The same generation gate as the MCP pair, and for the same reason: a
    /// superseded server's late report must neither restamp nor clear its
    /// successor's handle.
    #[test]
    fn a_superseded_dap_servers_late_reports_never_touch_its_successor() {
        let mut st = welcome();
        st.dap = Some(dap_handle(1));
        update(&mut st, Message::DapListening(1, 4849));

        let out = update(&mut st, Message::DapListening(0, 9999));
        assert!(!out.redraw);
        assert_eq!(
            st.dap_status(),
            crate::supervise::DapStatus::Listening {
                port: 4849,
                clients: 0
            }
        );

        let out = update(&mut st, Message::DapStopped(0, Some("boom".to_string())));
        assert!(!out.redraw);
        assert_eq!(
            st.dap.as_ref().map(|handle| handle.generation()),
            Some(1),
            "B's own handle — the one carrying its cancellation token — is still installed"
        );
        assert!(st.toasts.items.is_empty());
        assert_eq!(st.dap_error, None);

        // B's *own* stop report, by contrast, is applied.
        assert!(update(&mut st, Message::DapStopped(1, None)).redraw);
        assert_eq!(st.dap_status(), crate::supervise::DapStatus::Stopped);
    }

    #[test]
    fn a_failed_dap_server_toasts_and_retains_its_reason() {
        let mut st = welcome();
        st.dap = Some(dap_handle(0));
        update(
            &mut st,
            Message::DapStopped(0, Some("address already in use".to_string())),
        );
        assert_eq!(st.dap_status(), crate::supervise::DapStatus::Stopped);
        assert_eq!(st.toasts.items.len(), 1);
        assert!(st.toasts.items[0].text.contains("address already in use"));
        assert_eq!(
            st.dap_error.as_deref(),
            Some("address already in use"),
            "the reason is retained for the UI, not just toasted"
        );
    }

    // ── DAP settings dialog ───────────────────────────────────────────────────

    /// A workbench with a project open and a known detected IDE — the shape
    /// every generation path below needs.
    fn dap_workbench(detected: Option<frust_dap::ide_config::ParentIde>) -> AppState {
        let mut st = workbench_with_project();
        st.dap_settings.detected_ide = detected;
        st
    }

    #[test]
    fn the_dap_dialog_opens_reset_and_closes_committing_the_port() {
        let mut st = dap_workbench(None);
        assert!(update(&mut st, Message::OpenDapSettings).redraw);
        assert!(st.dap_settings_open);
        assert!(matches!(
            st.active_modal(),
            Some(crate::engine::ActiveModal::DapSettings(_))
        ));
        assert!(
            !update(&mut st, Message::OpenDapSettings).redraw,
            "re-opening an open dialog is a no-op"
        );

        // Type a new port and close: the edit is committed and persisted on
        // the way out rather than evaporating.
        update(&mut st, Message::DapSettingsFocus(DapFocus::Port));
        for _ in 0..5 {
            update(&mut st, Message::DapSettingsBackspace);
        }
        for c in "5005".chars() {
            update(&mut st, Message::DapSettingsInput(c));
        }
        let out = update(&mut st, Message::CloseDapSettings);
        assert!(!st.dap_settings_open);
        assert_eq!(st.dap_settings.port, 5005);
        assert_eq!(
            out.effect,
            Some(Effect::SaveDapSetting(DapSetting::Port(5005)))
        );
        assert!(
            !update(&mut st, Message::CloseDapSettings).redraw,
            "closing a closed dialog is a no-op"
        );
    }

    #[test]
    fn a_rejected_port_keeps_the_committed_one_and_warns_instead_of_persisting() {
        let mut st = dap_workbench(None);
        update(&mut st, Message::OpenDapSettings);
        update(&mut st, Message::DapSettingsFocus(DapFocus::Port));
        for c in "abc".chars() {
            update(&mut st, Message::DapSettingsInput(c));
        }
        let out = update(&mut st, Message::DapSettingsActivate);
        assert_eq!(out.effect, None, "nothing invalid is ever persisted");
        assert_eq!(st.dap_settings.port, crate::engine::DEFAULT_DAP_PORT);
        assert_eq!(st.dap_settings.port_input, "4849");
        assert_eq!(st.toasts.items.len(), 1);
        assert!(st.toasts.items[0].text.contains("not a valid port"));
    }

    /// Each control's activation produces the effect that control means —
    /// the dialog's whole contract with the runner.
    #[test]
    fn activating_each_control_produces_its_own_effect() {
        let mut st = dap_workbench(Some(frust_dap::ide_config::ParentIde::VSCode));
        update(&mut st, Message::OpenDapSettings);

        // Server (focus starts here): start, then stop.
        assert_eq!(st.dap_settings.focus, DapFocus::Server);
        let out = update(&mut st, Message::DapSettingsActivate);
        assert_eq!(
            out.effect,
            Some(Effect::StartDapServer {
                port: crate::engine::DEFAULT_DAP_PORT
            })
        );
        st.dap = Some(dap_handle(0));
        let out = update(&mut st, Message::DapSettingsActivate);
        assert_eq!(out.effect, Some(Effect::StopDapServer));
        st.dap = None;

        // Checkboxes persist immediately, each its own key.
        update(&mut st, Message::DapSettingsFocus(DapFocus::AutoStart));
        let out = update(&mut st, Message::DapSettingsActivate);
        assert!(!st.dap_settings.auto_start_in_ide);
        assert_eq!(
            out.effect,
            Some(Effect::SaveDapSetting(DapSetting::AutoStartInIde(false)))
        );
        update(&mut st, Message::DapSettingsFocus(DapFocus::AutoConfigure));
        let out = update(&mut st, Message::DapSettingsActivate);
        assert!(!st.dap_settings.auto_configure_ide);
        assert_eq!(
            out.effect,
            Some(Effect::SaveDapSetting(DapSetting::AutoConfigureIde(false)))
        );

        // The IDE selector cycles off `detected` onto the first override.
        update(&mut st, Message::DapSettingsFocus(DapFocus::Ide));
        let out = update(&mut st, Message::DapSettingsActivate);
        assert_eq!(
            st.dap_settings.ide_override,
            Some(crate::engine::IDE_OVERRIDES[0])
        );
        assert_eq!(
            out.effect,
            Some(Effect::SaveDapSetting(DapSetting::IdeOverride(Some(
                crate::engine::IDE_OVERRIDES[0]
            ))))
        );

        // Generate asks the runner for the file I/O, never doing it here.
        update(&mut st, Message::DapSettingsFocus(DapFocus::Generate));
        let out = update(&mut st, Message::DapSettingsActivate);
        assert_eq!(
            out.effect,
            Some(Effect::GenerateIdeConfig(crate::engine::IdeConfigRequest {
                ide: crate::engine::IDE_OVERRIDES[0],
                port: crate::engine::DEFAULT_DAP_PORT,
                project_root: PathBuf::from("/tmp/huddle"),
                mode: WriteMode::Refresh,
            }))
        );
    }

    #[test]
    fn focus_walks_the_dialog_in_both_directions() {
        let mut st = dap_workbench(None);
        update(&mut st, Message::OpenDapSettings);
        assert_eq!(st.dap_settings.focus, DapFocus::Server);
        update(&mut st, Message::DapSettingsFocusNext);
        assert_eq!(st.dap_settings.focus, DapFocus::Port);
        update(&mut st, Message::DapSettingsFocusPrev);
        assert_eq!(st.dap_settings.focus, DapFocus::Server);
        update(&mut st, Message::DapSettingsFocusPrev);
        assert_eq!(st.dap_settings.focus, DapFocus::Generate);
    }

    /// The auto-start decision, driven entirely through injected state: no
    /// process environment is read or mutated here.
    ///
    /// `intro_seen` is set deliberately on every case: this is the
    /// already-acknowledged install, where auto-start is the silent start it
    /// has always been. The first-run gate has its own tests below.
    #[test]
    fn the_startup_auto_start_truth_table_produces_the_start_effect() {
        let ide = Some(frust_dap::ide_config::ParentIde::VSCode);
        let cases = [
            // (enabled, auto_start_in_ide, detected, starts?)
            (true, false, None, true),
            (false, true, ide, true),
            (false, true, None, false),
            (false, false, ide, false),
            (false, false, None, false),
        ];
        for (enabled, auto, detected, starts) in cases {
            let mut st = dap_workbench(detected);
            st.dap_settings.enabled = enabled;
            st.dap_settings.auto_start_in_ide = auto;
            st.dap_settings.port = 5005;
            st.dap_settings.intro_seen = true;
            let out = update(&mut st, Message::DapAutoStart);
            let expected = starts.then_some(Effect::StartDapServer { port: 5005 });
            assert_eq!(
                out.effect, expected,
                "enabled={enabled} auto={auto} detected={detected:?}"
            );
            assert!(
                !st.dap_settings_open,
                "an acknowledged install never re-opens the dialog"
            );
            assert_eq!(st.dap_settings.intro_port, None);
        }
    }

    #[test]
    fn auto_start_never_starts_a_second_server_over_a_running_one() {
        let mut st = dap_workbench(Some(frust_dap::ide_config::ParentIde::VSCode));
        st.dap_settings.intro_seen = true;
        st.dap = Some(dap_handle(0));
        assert_eq!(update(&mut st, Message::DapAutoStart).effect, None);
    }

    /// The first auto-start on a fresh install opens the dialog with the
    /// notice and starts *nothing* — the whole point of the gate is that no
    /// listener binds until the user says so on this one run.
    #[test]
    fn the_first_ever_auto_start_shows_the_notice_instead_of_binding_a_listener() {
        let mut st = dap_workbench(Some(frust_dap::ide_config::ParentIde::VSCode));
        st.dap_settings.port = 5005;
        assert!(!st.dap_settings.intro_seen, "a fresh install");

        let out = update(&mut st, Message::DapAutoStart);
        assert!(out.redraw);
        assert_eq!(
            out.effect,
            Some(Effect::SaveDapSetting(DapSetting::IntroSeen(true))),
            "the notice is spent immediately, so quitting does not bring it back"
        );
        assert!(
            st.dap_settings_open,
            "the dialog is how the notice is shown"
        );
        assert_eq!(st.dap_settings.intro_port, Some(5005));
        assert!(st.dap_settings.intro_seen);
        assert!(
            st.dap_settings.intro_notice().unwrap().contains("5005"),
            "the notice names the port that would have been bound"
        );
        assert!(st.dap.is_none(), "nothing was started");

        // The user answers by starting it: the notice is done, and this is
        // the only start on this run.
        let out = update(&mut st, Message::DapSettingsActivate);
        assert_eq!(out.effect, Some(Effect::StartDapServer { port: 5005 }));
        assert_eq!(st.dap_settings.intro_port, None);
        assert_eq!(st.dap_settings.intro_notice(), None);
    }

    /// Dismissing the dialog is the other answer: still no listener, and the
    /// notice is gone for good.
    #[test]
    fn dismissing_the_first_run_notice_starts_nothing_and_never_repeats_it() {
        let mut st = dap_workbench(Some(frust_dap::ide_config::ParentIde::VSCode));
        update(&mut st, Message::DapAutoStart);
        update(&mut st, Message::CloseDapSettings);
        assert!(!st.dap_settings_open);
        assert_eq!(st.dap_settings.intro_port, None);

        // A second `DapAutoStart` (the next launch, with `intro_seen` reloaded
        // from disk) is the silent start the preferences ask for.
        let out = update(&mut st, Message::DapAutoStart);
        assert_eq!(
            out.effect,
            Some(Effect::StartDapServer {
                port: crate::engine::DEFAULT_DAP_PORT
            })
        );
        assert!(!st.dap_settings_open, "and no second notice");
    }

    /// The gate spends `intro_seen` only when it actually fires: a launch
    /// outside an IDE, with auto-start wanted by nobody, leaves the notice
    /// unspent for the first launch *inside* one.
    #[test]
    fn a_launch_that_would_not_auto_start_never_spends_the_notice() {
        let mut st = dap_workbench(None);
        st.dap_settings.enabled = false;
        st.dap_settings.auto_start_in_ide = false;

        let out = update(&mut st, Message::DapAutoStart);
        assert_eq!(out.effect, None);
        assert!(!out.redraw);
        assert!(!st.dap_settings_open);
        assert!(!st.dap_settings.intro_seen, "unspent");
        assert_eq!(st.dap_settings.intro_port, None);

        // Now the same install, launched inside an IDE terminal: the notice
        // is still there to be shown.
        st.dap_settings.auto_start_in_ide = true;
        st.dap_settings.detected_ide = Some(frust_dap::ide_config::ParentIde::VSCode);
        let out = update(&mut st, Message::DapAutoStart);
        assert_eq!(
            out.effect,
            Some(Effect::SaveDapSetting(DapSetting::IntroSeen(true)))
        );
        assert!(st.dap_settings_open);
    }

    /// A bind with no session writes nothing: the workbench's active project
    /// at startup is merely the first `frust.toml` found, not a project
    /// anyone launched.
    #[test]
    fn a_fresh_listener_without_a_session_writes_no_ide_config() {
        let mut st = dap_workbench(Some(frust_dap::ide_config::ParentIde::Zed));
        assert!(st.dap_settings.auto_configure_ide);
        st.dap = Some(dap_handle(0));
        let out = update(&mut st, Message::DapListening(0, 41_234));
        assert!(out.redraw, "the port still promotes the handle");
        assert_eq!(out.effect, None);
        assert_eq!(st.dap_settings.last_ide_config, None);
    }

    /// The auto-configure-on-bind path end to end at the message level: with
    /// a session active, its project (not the workbench's) is configured,
    /// the bound port (not the configured one) is what the editor is pointed
    /// at, an existing entry is never rewritten, and the outcome the runner
    /// reports back is retained for the dialog.
    #[test]
    fn a_fresh_listener_configures_the_active_sessions_project_and_stores_the_outcome() {
        let mut st = dap_workbench(Some(frust_dap::ide_config::ParentIde::VSCode));
        register_on(&mut st, 0, "/tmp/other-app", SessionTarget::Desktop);
        assert!(st.active_session().is_some());
        st.dap = Some(dap_handle(0));
        // Started on `0`: the OS-assigned port is only known here.
        let out = update(&mut st, Message::DapListening(0, 41_234));
        assert_eq!(
            out.effect,
            Some(Effect::GenerateIdeConfig(crate::engine::IdeConfigRequest {
                ide: frust_dap::ide_config::ParentIde::VSCode,
                port: 41_234,
                project_root: PathBuf::from("/tmp/other-app"),
                mode: WriteMode::IfAbsent,
            })),
            "the config must name the port actually bound, in the session's project"
        );
        assert_eq!(
            update(&mut st, Message::DapListening(0, 41_234)).effect,
            None,
            "a repeat report for the same port is not a second bind"
        );

        let result = frust_dap::ide_config::IdeConfigResult {
            path: PathBuf::from("/tmp/huddle/.vscode/launch.json"),
            action: frust_dap::ide_config::ConfigAction::Created,
        };
        let report = crate::engine::DapIdeReport::Written {
            ide: frust_dap::ide_config::ParentIde::VSCode,
            result,
        };
        let out = update(&mut st, Message::DapIdeConfig(report.clone()));
        assert!(out.redraw);
        assert_eq!(st.dap_settings.last_ide_config, Some(report));
        assert_eq!(st.toasts.items.len(), 1);
        assert!(st.toasts.items[0].text.contains("created"));
    }

    #[test]
    fn a_failed_generation_is_retained_and_surfaced_as_an_error() {
        let mut st = dap_workbench(None);
        let report = crate::engine::DapIdeReport::Failed("permission denied".to_string());
        update(&mut st, Message::DapIdeConfig(report.clone()));
        assert_eq!(st.dap_settings.last_ide_config, Some(report));
        assert_eq!(st.toasts.items[0].kind, ToastKind::Error);
    }

    /// An automatic report, as the runner posts it: tagged with its project
    /// and IDE.
    fn auto_report(
        root: &str,
        ide: frust_dap::ide_config::ParentIde,
        report: crate::engine::DapIdeReport,
    ) -> crate::engine::DapIdeReport {
        crate::engine::DapIdeReport::Auto {
            project_root: PathBuf::from(root),
            ide,
            report: Box::new(report),
        }
    }

    fn stale_report(root: &str) -> crate::engine::DapIdeReport {
        crate::engine::DapIdeReport::Written {
            ide: frust_dap::ide_config::ParentIde::VSCode,
            result: frust_dap::ide_config::IdeConfigResult {
                path: PathBuf::from(root).join(".vscode/launch.json"),
                action: frust_dap::ide_config::ConfigAction::StalePort {
                    existing: 1111,
                    bound: 41_234,
                },
            },
        }
    }

    /// A kept entry naming a stale port warns once per (root, IDE) per run,
    /// naming the file, both ports and the fix — and never again on a repeat,
    /// while the dialog still shows the latest outcome.
    #[test]
    fn an_automatic_stale_port_toasts_once_per_root_and_ide() {
        use frust_dap::ide_config::ParentIde;
        let mut st = dap_workbench(Some(ParentIde::VSCode));
        let stale = stale_report("/tmp/a");

        let out = update(
            &mut st,
            Message::DapIdeConfig(auto_report("/tmp/a", ParentIde::VSCode, stale.clone())),
        );
        assert!(out.redraw);
        assert_eq!(st.toasts.items.len(), 1);
        let toast = &st.toasts.items[0];
        assert_eq!(toast.kind, ToastKind::Warn);
        // Extract the path from the stale report to get the exact rendering
        let config_path_str =
            if let crate::engine::DapIdeReport::Written { ide: _, result } = &stale {
                result.path.display().to_string()
            } else {
                panic!("stale report is not Written variant")
            };
        assert!(
            toast.text.contains(&config_path_str),
            "Config path '{}' not found in toast text: {}",
            config_path_str,
            toast.text
        );
        assert!(toast.text.contains("1111"), "{}", toast.text);
        assert!(toast.text.contains("41234"), "{}", toast.text);
        assert!(
            toast.text.contains("press g in DAP settings to refresh"),
            "{}",
            toast.text
        );
        assert_eq!(
            st.dap_settings.last_ide_config,
            Some(stale.clone()),
            "the dialog stores the unwrapped report"
        );

        // The next launch / bind: same pair, no second toast.
        for _ in 0..3 {
            update(
                &mut st,
                Message::DapIdeConfig(auto_report("/tmp/a", ParentIde::VSCode, stale.clone())),
            );
        }
        assert_eq!(st.toasts.items.len(), 1, "a repeat never toasts again");
        assert_eq!(st.dap_settings.last_ide_config, Some(stale));

        // Another project is its own pair.
        update(
            &mut st,
            Message::DapIdeConfig(auto_report(
                "/tmp/b",
                ParentIde::VSCode,
                stale_report("/tmp/b"),
            )),
        );
        assert_eq!(st.toasts.items.len(), 2);
    }

    /// An automatic failure (e.g. an unparseable existing config) errors once
    /// per (root, IDE) per run instead of on every launch.
    #[test]
    fn an_automatic_failure_toasts_once_per_root_and_ide() {
        use frust_dap::ide_config::ParentIde;
        let mut st = dap_workbench(Some(ParentIde::Zed));
        let failed = crate::engine::DapIdeReport::Failed("invalid JSON in debug.json".to_string());
        for _ in 0..3 {
            update(
                &mut st,
                Message::DapIdeConfig(auto_report("/tmp/a", ParentIde::Zed, failed.clone())),
            );
        }
        assert_eq!(st.toasts.items.len(), 1);
        assert_eq!(st.toasts.items[0].kind, ToastKind::Error);
        assert_eq!(st.dap_settings.last_ide_config, Some(failed.clone()));

        update(
            &mut st,
            Message::DapIdeConfig(auto_report("/tmp/a", ParentIde::Emacs, failed)),
        );
        assert_eq!(st.toasts.items.len(), 2, "another IDE is its own pair");
    }

    /// The explicit `g` path's reports arrive untagged and always toast —
    /// even an outcome the automatic path has already spent its toast on.
    #[test]
    fn the_explicit_path_reports_every_outcome_every_time() {
        use frust_dap::ide_config::ParentIde;
        let mut st = dap_workbench(Some(ParentIde::VSCode));
        let failed = crate::engine::DapIdeReport::Failed("permission denied".to_string());
        update(
            &mut st,
            Message::DapIdeConfig(auto_report("/tmp/a", ParentIde::VSCode, failed.clone())),
        );
        assert_eq!(st.toasts.items.len(), 1);
        st.toasts.items.clear(); // (the toast queue holds at most three)
        for _ in 0..2 {
            update(&mut st, Message::DapIdeConfig(failed.clone()));
        }
        let skipped = crate::engine::DapIdeReport::Written {
            ide: ParentIde::VSCode,
            result: frust_dap::ide_config::IdeConfigResult {
                path: PathBuf::from("/tmp/a/.vscode/launch.json"),
                action: frust_dap::ide_config::ConfigAction::Skipped(
                    "content unchanged".to_string(),
                ),
            },
        };
        update(&mut st, Message::DapIdeConfig(skipped.clone()));
        assert_eq!(st.toasts.items.len(), 3);
        assert_eq!(st.toasts.items[0].kind, ToastKind::Error);
        assert_eq!(st.toasts.items[1].kind, ToastKind::Error);
        assert!(
            st.toasts.items[2]
                .text
                .contains("skipped (content unchanged)")
        );
        assert_eq!(st.dap_settings.last_ide_config, Some(skipped));
    }

    #[test]
    fn notify_pushes_the_given_toast_kind_and_text() {
        let mut st = welcome();
        let out = update(
            &mut st,
            Message::Notify {
                level: ToastKind::Warn,
                text: "Copy failed: system clipboard unavailable".to_string(),
            },
        );
        assert!(out.redraw);
        assert_eq!(st.toasts.items.len(), 1);
        assert_eq!(st.toasts.items[0].kind, ToastKind::Warn);
        assert_eq!(
            st.toasts.items[0].text,
            "Copy failed: system clipboard unavailable"
        );
    }

    #[test]
    fn auto_configure_off_leaves_a_fresh_listener_alone() {
        let mut st = dap_workbench(Some(frust_dap::ide_config::ParentIde::VSCode));
        register_on(&mut st, 0, "/tmp/huddle", SessionTarget::Desktop);
        st.dap_settings.auto_configure_ide = false;
        st.dap = Some(dap_handle(0));
        let out = update(&mut st, Message::DapListening(0, 4849));
        assert!(out.redraw, "the port still promotes the handle");
        assert_eq!(out.effect, None);
        assert_eq!(st.dap_settings.last_ide_config, None);
    }

    /// The automatic path's refusals are silent: no effect, no stored
    /// report, no toast — unlike the explicit "generate now".
    #[test]
    fn auto_configure_refusals_are_silent() {
        for detected in [None, Some(frust_dap::ide_config::ParentIde::IntelliJ)] {
            let mut st = dap_workbench(detected);
            register_on(&mut st, 0, "/tmp/huddle", SessionTarget::Desktop);
            st.dap = Some(dap_handle(0));
            let out = update(&mut st, Message::DapListening(0, 4849));
            assert_eq!(out.effect, None, "detected={detected:?}");
            assert_eq!(st.dap_settings.last_ide_config, None);
            assert!(st.toasts.items.is_empty());

            let out = launch_from_run_config(&mut st);
            assert!(
                matches!(out.effect, Some(Effect::LaunchSessions(_))),
                "a refused config never holds up the launch, got {:?}",
                out.effect
            );
            assert_eq!(st.dap_settings.last_ide_config, None);
        }
    }

    /// Stop the one registered session (so its target is free again) and
    /// launch the desktop target from the run-config modal.
    fn launch_from_run_config(st: &mut AppState) -> Outcome {
        let ids: Vec<SessionId> = st.sessions.iter().map(|s| s.id).collect();
        for session in ids {
            update(
                st,
                Message::Session(SessionEvent {
                    id: session,
                    kind: SessionEventKind::State(crate::supervise::SessionState::Exited(true)),
                }),
            );
        }
        update(st, Message::OpenRunConfig); // desktop checked by default
        update(st, Message::RunConfigLaunch)
    }

    /// A launch while the server listens (auto-configure on) also writes the
    /// launched project's config, never rewriting an existing entry.
    #[test]
    fn a_launch_with_dap_listening_batches_an_ide_config_for_the_launched_project() {
        let mut st = dap_workbench(Some(frust_dap::ide_config::ParentIde::Zed));
        st.dap = Some(dap_handle(0));
        update(&mut st, Message::DapListening(0, 41_234));
        update(&mut st, Message::OpenRunConfig);

        let out = update(&mut st, Message::RunConfigLaunch);

        let Some(Effect::Batch(effects)) = out.effect else {
            panic!("expected a Batch, got {:?}", out.effect);
        };
        assert_eq!(effects.len(), 2);
        let Effect::LaunchSessions(specs) = &effects[0] else {
            panic!("the launch comes first, got {:?}", effects[0]);
        };
        assert_eq!(specs.len(), 1);
        assert_eq!(
            effects[1],
            Effect::GenerateIdeConfig(crate::engine::IdeConfigRequest {
                ide: frust_dap::ide_config::ParentIde::Zed,
                port: 41_234,
                project_root: specs[0].project_root.clone(),
                mode: WriteMode::IfAbsent,
            })
        );
        assert_eq!(specs[0].project_root, PathBuf::from("/tmp/huddle"));
    }

    /// Several targets of one project are one config write, not one per
    /// target.
    #[test]
    fn a_multi_target_launch_writes_one_config_per_distinct_project() {
        let mut st = dap_workbench(Some(frust_dap::ide_config::ParentIde::VSCode));
        st.dap = Some(dap_handle(0));
        update(&mut st, Message::DapListening(0, 4849));
        run_config_on_devices(&mut st, vec![pixel_7()]);
        update(&mut st, Message::RunConfigToggleTargetAt(0)); // + desktop

        let out = update(&mut st, Message::RunConfigLaunch);

        let Some(Effect::Batch(effects)) = out.effect else {
            panic!("expected a Batch, got {:?}", out.effect);
        };
        assert!(matches!(&effects[0], Effect::LaunchSessions(specs) if specs.len() == 2));
        let configs: Vec<_> = effects[1..]
            .iter()
            .filter(|e| matches!(e, Effect::GenerateIdeConfig(_)))
            .collect();
        assert_eq!(configs.len(), 1, "one project, one config: {effects:?}");
        assert_eq!(effects.len(), 2);
    }

    /// No listening server, or auto-configure off: a launch is only a launch.
    #[test]
    fn a_launch_without_a_listening_server_or_with_auto_configure_off_is_plain() {
        // No server at all.
        let mut st = dap_workbench(Some(frust_dap::ide_config::ParentIde::VSCode));
        update(&mut st, Message::OpenRunConfig);
        let out = update(&mut st, Message::RunConfigLaunch);
        assert!(
            matches!(out.effect, Some(Effect::LaunchSessions(ref specs)) if specs.len() == 1),
            "got {:?}",
            out.effect
        );

        // A server still starting (no bound port yet).
        let mut st = dap_workbench(Some(frust_dap::ide_config::ParentIde::VSCode));
        st.dap = Some(dap_handle(0));
        update(&mut st, Message::OpenRunConfig);
        let out = update(&mut st, Message::RunConfigLaunch);
        assert!(
            matches!(out.effect, Some(Effect::LaunchSessions(_))),
            "got {:?}",
            out.effect
        );

        // Listening, but auto-configure off.
        let mut st = dap_workbench(Some(frust_dap::ide_config::ParentIde::VSCode));
        st.dap_settings.auto_configure_ide = false;
        st.dap = Some(dap_handle(0));
        update(&mut st, Message::DapListening(0, 4849));
        update(&mut st, Message::OpenRunConfig);
        let out = update(&mut st, Message::RunConfigLaunch);
        assert!(
            matches!(out.effect, Some(Effect::LaunchSessions(_))),
            "got {:?}",
            out.effect
        );
    }

    /// "Run on all devices" is the other launch path; it batches the config
    /// write the same way.
    #[test]
    fn run_on_all_devices_batches_the_ide_config_too() {
        let mut st = dap_workbench(Some(frust_dap::ide_config::ParentIde::Zed));
        st.dap = Some(dap_handle(0));
        update(&mut st, Message::DapListening(0, 4849));
        update(&mut st, Message::DevicesLoaded(vec![pixel_7()]));

        let out = run_on_all_devices(&mut st);

        let Some(Effect::Batch(effects)) = out.effect else {
            panic!("expected a Batch, got {:?}", out.effect);
        };
        assert!(matches!(&effects[0], Effect::LaunchSessions(_)));
        assert!(matches!(
            &effects[1],
            Effect::GenerateIdeConfig(request)
                if request.mode == WriteMode::IfAbsent
                    && request.project_root == std::path::Path::new("/tmp/huddle")
        ));
    }

    /// Every refusal is decided in the pure core and *reported*, never a
    /// silent no-op: no IDE, an IDE with no DAP config format, and no project.
    #[test]
    fn generation_refusals_are_reported_rather_than_requested() {
        let mut st = dap_workbench(None);
        let out = update(&mut st, Message::DapSettingsGenerate);
        assert_eq!(out.effect, None);
        assert_eq!(
            st.dap_settings.last_ide_config,
            Some(crate::engine::DapIdeReport::NoIde)
        );

        let mut st = dap_workbench(Some(frust_dap::ide_config::ParentIde::IntelliJ));
        let out = update(&mut st, Message::DapSettingsGenerate);
        assert_eq!(out.effect, None);
        assert_eq!(
            st.dap_settings.last_ide_config,
            Some(crate::engine::DapIdeReport::Unsupported(
                frust_dap::ide_config::ParentIde::IntelliJ
            ))
        );

        let mut st = welcome(); // no project open
        st.dap_settings.detected_ide = Some(frust_dap::ide_config::ParentIde::VSCode);
        let out = update(&mut st, Message::DapSettingsGenerate);
        assert_eq!(out.effect, None);
        assert!(matches!(
            st.dap_settings.last_ide_config,
            Some(crate::engine::DapIdeReport::Failed(ref why)) if why.contains("no project open")
        ));
    }

    /// A "generate now" while the server is listening points the editor at the
    /// live port, not at a configured one it isn't using.
    #[test]
    fn generate_now_prefers_the_listening_port_over_the_configured_one() {
        let mut st = dap_workbench(Some(frust_dap::ide_config::ParentIde::Zed));
        st.dap_settings.port = 5005;
        st.dap = Some(dap_handle(0));
        update(&mut st, Message::DapListening(0, 41_234));
        st.dap_settings.last_ide_config = None;
        let out = update(&mut st, Message::DapSettingsGenerate);
        assert_eq!(
            out.effect,
            Some(Effect::GenerateIdeConfig(crate::engine::IdeConfigRequest {
                ide: frust_dap::ide_config::ParentIde::Zed,
                port: 41_234,
                project_root: PathBuf::from("/tmp/huddle"),
                mode: WriteMode::Refresh,
            })),
            "the explicit path always refreshes"
        );
    }

    /// A port changed under a running server applies to the next start —
    /// never a surprise restart, and the dialog says so.
    #[test]
    fn a_port_change_under_a_listening_server_only_says_so() {
        let mut st = dap_workbench(None);
        st.dap = Some(dap_handle(0));
        update(&mut st, Message::DapListening(0, 4849));
        update(&mut st, Message::OpenDapSettings);
        update(&mut st, Message::DapSettingsFocus(DapFocus::Port));
        for _ in 0..5 {
            update(&mut st, Message::DapSettingsBackspace);
        }
        for c in "5005".chars() {
            update(&mut st, Message::DapSettingsInput(c));
        }
        let out = update(&mut st, Message::DapSettingsActivate);
        assert_eq!(
            out.effect,
            Some(Effect::SaveDapSetting(DapSetting::Port(5005))),
            "the change is persisted, and nothing restarts the server"
        );
        assert_eq!(
            st.dap_status(),
            crate::supervise::DapStatus::Listening {
                port: 4849,
                clients: 0
            }
        );
        assert!(
            st.toasts
                .items
                .iter()
                .any(|t| t.text.contains("takes effect on the next start"))
        );
        assert!(st.dap_settings.port_awaits_restart(Some(4849)));
    }

    #[test]
    fn the_mcp_panel_opens_and_closes_idempotently() {
        let mut st = welcome();
        assert!(update(&mut st, Message::OpenMcpPanel).redraw);
        assert!(st.mcp_panel_open);
        assert!(matches!(
            st.active_modal(),
            Some(super::super::ActiveModal::McpPanel)
        ));
        assert!(!update(&mut st, Message::OpenMcpPanel).redraw);

        assert!(update(&mut st, Message::CloseMcpPanel).redraw);
        assert!(!st.mcp_panel_open);
        assert!(!update(&mut st, Message::CloseMcpPanel).redraw);
    }

    /// Closing the panel is never a stop: the server outlives the view of it.
    #[test]
    fn closing_the_panel_leaves_the_server_running() {
        let mut st = welcome();
        st.mcp = Some(mcp_handle(0));
        st.mcp_panel_open = true;
        let out = update(&mut st, Message::CloseMcpPanel);
        assert!(out.effect.is_none());
        assert_eq!(st.mcp_status(), crate::supervise::McpStatus::Starting);
    }

    /// The open panel reads its client list live off the registry, so the
    /// tick is what keeps it current — the one thing that dirties a frame
    /// with no session running and no toast alive.
    #[test]
    fn the_open_mcp_panel_keeps_the_frame_ticking() {
        let mut st = welcome();
        assert!(!st.animating());
        assert!(!update(&mut st, Message::Tick).redraw);

        st.mcp_panel_open = true;
        assert!(st.animating());
        assert!(update(&mut st, Message::Tick).redraw);
    }

    /// An MCP command reaching the pure core (nothing is serving it) drops
    /// its reply channel rather than answering — the backend reports that as
    /// a typed error, never as a fabricated value.
    #[test]
    fn an_mcp_command_is_a_noop_in_the_pure_core() {
        let mut st = welcome();
        let (backend_tx, mut backend_rx) = tokio::sync::mpsc::unbounded_channel();
        let backend = crate::supervise::TuiSessionBackend::new(
            backend_tx,
            std::sync::Arc::new(frust_drive::process::FakeProcessRunner::new()),
        );
        let asking = std::thread::spawn(move || {
            use frust_mcp::SessionBackend;
            backend.sessions()
        });

        let command = loop {
            match backend_rx.try_recv() {
                Ok(Message::Mcp(command)) => break command,
                Ok(_) => {}
                Err(_) => std::thread::yield_now(),
            }
        };
        let out = update(&mut st, Message::Mcp(command));
        assert!(!out.redraw);
        assert!(out.effect.is_none());
        assert!(
            asking
                .join()
                .expect("the asking thread panicked")
                .is_empty(),
            "an unserved command yields nothing, never an invented session"
        );
    }
}
