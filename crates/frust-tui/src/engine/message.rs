//! TEA messages and the semantic region ids they are routed through.
//!
//! Every message is a *pure* input to [`super::update`]: the terminal event
//! loop (`crate::runner`) translates raw crossterm events + mouse-region
//! hit-tests into `Message`s, and the engine's single mutation point applies
//! them. Background tasks (the session supervisor) feed the same channel via
//! [`Message::Session`] — see [`super::Engine`] and [`crate::supervise`].

use std::path::PathBuf;

use frust_drive::devices::Device;
use frust_drive::doctor::DoctorReport;
use frust_drive::hotpatch::session::Outcome as HotOutcome;
use frust_drive::metrics::MetricsSample;
use frust_drive::plugin::AddReport;

use super::build_launcher::BuildFocus;
use super::dap_settings::{DapFocus, DapIdeReport};
use super::devtools::{ConnEvent, DevtoolsLaunch, InspectorEvent};
use super::doctor::DoctorCheck;
use super::logstyle::LevelFilter;
use super::run_config::RunFocus;
use super::session_view::SessionTarget;
use super::toast::ToastKind;
use crate::supervise::mcp_backend::McpCommand;
use crate::supervise::{SessionEvent, SessionId};

/// A semantic id for a per-frame mouse region.
///
/// Ids are stable identities the render pass tags its clickable/hoverable
/// surfaces with (see `crate::ui::mouse`); the event loop compares the
/// hovered id against `AppState::hover` to decide whether a hover change is
/// worth a redraw, and drives press/release off the id under the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegionId {
    /// The single large "Create a new Frust project" button on the welcome
    /// screen; opens the create wizard.
    CreateButton,
    /// The workbench "New project" action (sidebar ACTIONS row); opens the
    /// create wizard.
    NewProjectAction,
    /// An architecture card in the create wizard (0-based index into the
    /// wizard's `arches`); click highlights it.
    WizardArchCard(usize),
    /// The create wizard's Next / Create button (advances / scaffolds).
    WizardNext,
    /// The create wizard's Back button (steps back).
    WizardBack,
    /// The create wizard's Cancel button (closes it).
    WizardCancel,
    /// A session tab in the main-area tab bar (0-based index into the ordered
    /// session list); click selects it.
    SessionTab(usize),
    /// The scrollable log-view pane (pointer-aware wheel scroll region).
    LogView,
    /// The devices-panel "refresh" affordance; click re-runs discovery.
    RefreshDevices,
    /// A device row in the sidebar DEVICES section (0-based index into
    /// `AppState::devices`); click moves the cursor there and toggles select.
    DeviceRow(usize),
    /// A target checkbox row in the run-config modal (0-based index into the
    /// modal's `targets`); click toggles it.
    RunTargetRow(usize),
    /// The run-config modal's build-mode selector; click cycles it.
    RunModeRow,
    /// The run-config modal's flavor text field; click focuses it.
    RunFlavorRow,
    /// The run-config modal's defines text field; click focuses it.
    RunDefinesRow,
    /// The run-config modal's "Run hot" checkbox row; click toggles it.
    RunHotRow,
    /// The run-config modal's "Auto-apply on save" checkbox row; click
    /// toggles it.
    RunAutoApplyRow,
    /// The run-config modal's launch button.
    RunLaunch,
    /// The run-config modal's cancel button.
    RunCancel,
    /// The titlebar project-switcher toggle (the project name + `▾` chevron).
    ProjectSwitcherToggle,
    /// A project row in the sidebar PROJECTS section (0-based index into
    /// `AppState::projects`); click switches the active project directly.
    ProjectRow(usize),
    /// A row inside the open titlebar switcher dropdown (0-based index into
    /// `AppState::projects`); click switches and closes the dropdown.
    ProjectMenuItem(usize),
    /// The titlebar toolchain chip; click opens the bootstrap wizard directly
    /// (the doctor detail panel stays reachable via `i` / the sidebar
    /// "Doctor" action, and offers its own "Toolchain setup" button through
    /// to this same wizard).
    DoctorChip,
    /// A step-tree row in the bootstrap wizard (0-based index into the wizard's
    /// visible node list); click selects it (a `Platforms` header row toggles
    /// its expansion).
    BootstrapStep(usize),
    /// A fix-command row in the bootstrap wizard's detail pane (0-based index
    /// into the selected step's fix list); click selects it.
    BootstrapFix(usize),
    /// The bootstrap wizard's "Run in session" affordance (runs the selected
    /// auto-runnable fix as a supervised session).
    BootstrapRunFix,
    /// The bootstrap wizard's "copy" affordance (copies the selected fix's
    /// command / doc link to the clipboard).
    BootstrapCopyFix,
    /// The bootstrap wizard's close affordance (the `[Esc] Close` title
    /// button).
    BootstrapClose,
    /// A plugin selection card in the Add Plugin dialog (0-based index into the
    /// dialog's `entries`); click highlights it (a disabled card can be
    /// highlighted to read its reason but not chosen).
    AddPluginCard(usize),
    /// An optional-feature checkbox row in the Add Plugin dialog (0-based index
    /// into the dialog's `features`); click toggles it.
    AddPluginFeature(usize),
    /// The Add Plugin dialog's primary button (Next on Select, Apply on
    /// Options, Retry on Error, Done on Report).
    AddPluginApply,
    /// The Add Plugin dialog's Back button (steps back).
    AddPluginBack,
    /// The Add Plugin dialog's Cancel button (closes it).
    AddPluginCancel,
    /// The Add Plugin dialog's `[Esc] Close` title affordance.
    AddPluginClose,
    /// The sidebar "Add plugin" action row; click opens the Add Plugin dialog.
    AddPluginAction,
    /// The sidebar "Doctor" action row; click opens the doctor panel.
    DoctorAction,
    /// The doctor panel's re-run affordance.
    DoctorRerun,
    /// The doctor panel's close button.
    DoctorClose,
    /// The doctor panel's "Toolchain setup" button (`t` keyboard parity);
    /// closes the panel and opens the bootstrap wizard.
    DoctorToolchainSetup,
    /// The sidebar "Build" action row; click opens the build launcher.
    BuildAction,
    /// The build-launcher modal's artifact-kind selector.
    BuildKindRow,
    /// The build-launcher modal's build-mode selector.
    BuildModeRow,
    /// The build-launcher modal's flavor text field.
    BuildFlavorRow,
    /// The build-launcher modal's defines text field.
    BuildDefinesRow,
    /// The build-launcher modal's split-per-ABI toggle (Apk only).
    BuildSplitPerAbiRow,
    /// The build-launcher modal's Simulator toggle (Ios only).
    BuildSimulatorRow,
    /// The build-launcher modal's no-codesign toggle (Ios only).
    BuildNoCodesignRow,
    /// The build-launcher modal's export-method text field (Ipa only).
    BuildExportMethodRow,
    /// The build-launcher modal's launch button.
    BuildLaunchButton,
    /// The build-launcher modal's cancel button.
    BuildCancelButton,
    /// The sidebar "Clean" action row; click opens the clean confirm dialog.
    CleanAction,
    /// The clean-confirm dialog's confirm button.
    CleanConfirmYes,
    /// The clean-confirm dialog's cancel button.
    CleanConfirmNo,
    /// The quit-confirm dialog's confirm button.
    QuitConfirmYes,
    /// The quit-confirm dialog's cancel button.
    QuitConfirmNo,
    /// The log status row's "copy built artifact path(s)" affordance.
    CopyArtifactsAction,
    /// A command row in the open command palette (0-based index into the
    /// ranked result list); click executes it.
    PaletteRow(usize),
    /// The command palette's `[Esc] Close` title affordance.
    PaletteClose,
    /// A row in the open context menu (0-based index into the menu's entries);
    /// hover highlights it, click activates it.
    ContextMenuItem(usize),
    /// The keyboard/help overlay — a click anywhere in the panel
    /// closes it (mouse parity for `Esc`).
    HelpClose,
    /// A drawn log row, carrying the absolute index of the line it draws;
    /// click sets the line-selection mode's anchor / range end (outside the
    /// mode the click is idle — the region is registered every frame either
    /// way, so a scrolled viewport always maps rows to current lines).
    LogRow(u64),
    /// A panic/backtrace block's `▶ n frames…` fold affordance row (the
    /// block's identity: the absolute index of its panic-header line); click
    /// toggles its collapsed state.
    LogFoldToggle(u64),
    /// A segment of the log status bar's level-filter chip (its full-pill
    /// form); click jumps the active session straight to it.
    LevelFilterSegment(LevelFilter),
    /// The level-filter chip's degraded (compact/minimal) single-token form
    /// — a narrow terminal collapses the segmented pill into one region;
    /// click cycles the filter (`Message::CycleLevelFilter(1)`, same as the
    /// `l` key) rather than jumping to a specific segment, since there's no
    /// room to show every segment to jump to (see
    /// `ui::views::sessions::render_log_status`'s graduated chip degrade).
    LevelFilterChip,
    /// A DevTools tab pill (0-based index into
    /// [`super::DevtoolsTab::ALL`]); click selects that tab (keyboard parity:
    /// `1`–`4`).
    DevtoolsTabPill(usize),
    /// The DevTools failed-state Retry button (keyboard parity: `r`).
    DevtoolsRetry,
    /// The DevTools status row's "back to log" affordance (keyboard parity:
    /// `Esc` / `d`).
    DevtoolsBack,
    /// A Performance-tab chart column, identified by the frame that column
    /// drew ([`super::PerfFrame::n`], captured at render time — *not* its
    /// position in the window, which the next frame batch would re-point at
    /// another frame); click selects that frame (keyboard parity: `←`/`→`
    /// scrubbing to it).
    DevtoolsPerfColumn(u64),
    /// An Inspector-tab tree row (0-based index into
    /// [`super::InspectorTab::rows`]); click selects it (keyboard parity:
    /// `↑↓`/`j`/`k`).
    DevtoolsInspectorRow(usize),
    /// An Inspector-tab row's `▸`/`▾` affordance (the same row index); click
    /// expands/collapses that node (keyboard parity: `→`/`←`).
    DevtoolsInspectorTwisty(usize),
    /// The sidebar ACTIONS "MCP" row (workbook §B13); click starts the
    /// embedded MCP server, or stops the running one (keyboard parity: `M`).
    McpToggle,
    /// The MCP panel's Start/Stop button (keyboard parity: `s`).
    McpPanelToggleServer,
    /// The MCP panel's Close button (keyboard parity: `Esc` / `m`).
    McpPanelClose,
    /// The sidebar ACTIONS "DAP" row; click opens the DAP settings dialog
    /// (keyboard parity: `D`). Unlike the MCP row this *opens* rather than
    /// toggles — the server switch lives inside the dialog, beside the
    /// preferences that decide when it starts by itself.
    DapAction,
    /// The DAP settings dialog's Start/Stop server action (keyboard parity:
    /// `s`, or `Enter` with the action focused).
    DapSettingsToggleServer,
    /// The DAP settings dialog's port field; click focuses it.
    DapSettingsPortRow,
    /// The DAP settings dialog's "auto-start in an IDE terminal" checkbox.
    DapSettingsAutoStartRow,
    /// The DAP settings dialog's "auto-configure the IDE" checkbox.
    DapSettingsAutoConfigureRow,
    /// The DAP settings dialog's IDE selector; click cycles it.
    DapSettingsIdeRow,
    /// The DAP settings dialog's "Generate IDE config now" action (keyboard
    /// parity: `g`, or `Enter` with the action focused).
    DapSettingsGenerate,
    /// The DAP settings dialog's Close button (keyboard parity: `Esc` / `D`).
    DapSettingsClose,
}

