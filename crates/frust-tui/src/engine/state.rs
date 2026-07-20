//! The application model — the single source of truth the render pass reads
//! (`&AppState`) and [`super::update`] mutates.

use std::path::{Path, PathBuf};

use super::message::RegionId;

/// The top-level screen the workbench is showing.
///
/// The skeleton has two: the [`Screen::Welcome`] splash (no project detected)
/// and the [`Screen::Workbench`] shell (a project is open). Phase 2 adds the
/// project switcher, sessions, modals, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Run-from-anywhere splash with the single Create button (D6b).
    Welcome,
    /// The titlebar/sidebar/main/status workbench shell (static in Phase 1).
    Workbench,
}

/// The toast shown when the (Phase 2) create wizard is requested from the
/// skeleton.
pub const CREATE_TOAST: &str = "Create wizard arrives in Phase 2";

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
    /// A transient status message shown in the status bar. Cleared on the next
    /// meaningful interaction.
    pub toast: Option<String>,
    /// The detected project root (`frust.toml` dir), if any. `None` on the
    /// welcome screen.
    pub project_root: Option<PathBuf>,
}

impl AppState {
    /// Build the initial model from the current working directory: a
    /// `frust.toml` in `cwd` opens the workbench, otherwise the welcome screen.
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::detect(&cwd)
    }

    /// Detection core (testable without touching the process cwd): a
    /// `frust.toml` marker in `dir` means a project is open.
    pub fn detect(dir: &Path) -> Self {
        let project_root = dir.join("frust.toml").is_file().then(|| dir.to_path_buf());
        let screen = if project_root.is_some() {
            Screen::Workbench
        } else {
            Screen::Welcome
        };
        Self {
            screen,
            should_quit: false,
            hover: None,
            create_pressed: false,
            toast: None,
            project_root,
        }
    }

    /// Whether any animation is in flight and the loop must keep drawing on
    /// each tick. The skeleton animates nothing, so this is always `false`
    /// (the single knob a later animated widget flips to opt out of the
    /// dirty-frame skip).
    pub fn animating(&self) -> bool {
        false
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
            toast: None,
            project_root: None,
        }
    }
}
