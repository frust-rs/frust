//! VS Code DAP configuration generator.
//!
//! Generates or merges a `.vscode/launch.json` entry so that VS Code, VS Code
//! Insiders, and Cursor can connect to `frust dap`'s server via the
//! `debugServer` field (a VS Code-internal mechanism that redirects the debug
//! adapter transport to an already-running TCP server, rather than spawning
//! one). `request` is `"launch"`, not `"attach"` — the frust DAP server is
//! launch-based (`docs/CLI_ARCHITECTURE.md`'s `frust-dap` row), and there is
//! no separate "already running app" for VS Code to attach to. The entry
//! carries no `cwd`/`projectRoot` field: the server always launches from its
//! own process's project root by design (`docs/LIMITATIONS.md`'s
//! `dap-tcp-unauthenticated-v1` — on TCP that is the server's cwd, not the
//! client's), so there is nothing for VS Code to tell it.

use std::path::{Path, PathBuf};

use serde_json::json;

use super::merge::{FRUST_CONFIG_NAME, clean_jsonc, merge_json_array_entry, to_pretty_json};
use super::{IdeConfigError, IdeConfigGenerator, Result};

/// Detect the workspace root for a frust project.
///
/// Walks up from `project_root` looking for a directory that contains either
/// `.vscode/` (existing VS Code workspace) or `.git/` (repository root).
/// Returns the nearest ancestor that qualifies, or `project_root` itself if
/// no workspace marker is found.
///
/// The search stops at the filesystem root. Home-directory boundaries are not
/// treated specially — the walk stops at the first match or the root.
///
/// # Priority
///
/// 1. A parent with `.vscode/` is preferred (VS Code was opened there).
/// 2. A parent with `.git/` is the fallback (likely the repo root).
/// 3. If neither is found, the project root is returned unchanged.
pub fn detect_workspace_root(project_root: &Path) -> PathBuf {
    let canonical = match project_root.canonicalize() {
        Ok(p) => p,
        Err(_) => return project_root.to_path_buf(),
    };

    let mut git_root: Option<PathBuf> = None;

    for ancestor in canonical.ancestors().skip(1) {
        if ancestor.join(".vscode").is_dir() {
            return ancestor.to_path_buf();
        }
        if git_root.is_none() && ancestor.join(".git").exists() {
            git_root = Some(ancestor.to_path_buf());
        }
    }

    git_root.unwrap_or(canonical)
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

impl IdeConfigGenerator for VSCodeGenerator {
    fn config_path(&self, project_root: &Path) -> PathBuf {
        let workspace_root = detect_workspace_root(project_root);
        workspace_root.join(".vscode").join("launch.json")
    }

    fn generate(&self, port: u16, _project_root: &Path) -> Result<String> {
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

    // ── detect_workspace_root ────────────────────────────────────

    #[test]
    fn test_detect_workspace_root_project_has_vscode_returns_project() {
        // detect_workspace_root walks ANCESTORS, so .vscode in project_root
        // itself is NOT found (the project_root itself is skipped in the
        // walk) => returns canonical(project_root) since no ancestor has
        // .vscode or .git.
        let dir = unique_temp_dir("vscode-self-vscode");
        std::fs::create_dir_all(dir.join(".vscode")).unwrap();
        let detected = detect_workspace_root(&dir);
        assert_eq!(
            detected.canonicalize().unwrap(),
            dir.canonicalize().unwrap()
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
}
