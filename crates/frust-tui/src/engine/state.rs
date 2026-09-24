//! The application model — the single source of truth the render pass reads
//! (`&AppState`) and [`super::update`] mutates.

use std::fs;
use std::path::{Path, PathBuf};

use super::add_plugin::AddPluginDialog;
use super::bootstrap::{BootstrapState, BootstrapWizard};
use super::build_launcher::BuildLauncher;
use super::context_menu::ContextMenu;
use super::create_wizard::CreateWizard;
use super::dap_settings::DapSettings;
use super::doctor::DoctorState;
use super::message::{DragKind, RegionId};
use super::palette::Palette;
use super::run_config::{DeviceRow, RunConfig};
use super::session_view::{SessionTarget, SessionView};
use super::toast::Toasts;
use crate::supervise::{
    DapServerHandle, DapStatus, McpServerHandle, McpStatus, SessionId, SessionState,
};
use frust_dap::DapClientEntry;
use frust_mcp::ClientEntry;

/// Bounded-walk depth cap for [`detect`]/[`find_projects`]: `dir` itself is
/// depth 0, its children depth 1, its grandchildren depth 2 — nothing past
/// that is ever read. Keeps a repo-root walk instant regardless of tree size.
const MAX_DETECT_DEPTH: u32 = 2;

/// Sidebar drag-to-resize bounds. Kept **by value** in sync with
/// `crate::ui::layout::SIDEBAR_WIDTH` (the default) — the engine stays
/// render-free, so the two layers share the number, not a symbol (the
/// `LOG_LINE_CAP` cross-layer-constant precedent).
pub const SIDEBAR_MIN_WIDTH: u16 = 18;
/// Maximum sidebar width (columns) a drag can grow the sidebar to.
pub const SIDEBAR_MAX_WIDTH: u16 = 50;
/// Default sidebar width (columns) before any drag-resize.
pub const SIDEBAR_DEFAULT_WIDTH: u16 = 26;

/// Clamp a proposed sidebar width into the drag-resize bounds.
pub fn clamp_sidebar_width(width: u16) -> u16 {
    width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH)
}

/// The top-level screen the workbench is showing.
///
/// This enum has two: the [`Screen::Welcome`] splash (no project detected)
/// and the [`Screen::Workbench`] shell (a project is open). The project
/// switcher, sessions, and modals all layer on as separate optional state on
/// `AppState` rather than additional `Screen` variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Run-from-anywhere splash with the single Create button.
    Welcome,
    /// The titlebar/sidebar/main/status workbench shell.
    Workbench,
}

/// The log search/filter overlay state.
///
/// While `open`, keystrokes edit `query` live; committing (`Enter`) promotes it
/// to `filter`, which restricts the visible log lines to those that match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchState {
    /// Whether the overlay is open and capturing keystrokes.
    pub open: bool,
    /// The live query being typed.
    pub query: String,
    /// The committed filter applied to the visible log lines (`None` = show
    /// everything).
    pub filter: Option<String>,
}

