//! Right-click context menus: a small positioned popup whose
//! entries are built from the [`ContextTarget`] the click landed on.
//!
//! Like the command palette ([`super::palette`]), the menu is a *launcher*, not
//! a place new command semantics live: every entry carries an **existing**
//! [`Message`], and activating one re-dispatches that message through the same
//! `update` every other input path uses (see `super::update`). This keeps the
//! menu keyboard-reachable-by-parity — each entry has a palette command and/or
//! a keybinding — so a terminal that never delivers a right-click (many IDE
//! integrated terminals) loses no capability.
//!
//! ## Menu-entry → keyboard/palette parity
//!
//! | Target · entry            | Message              | Keyboard / palette parity     |
//! |---------------------------|----------------------|-------------------------------|
//! | Session tab · Select      | `SelectTab`          | `1`–`9`, `Tab`/`Shift+Tab`    |
//! | Session tab · Stop        | `StopSession`        | `x` / `Ctrl+C` · palette "Stop session" |
//! | Session tab · Follow      | `ToggleFollow`       | `f` · palette "Toggle follow-tail" |
//! | Session tab · Copy path   | `CopyBuiltArtifacts` | `c` (build session) / status-row affordance |
//! | Device row · Run…         | `OpenRunConfig`      | `r` / `Enter` · palette "Run on device(s)…" |
//! | Device row · Toggle select| `SelectDeviceAt`     | `Space` (device panel)        |
//! | Project row · Open        | `SwitchProject`      | sidebar click / switcher `Enter`/digit |
//! | Log view · Copy selection | `CopySelection`      | `y` · palette (via selection) |
//! | Log view · Follow         | `ToggleFollow`       | `f` · palette "Toggle follow-tail" |
//! | Log view · Search…        | `SearchOpen`         | `/` · palette "Search logs…"  |
//!
//! Entries the v1 target list names but no existing `Message` covers
//! (session-tab "restart"/"close tab", project-row "remove from recents") are
//! deliberately omitted rather than wired to a menu-only command — the same
//! discipline `super::palette::commands`'s doc comment records.

use super::message::{ContextTarget, Message};
use super::state::AppState;

/// One context-menu row: a fixed label, a short keyhint, the **existing**
/// [`Message`] it re-dispatches, and its enabled gate (a disabled row shows
/// muted and can't be activated — the same pattern as [`super::PaletteCommand`]).
///
/// `PartialEq` only, following [`Message`]'s own relaxation.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuEntry {
    /// The row label.
    pub label: &'static str,
    /// A short keybinding / context hint shown right-aligned.
    pub hint: &'static str,
    /// The message activating this row emits — routed through `update` unchanged.
    pub message: Message,
    /// Whether the entry is currently activatable.
    pub enabled: bool,
}

impl MenuEntry {
    fn on(label: &'static str, hint: &'static str, message: Message) -> Self {
        Self {
            label,
            hint,
            message,
            enabled: true,
        }
    }

    fn gated(label: &'static str, hint: &'static str, message: Message, enabled: bool) -> Self {
        Self {
            label,
            hint,
            message,
            enabled,
        }
    }
}

/// The open context menu: its anchor position, the target it was built for, its
/// entries, and the highlighted row.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextMenu {
    /// The column the menu's top-left corner prefers (clamped into the frame at
    /// render time).
    pub x: u16,
    /// The row the menu's top-left corner prefers (clamped at render time).
    pub y: u16,
    /// What was right-clicked (kept for reference / tests).
    pub target: ContextTarget,
    /// The menu rows, top to bottom.
    pub entries: Vec<MenuEntry>,
    /// The highlighted row (index into `entries`, clamped).
    pub cursor: usize,
}

impl ContextMenu {
    /// The inner content width (columns) needed for the widest `label + hint`
    /// row, plus separation — excludes the border.
    pub fn inner_width(&self) -> u16 {
        let widest = self
            .entries
            .iter()
            .map(|e| e.label.chars().count() + e.hint.chars().count() + 5)
            .max()
            .unwrap_or(8);
        widest as u16
    }

    /// The full popup width including the 1-col border on each side.
    pub fn width(&self) -> u16 {
        self.inner_width() + 2
    }

    /// The full popup height including the 1-row border top and bottom.
    pub fn height(&self) -> u16 {
        self.entries.len() as u16 + 2
    }

    /// Move the highlight up one row; returns whether it moved.
    pub fn cursor_up(&mut self) -> bool {
        if self.cursor > 0 {
            self.cursor -= 1;
            true
        } else {
            false
        }
    }

