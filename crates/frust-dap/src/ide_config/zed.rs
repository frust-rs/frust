//! Zed DAP client configuration generator.
//!
//! Generates or merges a `.zed/debug.json` entry that connects to `frust
//! dap`'s server via TCP, so Zed's debug panel recognises the entry and
//! connects to the running DAP server rather than spawning one.
//!
//! # Format
//!
//! `.zed/debug.json` is a flat JSON **array** (no wrapping object), e.g.:
//!
//! ```json
//! [
//!   {
//!     "label": "Frust (TUI DAP)",
//!     "adapter": "CodeLLDB",
//!     "request": "launch",
//!     "tcp_connection": { "host": "127.0.0.1", "port": 4711 }
//!   }
//! ]
//! ```

use std::path::{Path, PathBuf};

use super::merge::{merge_json_array_entry, to_pretty_json};
use super::{IdeConfigError, IdeConfigGenerator, Result};

/// The label used to identify the frust DAP entry in Zed's `debug.json`.
const ZED_FRUST_LABEL: &str = "Frust (TUI DAP)";

/// The TCP host for the DAP server connection.
const ZED_DAP_HOST: &str = "127.0.0.1";

/// The Zed debug-adapter name this entry claims.
///
/// **Best-effort, unverified against a real Zed release.** fdemon-pro's own
/// Zed generator hit the same gap this one does — Zed ships no Dart/Flutter
/// (or, here, frust) native adapter — and worked around it by naming an
/// adapter Zed's debug panel already recognises (`"Delve"`, Go's adapter),
/// reasoning from fdemon's own doc comment that a TCP-forwarded connection
/// isn't validated against the project's language. `"CodeLLDB"` is the
/// closer generic match for a Rust project (Zed's native Rust/LLDB
/// adapter), but frust-dap implements no breakpoint/stack/variable protocol
/// (`docs/CLI_ARCHITECTURE.md`'s ruling D6) — whether Zed's debug panel
/// accepts a CodeLLDB entry pointed at a non-lldb TCP peer, and whether a
/// future Zed release starts validating the adapter/language pairing (the
/// same risk fdemon's own comment flagged for Delve), is unconfirmed.
/// Verify against a real Zed instance before this shows up in user-facing
/// docs.
const ZED_ADAPTER: &str = "CodeLLDB";

/// Zed DAP configuration generator.
///
/// Produces or updates a `.zed/debug.json` entry that connects to `frust
/// dap`'s server via TCP.
pub struct ZedGenerator;

impl ZedGenerator {
    /// Build the frust DAP entry for Zed's `debug.json`. See [`ZED_ADAPTER`]'s
    /// doc comment for the adapter-name caveat.
    fn frust_entry(port: u16) -> serde_json::Value {
        serde_json::json!({
            "label": ZED_FRUST_LABEL,
            "adapter": ZED_ADAPTER,
            "request": "launch",
            "tcp_connection": {
                "host": ZED_DAP_HOST,
                "port": port
            }
        })
    }
}

impl IdeConfigGenerator for ZedGenerator {
    /// Returns the path to Zed's debug config file within the project root.
    fn config_path(&self, project_root: &Path) -> PathBuf {
        project_root.join(".zed").join("debug.json")
    }

    /// Generate a fresh `.zed/debug.json` containing a single frust entry.
    fn generate(&self, port: u16, _project_root: &Path) -> Result<String> {
        let config = serde_json::json!([Self::frust_entry(port)]);
        Ok(to_pretty_json(&config))
    }

    /// Merge the frust DAP entry into an existing `.zed/debug.json`.
    ///
    /// Finds an existing frust entry by `"label" == "Frust (TUI DAP)"` and
    /// updates its `tcp_connection.port`. If no matching entry is found,
    /// appends a new one. All non-frust entries are preserved unchanged.
    ///
    /// # Errors
    ///
    /// Returns an error if `existing` is not valid JSON or is not a JSON
    /// array.
    fn merge_config(&self, existing: &str, port: u16, _project_root: &Path) -> Result<String> {
        let mut array: Vec<serde_json::Value> = serde_json::from_str(existing)
            .map_err(|e| IdeConfigError::message(format!("invalid JSON in debug.json: {e}")))?;

        merge_json_array_entry(
            &mut array,
            "label",
            ZED_FRUST_LABEL,
            Self::frust_entry(port),
        );

        Ok(to_pretty_json(&serde_json::Value::Array(array)))
    }