/// The whole application model.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Which screen is active.
    pub screen: Screen,
    /// Set once the user asks to quit; the event loop exits on the next turn.
    pub should_quit: bool,
    /// The region currently under the pointer (drives hover chrome). `None`
    /// when the pointer is over no registered region.
    pub hover: Option<RegionId>,
    /// Whether the Create button is showing its pressed chrome.
    pub create_pressed: bool,
    /// The auto-dismiss toast stack rendered in the status-bar area layer.
    /// Bounded + drop-oldest, aged on the frame tick — see
    /// [`super::toast::Toasts`].
    pub toasts: Toasts,
    /// The fuzzy command palette, when open (`Ctrl+P` / `:`). While `Some`, it
    /// captures input and suppresses background mouse regions like the other
    /// modals — see [`super::palette::Palette`].
    pub palette: Option<Palette>,
    /// The active project root (`projects.first()`, the one the main area
    /// shows), if any. `None` on the welcome screen. Kept alongside
    /// `projects` for the call sites that only care about "the" open
    /// project — the full switcher (`project_switcher_open`) lets the user
    /// change which one is active.
    pub project_root: Option<PathBuf>,
    /// Every project the sidebar/switcher lists, ordered `[local...,
    /// previous...]` (decision D6) — see [`Self::local_project_count`] for
    /// the boundary. "Local" is every project the bounded [`detect`] walk
    /// found under `cwd` (plus a still-existing recent under `cwd` the walk
    /// missed); "previous" is every other still-existing persisted recent.
    /// Every index-based path (`Message::SwitchProject`,
    /// `ContextTarget::ProjectRow`, `project_switcher_cursor`, the digit
    /// shortcuts) indexes this one vec — the split only changes how the
    /// sidebar/switcher render it, never what an index means. Empty on the
    /// welcome screen.
    pub projects: Vec<PathBuf>,
    /// The working directory `detect` walked, recorded once so a later
    /// `state.projects` insertion (e.g. a freshly scaffolded project — see
    /// [`Self::insert_project`]) can classify the new root the same way
    /// [`super::persist::split_local_and_previous`] classified everything
    /// else at startup. Never re-read from the process environment after
    /// [`Self::new`]/[`Self::detect`] set it.
    pub cwd: PathBuf,
    /// The `projects` boundary: `projects[..local_project_count]` is
    /// "local", `projects[local_project_count..]` is "previous" (decision
    /// D6) — read through [`Self::local_count`], which clamps to
    /// `projects.len()`, rather than this raw field directly. Defaults to
    /// `usize::MAX` ([`Default`]/most hand-built test fixtures never set
    /// this new field explicitly), which clamps to "every project is
    /// local" — the pre-split, single-PROJECTS-section behavior — rather
    /// than silently reclassifying an untouched fixture's projects as
    /// "previous".
    pub local_project_count: usize,
    /// Every supervised session's view-model, in start (id) order. The tab bar
    /// renders these grouped by project; `active_session` indexes this vec.
    pub sessions: Vec<SessionView>,
    /// The index into `sessions` of the tab whose log view is shown, if any.
    pub active_session: Option<usize>,
    /// Whether the log view soft-wraps long lines (`w` toggles).
    pub wrap: bool,
    /// The log search/filter overlay state.
    pub search: SearchState,
    /// The discovered devices list (sidebar DEVICES section), each with its
    /// panel multi-select flag. Populated by a background discovery task.
    pub devices: Vec<DeviceRow>,
    /// The highlighted row in the devices panel (`↑`/`↓` move it); clamped to
    /// `devices` on every mutation.
    pub device_cursor: usize,
    /// Whether a device refresh is in flight (shows a spinner/label).
    pub devices_refreshing: bool,
    /// The run-config modal, when open (`r`/`Enter` from the devices panel).
    /// While `Some`, it captures input and suppresses background mouse regions.
    pub run_config: Option<RunConfig>,
    /// Whether the titlebar project-switcher dropdown is open.
    /// While `true`, it captures input and suppresses background mouse
    /// regions, the same base-layer suppression the run-config modal uses.
    pub project_switcher_open: bool,
    /// The highlighted row while the switcher is open (`↑`/`↓` move it,
    /// `Enter` switches to it); meaningless while closed.
    pub project_switcher_cursor: usize,
    /// The create-project wizard, when open (the welcome Create button, or the
    /// workbench "New project" action / `n`). While `Some`, it captures input
    /// and suppresses background mouse regions — the same base-layer
    /// suppression the run-config modal uses.
    pub create_wizard: Option<CreateWizard>,
    /// The doctor panel's cached validator results + titlebar chip source.
    /// Populated by a startup preflight and refreshed on demand;
    /// present regardless of whether the panel itself is open.
    pub doctor: DoctorState,
    /// Whether the doctor panel is open (`i` from either screen, or the
    /// sidebar "Doctor" action). While `true`, it captures input and
    /// suppresses background mouse regions like the other modals.
    pub doctor_panel_open: bool,
    /// The build-launcher modal, when open (`b` from the workbench, or the
    /// sidebar "Build" action). While `Some`, it captures input and
    /// suppresses background mouse regions like the other modals.
    pub build_launcher: Option<BuildLauncher>,
    /// The clean-confirm dialog, when open (`c` from the workbench, or the
    /// sidebar "Clean" action), holding the project root a confirmed clean
    /// runs against. While `Some`, it captures input and suppresses
    /// background mouse regions like the other modals.
    pub clean_confirm: Option<PathBuf>,
    /// The quit-confirm dialog's open flag (`q` / the palette Quit entry,
    /// with a live session). A bool is enough — the running-session count it
    /// warns about is read live at render via
    /// [`Self::live_session_count`], never cached here. While `true`, it
    /// captures input and suppresses background mouse regions like the other
    /// modals.
    pub quit_confirm: bool,
    /// The cached component-level toolchain report + titlebar-chip rollup
    /// source. Populated by a startup preflight and re-run after a
    /// guided-fix session; present regardless of whether the wizard is open.
    pub bootstrap: BootstrapState,
    /// The bootstrap wizard, when open (a titlebar toolchain-chip click, or
    /// the doctor panel's `t` key / "Toolchain setup" button). While `Some`,
    /// it captures input and suppresses background mouse regions like the
    /// other modals.
    pub bootstrap_wizard: Option<BootstrapWizard>,
    /// The Add Plugin dialog, when open (`a`, the sidebar "Add plugin" action,
    /// or the palette — `frust-secure-storage`). While `Some`, it
    /// captures input and suppresses background mouse regions like the other
    /// modals.
    pub add_plugin: Option<AddPluginDialog>,
    /// The sidebar width (columns), drag-resizable via the splitter.
    /// Clamped to [`SIDEBAR_MIN_WIDTH`]..=[`SIDEBAR_MAX_WIDTH`]. This field is
    /// the in-memory copy; the persisted value round-trips through
    /// `tui.toml` (see `persist::load_settings`/`persist::save_sidebar_width`).
    pub sidebar_width: u16,
    /// Whether crossterm mouse capture is on (`Alt+m` / palette toggle). Off
    /// hands the terminal its native text selection back; keyboard operation
    /// stays complete either way. This field is the in-memory copy; the
    /// preference is persisted via `persist::save_mouse_capture`.
    pub mouse_capture: bool,
    /// The drag currently in progress (a splitter or scrollbar-thumb grab),
    /// tracked from press to release so move/up events route to it.
    pub active_drag: Option<DragKind>,
    /// The open right-click context menu, if any. While `Some`, it
    /// captures keyboard nav and suppresses the base layer's mouse regions like
    /// a modal, but renders as a small popup over the (still-visible) workbench.
    pub context_menu: Option<ContextMenu>,
    /// Whether the sidebar renders as a toggleable floating overlay instead
    /// of its normal inline column — the narrow-terminal responsive
    /// breakpoint. Meaningless (ignored) above
    /// [`crate::ui::layout::NARROW_WIDTH`]; `false` by default so a narrow
    /// terminal starts with the sidebar collapsed, not covering the log view.
    pub sidebar_overlay_open: bool,
    /// The keyboard/help overlay (`?`), when open.
    pub help_open: bool,
    /// A monotonically advancing frame counter, incremented once per
    /// [`super::message::Message::Tick`] while [`Self::animating`] is `true`
    /// (the runner only delivers ticks then — see `crate::runner`). Pure
    /// animation math (`crate::ui::anim`) derives from this: `spinner_char`
    /// wants the raw counter divided by its cadence constant, `shimmer_phase`
    /// wants it directly. Wraps via `wrapping_add`, which every consumer's
    /// modulo math already tolerates.
    pub animation_frame: u64,
    /// The embedded MCP server, while one is running (`None` = stopped).
    ///
    /// The one field holding a live resource handle rather than a value:
    /// [`super::Engine::start_mcp`]/[`super::Engine::stop_mcp`] own its
    /// lifetime, and the pure `update` only records the server's own
    /// asynchronous reports (bound port, stopped) against it. Read it through
    /// [`Self::mcp_status`], never directly.
    pub mcp: Option<McpServerHandle>,
    /// Whether the MCP panel is open (`m`, or the palette) — workbook §B13's
    /// server state + connected-client list. While `true` it captures input
    /// and suppresses background mouse regions like the other modals.
    pub mcp_panel_open: bool,
    /// Why the embedded MCP server last stopped unexpectedly (a bind failure,
    /// or a server task that ended with an error), retained so the sidebar
    /// row and the panel can *show* the reason rather than leaving a failed
    /// start looking like a silent no-op. Cleared when the next start is
    /// requested.
    pub mcp_error: Option<String>,
    /// The embedded DAP server, while one is running (`None` = stopped) —
    /// [`Self::mcp`]'s exact counterpart, with the same live-resource
    /// ownership rule: [`super::Engine::start_dap`]/`stop_dap` own its
    /// lifetime, and the pure `update` only records the server's own
    /// asynchronous reports against it. Read it through [`Self::dap_status`],
    /// never directly.
    pub dap: Option<DapServerHandle>,
    /// Why the embedded DAP server last stopped unexpectedly (a bind failure,
    /// or a server task that ended with an error), retained so a failed start
    /// never reads as a silent no-op. Cleared when the next start is
    /// requested.
    pub dap_error: Option<String>,
    /// The workbench's DAP preferences (the persisted `[dap]` table) plus the
    /// settings dialog's edit state. Present regardless of whether the dialog
    /// is open: startup auto-start and the auto-configure that follows a
    /// `DapListening` report both read it.
    pub dap_settings: DapSettings,
    /// Whether the DAP settings dialog is open (`D`, the sidebar ACTIONS "DAP"
    /// row, or the palette). While `true` it captures input and suppresses
    /// background mouse regions like the other modals.
    pub dap_settings_open: bool,
}

