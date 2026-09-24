//! Emacs DAP config generator for `dap-mode`.
//!
//! Generates a `.frust/dap-emacs.el` Elisp snippet that registers frust as a
//! `dap-mode` debug provider. Because the target file lives inside frust's
//! own `.frust/` directory, the file is always overwritten rather than
//! merged — there is no user-managed content to preserve.
//!
//! ## Always regenerated, even under `IfAbsent`
//!
//! [`WriteMode::IfAbsent`](super::WriteMode::IfAbsent)'s "never rewrite an
//! existing frust entry" rule is for **user-editable** configs (VS Code's
//! `launch.json`, Zed's `debug.json`, the Neovim pair). This file is
//! frust-**owned** and users `load-file` it — it is code Emacs runs — so a
//! repo-supplied `.frust/dap-emacs.el` that merely carries the frust marker
//! next to arbitrary Elisp must never be kept as if frust had written it. Its
//! entry detection therefore always answers "absent", and every run
//! regenerates the file; the only write skipped is a byte-identical one
//! (`run_generator`'s unchanged check, which leaves the mtime alone).
//!
//! ## Usage
//!
//! After generation the user must manually load the file:
//!
//! ```text
//! M-x load-file RET /path/to/.frust/dap-emacs.el RET
//! ```
//!
//! Or add the following to their Emacs init:
//!
//! ```elisp
//! (load-file "/path/to/.frust/dap-emacs.el")
//! ```

use std::path::{Path, PathBuf};

use super::{IdeConfigGenerator, Result};

/// Generates an Elisp snippet for Emacs `dap-mode` integration.
///
/// The output file `.frust/dap-emacs.el` is always overwritten because it
/// lives entirely within frust's own `.frust/` directory — no user content
/// needs to be preserved.
pub struct EmacsGenerator;

impl IdeConfigGenerator for EmacsGenerator {
    /// Returns `.frust/dap-emacs.el` relative to `project_root`.
    fn config_path(&self, project_root: &Path) -> PathBuf {
        project_root.join(".frust").join("dap-emacs.el")
    }

    /// Generate the full Elisp file content, embedding the absolute path of
    /// the generated file in the loading instructions.
    fn generate(&self, port: u16, project_root: &Path) -> Result<String> {
        let path = self.config_path(project_root);
        Ok(generate_elisp(port, to_lisp_path(&path)))
    }

    /// Regenerate the file from scratch (overwrite semantics) — the existing
    /// content is always discarded, since `.frust/dap-emacs.el` is
    /// frust-owned.
    fn merge_config(&self, _existing: &str, port: u16, project_root: &Path) -> Result<String> {
        let path = self.config_path(project_root);
        Ok(generate_elisp(port, to_lisp_path(&path)))
    }

    /// Always `false`: the file is frust-owned and regenerated on every run,
    /// whatever it contains — a marker-bearing file is not trusted as frust's
    /// own (see the module doc).
    fn has_frust_entry(&self, _existing: &str) -> Result<bool> {
        Ok(false)
    }

    /// Always `None`, for the same reason as
    /// [`has_frust_entry`](Self::has_frust_entry).
    fn frust_entry_port(&self, _existing: &str) -> Result<Option<u16>> {
        Ok(None)
    }

    /// Display name used in log messages.
    fn ide_name(&self) -> &'static str {
        "Emacs"
    }
}

// ─────────────────────────────────────────────────────────────────
// Elisp generation
// ─────────────────────────────────────────────────────────────────

