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
//! ## User-editable versus frust-owned files
//!
//! [`WriteMode::IfAbsent`]'s leave-alone rule protects files a **user** edits
//! — VS Code's `launch.json`, Zed's `debug.json`, the Neovim pair — so an
//! automatic write never rewrites a frust entry the user already has. It does
//! not apply to frust-**owned** generated files: Emacs'
//! `.frust/dap-emacs.el` is regenerated on every run (unless byte-identical),
//! because it is `load-file`d as code and a repo-supplied file that merely
//! carries the frust marker must never be kept as if frust had written it.
//!
//! ## Writes
//!
//! Every file this module writes goes through [`write_contained`]: a temp
//! file in the target's own directory, then a rename over the target. Before
//! reading or writing, the target file and its config directory are resolved
//! through any symlink and must stay inside the generator's
//! [`containment root`](IdeConfigGenerator::containment_root) — a
//! `.vscode/`/`.zed/`/`.frust/` directory (or the file itself) that is a
//! symlink out of the project is refused with [`IdeConfigError::Refused`].
//! A symlink that resolves *inside* the root is followed: the write lands on
//! the resolved file, so an in-project link survives the write.
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
    /// A write refused because its target (or the target's config directory)
    /// resolves outside the project — see the module doc's "Writes".
    #[error("{0}")]
    Refused(String),
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

    fn refused(msg: impl Into<String>) -> Self {
        Self::Refused(msg.into())
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

/// How [`generate_ide_config`] treats a config file that already exists.
///
/// The two callers want different things: an explicit "generate now" wants
/// the frust entry brought up to date (the port may have moved), while an
/// automatic write must never touch an entry the user already has — they may
/// have edited it, and rewriting it on every launch is exactly the churn the
/// automatic path exists to avoid.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WriteMode {
    /// Create the file when missing; otherwise merge the frust entry in,
    /// replacing an existing one (its port is refreshed). The explicit
    /// "generate now" semantics.
    #[default]
    Refresh,
    /// Create the file when missing; append the frust entry to a file that
    /// has none; but leave a file that **already contains** a frust entry
    /// untouched (whatever port it names). The outcome says whether the kept
    /// entry still names the port being configured: the same port (or one
    /// that cannot be read) reports [`ConfigAction::Skipped`] with
    /// [`ENTRY_PRESENT_REASON`]; a different port reports
    /// [`ConfigAction::StalePort`] — still with nothing written. The
    /// automatic-write semantics. Frust-owned files (Emacs) are outside this
    /// rule; see the module doc.
    IfAbsent,
}

/// The [`ConfigAction::Skipped`] reason [`WriteMode::IfAbsent`] reports when
/// the file already carries a frust entry.
pub const ENTRY_PRESENT_REASON: &str = "frust entry already present";

