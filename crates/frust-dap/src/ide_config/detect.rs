//! Parent-IDE detection: recognizing which terminal/editor is hosting the
//! embedded DAP server (there is no standalone `frust dap` process — see
//! `docs/CLI_ARCHITECTURE.md`'s `frust-dap` row), so a DAP client config can
//! be generated automatically.
//!
//! Ported from fdemon-pro's `config::{types, settings}` `ParentIde`/
//! `detect_parent_ide`/`should_auto_start_dap` shapes (fdemon is Ed's own
//! project, F0X IT LLC — copying into frust is authorized). `should_auto_start_dap`
//! takes plain `bool`s rather than fdemon's `Settings` struct: this crate
//! holds no settings type of its own — a later task's `frust-tui`/config
//! layer supplies both flags from wherever it stores them.

use std::path::PathBuf;

/// A detected parent IDE/editor hosting the terminal the embedded DAP
/// server was started from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentIde {
    VSCode,
    VSCodeInsiders,
    Cursor,
    Zed,
    IntelliJ,
    AndroidStudio,
    Neovim,
    Emacs,
    Helix,
}

impl ParentIde {
    /// Human-readable name used in log messages.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::VSCode => "VS Code",
            Self::VSCodeInsiders => "VS Code Insiders",
            Self::Cursor => "Cursor",
            Self::Zed => "Zed",
            Self::IntelliJ => "IntelliJ IDEA",
            Self::AndroidStudio => "Android Studio",
            Self::Neovim => "Neovim",
            Self::Emacs => "Emacs",
            Self::Helix => "Helix",
        }
    }

    /// Whether this IDE has a DAP client-config generator in
    /// [`super::generate_ide_config`].
    ///
    /// IntelliJ and Android Studio use JetBrains' proprietary debugging
    /// protocol — no standard DAP client-config path exists for either.
    pub fn supports_dap_config(&self) -> bool {
        !matches!(self, Self::IntelliJ | Self::AndroidStudio)
    }

    /// Where this IDE's generated DAP config would live under `project_root`,
    /// for callers that want the path without running generation (e.g. a
    /// status line). Mirrors each generator's own `config_path` — `None` for
    /// an IDE [`supports_dap_config`](Self::supports_dap_config) refuses.
    pub fn dap_config_path(&self, project_root: &std::path::Path) -> Option<PathBuf> {
        match self {
            Self::VSCode | Self::VSCodeInsiders | Self::Cursor | Self::Neovim => {
                Some(project_root.join(".vscode").join("launch.json"))
            }
            Self::Helix => Some(project_root.join(".helix").join("languages.toml")),
            Self::Zed => Some(project_root.join(".zed").join("debug.json")),
            Self::Emacs => Some(project_root.join(".frust").join("dap-emacs.el")),
            Self::IntelliJ | Self::AndroidStudio => None,
        }
    }
}

/// Abstracts environment-variable lookups so [`detect_parent_ide`] is
/// testable without mutating the real, global process environment —
/// `std::env::set_var` is `unsafe` as of this workspace's edition/toolchain,
/// and this crate stays `unsafe`-free (`docs/CODE_STANDARDS.md`'s sanctioned-unsafe
/// list has no entry for `frust-dap`). Mirrors `frust-drive::doctor`'s own
/// `EnvLookup` seam, kept local here since that one is crate-private to
/// `frust-drive`.
trait EnvLookup {
    fn get(&self, key: &str) -> Option<String>;
}

struct RealEnv;

impl EnvLookup for RealEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

/// Detect the IDE/editor hosting the current terminal.
///
/// Checks environment variables in the same order fdemon-pro's own detector
/// does (most reliable signal first):
///
/// 1. `TERM_PROGRAM` (VS Code family, Zed)
/// 2. `ZED_TERM`
/// 3. `VSCODE_IPC_HOOK_CLI` (VS Code/Cursor backup signal)
/// 4. `TERMINAL_EMULATOR` (JetBrains IntelliJ/Android Studio)
/// 5. `NVIM`
/// 6. `INSIDE_EMACS`
/// 7. `HELIX_RUNTIME`
///
/// Returns `None` if no known IDE terminal is detected.
pub fn detect_parent_ide() -> Option<ParentIde> {
    detect_parent_ide_with(&RealEnv)
}

