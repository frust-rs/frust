//! IDE-specific DAP client configuration generation.
//!
//! Ported from fdemon-pro's `fdemon-app::ide_config` (fdemon is Ed's own
//! project, F0X IT LLC — copying into frust is authorized), adapted for
//! frust: the entry name/type is `"Frust (TUI DAP)"`/`"frust"`, every
//! generated entry is `request: "launch"` (frust-dap has no attach story —
//! see [`vscode`]'s module doc), and Helix gets no generated config at all
//! (see [`helix`]'s module doc).
//!
//! This module is pure and UI-free: it does not print, does not know about
//! `frust-tui`, and has no CLI entry point of its own — a later task's
//! `frust-tui` calls [`generate_ide_config`] directly.
//!
//! ## Design
//!
//! [`run_generator`] owns all file I/O (read / mkdir / write) so that
//! [`IdeConfigGenerator`] implementations only need to produce or transform
//! string content — pure and straightforward to unit-test without touching
//! the filesystem.
//!
//! ## Submodules
//!
//! | Module | IDE |
//! |--------|-----|
//! | [`detect`] | `ParentIde` detection (env-var sniffing) |
//! | [`vscode`] | VS Code, VS Code Insiders, Cursor |
//! | [`neovim`] | Neovim (nvim-dap), delegating to [`vscode`] |
//! | [`helix`]  | Helix — always [`ConfigAction::Skipped`], see its module doc |
//! | [`zed`]    | Zed |
//! | [`emacs`]  | Emacs (dap-mode) |

pub mod detect;
pub mod emacs;
pub mod helix;
pub(crate) mod merge;
pub mod neovim;
pub mod vscode;
pub mod zed;

#[cfg(test)]
pub(crate) mod test_support;

use std::path::{Path, PathBuf};

pub use detect::{ParentIde, detect_parent_ide, should_auto_start_dap};

// ─────────────────────────────────────────────────────────────────
// Errors
// ─────────────────────────────────────────────────────────────────

/// Errors from IDE DAP-config generation: filesystem I/O and JSON-parse
/// failures, plus ad-hoc shape errors a generator raises itself (e.g. "not a
/// JSON array").
#[derive(Debug, thiserror::Error)]
pub enum IdeConfigError {
    /// Reading or writing a config file (or its parent directory) failed.
    #[error("`{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// A parse/shape failure a generator raises itself — malformed JSON, a
    /// `configurations`/array field that isn't the type expected, or an
    /// unrecognised `--ide` name.
    #[error("{0}")]
    Message(String),
}

impl IdeConfigError {
    fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    fn message(msg: impl Into<String>) -> Self {
        Self::Message(msg.into())
    }
}

impl From<serde_json::Error> for IdeConfigError {
    fn from(e: serde_json::Error) -> Self {
        Self::Message(format!("invalid JSON: {e}"))
    }
}

/// This module's result alias — `pub` because it appears in
/// [`IdeConfigGenerator`]'s public trait methods and in
/// [`generate_ide_config`]'s/[`parse_ide_name`]'s public signatures.
pub type Result<T> = std::result::Result<T, IdeConfigError>;

// ─────────────────────────────────────────────────────────────────
// Public types
// ─────────────────────────────────────────────────────────────────

/// Result of an IDE config generation attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdeConfigResult {
    /// Path to the config file that was created, updated, or would have been
    /// written had generation not been skipped.
    pub path: PathBuf,
    /// What action was taken.
    pub action: ConfigAction,
}

/// What happened during config generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigAction {
    /// Config file was created (did not previously exist).
    Created,
    /// Existing config file was updated with a new/changed frust entry.
    Updated,
    /// Config generation was skipped (with reason).
    Skipped(String),
}

// ─────────────────────────────────────────────────────────────────
// IdeConfigGenerator trait
// ─────────────────────────────────────────────────────────────────

/// Trait for generating IDE-specific DAP client configuration files.
///
/// Each IDE has its own config format and file location. Implementations
/// handle both fresh creation and merging into existing config files.
pub trait IdeConfigGenerator {
    /// Returns the path where this IDE's config file should be written,
    /// relative to the project root.
    fn config_path(&self, project_root: &Path) -> PathBuf;

