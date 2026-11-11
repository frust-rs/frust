//! `frust plugin add`: applies a registry plugin to the project in the
//! current directory, the shell route to the TUI's Add Plugin dialog.
//!
//! Delegates to [`frust_drive::plugin::add_plugin`], so it inherits that
//! function's behaviour unchanged, including resolving packages with
//! `LockfilePolicy::AllowUpdate` (an existing `Cargo.lock` may gain the new
//! dependency).

use std::io::Write;
use std::path::Path;

use anyhow::Result;

use frust_drive::plugin::{AddOutcome, AddReport, PluginAddError, add_plugin, known_plugins};

/// Adds plugin `id` with `features` to the project rooted at `project_root`
/// (no walk-up), printing the report on stdout or the error on stderr.
/// Returns the process exit code: 0 on success (including a no-op re-run),
/// 1 on a [`PluginAddError`].
pub fn add(project_root: &Path, id: &str, features: &[String]) -> Result<u8> {
    run_with(
        add_plugin,
        project_root,
        id,
        features,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    )
}

/// The testable core of [`add`], with the apply function and output sinks
/// injected.
pub(crate) fn run_with(
    apply: impl FnOnce(&Path, &str, &[&str]) -> Result<AddReport, PluginAddError>,
    project_root: &Path,
    id: &str,
    features: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<u8> {
    let feature_refs: Vec<&str> = features.iter().map(String::as_str).collect();
    match apply(project_root, id, &feature_refs) {
        Ok(report) => {
            let (applied, already, at_build) = report.outcome_counts();
            writeln!(
                out,
                "Plugin `{}`: {applied} applied, {already} already present, {at_build} applied at build",
                report.plugin_id
            )?;
            for item in &report.items {
                let label = match item.outcome {
                    AddOutcome::Applied => "applied",
                    AddOutcome::AlreadyPresent => "already present",
                    AddOutcome::AppliedAtBuild => "applied at build",
                };
                writeln!(out, "  [{label}] {}", item.description)?;
            }
            Ok(0)
        }
        Err(error) => {
            writeln!(err, "Error: {error}")?;
            match &error {
                PluginAddError::UnknownPlugin(_) => {
                    let ids: Vec<&str> = known_plugins().iter().map(|p| p.id).collect();
                    writeln!(err, "Valid plugins: {}", ids.join(", "))?;
                }
                PluginAddError::UnknownFeature { plugin, .. } => {
                    let valid: Vec<&str> = known_plugins()
                        .iter()
                        .find(|p| p.id == plugin)
                        .map(|p| p.optional_features.iter().map(|f| f.id).collect())
                        .unwrap_or_default();
                    if valid.is_empty() {
                        writeln!(err, "Plugin `{plugin}` has no optional features.")?;
                    } else {
                        writeln!(err, "Valid features for `{plugin}`: {}", valid.join(", "))?;
                    }
                }
                _ => {}
            }
            Ok(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frust_drive::packages::StubLocator;
    use frust_drive::plugin::add_plugin_with;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn project(tag: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-plugin-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nfrust = { package = \"frust-ui\", version = \"0.6\" }\n",
        )
        .unwrap();
        dir
    }

    fn run(dir: &Path, id: &str, features: &[&str]) -> (u8, String, String) {
        let features: Vec<String> = features.iter().map(|f| f.to_string()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run_with(
            |root, id, feats| add_plugin_with(&StubLocator::new(), root, id, feats),
            dir,
            id,
            &features,
            &mut out,
            &mut err,
        )
        .unwrap();
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn success_then_idempotent_rerun() {
        let dir = project("ok");
        let (code, out, _) = run(&dir, "device-info", &[]);
        assert_eq!(code, 0);
        assert!(out.contains("[applied]"), "{out}");
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(manifest.contains("frust-device-info"));

        let (code, out, _) = run(&dir, "device-info", &[]);
        assert_eq!(code, 0);
        assert!(out.contains("0 applied"), "{out}");
        assert!(!out.contains("[applied]"), "{out}");
        assert_eq!(
            std::fs::read_to_string(dir.join("Cargo.toml")).unwrap(),
            manifest
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_id_lists_valid_plugins() {
        let dir = project("badid");
        let (code, _, err) = run(&dir, "nope", &[]);
        assert_eq!(code, 1);
        assert!(err.contains("unknown plugin `nope`"), "{err}");
        assert!(err.contains("device-info"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_feature_lists_valid_features() {
        let dir = project("badfeat");
        let (code, _, err) = run(&dir, "network-image", &["nope"]);
        assert_eq!(code, 1);
        assert!(
            err.contains("Valid features for `network-image`: webp"),
            "{err}"
        );
        let (code, _, err) = run(&dir, "device-info", &["nope"]);
        assert_eq!(code, 1);
        assert!(err.contains("no optional features"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn outside_a_project_fails() {
        let dir =
            std::env::temp_dir().join(format!("frust-cli-plugin-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (code, _, err) = run(&dir, "device-info", &[]);
        assert_eq!(code, 1);
        assert!(err.contains("Cargo.toml"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