fn detect_parent_ide_with(env: &dyn EnvLookup) -> Option<ParentIde> {
    if let Some(term_program) = env.get("TERM_PROGRAM") {
        match term_program.as_str() {
            "vscode" => return Some(ParentIde::VSCode),
            "vscode-insiders" => return Some(ParentIde::VSCodeInsiders),
            "cursor" => return Some(ParentIde::Cursor),
            "Zed" => return Some(ParentIde::Zed),
            _ => {}
        }
    }

    if env.get("ZED_TERM").is_some() {
        return Some(ParentIde::Zed);
    }

    if env.get("VSCODE_IPC_HOOK_CLI").is_some() {
        if env
            .get("TERM_PROGRAM")
            .map(|v| v == "cursor")
            .unwrap_or(false)
        {
            return Some(ParentIde::Cursor);
        }
        return Some(ParentIde::VSCode);
    }

    if let Some(terminal_emulator) = env.get("TERMINAL_EMULATOR")
        && terminal_emulator.starts_with("JetBrains")
    {
        if let Some(idea_dir) = env.get("IDEA_INITIAL_DIRECTORY")
            && idea_dir.contains("AndroidStudio")
        {
            return Some(ParentIde::AndroidStudio);
        }
        return Some(ParentIde::IntelliJ);
    }

    if env.get("NVIM").is_some() {
        return Some(ParentIde::Neovim);
    }

    if env.get("INSIDE_EMACS").is_some() {
        return Some(ParentIde::Emacs);
    }

    if env.get("HELIX_RUNTIME").is_some() {
        return Some(ParentIde::Helix);
    }

    None
}

/// Whether the embedded DAP server should auto-start at startup.
///
/// Decision tree (fdemon-pro's `should_auto_start_dap`, `enabled`/
/// `auto_start_in_ide` standing in for its `settings.dap.{enabled,
/// auto_start_in_ide}`):
///
/// 1. `enabled` (an explicit opt-in, e.g. a `--dap-port` CLI flag)? → yes.
/// 2. `auto_start_in_ide` AND an IDE terminal is detected via
///    [`detect_parent_ide`]? → yes.
/// 3. Neither? → no.
pub fn should_auto_start_dap(enabled: bool, auto_start_in_ide: bool) -> bool {
    if enabled {
        return true;
    }

    if auto_start_in_ide && let Some(ide) = detect_parent_ide() {
        log::info!(
            "Detected parent IDE: {} — auto-starting DAP server",
            ide.display_name()
        );
        return true;
    }

    false
}

