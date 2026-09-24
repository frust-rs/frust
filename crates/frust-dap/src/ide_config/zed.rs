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
//!     "adapter": "Delve",
//!     "request": "launch",
//!     "tcp_connection": { "host": "127.0.0.1", "port": 4711 }
//!   }
//! ]
//! ```

use std::path::{Path, PathBuf};

use super::merge::{find_json_entry_by_field, merge_json_array_entry, to_pretty_json};
use super::{IdeConfigError, IdeConfigGenerator, Result};

/// The label used to identify the frust DAP entry in Zed's `debug.json`.
const ZED_FRUST_LABEL: &str = "Frust (TUI DAP)";

/// The TCP host for the DAP server connection.
const ZED_DAP_HOST: &str = "127.0.0.1";

/// The Zed debug-adapter name this entry claims.
///
/// Mirrors flutter-demon's (fdemon's) own Zed generator, which names
/// `"Delve"` — Go's debug adapter — because Zed ships no native adapter for
/// its language either, and `"Delve"` is an adapter name Zed's debug panel
/// already recognises; the entry's `tcp_connection` forwards the session to
/// the already-running server, which speaks the actual protocol. Only the
/// adapter name is mirrored: fdemon writes `"request": "attach"`, frust keeps
/// `"request": "launch"` (frust-dap has no attach story — see the
/// [`super`] module doc).
///
/// **Unverified against a real Zed release.** fdemon's own comment flags the
/// risk that a future Zed release validates the adapter against the
/// project's language, which would break this workaround; confirm against a
/// real Zed instance before relying on it in user-facing docs.
const ZED_ADAPTER: &str = "Delve";

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

/// Parse `.zed/debug.json`'s flat array.
fn parse_debug_json(existing: &str) -> Result<Vec<serde_json::Value>> {
    serde_json::from_str(existing)
        .map_err(|e| IdeConfigError::message(format!("invalid JSON in debug.json: {e}")))
}

/// `existing`'s `"Frust (TUI DAP)"` entry, if any. An empty or
/// whitespace-only file has none (and is not an error).
fn frust_debug_entry(existing: &str) -> Result<Option<serde_json::Value>> {
    if existing.trim().is_empty() {
        return Ok(None);
    }
    let mut array = parse_debug_json(existing)?;
    Ok(
        find_json_entry_by_field(&array, "label", ZED_FRUST_LABEL)
            .map(|idx| array.swap_remove(idx)),
    )
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
    fn merge_config(&self, existing: &str, port: u16, project_root: &Path) -> Result<String> {
        // An empty/whitespace-only file is a fresh generation (the same
        // guard VS Code's merge has): an editor may create the file empty.
        if existing.trim().is_empty() {
            return self.generate(port, project_root);
        }
        let mut array = parse_debug_json(existing)?;

        merge_json_array_entry(
            &mut array,
            "label",
            ZED_FRUST_LABEL,
            Self::frust_entry(port),
        );

        Ok(to_pretty_json(&serde_json::Value::Array(array)))
    }

    /// Whether `existing` already has an entry labelled `"Frust (TUI DAP)"` —
    /// the marker [`merge_config`](Self::merge_config) matches on. An empty
    /// or whitespace-only file has none.
    ///
    /// # Errors
    ///
    /// Returns an error if `existing` is not valid JSON or is not a JSON
    /// array.
    fn has_frust_entry(&self, existing: &str) -> Result<bool> {
        Ok(frust_debug_entry(existing)?.is_some())
    }

    /// The `tcp_connection.port` of `existing`'s `"Frust (TUI DAP)"` entry —
    /// `None` when there is no such entry or it names no `u16` port.
    ///
    /// # Errors
    ///
    /// As [`has_frust_entry`](Self::has_frust_entry).
    fn frust_entry_port(&self, existing: &str) -> Result<Option<u16>> {
        Ok(frust_debug_entry(existing)?
            .and_then(|entry| {
                entry
                    .get("tcp_connection")
                    .and_then(|conn| conn.get("port"))
                    .and_then(serde_json::Value::as_u64)
            })
            .and_then(|port| u16::try_from(port).ok()))
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
        assert_eq!(parsed[0]["adapter"], "Delve");
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
            {"label": "Frust (TUI DAP)", "adapter": "Delve", "tcp_connection": {"host": "127.0.0.1", "port": 1234}}
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
            {"label": "Frust (TUI DAP)", "adapter": "Delve", "request": "launch",
             "tcp_connection": {"host": "127.0.0.1", "port": 1111}}
        ]"#;
        let generator = ZedGenerator;
        let merged = generator
            .merge_config(existing, 2222, Path::new(""))
            .unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&merged).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["tcp_connection"]["port"], 2222);
        assert_eq!(parsed[0]["adapter"], "Delve");
    }

    // ── empty file / entry port ─────────────────────────────────

    #[test]
    fn test_zed_empty_or_whitespace_file_has_no_entry_and_merges_fresh() {
        let generator = ZedGenerator;
        for empty in ["", "  \n\t "] {
            assert!(!generator.has_frust_entry(empty).unwrap());
            assert_eq!(generator.frust_entry_port(empty).unwrap(), None);
            let merged = generator.merge_config(empty, 4711, Path::new("")).unwrap();
            let parsed: Vec<serde_json::Value> = serde_json::from_str(&merged).unwrap();
            assert_eq!(parsed.len(), 1);
            assert_eq!(parsed[0]["tcp_connection"]["port"], 4711);
        }
    }

    #[test]
    fn test_zed_frust_entry_port_reads_the_marked_entry() {
        let generator = ZedGenerator;
        let existing = r#"[
            {"label": "Other", "tcp_connection": {"port": 9}},
            {"label": "Frust (TUI DAP)", "tcp_connection": {"host": "127.0.0.1", "port": 1234}}
        ]"#;
        assert_eq!(generator.frust_entry_port(existing).unwrap(), Some(1234));
        assert_eq!(
            generator
                .frust_entry_port(r#"[{"label": "Other"}]"#)
                .unwrap(),
            None
        );
        // A hand-edited entry without a readable port is present, portless.
        let portless = r#"[{"label": "Frust (TUI DAP)", "tcp_connection": {"port": "x"}}]"#;
        assert!(generator.has_frust_entry(portless).unwrap());
        assert_eq!(generator.frust_entry_port(portless).unwrap(), None);
        assert!(generator.frust_entry_port("not json").is_err());
    }

    // ── ide_name ─────────────────────────────────────────────────

    #[test]
    fn test_zed_ide_name() {
        assert_eq!(ZedGenerator.ide_name(), "Zed");
    }
}