/// The kind of an in-progress drag, identifying which draggable chrome the
/// pointer grabbed. Each variant carries the layout geometry
/// captured from the registered drag region at press time, so the pure engine
/// maps a later pointer position to a result (a sidebar width, or a log scroll
/// anchor) without ever knowing the terminal layout itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragKind {
    /// The sidebar splitter (its right border column). `body_left` is the x
    /// origin of the body area, so a drag to absolute column `x` yields a
    /// sidebar width of `x - body_left`, clamped to the sidebar min/max.
    SidebarSplitter {
        /// The x origin of the workbench body area.
        body_left: u16,
    },
    /// The log-view scrollbar thumb. The track spans terminal rows
    /// `track_top .. track_top + track_height`; a drag to absolute row `y` maps
    /// to a fraction of the track, then to an absolute log-line scroll anchor
    /// (respecting the existing exact scroll bounds — see
    /// [`super::SessionView::scroll_to_fraction`]).
    LogScrollbar {
        /// The top terminal row of the scrollbar track.
        track_top: u16,
        /// The height (rows) of the scrollbar track.
        track_height: u16,
    },
}

/// What a right-click landed on — the context a [`super::ContextMenu`] is built
/// from. Each variant names the row/pane under the cursor so
/// [`super::context_menu::entries_for`] can offer target-specific entries whose
/// messages already exist (never a menu-only command — see that module).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextTarget {
    /// A session tab (flat index into `AppState::sessions`).
    SessionTab(usize),
    /// A device row (index into `AppState::devices`).
    DeviceRow(usize),
    /// A project row (index into `AppState::projects`).
    ProjectRow(usize),
    /// The log view pane. `row` is the absolute log-line index of the row
    /// under the cursor — `None` for the pane-wide region (empty space below
    /// the last line), which is why the row-specific entries are gated on it
    /// rather than assuming a row was hit.
    LogView {
        /// The absolute log-line index under the cursor, if a drawn row was
        /// hit.
        row: Option<u64>,
    },
}