impl AppState {
    /// Build the initial model from the current working directory, split
    /// against the persisted recent-projects list (decision D6): a bounded
    /// walk (see [`Self::detect`]) finds every `frust.toml` marker under the
    /// cwd, then [`super::persist::split_local_and_previous`] appends every
    /// still-existing recent not already counted as local — "local first,
    /// previous after, deduped". A cwd-detected project stays the active one
    /// (unsurprising `cd`-into-a-project-then-run behavior); with none
    /// detected, the most-recently-opened project opens instead of the
    /// welcome screen — "from any directory". Only the
    /// welcome screen (no active project either way) skips persistence
    /// entirely.
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut state = Self::detect(&cwd);
        let detected_active = state.project_root.clone();
        let recent = super::persist::load_recent_projects();
        let (projects, local_project_count) =
            super::persist::split_local_and_previous(&cwd, &recent, &state.projects);
        state.projects = projects;
        state.local_project_count = local_project_count;
        state.project_root = detected_active.or_else(|| state.projects.first().cloned());
        if state.project_root.is_some() {
            state.screen = Screen::Workbench;
        }
        let settings = super::persist::load_settings();
        state.sidebar_width = settings.sidebar_width;
        state.mouse_capture = settings.mouse_capture;
        // The DAP preferences and the IDE this process is hosted by: both are
        // read exactly once, here. Detection sniffs the environment the
        // workbench was launched into, which cannot change under a running
        // process, so re-sniffing per frame (or per transition) would only
        // move an impure read into the pure core.
        state.dap_settings = DapSettings::from_prefs(
            super::persist::load_dap_prefs(),
            frust_dap::ide_config::detect_parent_ide(),
        );
        state
    }

    /// Detection core (testable without touching the process cwd/reading a
    /// real project tree): [`find_projects`] walks `dir` for `frust.toml`
    /// markers; the first found (if any) becomes the active `project_root`.
    /// Every root it finds is "local" — `local_project_count` is set to the
    /// whole list's length — since a bounded walk never reaches outside
    /// `dir` in the first place.
    pub fn detect(dir: &Path) -> Self {
        let projects = find_projects(dir);
        let project_root = projects.first().cloned();
        let local_project_count = projects.len();
        let screen = if projects.is_empty() {
            Screen::Welcome
        } else {
            Screen::Workbench
        };
        Self {
            screen,
            should_quit: false,
            hover: None,
            create_pressed: false,
            toasts: Toasts::default(),
            palette: None,
            project_root,
            projects,
            cwd: dir.to_path_buf(),
            local_project_count,
            sessions: Vec::new(),
            active_session: None,
            wrap: false,
            search: SearchState::default(),
            devices: Vec::new(),
            device_cursor: 0,
            devices_refreshing: false,
            run_config: None,
            project_switcher_open: false,
            project_switcher_cursor: 0,
            create_wizard: None,
            doctor: DoctorState::default(),
            doctor_panel_open: false,
            build_launcher: None,
            clean_confirm: None,
            quit_confirm: false,
            bootstrap: BootstrapState::default(),
            bootstrap_wizard: None,
            add_plugin: None,
            sidebar_width: SIDEBAR_DEFAULT_WIDTH,
            mouse_capture: true,
            active_drag: None,
            context_menu: None,
            sidebar_overlay_open: false,
            help_open: false,
            animation_frame: 0,
            mcp: None,
            mcp_panel_open: false,
            mcp_error: None,
            dap: None,
            dap_error: None,
            dap_settings: DapSettings::default(),
            dap_settings_open: false,
        }
    }

    /// Whether the loop must keep processing the frame tick (to age toasts
    /// and advance `animation_frame`) and redraw on change. `true` while any
    /// toast is live (its TTL counts down off the tick loop, no ambient
    /// timer) OR any session is in a **transient** build/install phase (the
    /// tab spinner has something to advance); `false` otherwise, restoring
    /// the dirty-frame skip on an idle workbench. A session streaming stably
    /// (`SessionState::Running`) does NOT count — only the pre-`Running`
    /// phases the tab spinner covers do, so a long-lived running session
    /// never pins the tick interval on indefinitely.
    ///
    /// The open MCP panel counts too, for the same reason and with the same
    /// bound: its client list is read live off the registry at render time
    /// (nothing messages the engine when a client connects), so it needs the
    /// tick to stay current — but only *while the panel is open*, never for
    /// the whole life of a running server. The open DAP settings dialog counts
    /// for exactly the same reason (its attached-editor count is the same kind
    /// of live registry read).
    pub fn animating(&self) -> bool {
        !self.toasts.items.is_empty()
            || self.mcp_panel_open
            || self.dap_settings_open
            || self.sessions.iter().any(|s| is_transient(&s.state))
    }

    /// What the embedded MCP server is doing — the single read the UI (and
    /// anything else) should take, rather than reaching into
    /// [`Self::mcp`] itself.
    pub fn mcp_status(&self) -> McpStatus {
        match &self.mcp {
            Some(handle) => handle.status(),
            None => McpStatus::Stopped,
        }
    }

    /// Every MCP client connected to the embedded server right now, oldest
    /// first — the MCP panel's row source (workbook §B13). Empty while no
    /// server is running, which is the fact rather than a placeholder: a
    /// stopped server has no registry to read.
    ///
    /// Read live off the registry each call (the server's own threads
    /// register/unregister there), so two calls in one frame can legitimately
    /// disagree — take one snapshot per render, as the panel does.
    pub fn mcp_clients(&self) -> Vec<ClientEntry> {
        match &self.mcp {
            Some(handle) => handle.clients(),
            None => Vec::new(),
        }
    }

    /// What the embedded DAP server is doing — [`Self::mcp_status`]'s
    /// counterpart, and the single read anything else should take rather than
    /// reaching into [`Self::dap`] itself.
    pub fn dap_status(&self) -> DapStatus {
        match &self.dap {
            Some(handle) => handle.status(),
            None => DapStatus::Stopped,
        }
    }

    /// Every editor attached to the embedded DAP server right now, oldest
    /// first. Empty while no server is running — the fact, not a placeholder.
    ///
    /// Read live off the registry each call (the server's own tasks
    /// register/unregister there), so two calls in one frame can legitimately
    /// disagree — take one snapshot per render, as
    /// [`Self::mcp_clients`]'s consumers do.
    pub fn dap_clients(&self) -> Vec<DapClientEntry> {
        match &self.dap {
            Some(handle) => handle.clients(),
            None => Vec::new(),
        }
    }

    /// The active session's view-model, if a tab is selected.
    pub fn active_session(&self) -> Option<&SessionView> {
        self.active_session.and_then(|i| self.sessions.get(i))
    }

    /// The active session's view-model, mutably.
    pub fn active_session_mut(&mut self) -> Option<&mut SessionView> {
        match self.active_session {
            Some(i) => self.sessions.get_mut(i),
            None => None,
        }
    }

    /// The position of a session in `sessions` by id.
    pub fn session_index(&self, id: SessionId) -> Option<usize> {
        self.sessions.iter().position(|s| s.id == id)
    }

    /// Whether any tracked session is still in a live (non-terminal) state.
    pub fn any_session_running(&self) -> bool {
        self.sessions.iter().any(|s| !s.state.is_terminal())
    }

    /// The number of tracked sessions still in a live (non-terminal) state —
    /// what the quit-confirm dialog warns about before every one of them is
    /// force-stopped (see `crate::engine::update`'s `RequestQuit` arm and
    /// `crate::ui::views::quit_confirm`), read live at render rather than
    /// cached on the dialog itself.
    pub fn live_session_count(&self) -> usize {
        self.sessions
            .iter()
            .filter(|s| !s.state.is_terminal())
            .count()
    }

    /// The live session already running `project_root` on `target`, if there
    /// is one — the one-live-session-per-(project, target) guard every launch
    /// path consults before starting anything (the run-config modal,
    /// run-on-all-devices, and MCP/DAP's `run_app`).
    ///
    /// "Live" means non-terminal: a session that has `Exited` or been
    /// `Killed` never blocks a relaunch. Targets compare by
    /// [`SessionTarget::is_same_place_as`] (devices by id), and a session
    /// with no target at all — an ad-hoc build/clean/toolchain-fix tab —
    /// is never a match, so building a project does not stop it being run.
    /// The same project on a different device, or a different project on the
    /// same device, are both allowed: only the pair is exclusive.
    pub fn live_session_for(
        &self,
        project_root: &Path,
        target: &SessionTarget,
    ) -> Option<SessionId> {
        self.live_session_for_excluding(project_root, target, None)
    }

    /// [`Self::live_session_for`], ignoring one session id.
    ///
    /// The exclusion is what a restart needs: `restart_app` stops the session
    /// it is restarting before launching its spec again, so a 1-for-1 swap
    /// must not be refused by the very session it replaces — while any
    /// *other* live session on that target still refuses it (mirroring how
    /// `McpSessionRecords::live_count` excludes the restarted session from
    /// its own cap check).
    ///
    /// **Invariant:** `project_root` is compared here by lexical
    /// normalisation only ([`same_project_root`]) — no filesystem I/O. This
    /// method is reached from `update()` on the `RunConfigLaunch`,
    /// `run_on_all_devices`, and MCP `run_app` paths, and the engine's
    /// `update` must stay free of filesystem I/O, so canonicalisation is not
    /// an option here.
    ///
    /// `project_root` is **not** canonicalised at its entry points today:
    /// opening a project, the project switcher, and MCP's `run_app` all
    /// produce a raw, uncanonicalised `PathBuf`. So two paths that name the
    /// same directory on disk but differ in representation — a symlink and
    /// its target, or a `..`-relative path that resolves to the same place —
    /// still compare unequal here and can each spawn their own "live"
    /// session on the same project. Canonicalising at those entry points
    /// (where filesystem I/O is already expected) is a follow-up, not done
    /// by this method.
    pub fn live_session_for_excluding(
        &self,
        project_root: &Path,
        target: &SessionTarget,
        except: Option<SessionId>,
    ) -> Option<SessionId> {
        self.sessions
            .iter()
            .filter(|s| Some(s.id) != except)
            .filter(|s| !s.state.is_terminal() && same_project_root(&s.project_root, project_root))
            .find(|s| {
                s.target
                    .as_ref()
                    .is_some_and(|t| t.is_same_place_as(target))
            })
            .map(|s| s.id)
    }

    /// Clamp `device_cursor` into range after the device list changes (an
    /// empty list parks it at 0).
    pub fn clamp_device_cursor(&mut self) {
        if self.devices.is_empty() {
            self.device_cursor = 0;
        } else if self.device_cursor >= self.devices.len() {
            self.device_cursor = self.devices.len() - 1;
        }
    }

    /// The number of devices currently selected in the panel.
    pub fn selected_device_count(&self) -> usize {
        self.devices.iter().filter(|d| d.selected).count()
    }

    /// Clamp `project_switcher_cursor` into range after `projects` changes
    /// (an empty list parks it at 0) — mirrors `clamp_device_cursor`.
    pub fn clamp_project_switcher_cursor(&mut self) {
        if self.projects.is_empty() {
            self.project_switcher_cursor = 0;
        } else if self.project_switcher_cursor >= self.projects.len() {
            self.project_switcher_cursor = self.projects.len() - 1;
        }
    }

    /// The index of the active project in `projects` (defaults to 0 when the
    /// active root isn't found there, e.g. the welcome screen) — seeds the
    /// switcher's cursor when it opens.
    pub fn active_project_index(&self) -> usize {
        self.project_root
            .as_deref()
            .and_then(|root| self.projects.iter().position(|p| p == root))
            .unwrap_or(0)
    }

    /// The effective `projects` local/previous boundary (decision D6):
    /// `local_project_count` clamped to `projects.len()`. Every render/
    /// insert site reads through this rather than the raw field, so a value
    /// left at its `usize::MAX` default (or merely stale after `projects`
    /// shrinks) degrades to "everything is local" — the one section, no
    /// "PREVIOUS PROJECTS" heading, single sidebar list a fixture that never
    /// set this field expects — rather than panicking or under-counting into
    /// the previous section.
    pub fn local_count(&self) -> usize {
        self.local_project_count.min(self.projects.len())
    }

    /// Insert `root` into `projects`, keeping the D6 local/previous
    /// boundary invariant — the one seam every `state.projects` mutation
    /// site (today, `engine::update::open_project`, for a freshly scaffolded
    /// or newly opened project) goes through, so the boundary never drifts
    /// out of sync with a scattered set of `insert`/`push` calls. A root
    /// already present is left exactly where it is (no reorder, no
    /// duplicate) — this only handles a genuinely new one. A new root under
    /// `self.cwd` ([`super::persist::path_is_within`], the same test
    /// [`super::persist::split_local_and_previous`] uses) lands at the end
    /// of the local section, bumping the count; anything else lands at the
    /// head of the previous section — both are the same absolute index
    /// (`local_count()`, right where the previous section begins), so only
    /// whether the count is bumped differs.
    pub fn insert_project(&mut self, root: PathBuf) {
        if self.projects.contains(&root) {
            return;
        }
        let local_count = self.local_count();
        let becomes_local = super::persist::path_is_within(&root, &self.cwd);
        self.projects.insert(local_count, root);
        self.local_project_count = if becomes_local {
            local_count + 1
        } else {
            local_count
        };
    }

    /// The sessions grouped by project, preserving first-seen project order and
    /// each project's session order — the tab-bar / sidebar grouping. Each
    /// group is `(project_root, [(flat tab index, &session)])`, where the flat
    /// index is the position in `sessions` (what `SelectTab`/`active_session`
    /// use).
    pub fn sessions_grouped(&self) -> Vec<(&Path, Vec<(usize, &SessionView)>)> {
        let mut groups: Vec<(&Path, Vec<(usize, &SessionView)>)> = Vec::new();
        for (i, s) in self.sessions.iter().enumerate() {
            let root = s.project_root.as_path();
            match groups.iter_mut().find(|(r, _)| *r == root) {
                Some((_, v)) => v.push((i, s)),
                None => groups.push((root, vec![(i, s)])),
            }
        }
        groups
    }
}