/// What happened during config generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigAction {
    /// Config file was created (did not previously exist).
    Created,
    /// Existing config file was updated with a new/changed frust entry.
    Updated,
    /// Config generation was skipped (with reason).
    Skipped(String),
    /// [`WriteMode::IfAbsent`] kept an existing frust entry that names a
    /// different port than the one being configured — nothing was written,
    /// so the editor would connect to `existing` while the server listens
    /// on `bound`. Only an explicit [`WriteMode::Refresh`] rewrites it.
    StalePort {
        /// The port the retained entry names.
        existing: u16,
        /// The port generation was asked to configure (the bound port).
        bound: u16,
    },
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

    /// Whether `existing` (the current content of
    /// [`config_path`](Self::config_path)) already contains this generator's
    /// frust entry, identified by the same marker
    /// [`merge_config`](Self::merge_config) matches on.
    ///
    /// Consulted only under [`WriteMode::IfAbsent`]: a `true` here leaves the
    /// file untouched. Returns an error when `existing` cannot be parsed —
    /// the same failure `merge_config` would report for it. A frust-owned
    /// file (Emacs) always answers `false`, so it is always regenerated.
    fn has_frust_entry(&self, existing: &str) -> Result<bool>;

    /// The port `existing`'s frust entry names: `None` when there is no
    /// entry — or when the entry names no readable port (a hand-edited
    /// entry) — and `Some(port)` otherwise, found by the same marker
    /// [`merge_config`](Self::merge_config) matches on.
    ///
    /// Consulted under [`WriteMode::IfAbsent`] once
    /// [`has_frust_entry`](Self::has_frust_entry) found an entry, to tell a
    /// kept entry that still matches the bound port from a stale one
    /// ([`ConfigAction::StalePort`]). Errors exactly as `has_frust_entry`.
    fn frust_entry_port(&self, existing: &str) -> Result<Option<u16>>;

    /// The directory every file this generator writes must stay inside once
    /// symlinks are resolved (see the module doc's "Writes").
    ///
    /// Defaults to `project_root`; a generator whose config lives at a
    /// detected workspace root (VS Code, Neovim) answers that root instead —
    /// the root it already vets with its own home-boundary guard.
    fn containment_root(&self, project_root: &Path) -> PathBuf {
        project_root.to_path_buf()
    }

    /// Optional post-generation hook for secondary file writes.
    ///
    /// Called by [`run_generator`] after fresh creation, after merging, and
    /// when the primary write is skipped as unchanged — "skipped" describes
    /// only the primary file, so a secondary artifact (e.g. Neovim's
    /// `.nvim-dap.lua`) can still be missing or stale and must stay in sync
    /// regardless of whether the primary file was rewritten this run.
    ///
    /// `port` is the port the **primary file now names**. Under
    /// [`WriteMode::IfAbsent`], when an existing entry is kept, that is the
    /// *retained* entry's port, not the one generation was asked for — the
    /// secondary file must never disagree with the primary. When the kept
    /// entry names no readable port, the hook is not called at all.
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
/// 1. Compute the target path via [`IdeConfigGenerator::config_path`] and
///    refuse it ([`IdeConfigError::Refused`]) if it, or its config directory,
///    already resolves outside [`IdeConfigGenerator::containment_root`] —
///    before anything is read through it.
/// 2. If the file already exists, read it. Under [`WriteMode::IfAbsent`],
///    when [`IdeConfigGenerator::has_frust_entry`] finds the frust entry
///    already there, skip the primary write entirely and report
///    [`ConfigAction::StalePort`] when
///    [`IdeConfigGenerator::frust_entry_port`] names a different port, else
///    [`ConfigAction::Skipped`] with [`ENTRY_PRESENT_REASON`]. Step 7 runs
///    with the **retained** port (and not at all when the kept entry names
///    no readable port). Otherwise call [`IdeConfigGenerator::merge_config`].
/// 3. If the file does not exist, call [`IdeConfigGenerator::generate`] for fresh content.
/// 4. If the merged content is byte-identical to what's on disk, skip the
///    primary write (mtime untouched) and report [`ConfigAction::Skipped`] —
///    but step 7 still runs before returning: "unchanged" describes only the
///    primary file, and [`IdeConfigGenerator::post_write`]'s own secondary
///    artifact can still be missing or stale.
/// 5. Otherwise, ensure the parent directory exists (`create_dir_all`).
/// 6. Write the content via [`write_contained`] (containment re-checked,
///    temp file + rename) and return an [`IdeConfigResult`].
/// 7. Call [`IdeConfigGenerator::post_write`] for any secondary file writes.
fn run_generator(
    generator: &dyn IdeConfigGenerator,
    port: u16,
    project_root: &Path,
    mode: WriteMode,
) -> Result<Option<IdeConfigResult>> {
    let config_path = generator.config_path(project_root);
    let anchor = generator.containment_root(project_root);
    check_contained(&config_path, &anchor)?;

    let (content, action) = if generator.config_exists(project_root) {
        let existing = std::fs::read_to_string(&config_path)
            .map_err(|e| IdeConfigError::io(&config_path, e))?;
        if mode == WriteMode::IfAbsent && generator.has_frust_entry(&existing)? {
            let retained = generator.frust_entry_port(&existing)?;
            let action = match retained {
                Some(existing) if existing != port => ConfigAction::StalePort {
                    existing,
                    bound: port,
                },
                _ => ConfigAction::Skipped(ENTRY_PRESENT_REASON.to_string()),
            };
            log::info!(
                "{} DAP config kept ({ENTRY_PRESENT_REASON}: {action:?}) at {}",
                generator.ide_name(),
                config_path.display(),
            );
            // A secondary artifact must name the port the kept primary
            // entry names — never the port that was *not* written. An entry
            // with no readable port gives the secondary nothing to agree
            // with, so it is left as it is.
            if let Some(retained) = retained {
                generator.post_write(retained, project_root)?;
            }
            return Ok(Some(IdeConfigResult {
                path: config_path,
                action,
            }));
        }
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
    write_contained(&config_path, &anchor, &content)?;

    generator.post_write(port, project_root)?;

    log::info!(
        "{} DAP config {:?} at {}",
        generator.ide_name(),
        action,
        config_path.display(),
    );

    Ok(Some(IdeConfigResult {
        path: config_path,
        action,
    }))
}

// ─────────────────────────────────────────────────────────────────
// Contained writes
// ─────────────────────────────────────────────────────────────────

/// Refuse `path` if it, or its parent (config) directory, exists and resolves
/// outside `anchor` once symlinks are followed — the read-side half of
/// [`write_contained`]'s check, run before a file is read through a link.
///
/// A parent that does not exist yet passes (it will be created inside
/// `anchor`); a *dangling* symlink is refused, since creating through it
/// would land wherever it points.
pub(crate) fn check_contained(path: &Path, anchor: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Err(IdeConfigError::message(format!(
            "`{}` does not name a file",
            path.display()
        )));
    };
    match std::fs::symlink_metadata(parent) {
        Ok(_) => resolve_contained(path, anchor).map(|_| ()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(IdeConfigError::io(parent, e)),
    }
}

/// Resolve the file a write of `path` must land on, refusing any target that
/// escapes `anchor`: `path`'s parent directory must exist and resolve inside
/// `anchor`; `path` itself, when it is a symlink, must resolve inside
/// `anchor` too, and the resolved file is what gets written (so an
/// in-project link survives the write). A dangling symlink at either level
/// is refused.
fn resolve_contained(path: &Path, anchor: &Path) -> Result<PathBuf> {
    let anchor = anchor
        .canonicalize()
        .map_err(|e| IdeConfigError::io(anchor, e))?;
    let (Some(parent), Some(file_name)) = (path.parent(), path.file_name()) else {
        return Err(IdeConfigError::message(format!(
            "`{}` does not name a file",
            path.display()
        )));
    };
    let parent = parent.canonicalize().map_err(|e| {
        if is_symlink(parent) {
            dangling(parent, &e)
        } else {
            IdeConfigError::io(parent, e)
        }
    })?;
    ensure_inside(&parent, &anchor, path)?;

    if is_symlink(path) {
        let resolved = path.canonicalize().map_err(|e| dangling(path, &e))?;
        ensure_inside(&resolved, &anchor, path)?;
        Ok(resolved)
    } else {
        Ok(parent.join(file_name))
    }
}

/// Whether `path` itself (not what it points at) is a symlink.
fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// The refusal for a symlink that does not resolve.
fn dangling(path: &Path, e: &std::io::Error) -> IdeConfigError {
    IdeConfigError::refused(format!(
        "refusing to write DAP config: `{}` is a symlink that does not resolve ({e})",
        path.display()
    ))
}

/// Refuse `resolved` (what `requested` resolves to) unless it lies inside the
/// canonical `anchor`.
fn ensure_inside(resolved: &Path, anchor: &Path, requested: &Path) -> Result<()> {
    if resolved.starts_with(anchor) {
        return Ok(());
    }
    Err(IdeConfigError::refused(format!(
        "refusing to write DAP config: `{}` resolves to `{}`, outside the project \
         (`{}`) — a symlinked config file or directory must stay inside it",
        requested.display(),
        resolved.display(),
        anchor.display(),
    )))
}

/// Write `content` to `path` — the one write path every generated file takes.
///
/// The target is resolved and vetted by [`resolve_contained`] (it must stay
/// inside `anchor`), then written as a temp file in the resolved target's own
/// directory and renamed over it, so a reader never sees a torn file. An
/// existing target's permissions carry over to the replacement. `path`'s
/// parent directory must already exist.
pub(crate) fn write_contained(path: &Path, anchor: &Path, content: &str) -> Result<()> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    let target = resolve_contained(path, anchor)?;
    let (Some(dir), Some(file_name)) = (target.parent(), target.file_name()) else {
        return Err(IdeConfigError::message(format!(
            "`{}` does not name a file",
            target.display()
        )));
    };
    let temp = dir.join(format!(
        ".{}.frust-tmp-{}-{}",
        file_name.to_string_lossy(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
    ));
    std::fs::write(&temp, content).map_err(|e| IdeConfigError::io(&temp, e))?;
    if let Ok(meta) = std::fs::metadata(&target) {
        // Best-effort: failing leaves a fresh file's default permissions.
        let _ = std::fs::set_permissions(&temp, meta.permissions());
    }
    if let Err(e) = std::fs::rename(&temp, &target) {
        let _ = std::fs::remove_file(&temp);
        return Err(IdeConfigError::io(&target, e));
    }
    Ok(())
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
/// updated, or (Helix, always; any generator, when unchanged or — under
/// [`WriteMode::IfAbsent`] — when a frust entry is already present) skipped.
///
/// `mode` decides what happens to an existing file; see [`WriteMode`]. For
/// Neovim (primary `.vscode/launch.json` plus `.nvim-dap.lua`) the rule is
/// applied to the primary file.
pub fn generate_ide_config(
    ide: Option<ParentIde>,
    port: u16,
    project_root: &Path,
    mode: WriteMode,
) -> Result<Option<IdeConfigResult>> {
    let ide = match ide {
        Some(ide) if ide.supports_dap_config() => ide,
        _ => return Ok(None),
    };

    match ide {
        // VS Code, VS Code Insiders, and Cursor share the launch.json format.
        ParentIde::VSCode | ParentIde::VSCodeInsiders | ParentIde::Cursor => {
            run_generator(&vscode::VSCodeGenerator, port, project_root, mode)
        }
        // Neovim uses launch.json as primary (via load_launchjs) plus a Lua snippet.
        ParentIde::Neovim => run_generator(&neovim::NeovimGenerator, port, project_root, mode),
        // Helix has no working transport for frust-dap — see helix's module doc.
        ParentIde::Helix => Ok(Some(helix::HelixGenerator.skip_result(project_root))),
        // Emacs uses a .frust/dap-emacs.el Elisp snippet.
        ParentIde::Emacs => run_generator(&emacs::EmacsGenerator, port, project_root, mode),
        // Zed uses .zed/debug.json with tcp_connection.
        ParentIde::Zed => run_generator(&zed::ZedGenerator, port, project_root, mode),
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
        let result = generate_ide_config(None, 4711, Path::new("/unused"), WriteMode::Refresh);
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn test_generate_ide_config_intellij_returns_none() {
        let result = generate_ide_config(
            Some(ParentIde::IntelliJ),
            4711,
            Path::new("/unused"),
            WriteMode::Refresh,
        );
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn test_generate_ide_config_android_studio_returns_none() {
        let result = generate_ide_config(
            Some(ParentIde::AndroidStudio),
            4711,
            Path::new("/unused"),
            WriteMode::Refresh,
        );
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn test_standalone_config_generation_vscode() {
        let dir = unique_temp_dir("dispatch-vscode");
        let result =
            generate_ide_config(Some(ParentIde::VSCode), 4711, &dir, WriteMode::Refresh).unwrap();
        assert!(result.is_some());
        assert!(dir.join(".vscode/launch.json").exists());
    }

    #[test]
    fn test_generate_ide_config_helix_is_skipped_and_writes_nothing() {
        let dir = unique_temp_dir("dispatch-helix");
        let result = generate_ide_config(Some(ParentIde::Helix), 4711, &dir, WriteMode::IfAbsent)
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
        assert_ne!(
            ConfigAction::StalePort {
                existing: 1,
                bound: 2
            },
            ConfigAction::Skipped(ENTRY_PRESENT_REASON.to_string())
        );
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

        let result1 = run_generator(&vscode::VSCodeGenerator, 12345, &dir, WriteMode::Refresh)
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

        let result2 = run_generator(&vscode::VSCodeGenerator, 12345, &dir, WriteMode::Refresh)
            .unwrap()
            .unwrap();
        assert!(
            matches!(result2.action, ConfigAction::Skipped(ref reason) if reason == "content unchanged"),
            "expected Skipped(\"content unchanged\"), got {:?}",
            result2.action
        );

        let result3 = run_generator(&vscode::VSCodeGenerator, 54321, &dir, WriteMode::Refresh)
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

        run_generator(&vscode::VSCodeGenerator, 12345, &dir, WriteMode::Refresh)
            .unwrap()
            .unwrap();

        let config_path = vscode::VSCodeGenerator.config_path(&dir);
        let mtime_before = std::fs::metadata(&config_path).unwrap().modified().unwrap();

        // Give the clock a small window so a spurious write would be detectable.
        std::thread::sleep(Duration::from_millis(10));

        let result = run_generator(&vscode::VSCodeGenerator, 12345, &dir, WriteMode::Refresh)
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
        let result1 = run_generator(&neovim::NeovimGenerator, 4711, &dir, WriteMode::Refresh)
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
        let result2 = run_generator(&neovim::NeovimGenerator, 4711, &dir, WriteMode::Refresh)
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

    // ── WriteMode: IfAbsent vs Refresh ──────────────────────────

    /// A Zed `debug.json` carrying the frust entry at port 1111 next to a
    /// foreign entry.
    const ZED_WITH_FRUST_1111: &str = r#"[
  {"label": "Other", "adapter": "other"},
  {"label": "Frust (TUI DAP)", "adapter": "Delve", "request": "launch",
   "tcp_connection": {"host": "127.0.0.1", "port": 1111}}
]"#;

    /// A Zed `debug.json` with only foreign entries.
    const ZED_FOREIGN_ONLY: &str = r#"[
  {"label": "Config A", "adapter": "a"},
  {"label": "Config B", "adapter": "b"}
]"#;

    /// A VS Code `launch.json` (JSONC, with a comment) carrying the frust
    /// entry at port 1111 next to a foreign entry.
    const VSCODE_WITH_FRUST_1111: &str = r#"{
  // hand-written
  "version": "0.2.0",
  "configurations": [
    {"name": "Rust", "type": "lldb", "request": "launch"},
    {"name": "Frust (TUI DAP)", "type": "frust", "request": "launch", "debugServer": 1111},
  ]
}"#;

    /// A VS Code `launch.json` with only foreign entries.
    const VSCODE_FOREIGN_ONLY: &str = r#"{
  "version": "0.2.0",
  "configurations": [
    {"name": "Rust", "type": "lldb", "request": "launch"}
  ]
}"#;

    fn write_file(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn generate(ide: ParentIde, port: u16, dir: &Path, mode: WriteMode) -> IdeConfigResult {
        generate_ide_config(Some(ide), port, dir, mode)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn test_if_absent_on_a_missing_file_creates_it() {
        let dir = unique_temp_dir("if-absent-missing-zed");
        let result = generate(ParentIde::Zed, 4711, &dir, WriteMode::IfAbsent);
        assert_eq!(result.action, ConfigAction::Created);
        let parsed: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(dir.join(".zed/debug.json")).unwrap())
                .unwrap();
        assert_eq!(parsed[0]["tcp_connection"]["port"], 4711);

        let dir = unique_temp_dir("if-absent-missing-vscode");
        let result = generate(ParentIde::VSCode, 4711, &dir, WriteMode::IfAbsent);
        assert_eq!(result.action, ConfigAction::Created);
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&result.path).unwrap()).unwrap();
        assert_eq!(parsed["configurations"][0]["debugServer"], 4711);
    }

    /// A kept entry naming a different port is reported as stale — and the
    /// file is still left byte-identical.
    #[test]
    fn test_if_absent_reports_a_stale_port_and_leaves_the_bytes_zed() {
        let dir = unique_temp_dir("if-absent-stale-zed");
        let path = dir.join(".zed/debug.json");
        write_file(&path, ZED_WITH_FRUST_1111);

        let result = generate(ParentIde::Zed, 2222, &dir, WriteMode::IfAbsent);
        assert_eq!(
            result.action,
            ConfigAction::StalePort {
                existing: 1111,
                bound: 2222
            }
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            ZED_WITH_FRUST_1111,
            "the file is left byte-identical, stale port and all"
        );
    }

    #[test]
    fn test_if_absent_reports_a_stale_port_and_leaves_the_bytes_vscode() {
        let dir = unique_temp_dir("if-absent-stale-vscode");
        let path = dir.join(".vscode/launch.json");
        write_file(&path, VSCODE_WITH_FRUST_1111);

        let result = generate(ParentIde::VSCode, 2222, &dir, WriteMode::IfAbsent);
        assert_eq!(
            result.action,
            ConfigAction::StalePort {
                existing: 1111,
                bound: 2222
            }
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            VSCODE_WITH_FRUST_1111,
            "the file (comment and trailing comma included) is left byte-identical"
        );
    }

    /// A kept entry already naming the bound port is the plain, silent skip.
    #[test]
    fn test_if_absent_same_port_entry_is_the_plain_skip() {
        for (ide, rel, content) in [
            (ParentIde::Zed, ".zed/debug.json", ZED_WITH_FRUST_1111),
            (
                ParentIde::VSCode,
                ".vscode/launch.json",
                VSCODE_WITH_FRUST_1111,
            ),
        ] {
            let dir = unique_temp_dir("if-absent-same-port");
            let path = dir.join(rel);
            write_file(&path, content);
            let result = generate(ide, 1111, &dir, WriteMode::IfAbsent);
            assert_eq!(
                result.action,
                ConfigAction::Skipped(ENTRY_PRESENT_REASON.to_string()),
                "{ide:?}"
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        }
    }

    /// An entry with no readable port is kept as the plain skip — there is no
    /// old port to name.
    #[test]
    fn test_if_absent_portless_entry_is_the_plain_skip() {
        let dir = unique_temp_dir("if-absent-portless-zed");
        let path = dir.join(".zed/debug.json");
        let content = r#"[{"label": "Frust (TUI DAP)", "adapter": "Delve"}]"#;
        write_file(&path, content);
        let result = generate(ParentIde::Zed, 2222, &dir, WriteMode::IfAbsent);
        assert_eq!(
            result.action,
            ConfigAction::Skipped(ENTRY_PRESENT_REASON.to_string())
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
    }

    #[test]
    fn test_empty_zed_debug_json_is_treated_as_absent() {
        for mode in [WriteMode::IfAbsent, WriteMode::Refresh] {
            let dir = unique_temp_dir("empty-zed");
            let path = dir.join(".zed/debug.json");
            write_file(&path, " \n");
            let result = generate(ParentIde::Zed, 4711, &dir, mode);
            assert_eq!(result.action, ConfigAction::Updated, "{mode:?}");
            let parsed: Vec<serde_json::Value> =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(parsed.len(), 1);
            assert_eq!(parsed[0]["tcp_connection"]["port"], 4711);
        }
    }

    #[test]
    fn test_if_absent_appends_to_a_file_without_a_frust_entry_zed() {
        let dir = unique_temp_dir("if-absent-foreign-zed");
        let path = dir.join(".zed/debug.json");
        write_file(&path, ZED_FOREIGN_ONLY);

        let result = generate(ParentIde::Zed, 4711, &dir, WriteMode::IfAbsent);
        assert_eq!(result.action, ConfigAction::Updated);
        let parsed: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0]["label"], "Config A");
        assert_eq!(parsed[1]["label"], "Config B");
        assert_eq!(parsed[2]["label"], "Frust (TUI DAP)");
        assert_eq!(parsed[2]["tcp_connection"]["port"], 4711);
    }

    #[test]
    fn test_if_absent_appends_to_a_file_without_a_frust_entry_vscode() {
        let dir = unique_temp_dir("if-absent-foreign-vscode");
        let path = dir.join(".vscode/launch.json");
        write_file(&path, VSCODE_FOREIGN_ONLY);

        let result = generate(ParentIde::VSCode, 4711, &dir, WriteMode::IfAbsent);
        assert_eq!(result.action, ConfigAction::Updated);
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let configs = parsed["configurations"].as_array().unwrap();
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0]["name"], "Rust");
        assert_eq!(configs[1]["name"], "Frust (TUI DAP)");
        assert_eq!(configs[1]["debugServer"], 4711);
    }

    #[test]
    fn test_refresh_updates_the_port_of_an_existing_frust_entry() {
        let dir = unique_temp_dir("refresh-present-zed");
        let path = dir.join(".zed/debug.json");
        write_file(&path, ZED_WITH_FRUST_1111);
        let result = generate(ParentIde::Zed, 2222, &dir, WriteMode::Refresh);
        assert_eq!(result.action, ConfigAction::Updated);
        let parsed: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["label"], "Other");
        assert_eq!(parsed[1]["tcp_connection"]["port"], 2222);

        let dir = unique_temp_dir("refresh-present-vscode");
        let path = dir.join(".vscode/launch.json");
        write_file(&path, VSCODE_WITH_FRUST_1111);
        let result = generate(ParentIde::VSCode, 2222, &dir, WriteMode::Refresh);
        assert_eq!(result.action, ConfigAction::Updated);
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let configs = parsed["configurations"].as_array().unwrap();
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0]["name"], "Rust");
        assert_eq!(configs[1]["debugServer"], 2222);
    }

    /// Neovim applies the rule to its primary `launch.json`; the secondary
    /// `.nvim-dap.lua` is still written — naming the **retained** entry's
    /// port, never the one `launch.json` was not updated to.
    #[test]
    fn test_if_absent_neovim_lua_names_the_retained_port() {
        let dir = unique_temp_dir("if-absent-present-neovim");
        let path = dir.join(".vscode/launch.json");
        write_file(&path, VSCODE_WITH_FRUST_1111);
        let lua_path = dir.canonicalize().unwrap().join(".nvim-dap.lua");
        // A stale snippet from some earlier run, naming neither port.
        std::fs::write(&lua_path, "port = 9999").unwrap();

        let result = generate(ParentIde::Neovim, 2222, &dir, WriteMode::IfAbsent);
        assert_eq!(
            result.action,
            ConfigAction::StalePort {
                existing: 1111,
                bound: 2222
            }
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            VSCODE_WITH_FRUST_1111
        );
        let lua = std::fs::read_to_string(&lua_path).unwrap();
        assert!(lua.contains("port = 1111,"), "{lua}");
        assert!(!lua.contains("2222"), "{lua}");
        assert!(!lua.contains("9999"), "{lua}");
    }

    /// Emacs' snippet is frust-owned: `IfAbsent` regenerates it with the
    /// bound port, and never keeps a marker-bearing foreign file.
    #[test]
    fn test_if_absent_emacs_regenerates_over_a_marker_bearing_foreign_file() {
        let dir = unique_temp_dir("if-absent-foreign-emacs");
        let path = dir.join(".frust/dap-emacs.el");
        let foreign = "(message \"not written by frust\")\n\
                       (list :type \"frust\" :name \"Frust (TUI DAP)\")\n";
        write_file(&path, foreign);

        let result = generate(ParentIde::Emacs, 2222, &dir, WriteMode::IfAbsent);
        assert_eq!(result.action, ConfigAction::Updated);
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(!written.contains("not written by frust"), "{written}");
        assert!(written.contains(":debugServer 2222"), "{written}");

        // Byte-identical content is the only write skipped.
        let again = generate(ParentIde::Emacs, 2222, &dir, WriteMode::IfAbsent);
        assert_eq!(
            again.action,
            ConfigAction::Skipped("content unchanged".to_string())
        );
        // A new port regenerates it again, automatic path or not.
        let moved = generate(ParentIde::Emacs, 3333, &dir, WriteMode::IfAbsent);
        assert_eq!(moved.action, ConfigAction::Updated);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains(":debugServer 3333")
        );
    }

    // ── contained writes ────────────────────────────────────────

    /// The write lands via a rename: no temp file is left behind.
    #[test]
    fn test_write_leaves_no_temp_file_behind() {
        let dir = unique_temp_dir("write-no-temp");
        generate(ParentIde::Zed, 4711, &dir, WriteMode::Refresh);
        generate(ParentIde::Zed, 4712, &dir, WriteMode::Refresh);
        let names: Vec<_> = std::fs::read_dir(dir.join(".zed"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["debug.json".to_string()]);
    }

    /// A `.vscode` directory symlinked outside the project is refused —
    /// nothing is written through it, and nothing is read either.
    #[cfg(unix)]
    #[test]
    fn test_write_refuses_a_vscode_symlink_pointing_outside_the_project() {
        let outside = unique_temp_dir("symlink-outside-target");
        let project = unique_temp_dir("symlink-outside-project");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::os::unix::fs::symlink(&outside, project.join(".vscode")).unwrap();

        for mode in [WriteMode::Refresh, WriteMode::IfAbsent] {
            let err = generate_ide_config(Some(ParentIde::VSCode), 4711, &project, mode)
                .expect_err("an escaping .vscode symlink must be refused");
            assert!(matches!(err, IdeConfigError::Refused(_)), "{err:?}");
            assert!(err.to_string().contains("outside the project"), "{err}");
        }
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);

        // Same for a pre-existing file symlink that escapes.
        let project = unique_temp_dir("symlink-outside-file-project");
        std::fs::create_dir_all(project.join(".zed")).unwrap();
        let foreign = outside.join("debug.json");
        std::fs::write(&foreign, "[]").unwrap();
        std::os::unix::fs::symlink(&foreign, project.join(".zed/debug.json")).unwrap();
        let err = generate_ide_config(Some(ParentIde::Zed), 4711, &project, WriteMode::Refresh)
            .expect_err("an escaping file symlink must be refused");
        assert!(matches!(err, IdeConfigError::Refused(_)), "{err:?}");
        assert_eq!(std::fs::read_to_string(&foreign).unwrap(), "[]");

        // And a dangling one, which would otherwise be created through.
        let project = unique_temp_dir("symlink-dangling-project");
        std::os::unix::fs::symlink(outside.join("missing"), project.join(".frust")).unwrap();
        let err = generate_ide_config(Some(ParentIde::Emacs), 4711, &project, WriteMode::Refresh)
            .expect_err("a dangling .frust symlink must be refused");
        assert!(matches!(err, IdeConfigError::Refused(_)), "{err:?}");
        assert!(!outside.join("missing").exists());
    }

    /// A symlink that resolves *inside* the project is followed: the write
    /// lands on the resolved file and the link itself survives.
    #[cfg(unix)]
    #[test]
    fn test_write_follows_an_in_project_symlink() {
        let project = unique_temp_dir("symlink-inside-project");
        std::fs::create_dir_all(project.join("shared/zed")).unwrap();
        std::os::unix::fs::symlink(project.join("shared/zed"), project.join(".zed")).unwrap();

        let result = generate(ParentIde::Zed, 4711, &project, WriteMode::Refresh);
        assert_eq!(result.action, ConfigAction::Created);
        assert!(
            std::fs::symlink_metadata(project.join(".zed"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(project.join("shared/zed/debug.json").is_file());

        // A symlinked file inside the project: rewritten in place, link kept.
        std::fs::rename(
            project.join("shared/zed/debug.json"),
            project.join("shared/real.json"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            project.join("shared/real.json"),
            project.join("shared/zed/debug.json"),
        )
        .unwrap();
        let result = generate(ParentIde::Zed, 4712, &project, WriteMode::Refresh);
        assert_eq!(result.action, ConfigAction::Updated);
        assert!(
            std::fs::symlink_metadata(project.join("shared/zed/debug.json"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            std::fs::read_to_string(project.join("shared/real.json"))
                .unwrap()
                .contains("4712")
        );
    }

    /// Neovim's secondary `.nvim-dap.lua`, symlinked outside the workspace,
    /// is a reported refusal, not a best-effort warning.
    #[cfg(unix)]
    #[test]
    fn test_nvim_lua_symlink_outside_is_refused() {
        let outside = unique_temp_dir("symlink-lua-outside");
        let project = unique_temp_dir("symlink-lua-project");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        let foreign = outside.join("init.lua");
        std::fs::write(&foreign, "-- mine").unwrap();
        std::os::unix::fs::symlink(&foreign, project.join(".nvim-dap.lua")).unwrap();

        let err = generate_ide_config(Some(ParentIde::Neovim), 4711, &project, WriteMode::Refresh)
            .expect_err("an escaping .nvim-dap.lua symlink must be refused");
        assert!(matches!(err, IdeConfigError::Refused(_)), "{err:?}");
        assert_eq!(std::fs::read_to_string(&foreign).unwrap(), "-- mine");
    }

    #[test]
    fn test_if_absent_on_a_malformed_file_is_an_error() {
        let dir = unique_temp_dir("if-absent-malformed-zed");
        write_file(&dir.join(".zed/debug.json"), "not json");
        assert!(
            generate_ide_config(Some(ParentIde::Zed), 4711, &dir, WriteMode::IfAbsent).is_err()
        );
    }
}
