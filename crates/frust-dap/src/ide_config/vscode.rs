//! VS Code DAP configuration generator.
//!
//! Generates or merges a `.vscode/launch.json` entry so that VS Code, VS Code
//! Insiders, and Cursor can connect to the embedded DAP server (there is no
//! standalone `frust dap` process — see `docs/CLI_ARCHITECTURE.md`'s
//! `frust-dap` row) via the `debugServer` field (a VS Code-internal mechanism
//! that redirects the debug adapter transport to an already-running TCP
//! server, rather than spawning one). `request` is `"launch"`, not
//! `"attach"` — the frust DAP server is launch-based, and there is no
//! separate "already running app" for VS Code to attach to. The entry
//! carries no `cwd`/`projectRoot` field: every launch builds from the
//! workbench's currently open project, read live from the host's backend at
//! launch time, and a client-supplied `projectRoot` is never honored
//! (`docs/LIMITATIONS.md`'s `dap-tcp-unauthenticated-v1`) — so there is
//! nothing for VS Code to tell it.
//!
//! **The merge is semantic, not byte-preserving.** [`VSCodeGenerator::merge_config`]
//! parses the existing file as JSONC ([`super::merge::clean_jsonc`]) and
//! reprints the whole document ([`super::merge::to_pretty_json`]) — every
//! non-frust entry's *data* survives, but comments and hand formatting in a
//! pre-existing, hand-authored `launch.json` do not survive the first merge.
//! See `docs/LIMITATIONS.md`'s `dap-ide-config-normalizes-launchjson`.

use std::path::{Path, PathBuf};

use serde_json::json;

use super::merge::{
    FRUST_CONFIG_NAME, clean_jsonc, find_json_entry_by_field, merge_json_array_entry,
    to_pretty_json,
};
use super::{IdeConfigError, IdeConfigGenerator, Result};

/// Abstracts home-directory environment lookups so [`detect_workspace_root`]'s
/// boundary check is
/// testable without mutating the real, global process environment —
/// `std::env::set_var` is `unsafe` as of this workspace's edition/toolchain,
/// and this crate stays `unsafe`-free (`docs/CODE_STANDARDS.md`'s
/// sanctioned-unsafe list has no entry for `frust-dap`). Mirrors
/// [`super::detect`]'s own `EnvLookup` seam, kept local here since that one
/// is private to the `detect` module.
pub(crate) trait EnvLookup {
    fn get(&self, key: &str) -> Option<String>;
}

pub(crate) struct RealEnv;

impl EnvLookup for RealEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

/// Resolve the user's home directory — [`frust_drive::host_path::home_dir_from`]'s
/// shared `HOME`/`USERPROFILE`/`HOMEDRIVE`+`HOMEPATH` chain, fed by this
/// module's own [`EnvLookup`] seam. The same resolver backs
/// `frust-tui`'s `engine::persist::home_dir` and `frust-drive`'s
/// `android_run::home_dir`, so the three can no longer drift apart.
///
/// Returns `None` only when none of those resolve to a non-empty value.
/// Callers treat that as "the containment boundary is unknown" and **fail
/// closed** — [`detect_workspace_root`] considers no ancestor at all, and
/// [`guard_workspace_root`] refuses any root other than the project's own —
/// because the failure mode of guessing is the exact escape the boundary
/// exists to prevent: a walk that climbs into a user's home directory, which
/// holds a `.vscode/` for essentially every VS Code user.
fn home_dir(env: &dyn EnvLookup) -> Option<PathBuf> {
    frust_drive::host_path::home_dir_from(|key| env.get(key))
}