    /// Generate the full config file content for a fresh creation.
    fn generate(&self, port: u16, project_root: &Path) -> Result<String>;

    /// Check if a config file already exists at the expected path.
    ///
    /// The default implementation checks for file existence. Override only
    /// when a more sophisticated check is required.
    fn config_exists(&self, project_root: &Path) -> bool {
        self.config_path(project_root).exists()
    }

    /// Merge frust DAP config into an existing config file.
    ///
    /// Returns the merged content, or an error if the file cannot be parsed.
    ///
    /// Implementations must:
    /// - Find an existing frust entry (by marker) and update it
    /// - Append a new entry if no frust entry exists
    /// - Preserve all non-frust entries unchanged
    ///
    /// **"Preserve" is semantic, not byte-for-byte.** An implementation
    /// reading a commented/hand-formatted format (VS Code's JSONC
    /// `launch.json`, via [`vscode`]) parses the existing file into a value
    /// tree and reprints the whole document — every other entry's *data*
    /// survives, but comments and original formatting do not. See
    /// `docs/LIMITATIONS.md`'s `dap-ide-config-normalizes-launchjson`.
    fn merge_config(&self, existing: &str, port: u16, project_root: &Path) -> Result<String>;

    /// Optional post-generation hook for secondary file writes.
    ///
    /// Called by [`run_generator`] after fresh creation, after merging, and
    /// even when the primary write is skipped as unchanged — "unchanged"
    /// describes only the primary file, so a secondary artifact (e.g.
    /// Neovim's `.nvim-dap.lua`) can still be missing or stale and must stay
    /// kept in sync regardless of whether the primary file was rewritten
    /// this run.
    ///
    /// The default implementation is a no-op.
    fn post_write(&self, _port: u16, _project_root: &Path) -> Result<()> {
        Ok(())
    }

    /// The display name for this IDE (used in log messages).
    fn ide_name(&self) -> &'static str;
}

// ─────────────────────────────────────────────────────────────────
// File I/O helper
// ─────────────────────────────────────────────────────────────────

/// Execute a generator: handle file I/O and return an [`IdeConfigResult`].
///
/// The single location that owns all filesystem operations for IDE config
/// generation. Generator implementations stay pure (string in, string out)
/// and are easy to unit-test without touching the filesystem.
///
/// Steps:
/// 1. Compute the target path via [`IdeConfigGenerator::config_path`].
/// 2. If the file already exists, read it and call [`IdeConfigGenerator::merge_config`].
/// 3. If the file does not exist, call [`IdeConfigGenerator::generate`] for fresh content.
/// 4. If the merged content is byte-identical to what's on disk, skip the
///    primary write (mtime untouched) and report [`ConfigAction::Skipped`] —
///    but step 7 still runs before returning: "unchanged" describes only the
///    primary file, and [`IdeConfigGenerator::post_write`]'s own secondary
///    artifact can still be missing or stale.
/// 5. Otherwise, ensure the parent directory exists (`create_dir_all`).
/// 6. Write the content and return an [`IdeConfigResult`].
/// 7. Call [`IdeConfigGenerator::post_write`] for any secondary file writes —
///    on every path above, including the skip path in step 4.
fn run_generator(
    generator: &dyn IdeConfigGenerator,
    port: u16,
    project_root: &Path,
) -> Result<Option<IdeConfigResult>> {
    let config_path = generator.config_path(project_root);

    let (content, action) = if generator.config_exists(project_root) {
        let existing = std::fs::read_to_string(&config_path)
            .map_err(|e| IdeConfigError::io(&config_path, e))?;
        let merged = generator.merge_config(&existing, port, project_root)?;
        if merged == existing {
            log::info!(
                "{} DAP config skipped (content unchanged) at {}",
                generator.ide_name(),
                config_path.display(),
            );
            // The primary file needs no rewrite, but a secondary artifact
            // (e.g. Neovim's .nvim-dap.lua) is not covered by that
            // unchanged-ness check at all — it can be missing or stale, so
            // post_write still runs before returning (see its trait doc).
            generator.post_write(port, project_root)?;
            return Ok(Some(IdeConfigResult {
                path: config_path,
                action: ConfigAction::Skipped("content unchanged".to_string()),
            }));
        }
        (merged, ConfigAction::Updated)
    } else {
        let fresh = generator.generate(port, project_root)?;
        (fresh, ConfigAction::Created)
    };

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| IdeConfigError::io(parent, e))?;
    }
    std::fs::write(&config_path, &content).map_err(|e| IdeConfigError::io(&config_path, e))?;

    generator.post_write(port, project_root)?;

    let action_label = match &action {
        ConfigAction::Created => "created",
        ConfigAction::Updated => "updated",
        ConfigAction::Skipped(_) => "skipped",
    };
    log::info!(
        "{} DAP config {} at {}",
        generator.ide_name(),
        action_label,
        config_path.display(),
    );

    Ok(Some(IdeConfigResult {
        path: config_path,
        action,
    }))
}