impl Default for AppState {
    fn default() -> Self {
        // A project-free default (welcome screen) — used by render tests that
        // don't want to touch the filesystem.
        Self {
            screen: Screen::Welcome,
            should_quit: false,
            hover: None,
            create_pressed: false,
            toasts: Toasts::default(),
            palette: None,
            project_root: None,
            projects: Vec::new(),
            cwd: PathBuf::new(),
            // See the field doc: `usize::MAX` clamps (via `local_count`) to
            // "every project is local" for a fixture built via
            // `..Default::default()` that sets `projects` without also
            // setting this new field — most hand-rolled `AppState` literals
            // across the crate's own test suites.
            local_project_count: usize::MAX,
            sessions: Vec::new(),
            active_session: None,
            wrap: false,
            search: SearchState::default(),
            devices: Vec::new(),
            device_cursor: 0,
            devices_refreshing: false,
            run_config: None,
            project_switcher_open: false,
            project_switcher_cursor: 0,
            create_wizard: None,
            doctor: DoctorState::default(),
            doctor_panel_open: false,
            build_launcher: None,
            clean_confirm: None,
            quit_confirm: false,
            bootstrap: BootstrapState::default(),
            bootstrap_wizard: None,
            add_plugin: None,
            sidebar_width: SIDEBAR_DEFAULT_WIDTH,
            mouse_capture: true,
            active_drag: None,
            context_menu: None,
            sidebar_overlay_open: false,
            help_open: false,
            animation_frame: 0,
            mcp: None,
            mcp_panel_open: false,
            mcp_error: None,
            dap: None,
            dap_error: None,
            dap_settings: DapSettings::default(),
            dap_settings_open: false,
        }
    }
}

