//! Neovim (nvim-dap) DAP configuration generator.
//!
//! Neovim's nvim-dap plugin supports two configuration approaches:
//!
//! 1. **Primary**: `.vscode/launch.json` — loaded via
//!    `require("dap.ext.vscode").load_launchjs()`. This is the most common
//!    setup and is handled by delegating to [`super::vscode::VSCodeGenerator`].
//!
//! 2. **Secondary**: `.nvim-dap.lua` — a project-local Lua snippet users can
//!    source directly. Written as an informational best-effort file; failure
//!    to write it never fails the overall config generation.

use std::path::{Path, PathBuf};

use super::vscode::{self, VSCodeGenerator};
use super::{IdeConfigGenerator, Result};

/// Generates DAP config for Neovim's nvim-dap plugin.
///
/// The primary config target is `.vscode/launch.json` (same format as VS
/// Code), because nvim-dap can load VS Code launch configs via
/// `load_launchjs()`. Additionally writes a `.nvim-dap.lua` snippet at the
/// workspace root as an informational alternative for users who prefer
/// native nvim-dap configuration.
pub struct NeovimGenerator;

impl NeovimGenerator {
    /// Generate the Lua snippet content for `.nvim-dap.lua`.
    ///
    /// The snippet configures nvim-dap with a `frust` adapter that connects
    /// to the given `port` and appends a `rust` configuration. No `cwd` is
    /// set on the configuration entry — the frust DAP server always launches
    /// from its own process's project root (see `vscode`'s module doc).
    pub fn generate_lua_snippet(&self, port: u16) -> String {
        format!(
            r#"-- Frust DAP configuration for Neovim (auto-generated)
--
-- Option 1: Source this file in your Neovim config:
--   dofile(vim.fn.getcwd() .. '/.nvim-dap.lua')
--
-- Option 2: Use load_launchjs() to read .vscode/launch.json:
--   require('dap.ext.vscode').load_launchjs()
--
-- Option 2 is recommended -- `frust dap` auto-generates .vscode/launch.json

local dap = require('dap')

dap.adapters.frust = {{
  type = 'server',
  host = '127.0.0.1',
  port = {port},
}}

dap.configurations.rust = dap.configurations.rust or {{}}
table.insert(dap.configurations.rust, {{
  type = 'frust',
  request = 'launch',
  name = 'Frust (TUI DAP)',
}})
"#,
            port = port
        )
    }

    /// Write `.nvim-dap.lua` to the workspace root.
    ///
    /// Best-effort: errors (including a
    /// [`guard_workspace_root`](vscode::guard_workspace_root) containment
    /// violation — review Major E's defense-in-depth) are logged as warnings
    /// but do not propagate. The file is always overwritten (frust-owned).
    ///
    /// Placed at the workspace root (detected via
    /// [`vscode::detect_workspace_root`]) so Neovim finds it when opened at
    /// the same directory VS Code would be opened at.
    pub fn write_nvim_dap_lua(&self, port: u16, project_root: &Path) {
        self.write_nvim_dap_lua_with(port, project_root, &vscode::RealEnv);
    }

    /// Testable core of [`write_nvim_dap_lua`](Self::write_nvim_dap_lua),
    /// taking an injected [`vscode::EnvLookup`] so the `$HOME`-boundary tests
    /// can exercise it without mutating the real process environment (same
    /// seam `vscode`'s own tests use).
    fn write_nvim_dap_lua_with(&self, port: u16, project_root: &Path, env: &dyn vscode::EnvLookup) {
        let workspace_root = vscode::detect_workspace_root_with(project_root, env);
        if let Err(e) = vscode::guard_workspace_root_with(&workspace_root, project_root, env) {
            log::warn!("refusing to write .nvim-dap.lua: {e}");
            return;
        }
        let path = workspace_root.join(".nvim-dap.lua");
        let content = self.generate_lua_snippet(port);
        match std::fs::write(&path, content) {
            Ok(()) => log::debug!("wrote .nvim-dap.lua at {}", path.display()),
            Err(e) => log::warn!("failed to write .nvim-dap.lua: {e}"),
        }
    }
}

impl IdeConfigGenerator for NeovimGenerator {
    /// Returns the path to `.vscode/launch.json` at the workspace root — the
    /// primary config target for nvim-dap via `load_launchjs()`. Delegates to
    /// [`VSCodeGenerator::config_path`], which detects the workspace root
    /// internally.
    fn config_path(&self, project_root: &Path) -> PathBuf {
        VSCodeGenerator.config_path(project_root)
    }

    /// Generate a fresh `.vscode/launch.json` — identical to the VS Code
    /// generator's output. The secondary `.nvim-dap.lua` file is written by
    /// [`post_write`](IdeConfigGenerator::post_write) so it stays in sync on
    /// both the create and merge paths via `run_generator`.
    fn generate(&self, port: u16, project_root: &Path) -> Result<String> {
        VSCodeGenerator.generate(port, project_root)
    }

