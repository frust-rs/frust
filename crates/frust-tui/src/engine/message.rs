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

use super::build_launcher::BuildFocus;
use super::doctor::DoctorCheck;
use super::run_config::RunFocus;
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
    /// screen (D6b); opens the create wizard.
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
    /// The titlebar toolchain chip; click opens the bootstrap wizard (D6a —
    /// the workbook's toolchain chip → bootstrap flow; the doctor detail panel
    /// stays reachable via `d` / the sidebar "Doctor" action).
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
    /// The sidebar "Doctor" action row; click opens the doctor panel.
    DoctorAction,
    /// The doctor panel's re-run affordance.
    DoctorRerun,
    /// The doctor panel's close button.
    DoctorClose,
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
    /// The log status row's "copy built artifact path(s)" affordance.
    CopyArtifactsAction,
    /// A command row in the open command palette (0-based index into the
    /// ranked result list); click executes it.
    PaletteRow(usize),
    /// The command palette's `[Esc] Close` title affordance.
    PaletteClose,
}

/// A TEA message: the only way `AppState` ever changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Quit requested (`q` / `Ctrl+Q`). Sets `should_quit`; the loop exits.
    Quit,
    /// A tick from the frame interval. The skeleton has no animation, so this
    /// is a no-op (dirty-frame skip keeps it from forcing a draw).
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

    // ── Create-project wizard (D6b) ─────────────────────────────────────────
    /// Open the create wizard (the workbench "New project" action / `n`, or
    /// the palette). Primes an off-thread clean-signals sibling probe.
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
    /// The off-thread sibling probe finished: whether the `../clean-signals-rs`
    /// sibling checkout is present (gates the clean-signals arch card).
    CleanSignalsProbed(bool),
    /// The off-thread scaffold succeeded; open the new project in place.
    ScaffoldSucceeded {
        /// The absolute root of the freshly scaffolded project.
        project_root: PathBuf,
    },
    /// The off-thread scaffold failed; the wizard shows the error and offers a
    /// retry.
    ScaffoldFailed(String),

    // ── Sessions (D2/D6b): the supervisor feeds `Session`; the rest are
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
    /// Toggle follow-tail on the active session's log view (`f`).
    ToggleFollow,
    /// Toggle soft-wrap on the log view (`w`).
    ToggleWrap,
    /// Scroll the active log view up / down by `n` lines (wheel / arrows).
    LogScrollUp(u64),
    /// Scroll the active log view down by `n` lines.
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
    /// Begin a copy-while-scrolling selection at the newest line (`v`).
    SelectionBegin,
    /// Extend the selection toward older lines (`Shift+Up`).
    SelectionExtendUp(u64),
    /// Extend the selection toward newer lines (`Shift+Down`).
    SelectionExtendDown(u64),
    /// Clear the current selection (`Esc`).
    SelectionClear,
    /// Copy the current selection to the clipboard (`y`) — routed to the runner
    /// as an [`super::Effect::Copy`].
    CopySelection,

    // ── Devices panel + run-config modal (D6b) ──────────────────────────────
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
    /// Open the run-config modal, primed from the panel selection (`Enter`
    /// from the devices panel).
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

    // ── Project switcher + recent-projects persistence (F5 / D6b) ──────────
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

    // ── Doctor panel + titlebar chip (TUI2-07) ──────────────────────────────
    /// Run the validator set off-thread (`d` from the workbench, the sidebar
    /// "Doctor" action, the titlebar chip, or the panel's re-run affordance)
    /// — routed to the runner as [`super::Effect::RunDoctor`]. Also fired
    /// once at startup (`crate::runner`) to seed the chip.
    RunDoctor,
    /// The off-thread validator run finished: replace the cached results.
    DoctorResults(Vec<DoctorCheck>),
    /// Open the doctor panel.
    OpenDoctorPanel,
    /// Close the doctor panel (`Esc`).
    CloseDoctorPanel,

    // ── Bootstrap wizard + titlebar toolchain chip (D6a) ───────────────────
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
    /// Open the bootstrap wizard (the `i` key, or a titlebar toolchain-chip
    /// click) — seeds it from the cached report, requesting a preflight first
    /// if none is cached yet.
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

    // ── Build launcher (TUI2-07) ────────────────────────────────────────────
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

    // ── Clean confirm dialog (TUI2-07) ──────────────────────────────────────
    /// Open the clean-confirm dialog for the active project (`c` from the
    /// workbench, or the sidebar "Clean" action).
    OpenCleanConfirm,
    /// Close the dialog without cleaning (`Esc`).
    CloseCleanConfirm,
    /// Confirm the clean (`Enter`/`y`) — routed to the runner as
    /// [`super::Effect::RunClean`].
    ConfirmClean,

    // ── Build artifact copy-path (TUI2-07) ──────────────────────────────────
    /// Copy the active (build) session's reported artifact path(s) to the
    /// clipboard (`c` on a session tab with built artifacts, or the log
    /// status row's copy affordance) — routed to the runner as
    /// [`super::Effect::Copy`].
    CopyBuiltArtifacts,

    // ── Command palette (D5) ─────────────────────────────────────────────────
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

    // ── Run on all devices (D6b palette / device-header action) ──────────────
    /// Launch one supervised session per discovered device (debug mode) for the
    /// active project — the palette's "run on all devices" action, routed to the
    /// runner as [`super::Effect::LaunchSessions`]. A no-op with no project or
    /// no discovered devices.
    RunOnAllDevices,
}