/// Detect the workspace root for a frust project.
///
/// Walks up from `project_root` — starting at `project_root` itself — looking
/// for a directory that contains either `.vscode/` (existing VS Code
/// workspace) or `.git/` (repository root). Returns the nearest ancestor
/// (project-inclusive) that qualifies, or `project_root` itself if no
/// workspace marker is found.
///
/// # Priority
///
/// 1. A directory with `.vscode/` is preferred (VS Code was opened there),
///    checked nearest-first — `project_root`'s own `.vscode/` always wins
///    over an ancestor's.
/// 2. A directory with `.git/` is the fallback (likely the repo root), same
///    nearest-first order.
/// 3. If neither is found, the project root is returned unchanged.
///
/// # Home-directory boundary
///
/// The walk never climbs to (or past) the resolved `$HOME` directory: an
/// ancestor equal to `$HOME`, or above it, is never treated as a candidate,
/// even if it happens to contain `.vscode/` (VS Code's own extensions
/// directory, present for essentially every VS Code user) or `.git/` (a
/// dotfiles-tracked home). `project_root` itself is exempt from this
/// boundary — a project-local marker always wins even if the project happens
/// to live directly at `$HOME`. Without this boundary, the home directory
/// itself would silently swallow the walk and every subsequent DAP config
/// write for that user would land in `$HOME/.vscode/launch.json` (and
/// Neovim's `$HOME/.nvim-dap.lua`) instead of the project.
///
/// When [`home_dir`] resolves nothing at all, the walk **fails closed**: no
/// ancestor is considered and `project_root` is returned unchanged. An
/// unbounded walk is precisely the escape above, so an unknown boundary
/// forfeits ancestor detection (a monorepo root goes undetected, and the
/// config lands in the project) rather than risking the home directory.
pub fn detect_workspace_root(project_root: &Path) -> PathBuf {
    detect_workspace_root_with(project_root, &RealEnv)
}

pub(crate) fn detect_workspace_root_with(project_root: &Path, env: &dyn EnvLookup) -> PathBuf {
    let canonical = match project_root.canonicalize() {
        Ok(p) => p,
        Err(_) => return project_root.to_path_buf(),
    };

    // No resolvable (and canonicalizable) home means no known boundary, so the
    // walk is clamped to the project itself — every ancestor path below is
    // skipped rather than climbed unbounded.
    let Some(home) = home_dir(env).and_then(|h| h.canonicalize().ok()) else {
        return canonical;
    };

    let mut git_root: Option<PathBuf> = None;

    for ancestor in canonical.ancestors() {
        // The project's own directory is always inspected first, even if it
        // happens to sit at (or above) the home boundary below. Only
        // ancestors climbed to *beyond* the project are subject to it.
        if ancestor != canonical && (ancestor == home.as_path() || home.starts_with(ancestor)) {
            break;
        }

        if ancestor.join(".vscode").is_dir() {
            return ancestor.to_path_buf();
        }
        if git_root.is_none() && ancestor.join(".git").exists() {
            git_root = Some(ancestor.to_path_buf());
        }
    }

    git_root.unwrap_or(canonical)
}

/// Defense-in-depth: independently verify a detected workspace root never
/// lands at (or above) the resolved home boundary, regardless of how it was
/// derived — guards against a future regression in [`detect_workspace_root`]
/// silently reintroducing the home-swallows-the-config bug.
/// `project_root`'s own (canonicalized) root is always exempt: a project that
/// happens to live directly at the home directory is not an escape, it *is*
/// the project.
///
/// With no resolvable home ([`home_dir`] answering `None`) this **fails
/// closed** too: the project's own root is the only root it will approve, so
/// an unknown boundary can never be an approval of everything.
///
/// Called by [`VSCodeGenerator::generate`]/[`VSCodeGenerator::merge_config`]
/// (the `run_generator` write path) and by
/// [`super::neovim::NeovimGenerator::write_nvim_dap_lua`] before either
/// writes a file, so a violation refuses the write rather than merely
/// logging it.
pub(crate) fn guard_workspace_root(workspace_root: &Path, project_root: &Path) -> Result<()> {
    guard_workspace_root_with(workspace_root, project_root, &RealEnv)
}

pub(crate) fn guard_workspace_root_with(
    workspace_root: &Path,
    project_root: &Path,
    env: &dyn EnvLookup,
) -> Result<()> {
    let canonical_project = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    if workspace_root == canonical_project {
        return Ok(());
    }

    let Some(home) = home_dir(env).and_then(|h| h.canonicalize().ok()) else {
        return Err(IdeConfigError::message(format!(
            "refusing to write DAP config: no home directory could be resolved (none of HOME, \
             USERPROFILE, HOMEDRIVE+HOMEPATH names an existing directory), so the containment \
             boundary is unknown and only the project's own root (`{}`) can be approved — \
             detected workspace root `{}` is not it",
            canonical_project.display(),
            workspace_root.display(),
        )));
    };

    if workspace_root == home || home.starts_with(workspace_root) {
        return Err(IdeConfigError::message(format!(
            "refusing to write DAP config: detected workspace root `{}` is at or above \
             the home directory (`{}`)",
            workspace_root.display(),
            home.display(),
        )));
    }

    Ok(())
}