    /// Display name used in log messages.
    fn ide_name(&self) -> &'static str {
        "Zed"
    }
}

// ─────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ── config_path ─────────────────────────────────────────────

    #[test]
    fn test_zed_config_path() {
        let generator = ZedGenerator;
        assert_eq!(
            generator.config_path(Path::new("/project")),
            PathBuf::from("/project/.zed/debug.json")
        );
    }

    // ── generate (fresh) ────────────────────────────────────────

    #[test]
    fn test_zed_fresh_generation() {
        let generator = ZedGenerator;
        let content = generator.generate(4711, Path::new("/project")).unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["label"], "Frust (TUI DAP)");
        assert_eq!(parsed[0]["tcp_connection"]["port"], 4711);
        assert_eq!(parsed[0]["tcp_connection"]["host"], "127.0.0.1");
        assert_eq!(parsed[0]["adapter"], "CodeLLDB");
        assert_eq!(parsed[0]["request"], "launch");
    }

    #[test]
    fn test_zed_fresh_generation_embeds_correct_port() {
        let generator = ZedGenerator;
        let content = generator.generate(9999, Path::new("/project")).unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed[0]["tcp_connection"]["port"], 9999);
    }

    #[test]
    fn test_zed_fresh_generation_is_valid_json_array() {
        let generator = ZedGenerator;
        let content = generator.generate(4711, Path::new("/project")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(parsed.is_array());
    }

    // ── merge_config ─────────────────────────────────────────────

    #[test]
    fn test_zed_merge_updates_existing_entry() {
        let existing = r#"[
            {"label": "Other", "adapter": "other"},
            {"label": "Frust (TUI DAP)", "adapter": "CodeLLDB", "tcp_connection": {"host": "127.0.0.1", "port": 1234}}
        ]"#;
        let generator = ZedGenerator;
        let merged = generator
            .merge_config(existing, 5678, Path::new(""))
            .unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&merged).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["label"], "Other"); // preserved
        assert_eq!(parsed[1]["tcp_connection"]["port"], 5678); // updated
    }

    #[test]
    fn test_zed_merge_appends_when_no_frust_entry() {
        let existing = r#"[{"label": "Other", "adapter": "other"}]"#;
        let generator = ZedGenerator;
        let merged = generator
            .merge_config(existing, 4711, Path::new(""))
            .unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&merged).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1]["label"], "Frust (TUI DAP)");
    }

    #[test]
    fn test_zed_merge_empty_array() {
        let generator = ZedGenerator;
        let merged = generator.merge_config("[]", 4711, Path::new("")).unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&merged).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["label"], "Frust (TUI DAP)");
    }

    #[test]
    fn test_zed_merge_malformed_json_returns_error() {
        let generator = ZedGenerator;
        let result = generator.merge_config("not json", 4711, Path::new(""));
        assert!(result.is_err());
    }

    #[test]
    fn test_zed_merge_non_array_json_returns_error() {
        let generator = ZedGenerator;
        let result = generator.merge_config(r#"{"key": "value"}"#, 4711, Path::new(""));
        assert!(result.is_err());
    }

    #[test]
    fn test_zed_merge_preserves_non_frust_configs() {
        let existing = r#"[
            {"label": "Config A", "adapter": "a"},
            {"label": "Config B", "adapter": "b"}
        ]"#;
        let generator = ZedGenerator;
        let merged = generator
            .merge_config(existing, 4711, Path::new(""))
            .unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&merged).unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0]["label"], "Config A");
        assert_eq!(parsed[1]["label"], "Config B");
        assert_eq!(parsed[2]["label"], "Frust (TUI DAP)");
    }

    #[test]
    fn test_zed_merge_updates_port_only_keeps_full_entry() {
        let existing = r#"[
            {"label": "Frust (TUI DAP)", "adapter": "CodeLLDB", "request": "launch",
             "tcp_connection": {"host": "127.0.0.1", "port": 1111}}
        ]"#;
        let generator = ZedGenerator;
        let merged = generator
            .merge_config(existing, 2222, Path::new(""))
            .unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&merged).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["tcp_connection"]["port"], 2222);
        assert_eq!(parsed[0]["adapter"], "CodeLLDB");
    }

    // ── ide_name ─────────────────────────────────────────────────

    #[test]
    fn test_zed_ide_name() {
        assert_eq!(ZedGenerator.ide_name(), "Zed");
    }
}