/// A TEA message: the only way `AppState` ever changes.
///
/// `PartialEq` but not `Eq`: [`Message::DevtoolsInspector`] carries the
/// wire's widget bounds, which are `f64` (see
/// [`super::InspectorRow::bounds`]). Every comparison this crate makes is a
/// `==`/`assert_eq!`, which `PartialEq` alone satisfies.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// The deliberate quit bypass — `Ctrl+Q` only. Sets `should_quit`
    /// directly, skipping the quit-confirm dialog even with live sessions.
    /// Every other former `Quit` producer (`q` from the welcome/workbench
    /// screen or the DevTools pane, `Ctrl+C` with no running session, and the
    /// palette's Quit entry) now emits [`Self::RequestQuit`] instead.
    Quit,
    /// A tick from the frame interval — ages the toast stack (the app's only
    /// animated state) and is otherwise a no-op (dirty-frame skip keeps it
    /// from forcing a draw when nothing is animating).
    Tick,
    /// The terminal was resized; forces a redraw at the new size.
    Resize(u16, u16),
    /// The hovered region changed (or cleared). Deduped by `update` so an
    /// unchanged hover costs no redraw.
    HoverChanged(Option<RegionId>),
    /// A primary-button press landed on the Create button (shows pressed
    /// chrome).
    CreatePressed,
    /// A primary-button release landed on the Create button, or `Enter`/`c`
    /// activated it — opens the create wizard.
    CreateActivate,
    /// A press that started on the Create button was released elsewhere —
    /// clears the pressed chrome without activating.
    CreateCancel,

    // ── Create-project wizard ───────────────────────────────────────────────
    /// Open the create wizard (the workbench "New project" action / `n`, or
    /// the palette). Primes the [`super::update::Effect::ProbeCleanSignals`]
    /// priming effect (see its doc — no arch card is sibling-gated today).
    OpenCreateWizard,
    /// Close the wizard without scaffolding (the Cancel button).
    CloseCreateWizard,
    /// Type a character into the wizard's focused text field (name/directory).
    CreateWizardInput(char),
    /// Delete the last character of the wizard's focused text field.
    CreateWizardBackspace,
    /// Advance the wizard (Enter / the Next-or-Create button): validate and
    /// step forward, or — on the arch step — request the off-thread scaffold.
    CreateWizardAdvance,
    /// Step the wizard back (Esc / the Back button); closes it on the first
    /// step.
    CreateWizardBack,
    /// Move the arch-card highlight by `delta` (`←`/`→`/`↑`/`↓`).
    CreateWizardArchMove(isize),
    /// Highlight an arch card by index (mouse click parity).
    CreateWizardSelectArchAt(usize),
    /// The [`super::update::Effect::ProbeCleanSignals`] priming effect's
    /// reply — historically whether the `../clean-signals-rs` sibling
    /// checkout was present. No card is sibling-gated today, so the runner
    /// always replies `true` and this is effectively a no-op (see that
    /// effect's doc).
    CleanSignalsProbed(bool),
    /// The off-thread scaffold succeeded; open the new project in place.
    ScaffoldSucceeded {
        /// The absolute root of the freshly scaffolded project.
        project_root: PathBuf,
    },
    /// The off-thread scaffold failed; the wizard shows the error and offers a
    /// retry.
    ScaffoldFailed(String),

    // ── Sessions: the supervisor feeds `Session`; the rest are
    //    user-driven tab/log-view interactions. ──────────────────────────────
    /// A lifecycle/line event from a supervised session (see
    /// [`crate::supervise`]) — routed into the matching session's view-model.
    Session(SessionEvent),
    /// Register a newly-started session's metadata so its events have a home
    /// (the runner sends this when it starts a session — it holds the spec the
    /// bare [`SessionEvent`] doesn't carry).
    RegisterSession {
        /// The supervisor-assigned id.
        id: SessionId,
        /// The project the session runs (drives tab grouping).
        project_root: PathBuf,
        /// A short target label (`desktop`, or a device name).
        target_label: String,
        /// What the session's launch config says about reaching a devtools
        /// service (workbook §B12) — carried here because only the launcher
        /// knows the build mode and (for Android) the device serial. An
        /// ad-hoc build/clean/toolchain session passes
        /// [`DevtoolsLaunch::unavailable`].
        devtools: DevtoolsLaunch,
        /// *Where* the session runs, as the identity the one-live-session
        /// guard compares ([`SessionTarget`]) — `Some` for every app launch
        /// (the run-config modal, run-on-all-devices, and MCP/DAP's
        /// `run_app`), `None` for an ad-hoc build/clean/toolchain-fix
        /// session, which occupies no target.
        target: Option<SessionTarget>,
    },
    /// Select the next / previous session tab (`Tab` / `Shift+Tab`).
    NextTab,
    /// Select the previous session tab.
    PrevTab,
    /// Jump to a session tab by 0-based index (`1`–`9`).
    SelectTab(usize),
    /// Stop the active session (`Ctrl+C` / `x`) — routed to the supervisor as
    /// an [`super::Effect::StopSession`].
    StopSession,
    /// Stop the active session and relaunch its retained launch spec as a
    /// new session tab (`R` with a session active, and the palette's
    /// "Restart session" row) — the keyboard twin of MCP `restart_app` / DAP
    /// `frustRestart`
    /// (`crate::supervise::mcp_backend::restart_app`), routed to the
    /// supervisor as an [`super::Effect::RestartSession`]. Refused (no
    /// effect, a toast explains why) for an ad-hoc session (no
    /// [`SessionTarget`]) or when another live session already occupies the
    /// same (project, target); an already-terminal active session still
    /// restarts — a crashed session must be relaunchable, not just a running
    /// one. The relaunch's tab becomes active when it registers.
    RestartSession,
    /// Hot-patch the active session now (`r` with an app session active, and
    /// the palette's "Hot patch now" row): a live hot session is asked to
    /// replay every source and build input changed since its last applied
    /// patch ([`super::Effect::HotPatchNow`]), the manual twin of a watched
    /// save-burst — the answer comes back as [`Self::HotPatchOutcome`], and a
    /// `RestartRequired` restarts it whether or not its watcher is armed. An
    /// app session that is not hot restarts through the
    /// [`Self::RestartSession`] path with an Info toast saying so; no app
    /// session, nothing.
    HotPatchNow,
    /// Toggle "Watch: hot patch on save" on the active session (the
    /// palette's "Watch: hot patch on save" row, `W`): while on, a settled
    /// save-burst is offered to the running app as a hot patch
    /// ([`super::Effect::HotPatch`]) when the session runs hot, and restarts
    /// it through the [`Self::RestartSession`] path otherwise, once per
    /// burst. A watched session's relaunch runs hot (the runner launches a
    /// watched debug session through the hot-patch session start). Desktop,
    /// Android device and iOS simulator app sessions only — anything else
    /// refuses with a toast, the `frust run --watch` rule. Routed to the runner as
    /// [`super::Effect::WatchSet`], which starts/stops the session's
    /// `crate::supervise::SourceWatchers` entry.
    ToggleWatch,
    /// A watched session's sources settled after a change burst (posted by
    /// its `crate::supervise::SourceWatchers` debounce thread, already
    /// debounced to one per burst). A live hot session is asked to patch
    /// `paths` ([`super::Effect::HotPatch`]); anything else restarts through
    /// the shared restart path. Either way only while the session still has
    /// watch on and no restart is already pending for it; otherwise a no-op.
    WatchTriggered {
        /// The session whose sources changed.
        session: SessionId,
        /// Every path the burst touched, deduplicated and sorted — what a
        /// hot session's `on_change` classifies and replays.
        paths: Vec<PathBuf>,
        /// Whether the session runs as a hot-patch session, as the runner
        /// told the watcher when it started it.
        hot: bool,
    },
    /// What a hot session's `on_change` answered for one save-burst (posted
    /// by the runner, which runs `on_change` off the UI thread). Toasts
    /// `patched in N ms`, `restart required: <reason>` and so on; a
    /// `RestartRequired` additionally restarts the session through the
    /// shared restart path (once per burst: a restart already pending
    /// absorbs it).
    HotPatchOutcome {
        /// The session the patch was offered to.
        session: SessionId,
        /// `frust-drive`'s answer, verbatim.
        outcome: HotOutcome,
    },
    /// Mark a just-registered session as a **hot** session — posted by the
    /// runner after the [`Self::RegisterSession`] (and any
    /// [`Self::EnableWatch`]) of every launch it starts through the
    /// hot-patch session start (same channel, sent later), so the pure core
    /// knows `r` can patch it rather than restart it.
    HotSessionStarted {
        /// The freshly launched hot session.
        session: SessionId,
    },
    /// Turn watch on for a just-registered session without a toggle —
    /// posted by the runner right after a launch that should carry the flag
    /// registers: the relaunch of a watched session (how the flag survives a
    /// restart), or a hot launch from the run-config modal with "Auto-apply
    /// on save" ticked. Always arrives after that launch's
    /// [`Self::RegisterSession`] (same channel, sent later).
    EnableWatch {
        /// The freshly launched session.
        session: SessionId,
    },
    /// The runner could not start `session`'s source watcher: its watch flag
    /// is cleared and `reason` toasted.
    WatchFailed {
        /// The session whose watcher failed to start.
        session: SessionId,
        /// Why, for the toast.
        reason: String,
    },
    /// Close a tab by index into `sessions` (the context menu's "Close tab" /
    /// "Stop & close" entries, and the palette's "Close tab" command). A
    /// session already in a terminal state is removed immediately; a live
    /// one is stopped exactly like [`Self::StopSession`] and removed once its
    /// terminal event lands (see `super::update::on_session_event`).
    CloseTab(usize),
    /// [`Self::CloseTab`] for the active tab (`X`).
    CloseActiveTab,
    /// Toggle follow-tail on the active session's log view (`f`).
    ToggleFollow,
    /// Toggle soft-wrap on the log view (`w`).
    ToggleWrap,
    /// Scroll the active log view up / down by `n` *visible* lines (wheel /
    /// arrows) — steps through what the view actually draws (filters applied,
    /// a collapsed panic block counting as one), never raw line indices; see
    /// [`super::SessionView::visible_indices`].
    LogScrollUp(u64),
    /// Scroll the active log view down by `n` visible lines.
    LogScrollDown(u64),
    /// Jump the active log view to the oldest retained line (`Home`).
    LogScrollToTop,
    /// Jump the active log view back to the tail (`End`).
    LogScrollToBottom,
    /// Open the log search/filter overlay (`/`).
    SearchOpen,
    /// Append a char to the live search query.
    SearchInput(char),
    /// Delete the last char of the live search query.
    SearchBackspace,
    /// Commit the query as the active filter (`Enter`).
    SearchCommit,
    /// Close the search overlay without changing the committed filter (`Esc`).
    SearchCancel,
    /// Enter the active session's **line-selection mode** (`v`, the log
    /// view's context menu, the palette's "Select lines…"): anchor a
    /// one-line selection on the newest visible line and pause follow-tail.
    /// A no-op on an empty (or wholly filtered-out) log — see
    /// [`super::SessionView::enter_select_mode`].
    SelectEnter,
    /// Step the selection cursor `n` **visible** entries while in the mode
    /// (negative = toward older lines): `↑`/`k`/`Shift+↑` and
    /// `↓`/`j`/`Shift+↓`.
    SelectMove(i64),
    /// Move the selection cursor one page (sign only: `-1` = `PageUp`,
    /// `1` = `PageDown`).
    SelectPage(i8),
    /// Jump the selection cursor to the oldest visible line (`Home`).
    SelectHome,
    /// Jump the selection cursor to the newest visible line (`End`).
    SelectEnd,
    /// Leave line-selection mode, dropping the selection (`Esc`, or `v`
    /// again) — restores follow-tail only under
    /// [`super::SessionView::exit_select_mode`]'s rule.
    SelectExit,
    /// A left click landed on the log row drawing absolute line index `n`
    /// (every rendered row registers one; see `crate::ui::views::sessions`).
    /// The runner stays dumb about the mode: outside it this is idle, inside
    /// it the first click re-anchors the selection and every later one moves
    /// its range end.
    LogRowClicked(u64),
    /// Copy the current selection to the clipboard (`y`) — routed to the runner
    /// as an [`super::Effect::Copy`]. In line-selection mode this also leaves
    /// the mode, so `v`…`y` is a complete copy gesture.
    CopySelection,
    /// Copy the single log line at absolute index `n` (the log view's
    /// right-click "Copy line") — routed as an [`super::Effect::Copy`], or a
    /// warning when the ring has already evicted it.
    CopyLine(u64),
    /// Toggle a panic/backtrace block's fold state by its id (the block's
    /// panic-header absolute line index) — a click on its `▶ n frames…`
    /// affordance row.
    ToggleFold(u64),
    /// Toggle the fold state of whichever panic block is nearest the active
    /// session's current scroll position (`z`) — the keyboard-only path
    /// (workbook §B11's backtrace-fold affordance).
    ToggleNearestFold,
    /// Step the active session's level filter `delta` positions (`l`/`L`).
    CycleLevelFilter(isize),
    /// Jump the active session's level filter directly to `filter` (a
    /// filter-chip segment click).
    SetLevelFilter(LevelFilter),

    // ── Devices panel + run-config modal ─────────────────────────────────────
    /// Refresh the device list (`r` from the panel with no modal, or the
    /// panel refresh affordance) — routed to the runner as
    /// [`super::Effect::RefreshDevices`], which discovers off-thread.
    RefreshDevices,
    /// The background discovery task finished: replace the device list.
    DevicesLoaded(Vec<Device>),
    /// Move the devices-panel cursor up / down (`↑`/`↓` while the panel has
    /// focus and no modal is open).
    DeviceCursorUp,
    /// Move the devices-panel cursor down.
    DeviceCursorDown,
    /// Toggle the multi-select of the device under the cursor (`Space`).
    ToggleDeviceSelect,
    /// Move the cursor to a device by index and toggle it (mouse click parity).
    SelectDeviceAt(usize),
    /// Open the run-config modal, primed from the panel selection (`o` in
    /// the workbench, `Enter` from the devices panel).
    OpenRunConfig,
    /// Close the run-config modal without launching (`Esc`).
    CloseRunConfig,
    /// Move modal focus to the next / previous control (`Tab` / `↓`, `↑`).
    RunConfigFocusNext,
    /// Move modal focus to the previous control.
    RunConfigFocusPrev,
    /// Toggle the focused target checkbox (`Space`).
    RunConfigToggleTarget,
    /// Toggle a target checkbox by index (mouse click parity).
    RunConfigToggleTargetAt(usize),
    /// Toggle the modal's "Run hot" checkbox (mouse click parity; `Space` on
    /// the focused row reaches it through [`Self::RunConfigToggleTarget`]).
    RunConfigToggleRunHot,
    /// Toggle the modal's "Auto-apply on save" checkbox (mouse click parity;
    /// `Space` on the focused row reaches it through
    /// [`Self::RunConfigToggleTarget`]).
    RunConfigToggleAutoApply,
    /// Cycle the build mode by `delta` (`←`/`→`).
    RunConfigCycleMode(isize),
    /// Move modal focus to a specific control (mouse parity for the
    /// flavor/defines rows).
    RunConfigFocus(RunFocus),
    /// Type a character into the focused modal text field.
    RunConfigInput(char),
    /// Delete the last character of the focused modal text field.
    RunConfigBackspace,
    /// Launch one session per checked target (`Enter` on the launch button, or
    /// the launch affordance) — routed to the runner as
    /// [`super::Effect::LaunchSessions`].
    RunConfigLaunch,

    // ── Project switcher + recent-projects persistence ──────────────────────
    /// Toggle the titlebar `▾` project-switcher dropdown open/closed (click,
    /// or `Ctrl+O`/`p` from the workbench).
    ToggleProjectSwitcher,
    /// Close the switcher dropdown without switching (`Esc`).
    CloseProjectSwitcher,
    /// Move the switcher's highlighted row up (`↑` while open).
    ProjectSwitcherCursorUp,
    /// Move the switcher's highlighted row down (`↓` while open).
    ProjectSwitcherCursorDown,
    /// Switch the active project to `AppState::projects[index]` — from a
    /// sidebar project-row click, a switcher dropdown item click, `Enter`/a
    /// digit key while the switcher is open. Also requests the runner
    /// persist it as the most-recently-opened project
    /// ([`super::Effect::RecordRecentProject`]).
    SwitchProject(usize),

    // ── Doctor panel + titlebar chip ──────────────────────────────────────────
    /// Run the validator set off-thread (the panel's own `r` re-run
    /// affordance / Re-run button) — routed to the runner as
    /// [`super::Effect::RunDoctor`]. Also fired once at startup
    /// (`crate::runner`) to seed the chip.
    RunDoctor,
    /// The off-thread validator run finished: replace the cached results.
    DoctorResults(Vec<DoctorCheck>),
    /// Open the doctor panel (`i` from either screen, the sidebar "Doctor"
    /// action, or the palette).
    OpenDoctorPanel,
    /// Close the doctor panel (`Esc` / the panel's Close button).
    CloseDoctorPanel,
    /// Close the doctor panel and open the bootstrap wizard (the panel's `t`
    /// key / "Toolchain setup" button) — the toolchain setup moved here from
    /// its old direct `i` binding.
    OpenToolchainFromDoctor,

    // ── Bootstrap wizard + titlebar toolchain chip ───────────────────────────
    /// Run the component-level toolchain report off-thread
    /// ([`frust_drive::doctor::build_report`]) — routed to the runner as
    /// [`super::Effect::RunBootstrapReport`]. Fired once at startup to seed the
    /// chip, and again after a guided-fix session exits (re-preflight).
    RunBootstrapReport,
    /// The off-thread report finished: cache it (the titlebar chip's rollup
    /// source) and refresh the wizard if it's open. Carries the drive report
    /// directly (it derives `PartialEq`/`Eq`/`Clone`, so no mirror type is
    /// needed).
    BootstrapReport(DoctorReport),
    /// Open the bootstrap wizard (a titlebar toolchain-chip click, or the
    /// doctor panel's `t` key / "Toolchain setup" button via
    /// [`Message::OpenToolchainFromDoctor`]) — seeds it from the cached
    /// report, requesting a preflight first if none is cached yet.
    OpenBootstrapWizard,
    /// Close the bootstrap wizard (`Esc` / the `[Esc] Close` title button).
    CloseBootstrapWizard,
    /// Move the step-tree cursor up / down (`↑`/`↓`).
    BootstrapNavUp,
    /// Move the step-tree cursor down.
    BootstrapNavDown,
    /// Select a step-tree row by index (mouse click parity); a `Platforms`
    /// header row toggles its expansion.
    BootstrapSelectStep(usize),
    /// Toggle the selected `Platforms` header's expansion (`Space`/`Enter` on
    /// it) — a no-op on any other row.
    BootstrapToggleExpand,
    /// Move the detail-pane fix cursor up / down (`Shift+Tab` / `Tab`).
    BootstrapFixUp,
    /// Move the detail-pane fix cursor down.
    BootstrapFixDown,
    /// Select a fix-command row by index (mouse click parity).
    BootstrapSelectFix(usize),
    /// Run the selected auto-runnable fix as a supervised session (`r` / the
    /// "Run in session" button) — routed to the runner as
    /// [`super::Effect::RunBootstrapCommand`]; guidance-only fixes are a no-op.
    BootstrapRunFix,
    /// Copy the selected fix's command (or its doc link) to the clipboard (`c`
    /// / the "copy" affordance) — routed as [`super::Effect::Copy`].
    BootstrapCopyFix,

    // ── Add plugin dialog ─────────────────────────────────────────────────────
    /// Open the Add Plugin dialog for the active project (`a`, the sidebar
    /// "Add plugin" action, or the palette). Primes an off-thread sibling
    /// probe (shared with the create wizard's clean-signals gating). A no-op
    /// (with a warn toast) when no project is open.
    OpenAddPlugin,
    /// Close the dialog without applying (the Cancel button / Esc on the first
    /// step).
    CloseAddPlugin,
    /// Move the selection-card highlight by `delta` (`↑`/`↓`).
    AddPluginSelectMove(isize),
    /// Highlight a selection card by index (mouse click parity).
    AddPluginSelectAt(usize),
    /// Move the optional-feature cursor by `delta` (`↑`/`↓` on the options step).
    AddPluginFeatureMove(isize),
    /// Toggle the focused optional feature (`Space`).
    AddPluginToggleFeature,
    /// Toggle an optional feature by index (mouse click parity).
    AddPluginToggleFeatureAt(usize),
    /// Advance the dialog (Enter / the primary button): step forward, request
    /// the off-thread apply, retry, or close (on the report step).
    AddPluginAdvance,
    /// Step the dialog back (Esc / the Back button); closes it on the first or
    /// report step.
    AddPluginBack,
    /// The off-thread apply succeeded; the dialog shows the per-edit report.
    AddPluginSucceeded(AddReport),
    /// The off-thread apply failed; the dialog shows the error and offers a
    /// retry.
    AddPluginFailed(String),

    // ── Build launcher ────────────────────────────────────────────────────────
    /// Open the build launcher, primed for the active project (`b` from the
    /// workbench, or the sidebar "Build" action).
    OpenBuildLauncher,
    /// Close the build launcher without launching (`Esc`).
    CloseBuildLauncher,
    /// Move modal focus to the next / previous control (`Tab`/`↓`, `↑`).
    BuildFocusNext,
    /// Move modal focus to the previous control.
    BuildFocusPrev,
    /// Cycle the artifact kind by `delta` (`←`/`→` on the kind row).
    BuildCycleKind(isize),
    /// Cycle the build mode by `delta` (`←`/`→` on the mode row).
    BuildCycleMode(isize),
    /// Toggle the split-per-ABI flag (`Space` on that row, Apk only).
    BuildToggleSplitPerAbi,
    /// Toggle the Simulator flag (`Space` on that row, Ios only).
    BuildToggleSimulator,
    /// Toggle the no-codesign flag (`Space` on that row, Ios only).
    BuildToggleNoCodesign,
    /// Move modal focus to a specific control (mouse parity for the
    /// flavor/defines/export-method rows).
    BuildFocus(BuildFocus),
    /// Type a character into the focused modal text field.
    BuildInput(char),
    /// Delete the last character of the focused modal text field.
    BuildBackspace,
    /// Launch the resolved build (`Enter` on the launch button) — routed to
    /// the runner as [`super::Effect::LaunchBuild`].
    BuildLaunch,

    // ── Clean confirm dialog ──────────────────────────────────────────────────
    /// Open the clean-confirm dialog for the active project (`c` from the
    /// workbench, or the sidebar "Clean" action).
    OpenCleanConfirm,
    /// Close the dialog without cleaning (`Esc`).
    CloseCleanConfirm,
    /// Confirm the clean (`Enter`/`y`) — routed to the runner as
    /// [`super::Effect::RunClean`].
    ConfirmClean,

    // ── Quit confirm dialog ─────────────────────────────────────────────────────
    /// Ask to quit (`q` from the welcome/workbench screen or the DevTools
    /// pane, `Ctrl+C` with no running session, or the palette's Quit entry —
    /// every former [`Self::Quit`] producer except `Ctrl+Q`). With no live
    /// session ([`super::AppState::live_session_count`] `== 0`) this behaves
    /// exactly like [`Self::Quit`]; otherwise it opens the quit-confirm
    /// dialog (`state.quit_confirm = true`) instead of quitting outright.
    RequestQuit,
    /// Confirm the quit from the dialog (`Enter`/`y`) — the old unconditional
    /// `Quit` behaviour: sets `should_quit` and clears the dialog.
    ConfirmQuit,
    /// Close the quit-confirm dialog without quitting (`Esc`/`n`).
    CloseQuitConfirm,

    // ── Build artifact copy-path ──────────────────────────────────────────────
    /// Copy the active (build) session's reported artifact path(s) to the
    /// clipboard (`c` on a session tab with built artifacts, or the log
    /// status row's copy affordance) — routed to the runner as
    /// [`super::Effect::Copy`].
    CopyBuiltArtifacts,

    // ── Command palette ───────────────────────────────────────────────────────
    /// Open the fuzzy command palette (`Ctrl+P` / `:` from the base layer of
    /// either top-level screen).
    OpenPalette,
    /// Close the palette without executing (`Esc` / the `[Esc] Close` title
    /// affordance).
    ClosePalette,
    /// Append a character to the palette's live fuzzy query.
    PaletteInput(char),
    /// Delete the last character of the palette query.
    PaletteBackspace,
    /// Move the palette selection up / down (`↑`/`↓`).
    PaletteCursorUp,
    /// Move the palette selection down.
    PaletteCursorDown,
    /// Execute the palette's currently-selected command (`Enter`) — re-dispatches
    /// the command's existing `Message` through `update` (the palette never
    /// duplicates command logic). A disabled command is a no-op.
    PaletteExecute,
    /// Execute a palette command by ranked-row index (mouse click parity).
    PaletteExecuteAt(usize),

    // ── Drag-to-resize + scrollbar thumb ──────────────────────────────────────
    /// A drag started on a draggable region (the sidebar splitter or the log
    /// scrollbar thumb) — records the active drag so subsequent move/up events
    /// route to it (`crate::runner` gates them on `AppState::active_drag`).
    DragStart(DragKind),
    /// The pointer moved to absolute column/row `(x, y)` while a drag is active
    /// — applies the drag's effect (resize the sidebar, or move the log scroll
    /// anchor). A no-op with no active drag.
    DragMove(u16, u16),
    /// The drag button was released — clears the active drag.
    DragEnd,

    // ── Context menus ─────────────────────────────────────────────────────────
    /// Open a right-click context menu at `(x, y)` for whatever row/pane the
    /// click landed on — builds target-specific entries (see
    /// [`super::context_menu`]). Focuses the target row (mouse parity with a
    /// left click) as it opens.
    OpenContextMenu {
        /// The column the menu's top-left corner anchors at (clamped on render).
        x: u16,
        /// The row the menu's top-left corner anchors at (clamped on render).
        y: u16,
        /// What was right-clicked.
        target: ContextTarget,
    },
    /// Close the context menu without activating an entry (`Esc` / a
    /// click-outside).
    CloseContextMenu,
    /// Move the context-menu highlight up (`↑`).
    ContextMenuCursorUp,
    /// Move the context-menu highlight down (`↓`).
    ContextMenuCursorDown,
    /// Activate the highlighted context-menu entry (`Enter`) — re-dispatches its
    /// existing `Message` through `update`, exactly like the palette.
    ContextMenuActivate,
    /// Activate a context-menu entry by index (mouse click parity).
    ContextMenuActivateAt(usize),

    // ── Mouse-capture toggle ──────────────────────────────────────────────────
    /// Toggle crossterm mouse capture on/off (`Alt+m`, the palette command, or
    /// the status-bar indicator) — routed to the runner as
    /// [`super::Effect::SetMouseCapture`] so users can fall back to the
    /// terminal's own native text selection. Keyboard operation stays complete
    /// while capture is off.
    ToggleMouseCapture,

    // ── Run on all devices (palette / device-header action) ─────────────────
    /// Launch one supervised session per discovered device (debug mode) for the
    /// active project — the palette's "run on all devices" action, routed to the
    /// runner as [`super::Effect::LaunchSessions`]. A no-op with no project or
    /// no discovered devices.
    RunOnAllDevices,

    // ── DevTools mode (workbook §B12) ─────────────────────────────────────────
    /// Toggle the active session tab between its log view and DevTools (`d`).
    /// Per session — a tab keeps its own log/DevTools state, so switching
    /// tabs never forces you out of DevTools where you opened it. A no-op
    /// with no active session.
    DevtoolsToggle,
    /// Leave DevTools for the active session, back to its log view (`Esc`, or
    /// the status row's back affordance). A no-op when it isn't open.
    DevtoolsClose,
    /// Select a DevTools tab by 0-based index (`1`–`4`, or a tab-pill click).
    DevtoolsTab(usize),
    /// Step the DevTools tab selection `delta` positions, wrapping (`[`/`]`).
    DevtoolsTabCycle(isize),
    /// Retry a failed devtools connection for the active session (`r` on the
    /// failed screen, or its Retry button) — routed to the runner as
    /// [`super::Effect::DevtoolsConnect`]. A no-op once the session itself
    /// has ended (there is no service left to reach).
    DevtoolsRetry,
    /// A report from the session's devtools bridge thread
    /// ([`crate::supervise::DevtoolsBridge`]) — connection state changes and
    /// coalesced frame-stats batches.
    DevtoolsConn(SessionId, ConnEvent),
    /// A coalesced batch of System/Network metrics samples from the
    /// session's sampler thread ([`crate::supervise::MetricsBridge`]),
    /// oldest first — applied to [`super::DevtoolsState::metrics`].
    DevtoolsMetrics(SessionId, Vec<MetricsSample>),

    // ── Performance tab (workbook §B12) ─────────────────────────────────────
    /// Move the pinned frame `delta` frames along the current Performance
    /// window (`←`/`→`, ±1) — to the adjacent *retained* frame, clamped at
    /// both ends. A no-op with no active session.
    DevtoolsPerfScrub(isize),
    /// Pin a Performance-window frame directly by its own
    /// [`super::PerfFrame::n`] (a chart-column click, carrying the `n` the
    /// clicked column drew). A frame that has left the window since the click
    /// was registered pins nothing — see
    /// [`super::PerformanceTab::select_frame`].
    DevtoolsPerfSelectFrame(u64),
    /// Clear the Performance tab's scrubbed selection, back to the live tail
    /// — `Esc`'s first stage while a frame is selected (the second `Esc`,
    /// with nothing selected, falls through to [`Message::DevtoolsClose`]).
    DevtoolsPerfClearSelection,
    /// Cycle the Performance tab's pane focus between the chart and the
    /// breakdown bar (`Tab`, only while the Performance tab is active).
    DevtoolsPerfFocusCycle,

    // ── Inspector tab (workbook §B12) ───────────────────────────────────────
    /// Move the Inspector tree selection `delta` *visible* rows (`↑`/`↓`,
    /// `j`/`k`, ±1), clamped to the flattened row list. Moving onto a node
    /// whose props are neither cached nor in flight also emits
    /// [`super::Effect::DevtoolsFetchProps`] — §B12's per-selection call.
    DevtoolsInspectorSelect(isize),
    /// Select an Inspector tree row directly by its visible index (a row
    /// click), clamped — same props-fetch behavior as
    /// [`Message::DevtoolsInspectorSelect`].
    DevtoolsInspectorSelectRow(usize),
    /// Expand the selected Inspector node (`→`/`Enter`/`Space`); a no-op on a
    /// leaf or an already-open node.
    DevtoolsInspectorExpand,
    /// Collapse the selected Inspector node (`←`); a no-op on an
    /// already-collapsed node.
    DevtoolsInspectorCollapse,
    /// Toggle one Inspector node by id — the `▸`/`▾` click affordance.
    DevtoolsInspectorToggleNode(u64),
    /// Cycle the Inspector tab's pane focus between the tree and the props
    /// pane (`Tab`, only while the Inspector tab is active).
    DevtoolsInspectorFocusCycle,
    /// Pull a fresh `widget_tree` snapshot for the active session (`r` on the
    /// Inspector tab) — routed to the runner as
    /// [`super::Effect::DevtoolsFetchTree`]. A no-op while a pull is already
    /// in flight, or when the connection isn't up.
    DevtoolsInspectorRefresh,
    /// A report from the session's devtools bridge thread answering an
    /// on-demand Inspector request (`widget_tree` / `widget_props`).
    DevtoolsInspector(SessionId, InspectorEvent),

    // ── Perf sparkline panel ──────────────────────────────────────────────────
    /// Toggle the active session's perf sparkline panel (`t`) — a no-op with
    /// no active session.
    TogglePerfPanel,

    // ── Responsive breakpoints ────────────────────────────────────────────────
    /// Toggle the narrow-terminal sidebar overlay (`s` from the workbench, or
    /// `Esc` while it's open) — meaningless (but harmless) above
    /// `crate::ui::layout::NARROW_WIDTH`, where the sidebar already renders
    /// inline.
    ToggleSidebarOverlay,

    // ── Help overlay ──────────────────────────────────────────────────────────
    /// Open the keyboard/help overlay (`?` from either top-level screen).
    OpenHelpOverlay,
    /// Close the help overlay (`Esc` / `?` again).
    CloseHelpOverlay,

    // ── Embedded MCP server ───────────────────────────────────────────────────
    /// A question (or lifecycle request) from an embedded MCP server's
    /// [`crate::supervise::TuiSessionBackend`], carrying its own reply
    /// channel. **Served by `crate::runner`, not by [`super::update`]** — it
    /// needs the `Supervisor` the pure core deliberately cannot reach; see
    /// [`crate::supervise::serve_command`].
    Mcp(McpCommand),
    /// The embedded MCP server of this generation has its listener up on this
    /// port — the answer to `start_mcp`'s `ready` signal (the OS-assigned port
    /// when it was started on `0`).
    ///
    /// Applied only when the generation names the server currently installed
    /// on [`super::AppState::mcp`]; a report from a superseded one is dropped
    /// rather than stamping its port onto its successor.
    McpListening(u64, u16),
    /// The embedded MCP server of this generation returned: cleanly (`None`)
    /// or with the rendered error that ended it (`Some`).
    ///
    /// Clears the server handle from the model — but **only** when the
    /// generation names the handle installed there. A stop the user asked for
    /// has already cleared it (so this is a no-op), and a start that happened
    /// while the old server was still winding down installed a handle this
    /// report does not name (so it must not clear it: dropping a
    /// `CancellationToken` does not cancel it, and the new server would be
    /// left listening with nothing able to stop it).
    McpStopped(u64, Option<String>),
    /// Start the embedded MCP server, or stop the running one (`M`, the
    /// sidebar ACTIONS "MCP" row, the panel's Start/Stop button, or the
    /// palette) — workbook §B13. Routed to the runner as
    /// [`super::Effect::StartMcpServer`]/[`super::Effect::StopMcpServer`],
    /// since the server handle is a live resource the pure core cannot build.
    ToggleMcpServer,
    /// Open the MCP panel — the server's state plus its connected-client list
    /// (`m`, or the palette).
    OpenMcpPanel,
    /// Close the MCP panel (`Esc` / `m` / its Close button). The server keeps
    /// running: closing the panel is not a stop.
    CloseMcpPanel,

    // ── Embedded DAP server ───────────────────────────────────────────────────
    /// The embedded DAP server of this generation has its listener up on this
    /// port — the answer to `start_dap`'s `ready` signal (the OS-assigned port
    /// when it was started on `0`).
    ///
    /// Generation-gated exactly like [`Self::McpListening`], and for exactly
    /// the same reason.
    DapListening(u64, u16),
    /// The embedded DAP server of this generation returned: cleanly (`None`)
    /// or with the rendered error that ended it (`Some`).
    ///
    /// Generation-gated exactly like [`Self::McpStopped`]: a superseded
    /// server's late report must not clear its successor's handle, since
    /// dropping a `CancellationToken` does not cancel it and that successor
    /// would be left listening with nothing able to stop it.
    DapStopped(u64, Option<String>),
    /// Start the embedded DAP server, or stop the running one (`s` in the
    /// settings dialog, its Start/Stop button, or the palette) — the exact
    /// counterpart of [`Self::ToggleMcpServer`], routed to the runner as
    /// [`super::Effect::StartDapServer`]/[`super::Effect::StopDapServer`].
    ToggleDapServer,
    /// Decide, at startup, whether to start the DAP server without being
    /// asked: `enabled`, or `auto_start_in_ide` with an IDE detected (see
    /// [`super::should_auto_start`]). Sent once by `crate::runner` after the
    /// model is built, so the decision itself stays in the pure core.
    ///
    /// The *first* such start on an install opens the settings dialog with a
    /// one-time notice instead of binding a listener, and burns
    /// `[dap].intro_seen` (see
    /// [`super::DapSettings::intro_port`]); every launch after that starts
    /// silently.
    DapAutoStart,
    /// Open the DAP settings dialog (`D`, the sidebar ACTIONS "DAP" row, or
    /// the palette).
    OpenDapSettings,
    /// Close the DAP settings dialog (`Esc` / `D` / its Close button),
    /// committing any pending port edit on the way out. The server keeps
    /// running: closing the dialog is not a stop.
    CloseDapSettings,
    /// Move dialog focus to the next / previous control (`Tab` / `↓`, `↑`).
    DapSettingsFocusNext,
    /// Move dialog focus to the previous control.
    DapSettingsFocusPrev,
    /// Move dialog focus to a specific control (mouse click parity).
    DapSettingsFocus(DapFocus),
    /// Type a character into the dialog's port field.
    DapSettingsInput(char),
    /// Delete the last character of the dialog's port field.
    DapSettingsBackspace,
    /// Activate the focused control (`Enter`/`Space`): toggle the server or a
    /// checkbox, commit the port, cycle the IDE, or generate the config.
    DapSettingsActivate,
    /// Cycle the dialog's IDE selector by `delta` (`←`/`→`, or a click on the
    /// row).
    DapSettingsCycleIde(isize),
    /// Toggle the "auto-start in an IDE terminal" preference (checkbox click,
    /// or `Space`/`Enter` on it) — persisted immediately.
    DapSettingsToggleAutoStart,
    /// Toggle the "auto-configure the IDE" preference — persisted immediately.
    DapSettingsToggleAutoConfigure,
    /// Generate (or refresh) the IDE's DAP client config now (`g`, or the
    /// dialog's action) — the same path the automatic post-`DapListening`
    /// generation takes.
    DapSettingsGenerate,
    /// An IDE-config generation finished (or was refused before it started):
    /// its outcome, retained for the dialog's status area.
    DapIdeConfig(DapIdeReport),

    /// Show a toast from the runner — the seam an impure side effect the
    /// pure core cannot see into (e.g. `crate::clipboard::write`'s outcome)
    /// uses to reach `AppState::toasts`, since the runner has no direct
    /// access to the model. `update`'s handling is pure (just a
    /// `Toasts::push`); the runner decides the `level`/`text` and the
    /// moment to send it (a startup clipboard-unavailable warning, or a
    /// failed copy).
    Notify {
        /// The toast's severity.
        level: ToastKind,
        /// The toast's text.
        text: String,
    },
}
