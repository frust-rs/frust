//! Helix "DAP configuration" — deliberately a no-op.
//!
//! Helix always **spawns** the debug adapter binary and passes it a port via
//! `port-arg`; it has no pure-TCP/attach transport for `[language.debugger]`
//! (fdemon-pro's own `helix.rs`, the port source for this module, documents
//! only the spawn model — no attach form is mentioned there or anywhere in
//! Helix's own docs it cites). fdemon-pro's generator worked around this by
//! having Helix spawn a *second* `fdemon --dap-port <PORT>` instance — a
//! wholly separate process from any already-running session.
//!
//! `frust dap` has nothing equivalent to spawn: there is no standalone
//! `frust dap --port <PORT>`-as-adapter-binary story here (the DAP server is
//! `frust dap` itself, already running, not a per-debug-session child
//! process), so the fdemon workaround does not carry over. Rather than
//! generate a config that can never work, [`super::generate_ide_config`]
//! reports [`super::ConfigAction::Skipped`] for Helix without writing
//! anything — see [`SKIP_REASON`].

use std::path::{Path, PathBuf};

use super::{ConfigAction, IdeConfigResult};

/// Why Helix never gets a generated config — see the module doc above.
pub const SKIP_REASON: &str = "helix requires a spawnable adapter";

/// Stand-in for Helix's DAP config generation.
///
/// Unlike every other IDE in this module, `HelixGenerator` does not
/// implement [`super::IdeConfigGenerator`] — there is nothing for it to
/// generate or merge. Its only job is reporting *why* nothing was written,
/// via [`skip_result`](Self::skip_result).
pub struct HelixGenerator;

impl HelixGenerator {
    /// Where Helix's config *would* live, had frust-dap a way to populate
    /// it — reported in the [`IdeConfigResult`] purely for message context;
    /// nothing is ever written there.
    pub fn config_path(&self, project_root: &Path) -> PathBuf {
        project_root.join(".helix").join("languages.toml")
    }

    /// The `Skipped` result [`super::generate_ide_config`] returns for
    /// Helix. Performs no filesystem I/O.
    pub fn skip_result(&self, project_root: &Path) -> IdeConfigResult {
        IdeConfigResult {
            path: self.config_path(project_root),
            action: ConfigAction::Skipped(SKIP_REASON.to_string()),
        }
    }
}

// ─────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_helix_config_path() {
        let generator = HelixGenerator;
        assert_eq!(
            generator.config_path(Path::new("/project")),
            PathBuf::from("/project/.helix/languages.toml")
        );
    }

    #[test]
    fn test_helix_skip_result_reports_skipped_with_reason() {
        let generator = HelixGenerator;
        let result = generator.skip_result(Path::new("/project"));
        assert_eq!(result.path, PathBuf::from("/project/.helix/languages.toml"));
        assert_eq!(
            result.action,
            ConfigAction::Skipped(SKIP_REASON.to_string())
        );
    }

    #[test]
    fn test_helix_skip_result_writes_nothing() {
        use crate::ide_config::test_support::unique_temp_dir;

        let dir = unique_temp_dir("helix-skip-no-write");
        let generator = HelixGenerator;
        let _ = generator.skip_result(&dir);
        assert!(!dir.join(".helix").exists());
    }
}
