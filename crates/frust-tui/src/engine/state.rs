//! The application model — the single source of truth the render pass reads
//! (`&AppState`) and [`super::update`] mutates.

use std::fs;
use std::path::{Path, PathBuf};

use super::message::RegionId;

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
}

impl AppState {
    /// Build the initial model from the current working directory: zero
    /// `frust.toml` project markers found within the bounded walk opens the
    /// welcome screen, one or more opens the workbench with the list.
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::detect(&cwd)
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
            projects: Vec::new(),
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