    /// Move the highlight down one row; returns whether it moved.
    pub fn cursor_down(&mut self) -> bool {
        if self.cursor + 1 < self.entries.len() {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    /// The highlighted entry, if any.
    pub fn selected(&self) -> Option<&MenuEntry> {
        self.entries.get(self.cursor)
    }
}

/// Build the entries for a right-clicked `target`, gated against `state`. An
/// empty result means "nothing to offer here" — the caller opens no menu.
pub fn entries_for(state: &AppState, target: ContextTarget) -> Vec<MenuEntry> {
    match target {
        ContextTarget::SessionTab(i) => {
            let Some(session) = state.sessions.get(i) else {
                return Vec::new();
            };
            let running = !session.state.is_terminal();
            let has_artifacts = !session.built_artifact_paths().is_empty();
            vec![
                MenuEntry::on("Select tab", "1-9", Message::SelectTab(i)),
                MenuEntry::gated("Stop session", "x", Message::StopSession, running),
                MenuEntry::on("Toggle follow-tail", "f", Message::ToggleFollow),
                MenuEntry::gated(
                    "Copy artifact path(s)",
                    "c",
                    Message::CopyBuiltArtifacts,
                    has_artifacts,
                ),
            ]
        }
        ContextTarget::DeviceRow(i) => {
            if i >= state.devices.len() {
                return Vec::new();
            }
            let has_project = state.project_root.is_some() || !state.projects.is_empty();
            vec![
                MenuEntry::gated(
                    "Run on device(s)…",
                    "r",
                    Message::OpenRunConfig,
                    has_project,
                ),
                MenuEntry::on("Toggle select", "space", Message::SelectDeviceAt(i)),
            ]
        }
        ContextTarget::ProjectRow(i) => {
            if i >= state.projects.len() {
                return Vec::new();
            }
            vec![MenuEntry::on(
                "Open project",
                "↵",
                Message::SwitchProject(i),
            )]
        }
        ContextTarget::LogView => {
            let Some(session) = state.active_session() else {
                return Vec::new();
            };
            let has_selection = session.selection.is_some();
            vec![
                MenuEntry::gated("Copy selection", "y", Message::CopySelection, has_selection),
                MenuEntry::on("Toggle follow-tail", "f", Message::ToggleFollow),
                MenuEntry::on("Search logs…", "/", Message::SearchOpen),
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::state::{AppState, Screen};
    use crate::supervise::{SessionId, SessionState};
    use std::path::PathBuf;

    fn workbench() -> AppState {
        let root = PathBuf::from("/tmp/huddle");
        AppState {
            screen: Screen::Workbench,
            project_root: Some(root.clone()),
            projects: vec![root],
            ..AppState::default()
        }
    }

    #[test]
    fn session_tab_entries_gate_stop_on_running() {
        let mut st = workbench();
        crate::engine::update(
            &mut st,
            Message::RegisterSession {
                id: SessionId(0),
                project_root: PathBuf::from("/tmp/huddle"),
                target_label: "desktop".into(),
                devtools: crate::engine::DevtoolsLaunch::unavailable(),
                target: None,
            },
        );
        st.sessions[0].state = SessionState::Running;
        let entries = entries_for(&st, ContextTarget::SessionTab(0));
        let stop = entries.iter().find(|e| e.label == "Stop session").unwrap();
        assert!(stop.enabled, "a running session's Stop is enabled");
        st.sessions[0].state = SessionState::Exited(true);
        let entries = entries_for(&st, ContextTarget::SessionTab(0));
        let stop = entries.iter().find(|e| e.label == "Stop session").unwrap();
        assert!(!stop.enabled, "an exited session's Stop is disabled");
    }

    #[test]
    fn every_entry_message_is_an_existing_one() {
        // A representative build: each entry must map to a message the engine
        // already handles (the launcher discipline). We just assert the menu is
        // non-empty for each target that has data.
        let mut st = workbench();
        st.devices.push(crate::engine::DeviceRow {
            device: frust_drive::devices::Device {
                id: "d".into(),
                name: "Pixel".into(),
                platform: frust_drive::devices::Platform::Android,
                kind: frust_drive::devices::Kind::Emulator,
                os_version: None,
                connection_state: None,
            },
            selected: false,
        });
        assert!(!entries_for(&st, ContextTarget::DeviceRow(0)).is_empty());
        assert!(!entries_for(&st, ContextTarget::ProjectRow(0)).is_empty());
        // No session / no active session → log-view and session-tab menus empty.
        assert!(entries_for(&st, ContextTarget::LogView).is_empty());
        assert!(entries_for(&st, ContextTarget::SessionTab(0)).is_empty());
    }

    #[test]
    fn cursor_moves_and_clamps() {
        let mut menu = ContextMenu {
            x: 0,
            y: 0,
            target: ContextTarget::LogView,
            entries: vec![
                MenuEntry::on("a", "", Message::ToggleFollow),
                MenuEntry::on("b", "", Message::SearchOpen),
            ],
            cursor: 0,
        };
        assert!(!menu.cursor_up());
        assert!(menu.cursor_down());
        assert_eq!(menu.cursor, 1);
        assert!(!menu.cursor_down());
        assert!(menu.cursor_up());
        assert_eq!(menu.cursor, 0);
    }
}
