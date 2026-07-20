//! TEA messages and the semantic region ids they are routed through.
//!
//! Every message is a *pure* input to [`super::update`]: the terminal event
//! loop (`crate::runner`) translates raw crossterm events + mouse-region
//! hit-tests into `Message`s, and the engine's single mutation point applies
//! them. Background tasks (the session supervisor) feed the same channel via
//! [`Message::Session`] — see [`super::Engine`] and [`crate::supervise`].

use std::path::PathBuf;

use frust_drive::devices::Device;

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
    /// screen (D6b). The wizard it opens is Phase 2 — the skeleton emits a
    /// toast.
    CreateButton,
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
    /// activated it — opens the create flow (Phase 2; toast for now).
    CreateActivate,
    /// A press that started on the Create button was released elsewhere —
    /// clears the pressed chrome without activating.
    CreateCancel,

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
}