    /// Merge the frust entry into an existing `.vscode/launch.json` —
    /// delegates entirely to the VS Code generator's merge logic.
    fn merge_config(&self, existing: &str, port: u16, project_root: &Path) -> Result<String> {
        VSCodeGenerator.merge_config(existing, port, project_root)
    }

    /// Write (or overwrite) the secondary `.nvim-dap.lua` file at the
    /// workspace root, so it stays in sync with `.vscode/launch.json` on
    /// every DAP server start.
    fn post_write(&self, port: u16, project_root: &Path) -> Result<()> {
        self.write_nvim_dap_lua(port, project_root);
        Ok(())
    }

    fn ide_name(&self) -> &'static str {
        "Neovim"
    }
}

// ─────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ide_config::test_support::unique_temp_dir;

    #[test]
    fn test_neovim_config_path_is_vscode_launch_json() {
        let dir = unique_temp_dir("neovim-config-path");
        let project = dir.join("app");
        std::fs::create_dir_all(&project).unwrap();

        let generator = NeovimGenerator;
        let path = generator.config_path(&project);
        assert!(path.ends_with(".vscode/launch.json"));
    }

    #[test]
    fn test_neovim_config_path_monorepo_uses_workspace_root() {
        let workspace = unique_temp_dir("neovim-config-path-monorepo");
        let project = workspace.join("app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(workspace.join(".vscode")).unwrap();

        let generator = NeovimGenerator;
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

    #[test]
    fn test_neovim_fresh_generation_produces_valid_launch_json() {
        let dir = unique_temp_dir("neovim-fresh");
        let generator = NeovimGenerator;
        let content = generator.generate(4711, &dir).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["configurations"][0]["debugServer"], 4711);
    }

    #[test]
    fn test_neovim_fresh_generation_has_required_fields() {
        let dir = unique_temp_dir("neovim-fresh-fields");
        let generator = NeovimGenerator;
        let content = generator.generate(4711, &dir).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        let cfg = &parsed["configurations"][0];
        assert_eq!(cfg["name"], "Frust (TUI DAP)");
        assert_eq!(cfg["type"], "frust");
        assert_eq!(cfg["request"], "launch");
        assert!(cfg.get("cwd").is_none());
    }

    #[test]
    fn test_neovim_post_write_writes_lua_snippet() {
        let dir = unique_temp_dir("neovim-post-write");
        let generator = NeovimGenerator;
        generator.post_write(4711, &dir).unwrap();
        let lua_path = dir.canonicalize().unwrap().join(".nvim-dap.lua");
        assert!(lua_path.exists());
        let content = std::fs::read_to_string(&lua_path).unwrap();
        assert!(content.contains("port = 4711"));
        assert!(content.contains("dap.adapters.frust"));
        assert!(content.contains("type = 'server'"));
    }

    #[test]
    fn test_neovim_post_write_updates_lua_on_merge() {
        let dir = unique_temp_dir("neovim-post-write-updates");
        let old_lua_path = dir.join(".nvim-dap.lua");
        std::fs::write(&old_lua_path, "old port = 1234").unwrap();
        let generator = NeovimGenerator;
        generator.post_write(5678, &dir).unwrap();
        let content = std::fs::read_to_string(&old_lua_path).unwrap();
        assert!(content.contains("port = 5678"));
        assert!(!content.contains("old port"));
    }

    #[test]
    fn test_neovim_post_write_places_lua_at_workspace_root() {
        // Layout: workspace/.git/  workspace/app/
        // post_write should write .nvim-dap.lua at workspace, not app.
        let workspace = unique_temp_dir("neovim-post-write-workspace");
        let project = workspace.join("app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(workspace.join(".git")).unwrap();

        let generator = NeovimGenerator;
        generator.post_write(4711, &project).unwrap();

        let workspace_lua = workspace.canonicalize().unwrap().join(".nvim-dap.lua");
        assert!(
            workspace_lua.exists(),
            ".nvim-dap.lua should be at workspace root"
        );

        let project_lua = project.join(".nvim-dap.lua");
        assert!(
            !project_lua.exists(),
            ".nvim-dap.lua should not be at project root"
        );
    }

    #[test]
    fn test_neovim_lua_snippet_port_substitution() {
        let generator = NeovimGenerator;
        let lua = generator.generate_lua_snippet(9999);
        assert!(lua.contains("port = 9999"));
        assert!(!lua.contains("port = 4711"));
    }

    #[test]
    fn test_neovim_lua_snippet_contains_usage_instructions() {
        let generator = NeovimGenerator;
        let lua = generator.generate_lua_snippet(4711);
        assert!(lua.contains("load_launchjs"));
        assert!(lua.contains("dofile"));
        assert!(lua.contains("Option 1"));
        assert!(lua.contains("Option 2"));
    }

    #[test]
    fn test_neovim_lua_snippet_contains_adapter_config() {
        let generator = NeovimGenerator;
        let lua = generator.generate_lua_snippet(4711);
        assert!(lua.contains("dap.adapters.frust"));
        assert!(lua.contains("type = 'server'"));
        assert!(lua.contains("host = '127.0.0.1'"));
    }

    #[test]
    fn test_neovim_lua_snippet_contains_rust_configuration() {
        let generator = NeovimGenerator;
        let lua = generator.generate_lua_snippet(4711);
        assert!(lua.contains("dap.configurations.rust"));
        assert!(lua.contains("type = 'frust'"));
        assert!(lua.contains("request = 'launch'"));
        assert!(lua.contains("name = 'Frust (TUI DAP)'"));
    }

    #[test]
    fn test_neovim_merge_delegates_to_vscode() {
        let existing = r#"{
            "version": "0.2.0",
            "configurations": [
                {"name": "Dart", "type": "dart", "request": "launch"}
            ]
        }"#;
        let generator = NeovimGenerator;
        let dir = unique_temp_dir("neovim-merge-delegates");
        let merged = generator.merge_config(existing, 4711, &dir).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let configs = parsed["configurations"].as_array().unwrap();
        assert_eq!(configs.len(), 2);
    }

    #[test]
    fn test_neovim_merge_updates_existing_frust_entry() {
        let existing = r#"{
            "version": "0.2.0",
            "configurations": [
                {"name": "Frust (TUI DAP)", "debugServer": 1234}
            ]
        }"#;
        let generator = NeovimGenerator;
        let dir = unique_temp_dir("neovim-merge-updates");
        let merged = generator.merge_config(existing, 5678, &dir).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(parsed["configurations"][0]["debugServer"], 5678);
    }

    #[test]
    fn test_neovim_merge_malformed_json_returns_error() {
        let generator = NeovimGenerator;
        let dir = unique_temp_dir("neovim-merge-malformed");
        let result = generator.merge_config("not json {{{", 4711, &dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_neovim_ide_name() {
        assert_eq!(NeovimGenerator.ide_name(), "Neovim");
    }

    #[test]
    fn test_neovim_write_lua_snippet_creates_file() {
        let dir = unique_temp_dir("neovim-write-lua-creates");
        let generator = NeovimGenerator;
        generator.write_nvim_dap_lua(4711, &dir);
        let lua_path = dir.canonicalize().unwrap().join(".nvim-dap.lua");
        assert!(lua_path.exists());
    }

    #[test]
    fn test_neovim_write_lua_snippet_overwrites_existing() {
        let dir = unique_temp_dir("neovim-write-lua-overwrites");
        let lua_path = dir.join(".nvim-dap.lua");
        std::fs::write(&lua_path, "old content").unwrap();

        let generator = NeovimGenerator;
        generator.write_nvim_dap_lua(9999, &dir);

        let content = std::fs::read_to_string(&lua_path).unwrap();
        assert!(content.contains("port = 9999"));
        assert!(!content.contains("old content"));
    }

    #[test]
    fn test_neovim_write_lua_snippet_failure_does_not_panic() {
        let generator = NeovimGenerator;
        // Non-existent parent directory triggers a write error — should log
        // a warning and return, not panic.
        generator.write_nvim_dap_lua(4711, Path::new("/nonexistent/deep/path"));
    }

    // ── write_nvim_dap_lua: $HOME boundary (review Major E, item e) ──

    /// In-memory [`vscode::EnvLookup`] fixture — same shape as
    /// `vscode::tests::FakeEnv`, kept local since that one is private to
    /// `vscode`'s own test module.
    struct FakeEnv(std::collections::HashMap<&'static str, String>);

    impl FakeEnv {
        fn new() -> Self {
            Self(std::collections::HashMap::new())
        }

        fn set(mut self, key: &'static str, value: impl Into<String>) -> Self {
            self.0.insert(key, value.into());
            self
        }
    }

    impl vscode::EnvLookup for FakeEnv {
        fn get(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
    }

    #[test]
    fn test_neovim_post_write_target_obeys_home_boundary() {
        // Layout: <tmp-home>/dev/myapp/.git  <tmp-home>/.vscode
        // Mirrors vscode's own boundary test (b): the .nvim-dap.lua target
        // must land at myapp (the project's own .git wins), never climb past
        // <tmp-home> to <tmp-home>/.vscode.
        let home = unique_temp_dir("neovim-home-boundary");
        let project = home.join("dev").join("myapp");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::fs::create_dir_all(home.join(".vscode")).unwrap();

        let env = FakeEnv::new().set("HOME", home.canonicalize().unwrap().to_string_lossy());
        let generator = NeovimGenerator;
        generator.write_nvim_dap_lua_with(4711, &project, &env);

        let project_lua = project.canonicalize().unwrap().join(".nvim-dap.lua");
        assert!(
            project_lua.exists(),
            ".nvim-dap.lua should be written at the project's own root"
        );

        let home_lua = home.canonicalize().unwrap().join(".nvim-dap.lua");
        assert!(
            !home_lua.exists(),
            ".nvim-dap.lua must never be written at $HOME"
        );
    }
}