// ─────────────────────────────────────────────────────────────────
// Dispatch function
// ─────────────────────────────────────────────────────────────────

/// Generate IDE-specific DAP config for the detected (or specified) IDE.
///
/// Returns `Ok(None)` when:
/// - `ide` is `None`
/// - The IDE doesn't support DAP config (`ParentIde::IntelliJ`/`AndroidStudio`)
///
/// On success returns an [`IdeConfigResult`] describing what was created,
/// updated, or (Helix, always; any generator, when unchanged) skipped.
pub fn generate_ide_config(
    ide: Option<ParentIde>,
    port: u16,
    project_root: &Path,
) -> Result<Option<IdeConfigResult>> {
    let ide = match ide {
        Some(ide) if ide.supports_dap_config() => ide,
        _ => return Ok(None),
    };

    match ide {
        // VS Code, VS Code Insiders, and Cursor share the launch.json format.
        ParentIde::VSCode | ParentIde::VSCodeInsiders | ParentIde::Cursor => {
            run_generator(&vscode::VSCodeGenerator, port, project_root)
        }
        // Neovim uses launch.json as primary (via load_launchjs) plus a Lua snippet.
        ParentIde::Neovim => run_generator(&neovim::NeovimGenerator, port, project_root),
        // Helix has no working transport for frust-dap — see helix's module doc.
        ParentIde::Helix => Ok(Some(helix::HelixGenerator.skip_result(project_root))),
        // Emacs uses a .frust/dap-emacs.el Elisp snippet.
        ParentIde::Emacs => run_generator(&emacs::EmacsGenerator, port, project_root),
        // Zed uses .zed/debug.json with tcp_connection.
        ParentIde::Zed => run_generator(&zed::ZedGenerator, port, project_root),
        // Already excluded by supports_dap_config() above, but the compiler
        // requires exhaustive coverage.
        ParentIde::IntelliJ | ParentIde::AndroidStudio => Ok(None),
    }
}

// ─────────────────────────────────────────────────────────────────
// IDE name parsing
// ─────────────────────────────────────────────────────────────────

/// Parse a CLI IDE name string to a [`ParentIde`] variant.
///
/// Accepts common aliases in addition to canonical names (case-insensitive):
///
/// | Input | Result |
/// |-------|--------|
/// | `vscode`, `vs-code`, `code` | `ParentIde::VSCode` |
/// | `neovim`, `nvim` | `ParentIde::Neovim` |
/// | `helix`, `hx` | `ParentIde::Helix` |
/// | `zed` | `ParentIde::Zed` |
/// | `emacs` | `ParentIde::Emacs` |
///
/// Returns an error for unrecognised inputs.
pub fn parse_ide_name(name: &str) -> Result<ParentIde> {
    match name.to_lowercase().as_str() {
        "vscode" | "vs-code" | "code" => Ok(ParentIde::VSCode),
        "neovim" | "nvim" => Ok(ParentIde::Neovim),
        "helix" | "hx" => Ok(ParentIde::Helix),
        "zed" => Ok(ParentIde::Zed),
        "emacs" => Ok(ParentIde::Emacs),
        other => Err(IdeConfigError::message(format!(
            "unknown IDE '{other}'. Valid values: vscode, neovim, helix, zed, emacs"
        ))),
    }
}