/// Whether `state` is a transient (pre-`Running`) phase — the tab spinner
/// has something to advance. `Running` (streaming stably) and the terminal
/// states (`Exited`/`Killed`) are deliberately excluded: a session parked in
/// either would otherwise pin [`AppState::animating`] `true` forever, keeping
/// the tick interval (and its ~20 fps redraw budget) alive indefinitely for
/// no visible benefit.
pub(crate) fn is_transient(state: &SessionState) -> bool {
    matches!(
        state,
        SessionState::Configuring | SessionState::Building | SessionState::Installing
    )
}

/// Whether `a` and `b` name the same project root, compared *lexically* —
/// no filesystem I/O. [`Path::components`] already normalises away trailing
/// separators and `.` (current-dir) segments, so `/tmp/huddle`,
/// `/tmp/huddle/`, and `/tmp/huddle/.` all compare equal here. This is
/// deliberately not canonicalisation: two paths that are equal on disk but
/// differ in representation (a symlink and its target, `..` segments that
/// resolve to the same place) still compare unequal. See
/// [`AppState::live_session_for_excluding`] for why that gap is accepted.
fn same_project_root(a: &Path, b: &Path) -> bool {
    a.components().eq(b.components())
}

/// Bounded depth-2 walk from `root` for `frust.toml` project markers: `root`
/// itself (depth 0), its children (depth 1), then its grandchildren (depth
/// 2) — never past that, and never through a symlinked directory
/// (`DirEntry::file_type` reports the link's own type, not its target's, so
/// a symlink simply fails the `is_dir()` filter below rather than needing a
/// separate check). A hidden directory (name starting with `.`), `target/`,
/// or `node_modules/` is skipped entirely — neither checked for a marker nor
/// descended into. Only `frust.toml`'s existence is checked (no file reads),
/// so a repo-root walk stays instant regardless of tree size. Returns every
/// found project root in a deterministic (name-sorted at each level) order.
fn find_projects(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect_projects(root, 0, &mut found);
    found
}