/// Generates `.vscode/launch.json` DAP config for VS Code, VS Code Insiders,
/// and Cursor.
///
/// Uses the `debugServer` field, which tells VS Code to connect to an
/// already-running DAP server on the given port instead of spawning a debug
/// adapter process. `editors/vscode-frust` must be installed (provides
/// `"type": "frust"`).
pub struct VSCodeGenerator;

impl VSCodeGenerator {
    /// Build the frust launch configuration entry for a given port.
    fn frust_entry(port: u16) -> serde_json::Value {
        json!({
            "name": FRUST_CONFIG_NAME,
            "type": "frust",
            "request": "launch",
            "debugServer": port
        })
    }
}

/// `existing`'s `"Frust (TUI DAP)"` launch entry, found by the marker
/// [`VSCodeGenerator::merge_config`] matches on. An empty file, or one with
/// no `configurations` key, has none; a `configurations` that is not an
/// array is the same error `merge_config` reports.
fn frust_launch_entry(existing: &str) -> Result<Option<serde_json::Value>> {
    if existing.trim().is_empty() {
        return Ok(None);
    }
    let root: serde_json::Value = serde_json::from_str(&clean_jsonc(existing))?;
    let Some(configurations) = root.get("configurations") else {
        return Ok(None);
    };
    let configurations = configurations
        .as_array()
        .ok_or_else(|| IdeConfigError::message("`configurations` is not an array"))?;
    Ok(
        find_json_entry_by_field(configurations, "name", FRUST_CONFIG_NAME)
            .map(|idx| configurations[idx].clone()),
    )
}

impl IdeConfigGenerator for VSCodeGenerator {
    fn config_path(&self, project_root: &Path) -> PathBuf {
        let workspace_root = detect_workspace_root(project_root);
        workspace_root.join(".vscode").join("launch.json")
    }

    fn generate(&self, port: u16, project_root: &Path) -> Result<String> {
        let workspace_root = detect_workspace_root(project_root);
        guard_workspace_root(&workspace_root, project_root)?;

        let config = json!({
            "version": "0.2.0",
            "configurations": [Self::frust_entry(port)]
        });
        Ok(to_pretty_json(&config))
    }

    fn merge_config(&self, existing: &str, port: u16, project_root: &Path) -> Result<String> {
        // Treat an empty/whitespace-only file as a fresh generation.
        if existing.trim().is_empty() {
            return self.generate(port, project_root);
        }

        let workspace_root = detect_workspace_root(project_root);
        guard_workspace_root(&workspace_root, project_root)?;

        // Strip JSONC comments and trailing commas before parsing.
        let clean = clean_jsonc(existing);
        let mut root: serde_json::Value = serde_json::from_str(&clean)?;

        if root.get("configurations").is_none() {
            root["configurations"] = json!([]);
        }

        let configurations = root["configurations"]
            .as_array_mut()
            .ok_or_else(|| IdeConfigError::message("`configurations` is not an array"))?;

        merge_json_array_entry(
            configurations,
            "name",
            FRUST_CONFIG_NAME,
            Self::frust_entry(port),
        );

        Ok(to_pretty_json(&root))
    }

    /// Whether `existing`'s `configurations` already hold an entry named
    /// `"Frust (TUI DAP)"` — the marker [`merge_config`](Self::merge_config)
    /// matches on. An empty file, or one with no `configurations` key, has
    /// none; a `configurations` that is not an array is the same error
    /// `merge_config` reports.
    fn has_frust_entry(&self, existing: &str) -> Result<bool> {
        Ok(frust_launch_entry(existing)?.is_some())
    }

    /// The `debugServer` port of `existing`'s `"Frust (TUI DAP)"` entry —
    /// `None` when there is no such entry, or when its `debugServer` is not
    /// a number in `u16` range (a hand-edited entry).
    fn frust_entry_port(&self, existing: &str) -> Result<Option<u16>> {
        Ok(frust_launch_entry(existing)?
            .and_then(|entry| entry.get("debugServer").and_then(serde_json::Value::as_u64))
            .and_then(|port| u16::try_from(port).ok()))
    }

    /// The detected workspace root — where `.vscode/` lives, already vetted
    /// against the home boundary by [`guard_workspace_root`].
    fn containment_root(&self, project_root: &Path) -> PathBuf {
        detect_workspace_root(project_root)
    }

    fn ide_name(&self) -> &'static str {
        "VS Code"
    }
}

