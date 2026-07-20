//! The application model — the single source of truth the render pass reads
//! (`&AppState`) and [`super::update`] mutates.

use std::fs;
use std::path::{Path, PathBuf};

use super::build_launcher::BuildLauncher;
use super::create_wizard::CreateWizard;
use super::doctor::DoctorState;
use super::message::RegionId;
use super::run_config::{DeviceRow, RunConfig};
use super::session_view::SessionView;
use crate::supervise::SessionId;

/// Bounded-walk depth cap for [`detect`]/[`find_projects`]: `dir` itself is
/// depth 0, its children depth 1, its grandchildren depth 2 — nothing past
/// that is ever read. Keeps a repo-root walk instant regardless of tree size.
const MAX_DETECT_DEPTH: u32 = 2;

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

/// The log search/filter overlay state (D5's log-view search/filter).
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
    /// A transient status message shown in the status bar. Cleared on the next
    /// meaningful interaction.
    pub toast: Option<String>,
    /// The active project root (`projects.first()`, the one the main area
    /// shows), if any. `None` on the welcome screen. Kept alongside
    /// `projects` for the call sites that only care about "the" open project
    /// (Phase 2 adds a full switcher letting the user change which one is
    /// active).
    pub project_root: Option<PathBuf>,
    /// Every project root the bounded [`detect`] walk found, in a
    /// deterministic (name-sorted) order; `projects[0]` is `project_root`.
    /// Empty on the welcome screen.
    pub projects: Vec<PathBuf>,
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
    /// Whether the titlebar project-switcher dropdown is open (F5 + D6b).
    /// While `true`, it captures input and suppresses background mouse
    /// regions, the same D4 base-layer suppression the run-config modal uses.
    pub project_switcher_open: bool,
    /// The highlighted row while the switcher is open (`↑`/`↓` move it,
    /// `Enter` switches to it); meaningless while closed.
    pub project_switcher_cursor: usize,
    /// The create-project wizard, when open (the welcome Create button, or the
    /// workbench "New project" action / `n`). While `Some`, it captures input
    /// and suppresses background mouse regions — the same D4 base-layer
    /// suppression the run-config modal uses.
    pub create_wizard: Option<CreateWizard>,
    /// The doctor panel's cached validator results + titlebar chip source
    /// (TUI2-07). Populated by a startup preflight and refreshed on demand;
    /// present regardless of whether the panel itself is open.
    pub doctor: DoctorState,
    /// Whether the doctor panel is open (`d` from the workbench, or the
    /// titlebar chip / sidebar "Doctor" action). While `true`, it captures
    /// input and suppresses background mouse regions like the other modals.
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
}

impl AppState {
    /// Build the initial model from the current working directory, merged
    /// with the persisted recent-projects list (PLAN.md D6b): a bounded walk
    /// (see [`Self::detect`]) finds every `frust.toml` marker under the cwd,
    /// then [`super::persist::merge_recent_and_detected`] prepends any
    /// still-existing recently-opened project not already found, deduped —
    /// "recent first, deduped". A cwd-detected project stays the active one
    /// (unsurprising `cd`-into-a-project-then-run behavior); with none
    /// detected, the most-recently-opened project opens instead of the
    /// welcome screen — "from any directory" per PLAN.md D6b. Only the
    /// welcome screen (no active project either way) skips persistence
    /// entirely.
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut state = Self::detect(&cwd);
        let detected_active = state.project_root.clone();
        let recent = super::persist::load_recent_projects();
        state.projects = super::persist::merge_recent_and_detected(&recent, &state.projects);
        state.project_root = detected_active.or_else(|| state.projects.first().cloned());
        if state.project_root.is_some() {
            state.screen = Screen::Workbench;
        }
        state
    }

    /// Detection core (testable without touching the process cwd/reading a
    /// real project tree): [`find_projects`] walks `dir` for `frust.toml`
    /// markers; the first found (if any) becomes the active `project_root`.
    pub fn detect(dir: &Path) -> Self {
        let projects = find_projects(dir);
        let project_root = projects.first().cloned();
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
            toast: None,
            project_root,
            projects,
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
        }
    }

    /// Whether any animation is in flight and the loop must keep drawing on
    /// each tick. The skeleton animates nothing, so this is always `false`
    /// (the single knob a later animated widget flips to opt out of the
    /// dirty-frame skip).
    pub fn animating(&self) -> bool {
        false
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

    /// The sessions grouped by project, preserving first-seen project order and
    /// each project's session order — the tab-bar / sidebar grouping (D5). Each
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
            toast: None,
            project_root: None,
            projects: Vec::new(),
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
        }
    }
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
}