fn is_skipped_dir_name(name: &std::ffi::OsStr) -> bool {
    match name.to_str() {
        Some(s) => s.starts_with('.') || s == "target" || s == "node_modules",
        // A non-UTF-8 name can't match any skip rule; don't skip it.
        None => false,
    }
}

fn collect_projects(dir: &Path, depth: u32, found: &mut Vec<PathBuf>) {
    if dir.join("frust.toml").is_file() {
        found.push(dir.to_path_buf());
    }
    if depth >= MAX_DETECT_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|ft| ft.is_dir()))
        .filter(|entry| !is_skipped_dir_name(&entry.file_name()))
        .map(|entry| entry.path())
        .collect();
    children.sort();
    for child in children {
        collect_projects(&child, depth + 1, found);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-tui-detect-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch_project(dir: &Path) {
        fs::write(dir.join("frust.toml"), "").unwrap();
    }

    #[test]
    fn empty_dir_yields_welcome() {
        let root = unique_temp_dir("empty");
        let state = AppState::detect(&root);
        assert_eq!(state.screen, Screen::Welcome);
        assert!(state.projects.is_empty());
        assert_eq!(state.project_root, None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn project_at_root_is_found() {
        let root = unique_temp_dir("root-project");
        touch_project(&root);
        let state = AppState::detect(&root);
        assert_eq!(state.screen, Screen::Workbench);
        assert_eq!(state.projects, vec![root.clone()]);
        assert_eq!(state.project_root, Some(root.clone()));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn project_at_depth_one_and_two_are_found() {
        let root = unique_temp_dir("nested");
        let child = root.join("examples");
        fs::create_dir_all(&child).unwrap();
        touch_project(&child);
        let grandchild = child.join("bubblebench");
        fs::create_dir_all(&grandchild).unwrap();
        touch_project(&grandchild);
        let state = AppState::detect(&root);
        assert_eq!(state.screen, Screen::Workbench);
        assert_eq!(state.projects, vec![child, grandchild]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn project_past_depth_two_is_not_found() {
        let root = unique_temp_dir("too-deep");
        let too_deep = root.join("a").join("b").join("c");
        fs::create_dir_all(&too_deep).unwrap();
        touch_project(&too_deep);
        let state = AppState::detect(&root);
        assert_eq!(state.screen, Screen::Welcome);
        assert!(state.projects.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn hidden_target_and_node_modules_dirs_are_skipped() {
        let root = unique_temp_dir("skip-dirs");
        for name in [".git", "target", "node_modules"] {
            let dir = root.join(name);
            fs::create_dir_all(&dir).unwrap();
            touch_project(&dir);
        }
        let visible = root.join("visible");
        fs::create_dir_all(&visible).unwrap();
        touch_project(&visible);
        let state = AppState::detect(&root);
        assert_eq!(state.projects, vec![visible]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn multiple_sibling_projects_are_all_found_in_name_order() {
        let root = unique_temp_dir("multi");
        let bubblebench = root.join("bubblebench");
        let huddle = root.join("huddle");
        fs::create_dir_all(&bubblebench).unwrap();
        fs::create_dir_all(&huddle).unwrap();
        touch_project(&bubblebench);
        touch_project(&huddle);
        let state = AppState::detect(&root);
        assert_eq!(state.screen, Screen::Workbench);
        assert_eq!(state.projects, vec![bubblebench.clone(), huddle]);
        assert_eq!(state.project_root, Some(bubblebench));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn detect_counts_every_found_root_as_local() {
        let root = unique_temp_dir("detect-local-count");
        touch_project(&root);
        let child = root.join("examples");
        fs::create_dir_all(&child).unwrap();
        touch_project(&child);
        let state = AppState::detect(&root);
        assert_eq!(state.local_count(), state.projects.len());
        assert_eq!(state.cwd, root);
        let _ = fs::remove_dir_all(&root);
    }

    // ── local_count() / insert_project() (decision D6) ────────────────────

    #[test]
    fn local_count_clamps_the_default_sentinel_to_every_project() {
        let state = AppState {
            projects: vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")],
            ..AppState::default()
        };
        assert_eq!(
            state.local_count(),
            2,
            "a fixture that never set local_project_count treats every \
             project as local"
        );
    }

    #[test]
    fn insert_project_under_the_cwd_lands_at_the_end_of_the_local_section() {
        let mut state = AppState {
            cwd: PathBuf::from("/tmp/cwd"),
            projects: vec![PathBuf::from("/tmp/cwd/a"), PathBuf::from("/tmp/other/r1")],
            local_project_count: 1,
            ..AppState::default()
        };
        state.insert_project(PathBuf::from("/tmp/cwd/b"));
        assert_eq!(
            state.projects,
            vec![
                PathBuf::from("/tmp/cwd/a"),
                PathBuf::from("/tmp/cwd/b"),
                PathBuf::from("/tmp/other/r1"),
            ]
        );
        assert_eq!(state.local_count(), 2);
    }

    #[test]
    fn insert_project_outside_the_cwd_lands_at_the_head_of_previous() {
        let mut state = AppState {
            cwd: PathBuf::from("/tmp/cwd"),
            projects: vec![PathBuf::from("/tmp/cwd/a"), PathBuf::from("/tmp/other/r1")],
            local_project_count: 1,
            ..AppState::default()
        };
        state.insert_project(PathBuf::from("/tmp/other/r2"));
        assert_eq!(
            state.projects,
            vec![
                PathBuf::from("/tmp/cwd/a"),
                PathBuf::from("/tmp/other/r2"),
                PathBuf::from("/tmp/other/r1"),
            ]
        );
        assert_eq!(state.local_count(), 1, "the local section is unchanged");
    }

    #[test]
    fn insert_project_already_present_is_a_noop() {
        let mut state = AppState {
            cwd: PathBuf::from("/tmp/cwd"),
            projects: vec![PathBuf::from("/tmp/cwd/a")],
            local_project_count: 1,
            ..AppState::default()
        };
        state.insert_project(PathBuf::from("/tmp/cwd/a"));
        assert_eq!(state.projects, vec![PathBuf::from("/tmp/cwd/a")]);
        assert_eq!(state.local_count(), 1);
    }

    // ── animating() ─────────────────────────────────────────────────────────

    fn session_with_state(state: SessionState) -> SessionView {
        let mut view = SessionView::new(SessionId(0), PathBuf::from("/tmp/proj"), "desktop");
        view.state = state;
        view
    }

    #[test]
    fn animating_false_with_no_sessions_and_no_toasts() {
        let state = AppState::default();
        assert!(state.sessions.is_empty());
        assert!(!state.animating());
    }

    #[test]
    fn animating_true_while_a_session_is_building() {
        let mut state = AppState::default();
        state
            .sessions
            .push(session_with_state(SessionState::Building));
        assert!(state.animating());
    }

    #[test]
    fn animating_true_while_a_session_is_configuring_or_installing() {
        for s in [SessionState::Configuring, SessionState::Installing] {
            let mut state = AppState::default();
            state.sessions.push(session_with_state(s.clone()));
            assert!(state.animating(), "{s:?} should count as transient");
        }
    }

    #[test]
    fn animating_false_when_all_sessions_are_streaming() {
        let mut state = AppState::default();
        state
            .sessions
            .push(session_with_state(SessionState::Running));
        assert!(!state.animating());
    }

    #[test]
    fn animating_false_when_sessions_are_terminal() {
        let mut state = AppState::default();
        state
            .sessions
            .push(session_with_state(SessionState::Exited(true)));
        state
            .sessions
            .push(session_with_state(SessionState::Killed));
        assert!(!state.animating());
    }

    #[test]
    fn animating_true_when_toasts_live_even_with_no_sessions() {
        let mut state = AppState::default();
        state.toasts.push(crate::engine::ToastKind::Info, "hi");
        assert!(state.animating());
    }

    #[test]
    fn animating_true_when_one_session_builds_among_others_streaming() {
        let mut state = AppState::default();
        state
            .sessions
            .push(session_with_state(SessionState::Running));
        state
            .sessions
            .push(session_with_state(SessionState::Building));
        assert!(state.animating(), "one transient session is enough");
    }

    // ── live_session_for ────────────────────────────────────────────────────

    fn pixel_7() -> SessionTarget {
        SessionTarget::Device {
            id: "emulator-5554".to_string(),
            name: "Pixel 7".to_string(),
            platform: frust_drive::devices::Platform::Android,
        }
    }

    /// A live session on `root`/`target`, id `id`.
    fn running_on(id: u64, root: &str, target: Option<SessionTarget>) -> SessionView {
        let label = target
            .as_ref()
            .map_or_else(|| "build".to_string(), SessionTarget::label);
        let mut view = crate::engine::SessionView::with_devtools(
            SessionId(id),
            PathBuf::from(root),
            label,
            crate::engine::DevtoolsLaunch::unavailable(),
            target,
        );
        view.state = SessionState::Running;
        view
    }

    fn state_with(sessions: Vec<SessionView>) -> AppState {
        AppState {
            sessions,
            ..AppState::default()
        }
    }

    #[test]
    fn a_live_session_on_the_same_project_and_target_is_found() {
        let state = state_with(vec![running_on(3, "/tmp/huddle", Some(pixel_7()))]);
        assert_eq!(
            state.live_session_for(Path::new("/tmp/huddle"), &pixel_7()),
            Some(SessionId(3))
        );
        assert_eq!(
            state.live_session_for(Path::new("/tmp/huddle"), &SessionTarget::Desktop),
            None,
            "desktop and a device are different places"
        );
        assert_eq!(
            state.live_session_for(Path::new("/tmp/other"), &pixel_7()),
            None,
            "the pair is exclusive, not the device"
        );
    }

    #[test]
    fn a_device_matches_by_id_alone() {
        let state = state_with(vec![running_on(0, "/tmp/huddle", Some(pixel_7()))]);
        let renamed = SessionTarget::Device {
            id: "emulator-5554".to_string(),
            name: "Ed's Pixel".to_string(),
            platform: frust_drive::devices::Platform::Android,
        };
        assert_eq!(
            state.live_session_for(Path::new("/tmp/huddle"), &renamed),
            Some(SessionId(0)),
            "a rediscovered device with a new display name is the same phone"
        );
    }

    #[test]
    fn terminal_and_targetless_sessions_never_occupy_a_target() {
        let mut exited = running_on(0, "/tmp/huddle", Some(SessionTarget::Desktop));
        exited.state = SessionState::Exited(true);
        let mut killed = running_on(1, "/tmp/huddle", Some(SessionTarget::Desktop));
        killed.state = SessionState::Killed;
        // A live ad-hoc build of the same project: no target at all.
        let ad_hoc = running_on(2, "/tmp/huddle", None);
        let state = state_with(vec![exited, killed, ad_hoc]);
        assert_eq!(
            state.live_session_for(Path::new("/tmp/huddle"), &SessionTarget::Desktop),
            None
        );
    }

    #[test]
    fn the_excluded_session_does_not_block_its_own_relaunch() {
        let state = state_with(vec![
            running_on(0, "/tmp/huddle", Some(SessionTarget::Desktop)),
            running_on(1, "/tmp/huddle", Some(SessionTarget::Desktop)),
        ]);
        assert_eq!(
            state.live_session_for_excluding(
                Path::new("/tmp/huddle"),
                &SessionTarget::Desktop,
                Some(SessionId(0)),
            ),
            Some(SessionId(1)),
            "excluding one session must not hide another on the same target"
        );
        assert_eq!(
            state.live_session_for_excluding(
                Path::new("/tmp/huddle"),
                &SessionTarget::Desktop,
                Some(SessionId(1)),
            ),
            Some(SessionId(0))
        );
    }

    #[test]
    fn project_root_comparison_is_lexical_not_canonical() {
        // A trailing separator and a redundant `.` segment name the same
        // directory and must still match, purely from Path::components().
        assert!(same_project_root(
            Path::new("/tmp/huddle"),
            Path::new("/tmp/huddle/")
        ));
        assert!(same_project_root(
            Path::new("/tmp/huddle"),
            Path::new("/tmp/huddle/.")
        ));
        // A different directory never matches, lexically identical prefix or not.
        assert!(!same_project_root(
            Path::new("/tmp/huddle"),
            Path::new("/tmp/huddle2")
        ));

        // The same rule is what live_session_for_excluding relies on: a
        // trailing-slash variant of a live session's project_root still
        // finds it, with no filesystem access.
        let state = state_with(vec![running_on(
            0,
            "/tmp/huddle",
            Some(SessionTarget::Desktop),
        )]);
        assert_eq!(
            state.live_session_for(Path::new("/tmp/huddle/"), &SessionTarget::Desktop),
            Some(SessionId(0)),
            "a trailing separator names the same project root"
        );
    }
}