// ─────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// In-memory [`EnvLookup`] fixture — the same shape
    /// `frust-drive::doctor::FakeEnv` uses for the same reason.
    struct FakeEnv(HashMap<&'static str, &'static str>);

    impl FakeEnv {
        fn new() -> Self {
            Self(HashMap::new())
        }

        fn set(mut self, key: &'static str, value: &'static str) -> Self {
            self.0.insert(key, value);
            self
        }
    }

    impl EnvLookup for FakeEnv {
        fn get(&self, key: &str) -> Option<String> {
            self.0.get(key).map(|v| v.to_string())
        }
    }

    // ── detect_parent_ide_with ───────────────────────────────────

    #[test]
    fn test_detect_none_when_nothing_set() {
        assert_eq!(detect_parent_ide_with(&FakeEnv::new()), None);
    }

    #[test]
    fn test_detect_vscode_via_term_program() {
        let env = FakeEnv::new().set("TERM_PROGRAM", "vscode");
        assert_eq!(detect_parent_ide_with(&env), Some(ParentIde::VSCode));
    }

    #[test]
    fn test_detect_vscode_insiders_via_term_program() {
        let env = FakeEnv::new().set("TERM_PROGRAM", "vscode-insiders");
        assert_eq!(
            detect_parent_ide_with(&env),
            Some(ParentIde::VSCodeInsiders)
        );
    }

    #[test]
    fn test_detect_cursor_via_term_program() {
        let env = FakeEnv::new().set("TERM_PROGRAM", "cursor");
        assert_eq!(detect_parent_ide_with(&env), Some(ParentIde::Cursor));
    }

    #[test]
    fn test_detect_zed_via_term_program() {
        let env = FakeEnv::new().set("TERM_PROGRAM", "Zed");
        assert_eq!(detect_parent_ide_with(&env), Some(ParentIde::Zed));
    }

    #[test]
    fn test_detect_zed_via_zed_term_fallback() {
        let env = FakeEnv::new().set("ZED_TERM", "1");
        assert_eq!(detect_parent_ide_with(&env), Some(ParentIde::Zed));
    }

    #[test]
    fn test_detect_vscode_via_ipc_hook_cli() {
        let env = FakeEnv::new().set("VSCODE_IPC_HOOK_CLI", "/tmp/sock");
        assert_eq!(detect_parent_ide_with(&env), Some(ParentIde::VSCode));
    }

    #[test]
    fn test_detect_cursor_via_ipc_hook_cli_and_term_program() {
        let env = FakeEnv::new()
            .set("VSCODE_IPC_HOOK_CLI", "/tmp/sock")
            .set("TERM_PROGRAM", "cursor");
        assert_eq!(detect_parent_ide_with(&env), Some(ParentIde::Cursor));
    }

    #[test]
    fn test_detect_intellij_via_terminal_emulator() {
        let env = FakeEnv::new().set("TERMINAL_EMULATOR", "JetBrains-IDEA");
        assert_eq!(detect_parent_ide_with(&env), Some(ParentIde::IntelliJ));
    }

    #[test]
    fn test_detect_android_studio_via_idea_initial_directory() {
        let env = FakeEnv::new()
            .set("TERMINAL_EMULATOR", "JetBrains-IDEA")
            .set("IDEA_INITIAL_DIRECTORY", "/home/me/AndroidStudioProjects");
        assert_eq!(detect_parent_ide_with(&env), Some(ParentIde::AndroidStudio));
    }

    #[test]
    fn test_detect_neovim_via_nvim() {
        let env = FakeEnv::new().set("NVIM", "/tmp/nvim.sock");
        assert_eq!(detect_parent_ide_with(&env), Some(ParentIde::Neovim));
    }

    #[test]
    fn test_detect_emacs_via_inside_emacs() {
        let env = FakeEnv::new().set("INSIDE_EMACS", "vterm");
        assert_eq!(detect_parent_ide_with(&env), Some(ParentIde::Emacs));
    }

    #[test]
    fn test_detect_helix_via_helix_runtime() {
        let env = FakeEnv::new().set("HELIX_RUNTIME", "/usr/share/helix/runtime");
        assert_eq!(detect_parent_ide_with(&env), Some(ParentIde::Helix));
    }

    #[test]
    fn test_detect_term_program_takes_priority_over_zed_term() {
        let env = FakeEnv::new()
            .set("TERM_PROGRAM", "vscode")
            .set("ZED_TERM", "1");
        assert_eq!(detect_parent_ide_with(&env), Some(ParentIde::VSCode));
    }

    // ── supports_dap_config ──────────────────────────────────────

    #[test]
    fn test_supports_dap_config_true_for_all_except_jetbrains() {
        assert!(ParentIde::VSCode.supports_dap_config());
        assert!(ParentIde::VSCodeInsiders.supports_dap_config());
        assert!(ParentIde::Cursor.supports_dap_config());
        assert!(ParentIde::Zed.supports_dap_config());
        assert!(ParentIde::Neovim.supports_dap_config());
        assert!(ParentIde::Emacs.supports_dap_config());
        assert!(ParentIde::Helix.supports_dap_config());
    }

    #[test]
    fn test_supports_dap_config_false_for_intellij_and_android_studio() {
        assert!(!ParentIde::IntelliJ.supports_dap_config());
        assert!(!ParentIde::AndroidStudio.supports_dap_config());
    }

    // ── display_name ─────────────────────────────────────────────

    #[test]
    fn test_display_names() {
        assert_eq!(ParentIde::VSCode.display_name(), "VS Code");
        assert_eq!(ParentIde::VSCodeInsiders.display_name(), "VS Code Insiders");
        assert_eq!(ParentIde::Cursor.display_name(), "Cursor");
        assert_eq!(ParentIde::Zed.display_name(), "Zed");
        assert_eq!(ParentIde::IntelliJ.display_name(), "IntelliJ IDEA");
        assert_eq!(ParentIde::AndroidStudio.display_name(), "Android Studio");
        assert_eq!(ParentIde::Neovim.display_name(), "Neovim");
        assert_eq!(ParentIde::Emacs.display_name(), "Emacs");
        assert_eq!(ParentIde::Helix.display_name(), "Helix");
    }

    // ── should_auto_start_dap ────────────────────────────────────

    #[test]
    fn test_should_auto_start_true_when_enabled() {
        assert!(should_auto_start_dap(true, false));
    }

    #[test]
    fn test_should_auto_start_false_when_neither_set() {
        assert!(!should_auto_start_dap(false, false));
    }

    #[test]
    fn test_should_auto_start_false_when_auto_start_but_no_ide_detected() {
        // detect_parent_ide() reads the real environment; this only holds in
        // a clean host/CI environment with none of the IDE markers set. If
        // this ever flakes under a real IDE-hosted `cargo test`, it should
        // be routed through `detect_parent_ide_with` instead.
        assert!(!should_auto_start_dap(false, true) || detect_parent_ide().is_some());
    }
}