// ─────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use test_support::unique_temp_dir;

    // ── parse_ide_name ───────────────────────────────────────────

    #[test]
    fn test_parse_ide_name_vscode_canonical() {
        assert_eq!(parse_ide_name("vscode").unwrap(), ParentIde::VSCode);
    }

    #[test]
    fn test_parse_ide_name_vscode_aliases() {
        assert_eq!(parse_ide_name("vs-code").unwrap(), ParentIde::VSCode);
        assert_eq!(parse_ide_name("code").unwrap(), ParentIde::VSCode);
    }

    #[test]
    fn test_parse_ide_name_vscode_case_insensitive() {
        assert_eq!(parse_ide_name("VSCODE").unwrap(), ParentIde::VSCode);
        assert_eq!(parse_ide_name("VsCode").unwrap(), ParentIde::VSCode);
    }

    #[test]
    fn test_parse_ide_name_neovim() {
        assert_eq!(parse_ide_name("neovim").unwrap(), ParentIde::Neovim);
        assert_eq!(parse_ide_name("nvim").unwrap(), ParentIde::Neovim);
    }

    #[test]
    fn test_parse_ide_name_helix() {
        assert_eq!(parse_ide_name("helix").unwrap(), ParentIde::Helix);
        assert_eq!(parse_ide_name("hx").unwrap(), ParentIde::Helix);
    }

    #[test]
    fn test_parse_ide_name_zed() {
        assert_eq!(parse_ide_name("zed").unwrap(), ParentIde::Zed);
    }

    #[test]
    fn test_parse_ide_name_emacs() {
        assert_eq!(parse_ide_name("emacs").unwrap(), ParentIde::Emacs);
    }

    #[test]
    fn test_parse_ide_name_invalid_returns_error() {
        assert!(parse_ide_name("sublime").is_err());
        assert!(parse_ide_name("").is_err());
        assert!(parse_ide_name("idea").is_err());
        assert!(parse_ide_name("android-studio").is_err());
    }

    #[test]
    fn test_parse_ide_name_error_message_lists_valid_values() {
        let err = parse_ide_name("sublime").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("vscode"),
            "error should list valid values: {msg}"
        );
        assert!(
            msg.contains("neovim"),
            "error should list valid values: {msg}"
        );
    }

    // ── generate_ide_config — None / unsupported IDE paths ──────

    #[test]
    fn test_generate_ide_config_none_returns_none() {
        let result = generate_ide_config(None, 4711, Path::new("/unused"));
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn test_generate_ide_config_intellij_returns_none() {
        let result = generate_ide_config(Some(ParentIde::IntelliJ), 4711, Path::new("/unused"));
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn test_generate_ide_config_android_studio_returns_none() {
        let result =
            generate_ide_config(Some(ParentIde::AndroidStudio), 4711, Path::new("/unused"));
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn test_standalone_config_generation_vscode() {
        let dir = unique_temp_dir("dispatch-vscode");
        let result = generate_ide_config(Some(ParentIde::VSCode), 4711, &dir).unwrap();
        assert!(result.is_some());
        assert!(dir.join(".vscode/launch.json").exists());
    }

    #[test]
    fn test_generate_ide_config_helix_is_skipped_and_writes_nothing() {
        let dir = unique_temp_dir("dispatch-helix");
        let result = generate_ide_config(Some(ParentIde::Helix), 4711, &dir)
            .unwrap()
            .unwrap();
        assert_eq!(
            result.action,
            ConfigAction::Skipped(helix::SKIP_REASON.to_string())
        );
        assert!(!dir.join(".helix").exists());
    }

    // ── ConfigAction — enum variants ────────────────────────────

    #[test]
    fn test_config_action_variants_are_eq() {
        assert_eq!(ConfigAction::Created, ConfigAction::Created);
        assert_eq!(ConfigAction::Updated, ConfigAction::Updated);
        assert_eq!(
            ConfigAction::Skipped("reason".to_string()),
            ConfigAction::Skipped("reason".to_string())
        );
        assert_ne!(ConfigAction::Created, ConfigAction::Updated);
    }

    // ── run_generator: content comparison / skip behaviour ──────

    /// Verifies the create → skip → update sequence for `run_generator`.
    ///
    /// 1. First call: file does not exist → `ConfigAction::Created` and file written.
    /// 2. Second call with identical port: content unchanged → `ConfigAction::Skipped`.
    /// 3. Third call with different port: content changed → `ConfigAction::Updated`.
    #[test]
    fn test_run_generator_create_skip_update_sequence() {
        let dir = unique_temp_dir("run-generator-sequence");

        let result1 = run_generator(&vscode::VSCodeGenerator, 12345, &dir)
            .unwrap()
            .unwrap();
        assert!(
            matches!(result1.action, ConfigAction::Created),
            "expected Created, got {:?}",
            result1.action
        );
        assert!(
            result1.path.exists(),
            "config file should have been written"
        );

        let result2 = run_generator(&vscode::VSCodeGenerator, 12345, &dir)
            .unwrap()
            .unwrap();
        assert!(
            matches!(result2.action, ConfigAction::Skipped(ref reason) if reason == "content unchanged"),
            "expected Skipped(\"content unchanged\"), got {:?}",
            result2.action
        );

        let result3 = run_generator(&vscode::VSCodeGenerator, 54321, &dir)
            .unwrap()
            .unwrap();
        assert!(
            matches!(result3.action, ConfigAction::Updated),
            "expected Updated, got {:?}",
            result3.action
        );
    }

    /// Verifies that when content is unchanged the file modification time is
    /// not updated (i.e. no write occurs on skip).
    #[test]
    fn test_run_generator_skip_does_not_modify_file() {
        use std::time::Duration;

        let dir = unique_temp_dir("run-generator-skip-mtime");

        run_generator(&vscode::VSCodeGenerator, 12345, &dir)
            .unwrap()
            .unwrap();

        let config_path = vscode::VSCodeGenerator.config_path(&dir);
        let mtime_before = std::fs::metadata(&config_path).unwrap().modified().unwrap();

        // Give the clock a small window so a spurious write would be detectable.
        std::thread::sleep(Duration::from_millis(10));

        let result = run_generator(&vscode::VSCodeGenerator, 12345, &dir)
            .unwrap()
            .unwrap();
        assert!(matches!(result.action, ConfigAction::Skipped(_)));

        let mtime_after = std::fs::metadata(&config_path).unwrap().modified().unwrap();

        assert_eq!(
            mtime_before, mtime_after,
            "file mtime should not change when content is unchanged"
        );
    }

    /// Verifies `run_generator`'s skip path still runs `post_write`: when the
    /// primary `launch.json` is unchanged (`Skipped`) but the secondary
    /// `.nvim-dap.lua` has been deleted out from under it, the next call
    /// recreates the secondary file even though the primary write is
    /// skipped.
    #[test]
    fn test_run_generator_skip_still_recreates_deleted_secondary_artifact() {
        let dir = unique_temp_dir("run-generator-skip-recreates-secondary");

        // First call: creates both launch.json and .nvim-dap.lua.
        let result1 = run_generator(&neovim::NeovimGenerator, 4711, &dir)
            .unwrap()
            .unwrap();
        assert!(matches!(result1.action, ConfigAction::Created));
        let lua_path = dir.canonicalize().unwrap().join(".nvim-dap.lua");
        assert!(
            lua_path.exists(),
            ".nvim-dap.lua should exist after Created"
        );

        // Simulate the secondary artifact going missing (deleted by the user,
        // or never having existed on an older run) without touching the
        // primary file.
        std::fs::remove_file(&lua_path).unwrap();
        assert!(!lua_path.exists());

        // Second call with the same port: primary content is unchanged, so
        // this reports Skipped — but post_write must still have run and
        // recreated the secondary artifact.
        let result2 = run_generator(&neovim::NeovimGenerator, 4711, &dir)
            .unwrap()
            .unwrap();
        assert!(
            matches!(result2.action, ConfigAction::Skipped(_)),
            "expected Skipped, got {:?}",
            result2.action
        );
        assert!(
            lua_path.exists(),
            ".nvim-dap.lua should have been recreated by post_write even though \
             the primary launch.json write was skipped"
        );
    }
}