/// Render `path` as a forward-slash string suitable for embedding in Elisp.
///
/// Emacs accepts `/` on Windows, and `\` would be misinterpreted as escape
/// sequences (e.g. `\f`, `\n`, `\U`) in Elisp string literals.
fn to_lisp_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Produce the full Elisp file content.
///
/// `file_path_display` is embedded verbatim in the loading instructions so
/// the user can copy-paste the correct path. `:request "launch"` matches
/// every other generator's entry — frust-dap is launch-based, not
/// attach-based (see `vscode`'s module doc).
fn generate_elisp(port: u16, file_path_display: String) -> String {
    format!(
        r#";; Frust DAP configuration for Emacs dap-mode (auto-generated)
;;
;; Load this file to register frust as a DAP provider:
;;
;;   M-x load-file RET {file_path} RET
;;
;; Or add to your Emacs config:
;;
;;   (load-file "{file_path}")

(require 'dap-mode)

(dap-register-debug-provider
  "frust"
  (lambda (conf)
    (plist-put conf :debugServer {port})
    (plist-put conf :host "localhost")
    conf))

(dap-register-debug-template
  "Frust :: TUI DAP"
  (list :type "frust"
        :request "launch"
        :name "Frust (TUI DAP)"))
"#,
        file_path = file_path_display,
        port = port,
    )
}

// ─────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ide_config::test_support::unique_temp_dir;

    #[test]
    fn test_emacs_config_path() {
        let generator = EmacsGenerator;
        assert_eq!(
            generator.config_path(Path::new("/project")),
            PathBuf::from("/project/.frust/dap-emacs.el")
        );
    }

    #[test]
    fn test_emacs_fresh_generation() {
        let generator = EmacsGenerator;
        let content = generator.generate(4711, Path::new("/project")).unwrap();
        assert!(content.contains("dap-register-debug-provider"));
        assert!(content.contains(":debugServer 4711"));
        assert!(content.contains("dap-register-debug-template"));
        assert!(content.contains(":request \"launch\""));
        assert!(content.contains(":type \"frust\""));
        assert!(content.contains(":name \"Frust (TUI DAP)\""));
        assert!(content.contains("require 'dap-mode"));
    }

    #[test]
    fn test_emacs_port_substitution() {
        let generator = EmacsGenerator;
        let content = generator.generate(9999, Path::new("/project")).unwrap();
        assert!(content.contains(":debugServer 9999"));
        assert!(!content.contains(":debugServer 4711"));
    }

    #[test]
    fn test_emacs_merge_overwrites() {
        let generator = EmacsGenerator;
        let old_content = ";; old content";
        let new_content = generator
            .merge_config(old_content, 5678, Path::new("/project"))
            .unwrap();
        assert!(new_content.contains(":debugServer 5678"));
        assert!(!new_content.contains("old content"));
    }

    #[test]
    fn test_emacs_includes_loading_instructions() {
        let generator = EmacsGenerator;
        let content = generator.generate(4711, Path::new("/project")).unwrap();
        assert!(content.contains("load-file"));
        assert!(content.contains("M-x"));
    }

    #[test]
    fn test_emacs_entry_detection_always_answers_absent() {
        let generator = EmacsGenerator;
        let ours = generator.generate(4711, Path::new("/project")).unwrap();
        assert!(!generator.has_frust_entry(&ours).unwrap());
        assert_eq!(generator.frust_entry_port(&ours).unwrap(), None);
    }

    #[test]
    fn test_emacs_ide_name() {
        assert_eq!(EmacsGenerator.ide_name(), "Emacs");
    }

    #[test]
    fn test_emacs_elisp_parens_balanced() {
        let generator = EmacsGenerator;
        let content = generator.generate(4711, Path::new("/project")).unwrap();
        let open = content.chars().filter(|c| *c == '(').count();
        let close = content.chars().filter(|c| *c == ')').count();
        assert_eq!(open, close, "Unbalanced parentheses in generated Elisp");
    }

    #[test]
    fn test_emacs_generate_embeds_absolute_path() {
        let generator = EmacsGenerator;
        let content = generator
            .generate(4711, Path::new("/my/frust/app"))
            .unwrap();
        assert!(content.contains("/my/frust/app/.frust/dap-emacs.el"));
    }

    #[test]
    fn test_emacs_merge_uses_absolute_path() {
        let generator = EmacsGenerator;
        let content = generator
            .merge_config("", 4711, Path::new("/my/frust/app"))
            .unwrap();
        assert!(content.contains("/my/frust/app/.frust/dap-emacs.el"));
    }

    #[test]
    fn test_emacs_merge_produces_absolute_path() {
        let dir = unique_temp_dir("emacs-merge-absolute-path");
        let generator = EmacsGenerator;
        let existing = "(some old elisp)";
        let result = generator.merge_config(existing, 12345, &dir).unwrap();
        let expected_path = dir.join(".frust/dap-emacs.el");
        let expected = expected_path.to_string_lossy().replace('\\', "/");
        assert!(
            result.contains(&expected),
            "expected absolute path '{}' in merged output",
            expected
        );
        assert!(
            !result.contains("\".frust/dap-emacs.el\""),
            "merged output must not contain a relative placeholder"
        );
    }

    #[test]
    fn test_emacs_config_exists_false_for_temp_path() {
        let generator = EmacsGenerator;
        assert!(!generator.config_exists(Path::new("/nonexistent/path/for/testing")));
    }

    #[test]
    fn test_emacs_uses_debug_server_not_debug_port() {
        let generator = EmacsGenerator;
        let content = generator.generate(4711, Path::new("/project")).unwrap();
        assert!(content.contains(":debugServer 4711"));
        assert!(!content.contains(":debugPort"));
    }
}