// ─────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ide_config::test_support::unique_temp_dir;
    use std::collections::HashMap;

    /// In-memory [`EnvLookup`] fixture — the same shape
    /// `detect::tests::FakeEnv` uses for the same reason (a fake `HOME`
    /// value here, since paths are dynamically generated per test rather
    /// than `&'static str`).
    struct FakeEnv(HashMap<&'static str, String>);

    impl FakeEnv {
        fn new() -> Self {
            Self(HashMap::new())
        }

        fn set(mut self, key: &'static str, value: impl Into<String>) -> Self {
            self.0.insert(key, value.into());
            self
        }
    }

    impl EnvLookup for FakeEnv {
        fn get(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
    }

    // ── detect_workspace_root ────────────────────────────────────

    #[test]
    fn test_detect_workspace_root_project_has_vscode_returns_project() {
        // detect_workspace_root now inspects project_root itself first, so a
        // project-local .vscode/ matches immediately (review Major E, fix
        // item 1).
        let dir = unique_temp_dir("vscode-self-vscode");
        std::fs::create_dir_all(dir.join(".vscode")).unwrap();
        let detected = detect_workspace_root(&dir);
        assert_eq!(
            detected.canonicalize().unwrap(),
            dir.canonicalize().unwrap()
        );
    }

    #[test]
    fn test_detect_workspace_root_project_own_vscode_wins_over_ancestor() {
        // (a) Project-local .vscode/ wins even when an ancestor ALSO has one
        // — project_root is now checked first (nearest-first walk),
        // regressing the pre-fix `skip(1)` bug where the project's own
        // markers were never considered at all.
        let root = unique_temp_dir("vscode-own-wins-over-ancestor");
        let project = root.join("app");
        std::fs::create_dir_all(project.join(".vscode")).unwrap();
        std::fs::create_dir_all(root.join(".vscode")).unwrap();

        let detected = detect_workspace_root(&project);
        assert_eq!(
            detected.canonicalize().unwrap(),
            project.canonicalize().unwrap()
        );
    }

    #[test]
    fn test_detect_workspace_root_parent_has_vscode() {
        // Layout: workspace/.vscode/  workspace/app/  (project_root = workspace/app)
        let workspace = unique_temp_dir("vscode-parent-vscode");
        let project = workspace.join("app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(workspace.join(".vscode")).unwrap();

        let detected = detect_workspace_root(&project);
        assert_eq!(
            detected.canonicalize().unwrap(),
            workspace.canonicalize().unwrap()
        );
    }

    #[test]
    fn test_detect_workspace_root_grandparent_has_vscode() {
        // Layout: repo/.vscode/  repo/packages/myapp/
        let repo = unique_temp_dir("vscode-grandparent-vscode");
        let project = repo.join("packages").join("myapp");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(repo.join(".vscode")).unwrap();

        let detected = detect_workspace_root(&project);
        assert_eq!(
            detected.canonicalize().unwrap(),
            repo.canonicalize().unwrap()
        );
    }

    #[test]
    fn test_detect_workspace_root_parent_has_git() {
        // Layout: repo/.git/  repo/app/  (no .vscode anywhere)
        let repo = unique_temp_dir("vscode-parent-git");
        let project = repo.join("app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();

        let detected = detect_workspace_root(&project);
        assert_eq!(
            detected.canonicalize().unwrap(),
            repo.canonicalize().unwrap()
        );
    }

    #[test]
    fn test_detect_workspace_root_grandparent_has_git() {
        // Layout: repo/.git/  repo/packages/app/
        let repo = unique_temp_dir("vscode-grandparent-git");
        let project = repo.join("packages").join("app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();

        let detected = detect_workspace_root(&project);
        assert_eq!(
            detected.canonicalize().unwrap(),
            repo.canonicalize().unwrap()
        );
    }

    #[test]
    fn test_detect_workspace_root_no_git_no_vscode_returns_project() {
        let root = unique_temp_dir("vscode-no-markers");
        let project = root.join("app");
        std::fs::create_dir_all(&project).unwrap();

        let detected = detect_workspace_root(&project);
        assert_eq!(
            detected.canonicalize().unwrap(),
            project.canonicalize().unwrap()
        );
    }

    #[test]
    fn test_detect_workspace_root_vscode_preferred_over_git() {
        // Layout: repo/.git/  repo/workspace/.vscode/  repo/workspace/app/
        // .vscode is nearer to project_root than .git => .vscode wins.
        let repo = unique_temp_dir("vscode-preferred-over-git");
        let workspace = repo.join("workspace");
        let project = workspace.join("app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(workspace.join(".vscode")).unwrap();

        let detected = detect_workspace_root(&project);
        assert_eq!(
            detected.canonicalize().unwrap(),
            workspace.canonicalize().unwrap()
        );
    }

    // ── detect_workspace_root: $HOME boundary (review Major E) ──

    #[test]
    fn test_detect_workspace_root_stops_at_home_git_only_project_root_wins() {
        // (b) Layout: <tmp-home>/dev/myapp/.git  <tmp-home>/.vscode
        // project = <tmp-home>/dev/myapp. myapp's own .git wins; the walk
        // must never climb past <tmp-home> to reach <tmp-home>/.vscode, even
        // though it would otherwise "win" by matching before .git's fallback
        // priority — the home boundary stops the climb first.
        let home = unique_temp_dir("vscode-home-boundary-b");
        let project = home.join("dev").join("myapp");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::fs::create_dir_all(home.join(".vscode")).unwrap();

        let env = FakeEnv::new().set("HOME", home.canonicalize().unwrap().to_string_lossy());
        let detected = detect_workspace_root_with(&project, &env);

        assert_eq!(
            detected,
            project.canonicalize().unwrap(),
            "detected root must be the project itself, not <tmp-home>"
        );
    }

    #[test]
    fn test_detect_workspace_root_dotfiles_git_home_not_captured() {
        // (c) Layout: <tmp-home>/.git (a dotfiles-tracked home)
        // <tmp-home>/dev/myapp has no local markers at all. The dotfiles
        // .git at home must not capture the config — falls back to the
        // project root unchanged rather than climbing to <tmp-home>.
        let home = unique_temp_dir("vscode-home-boundary-c");
        let project = home.join("dev").join("myapp");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(home.join(".git")).unwrap();

        let env = FakeEnv::new().set("HOME", home.canonicalize().unwrap().to_string_lossy());
        let detected = detect_workspace_root_with(&project, &env);

        assert_eq!(
            detected,
            project.canonicalize().unwrap(),
            "a dotfiles .git at $HOME must not be treated as the workspace root"
        );
    }

    #[test]
    fn test_detect_workspace_root_legitimate_ancestor_below_home_still_works() {
        // (d) Layout: <tmp-home>/dev/monorepo/.vscode
        //             <tmp-home>/dev/monorepo/apps/myapp (no local markers)
        // The monorepo root is a legitimate ancestor climb — it sits BELOW
        // $HOME, not at or above it, so the boundary must not block it.
        let home = unique_temp_dir("vscode-home-boundary-d");
        let monorepo = home.join("dev").join("monorepo");
        let project = monorepo.join("apps").join("myapp");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(monorepo.join(".vscode")).unwrap();

        let env = FakeEnv::new().set("HOME", home.canonicalize().unwrap().to_string_lossy());
        let detected = detect_workspace_root_with(&project, &env);

        assert_eq!(
            detected,
            monorepo.canonicalize().unwrap(),
            "a legitimate ancestor below $HOME must still resolve"
        );
    }

    #[test]
    fn test_detect_workspace_root_project_at_home_itself_uses_own_markers() {
        // project_root == $HOME is exempt from the boundary: a project's own
        // markers always win even when the project happens to live directly
        // at home.
        let home = unique_temp_dir("vscode-home-boundary-project-is-home");
        std::fs::create_dir_all(home.join(".git")).unwrap();

        let env = FakeEnv::new().set("HOME", home.canonicalize().unwrap().to_string_lossy());
        let detected = detect_workspace_root_with(&home, &env);

        assert_eq!(detected, home.canonicalize().unwrap());
    }

    #[test]
    fn test_detect_workspace_root_userprofile_resolves_the_boundary() {
        // Windows sets USERPROFILE, not HOME. The boundary must hold there
        // too: <tmp-home>/.vscode must not capture a project below it.
        let home = unique_temp_dir("vscode-home-boundary-userprofile");
        let project = home.join("dev").join("myapp");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::fs::create_dir_all(home.join(".vscode")).unwrap();

        let env = FakeEnv::new().set(
            "USERPROFILE",
            home.canonicalize().unwrap().to_string_lossy(),
        );
        let detected = detect_workspace_root_with(&project, &env);

        assert_eq!(
            detected,
            project.canonicalize().unwrap(),
            "USERPROFILE must resolve the home boundary when HOME is unset"
        );
    }

    #[test]
    fn test_detect_workspace_root_homedrive_homepath_resolves_the_boundary() {
        // The older Windows pair, concatenated (`C:` + `\Users\someone`). The
        // fake values here are POSIX-shaped so the canonicalize step can
        // actually resolve on the test host; what is under test is the
        // two-variable fallback, not Windows path syntax.
        let home = unique_temp_dir("vscode-home-boundary-homedrive");
        let project = home.join("dev").join("myapp");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::fs::create_dir_all(home.join(".vscode")).unwrap();

        let canonical_home = home.canonicalize().unwrap();
        let home_text = canonical_home.to_string_lossy().into_owned();
        let (drive, rest) = home_text.split_at(1);
        let env = FakeEnv::new().set("HOMEDRIVE", drive).set("HOMEPATH", rest);
        let detected = detect_workspace_root_with(&project, &env);

        assert_eq!(
            detected,
            project.canonicalize().unwrap(),
            "HOMEDRIVE+HOMEPATH must resolve the home boundary when HOME/USERPROFILE are unset"
        );
    }

    #[test]
    fn test_detect_workspace_root_no_home_env_fails_closed_to_project_root() {
        // No home variable resolves at all: the boundary is unknown, so the
        // walk considers no ancestor rather than climbing unbounded — even an
        // ancestor carrying .vscode/ (the very shape a home directory has for
        // essentially every VS Code user) must not be detected.
        let repo = unique_temp_dir("vscode-home-boundary-no-home");
        let project = repo.join("app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(repo.join(".vscode")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();

        let env = FakeEnv::new(); // no HOME, USERPROFILE, HOMEDRIVE/HOMEPATH
        let detected = detect_workspace_root_with(&project, &env);

        assert_eq!(
            detected,
            project.canonicalize().unwrap(),
            "an unknown home boundary must clamp the walk to the project itself"
        );
    }

    #[test]
    fn test_detect_workspace_root_empty_home_env_fails_closed() {
        // Set-but-empty is the same as unset for every variable consulted.
        let repo = unique_temp_dir("vscode-home-boundary-empty-home");
        let project = repo.join("app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(repo.join(".vscode")).unwrap();

        let env = FakeEnv::new().set("HOME", "").set("USERPROFILE", "");
        let detected = detect_workspace_root_with(&project, &env);

        assert_eq!(detected, project.canonicalize().unwrap());
    }

    // ── guard_workspace_root: defense-in-depth ───────────────────

    #[test]
    fn test_guard_workspace_root_rejects_root_equal_to_home() {
        let home = unique_temp_dir("vscode-guard-root-equal-home");
        let project = home.join("dev").join("myapp");
        std::fs::create_dir_all(&project).unwrap();
        let canonical_home = home.canonicalize().unwrap();

        let env = FakeEnv::new().set("HOME", canonical_home.to_string_lossy());
        // Deliberately fabricate an out-of-bounds root, independent of
        // detect_workspace_root_with, to prove the guard itself refuses —
        // this is the case a future regression in the walk could reproduce.
        let result = guard_workspace_root_with(&canonical_home, &project, &env);

        assert!(result.is_err(), "expected a refusal, got {result:?}");
    }

    #[test]
    fn test_guard_workspace_root_rejects_root_above_home() {
        let home = unique_temp_dir("vscode-guard-root-above-home")
            .join("users")
            .join("someone");
        std::fs::create_dir_all(&home).unwrap();
        let project = home.join("dev").join("myapp");
        std::fs::create_dir_all(&project).unwrap();
        let canonical_home = home.canonicalize().unwrap();
        let above_home = canonical_home.parent().unwrap().to_path_buf();

        let env = FakeEnv::new().set("HOME", canonical_home.to_string_lossy());
        let result = guard_workspace_root_with(&above_home, &project, &env);

        assert!(result.is_err(), "expected a refusal, got {result:?}");
    }

    #[test]
    fn test_guard_workspace_root_accepts_root_below_home() {
        let home = unique_temp_dir("vscode-guard-root-below-home");
        let root = home.join("dev").join("monorepo");
        let project = root.join("apps").join("myapp");
        std::fs::create_dir_all(&project).unwrap();

        let env = FakeEnv::new().set("HOME", home.canonicalize().unwrap().to_string_lossy());
        let result = guard_workspace_root_with(&root.canonicalize().unwrap(), &project, &env);

        assert!(result.is_ok(), "expected Ok, got {result:?}");
    }

    #[test]
    fn test_guard_workspace_root_accepts_project_root_even_if_home() {
        let home = unique_temp_dir("vscode-guard-project-is-home");
        std::fs::create_dir_all(&home).unwrap();

        let env = FakeEnv::new().set("HOME", home.canonicalize().unwrap().to_string_lossy());
        let result = guard_workspace_root_with(&home.canonicalize().unwrap(), &home, &env);

        assert!(result.is_ok(), "project-local root must always be exempt");
    }

    #[test]
    fn test_guard_workspace_root_no_home_env_refuses_any_non_project_root() {
        // An unknown boundary must not be an approval of everything: with no
        // home variable resolvable, only the project's own root passes.
        let root = unique_temp_dir("vscode-guard-no-home");
        let project = root.join("app");
        std::fs::create_dir_all(&project).unwrap();

        let env = FakeEnv::new(); // no home variable at all
        let refused = guard_workspace_root_with(&root.canonicalize().unwrap(), &project, &env);
        assert!(
            refused.is_err(),
            "expected a refusal with no boundary known, got {refused:?}"
        );

        let allowed = guard_workspace_root_with(&project.canonicalize().unwrap(), &project, &env);
        assert!(
            allowed.is_ok(),
            "the project's own root stays writable, got {allowed:?}"
        );
    }

    #[test]
    fn test_guard_workspace_root_userprofile_only_env_is_honored() {
        // The Windows convention resolves the boundary for the guard too — a
        // root at USERPROFILE is refused rather than waved through.
        let home = unique_temp_dir("vscode-guard-userprofile");
        let project = home.join("dev").join("myapp");
        std::fs::create_dir_all(&project).unwrap();
        let canonical_home = home.canonicalize().unwrap();

        let env = FakeEnv::new().set("USERPROFILE", canonical_home.to_string_lossy());
        let result = guard_workspace_root_with(&canonical_home, &project, &env);

        assert!(result.is_err(), "expected a refusal, got {result:?}");
    }

    // ── config_path ──────────────────────────────────────────────

    #[test]
    fn test_vscode_config_path_single_project() {
        let dir = unique_temp_dir("vscode-config-path-single");
        let project = dir.join("app");
        std::fs::create_dir_all(&project).unwrap();

        let generator = VSCodeGenerator;
        let path = generator.config_path(&project);
        assert!(path.ends_with(".vscode/launch.json"));
        let canonical_project = project.canonicalize().unwrap();
        assert_eq!(path, canonical_project.join(".vscode").join("launch.json"));
    }

    #[test]
    fn test_vscode_config_path_monorepo_uses_workspace_root() {
        let workspace = unique_temp_dir("vscode-config-path-monorepo");
        let project = workspace.join("app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(workspace.join(".vscode")).unwrap();

        let generator = VSCodeGenerator;
        let path = generator.config_path(&project);
        assert_eq!(
            path,
            workspace
                .canonicalize()
                .unwrap()
                .join(".vscode")
                .join("launch.json")
        );
    }

    // ── fresh generation ─────────────────────────────────────────

    #[test]
    fn test_vscode_fresh_generation() {
        let generator = VSCodeGenerator;
        let dir = unique_temp_dir("vscode-fresh");
        let content = generator.generate(4711, &dir).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["version"], "0.2.0");
        let configs = parsed["configurations"].as_array().unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0]["name"], "Frust (TUI DAP)");
        assert_eq!(configs[0]["debugServer"], 4711);
        assert_eq!(configs[0]["type"], "frust");
        assert_eq!(configs[0]["request"], "launch");
        assert!(configs[0].get("cwd").is_none());
        assert!(configs[0].get("projectRoot").is_none());
    }

    #[test]
    fn test_vscode_fresh_generation_port_substitution() {
        let generator = VSCodeGenerator;
        let dir = unique_temp_dir("vscode-fresh-port");
        let content = generator.generate(9999, &dir).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["configurations"][0]["debugServer"], 9999);
    }

    // ── merge_config ─────────────────────────────────────────────

    #[test]
    fn test_vscode_merge_updates_existing_entry() {
        let existing = r#"{
            "version": "0.2.0",
            "configurations": [
                {"name": "Dart", "type": "dart", "request": "launch"},
                {"name": "Frust (TUI DAP)", "type": "frust", "debugServer": 1234}
            ]
        }"#;
        let generator = VSCodeGenerator;
        let dir = unique_temp_dir("vscode-merge-updates");
        let merged = generator.merge_config(existing, 5678, &dir).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let configs = parsed["configurations"].as_array().unwrap();
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0]["name"], "Dart"); // preserved
        assert_eq!(configs[1]["debugServer"], 5678); // updated
    }

    #[test]
    fn test_vscode_merge_appends_when_no_frust_entry() {
        let existing = r#"{
            "version": "0.2.0",
            "configurations": [
                {"name": "Dart", "type": "dart", "request": "launch"}
            ]
        }"#;
        let generator = VSCodeGenerator;
        let dir = unique_temp_dir("vscode-merge-appends");
        let merged = generator.merge_config(existing, 4711, &dir).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let configs = parsed["configurations"].as_array().unwrap();
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[1]["name"], "Frust (TUI DAP)");
    }

    #[test]
    fn test_vscode_merge_handles_jsonc_comments() {
        let existing = r#"{
            // This is a comment
            "version": "0.2.0",
            "configurations": [
                {"name": "Dart", "type": "dart"}
            ]
        }"#;
        let generator = VSCodeGenerator;
        let dir = unique_temp_dir("vscode-merge-jsonc");
        let result = generator.merge_config(existing, 4711, &dir);
        assert!(result.is_ok());
    }

    #[test]
    fn test_vscode_merge_malformed_json_returns_error() {
        let generator = VSCodeGenerator;
        let dir = unique_temp_dir("vscode-merge-malformed");
        let result = generator.merge_config("not json at all {{{", 4711, &dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_vscode_merge_preserves_version() {
        let existing = r#"{"version": "0.2.0", "configurations": []}"#;
        let generator = VSCodeGenerator;
        let dir = unique_temp_dir("vscode-merge-preserves-version");
        let merged = generator.merge_config(existing, 4711, &dir).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(parsed["version"], "0.2.0");
    }

    #[test]
    fn test_vscode_merge_no_configurations_key() {
        let existing = r#"{"version": "0.2.0"}"#;
        let generator = VSCodeGenerator;
        let dir = unique_temp_dir("vscode-merge-no-configs-key");
        let merged = generator.merge_config(existing, 4711, &dir).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert!(parsed["configurations"].is_array());
    }

    #[test]
    fn test_vscode_merge_empty_file_acts_as_fresh_generation() {
        let generator = VSCodeGenerator;
        let dir = unique_temp_dir("vscode-merge-empty");
        let result = generator.merge_config("", 4711, &dir);
        assert!(result.is_ok());
        let parsed: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(parsed["configurations"][0]["debugServer"], 4711);
    }

    #[test]
    fn test_vscode_merge_preserves_other_configurations() {
        let existing = r#"{
            "version": "0.2.0",
            "configurations": [
                {"name": "Config A", "type": "dart"},
                {"name": "Frust (TUI DAP)", "debugServer": 1000},
                {"name": "Config B", "type": "dart"}
            ]
        }"#;
        let generator = VSCodeGenerator;
        let dir = unique_temp_dir("vscode-merge-preserves-others");
        let merged = generator.merge_config(existing, 4711, &dir).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let configs = parsed["configurations"].as_array().unwrap();
        assert_eq!(configs.len(), 3);
        assert_eq!(configs[0]["name"], "Config A");
        assert_eq!(configs[1]["debugServer"], 4711);
        assert_eq!(configs[2]["name"], "Config B");
    }

    #[test]
    fn test_vscode_ide_name() {
        assert_eq!(VSCodeGenerator.ide_name(), "VS Code");
    }

    #[test]
    fn test_vscode_frust_entry_port_reads_the_marked_entry() {
        let generator = VSCodeGenerator;
        let existing = r#"{
            // JSONC is fine
            "configurations": [
                {"name": "Rust", "debugServer": 9},
                {"name": "Frust (TUI DAP)", "type": "frust", "debugServer": 1234},
            ]
        }"#;
        assert_eq!(generator.frust_entry_port(existing).unwrap(), Some(1234));
        assert_eq!(generator.frust_entry_port("").unwrap(), None);
        assert_eq!(
            generator
                .frust_entry_port(r#"{"configurations": [{"name": "Rust"}]}"#)
                .unwrap(),
            None
        );
        // Present but portless (or out of range): present, no port to name.
        for portless in [
            r#"{"configurations": [{"name": "Frust (TUI DAP)"}]}"#,
            r#"{"configurations": [{"name": "Frust (TUI DAP)", "debugServer": 70000}]}"#,
        ] {
            assert!(generator.has_frust_entry(portless).unwrap());
            assert_eq!(generator.frust_entry_port(portless).unwrap(), None);
        }
        assert!(
            generator
                .frust_entry_port(r#"{"configurations": 1}"#)
                .is_err()
        );
    }
}
