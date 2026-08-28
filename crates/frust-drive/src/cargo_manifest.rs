//! App-manifest inspection at the drive boundary: does a generated app's
//! `Cargo.toml` positively declare a given `[features]` entry?
//!
//! Used by the release-lean preflight ([`resolve_release_features`]) so a
//! `--release` build/run against a *legacy* app — one scaffolded before the
//! release-lean `lean` feature existed — drops that feature with a one-time
//! migration warning instead of hard-failing with cargo's opaque "none of the
//! selected packages contains these features: lean" error. Mirrors
//! `plugin::apply`'s `toml_edit` seam: read + parse once, and **fail open**
//! (assume the feature is declared) on any missing/unreadable/unparseable
//! manifest, so a real app is never wrongly stripped of its release log
//! ceiling because its manifest couldn't be read this once.

use std::path::Path;

use toml_edit::DocumentMut;

use crate::build_info::BuildMode;

/// The one-line migration a legacy app needs to opt into release-lean log
/// parity — named in [`resolve_release_features`]'s warning so the fix is
/// copy-pasteable rather than a bare "feature missing" complaint.
const LEAN_MIGRATION_HINT: &str = "\
note: this app's Cargo.toml declares no `lean` feature, so `frust --release` builds without it \
(release log level not capped to warn, unlike Flutter's release parity). To opt in, add under [features]:\n    \
lean = [\"log/release_max_level_warn\"]\n\
and a `log` dependency under [dependencies].";

/// Whether the app manifest at `<project_root>/Cargo.toml` positively
/// declares `[features].<feature>`.
///
/// **Fails open.** A missing, unreadable, or unparseable manifest returns
/// `true` (assume declared) — mirroring `plugin::apply`'s "never rewrite a
/// manifest we can't parse" stance: this inspection is a courtesy for legacy
/// apps and must never strip a feature from a real app just because its
/// manifest couldn't be read. Only a manifest that *positively parses and
/// lacks* the feature returns `false`.
pub fn declares_feature(project_root: &Path, feature: &str) -> bool {
    let path = project_root.join("Cargo.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return true; // fail open: missing/unreadable
    };
    let Ok(doc) = text.parse::<DocumentMut>() else {
        return true; // fail open: unparseable
    };
    doc.get("features")
        .and_then(|features| features.as_table_like())
        .is_some_and(|table| table.contains_key(feature))
}

/// Resolves the cargo `--features` a `mode` build selects for the app rooted
/// at `project_root`, plus an optional one-time warning to surface through the
/// caller's own output channel (a CLI `println!` or a drive-core `on_line`
/// sink).
///
/// The debug/profile arm's `frust/perf-trace` and `frust/devtools` are both
/// features of the *frust* crate (always present in a generated app's
/// dependency graph), so neither is ever filtered — those modes pass
/// [`BuildMode::cargo_features`] through verbatim with no warning. The release arm's `lean` is the *app's own*
/// feature; an app scaffolded before release-lean existed does not declare it,
/// and `--features lean` against such an app fails the whole build with
/// cargo's opaque message. So for release we drop `lean` when the manifest
/// positively parses and lacks it, returning the migration hint as the
/// warning; a declaring app — or one whose manifest can't be read, see
/// [`declares_feature`] — keeps byte-identical features and gets no warning.
///
/// `extra` is the front-end's `--features` passthrough, appended **after** the
/// mode's own selection and never filtered: the legacy-`lean` drop above is a
/// migration courtesy for a feature this funnel chose by itself, whereas an
/// `extra` name was asked for explicitly, so an app that doesn't declare it
/// deserves cargo's own error rather than a silent removal. Appending rather
/// than prepending is what keeps a mode's instrumentation first in the argv
/// (and first in the Android/iOS CSVs), so a capture reads in selection order.
/// An empty `extra` leaves the result byte-identical to the mode's own list.
pub fn resolve_release_features(
    project_root: &Path,
    mode: BuildMode,
    extra: &[String],
) -> (Vec<String>, Option<String>) {
    let features = mode.cargo_features();
    let (kept, warning): (Vec<&'static str>, Option<String>) =
        if mode != BuildMode::Release || declares_feature(project_root, "lean") {
            (features.to_vec(), None)
        } else {
            (
                features.iter().copied().filter(|f| *f != "lean").collect(),
                Some(LEAN_MIGRATION_HINT.to_string()),
            )
        };
    let resolved = kept
        .into_iter()
        .map(str::to_string)
        .chain(extra.iter().cloned())
        .collect();
    (resolved, warning)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-drive-cargo-manifest-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_manifest(dir: &Path, body: &str) {
        fs::write(dir.join("Cargo.toml"), body).unwrap();
    }

    /// No `--features` passthrough — the shape every pre-passthrough call had.
    const NO_EXTRA: &[String] = &[];

    fn extra(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    #[test]
    fn declares_feature_true_when_present() {
        let dir = unique_dir("declares-present");
        write_manifest(
            &dir,
            "[package]\nname = \"app\"\n\n[features]\nlean = [\"log/release_max_level_warn\"]\n",
        );
        assert!(declares_feature(&dir, "lean"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn declares_feature_false_when_features_table_lacks_it() {
        let dir = unique_dir("declares-absent");
        write_manifest(
            &dir,
            "[package]\nname = \"app\"\n\n[features]\nother = []\n",
        );
        assert!(!declares_feature(&dir, "lean"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn declares_feature_false_when_no_features_table() {
        let dir = unique_dir("declares-no-table");
        write_manifest(&dir, "[package]\nname = \"app\"\n");
        assert!(!declares_feature(&dir, "lean"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn declares_feature_fails_open_on_missing_manifest() {
        let dir = unique_dir("declares-missing");
        // No Cargo.toml written.
        assert!(declares_feature(&dir, "lean"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn declares_feature_fails_open_on_unparseable_manifest() {
        let dir = unique_dir("declares-garbage");
        write_manifest(&dir, "this is { not ]= valid toml");
        assert!(declares_feature(&dir, "lean"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_release_drops_lean_and_warns_for_legacy_app() {
        let dir = unique_dir("resolve-legacy");
        write_manifest(&dir, "[package]\nname = \"app\"\n");
        let (features, warning) = resolve_release_features(&dir, BuildMode::Release, NO_EXTRA);
        assert!(features.is_empty(), "{features:?}");
        let warning = warning.expect("legacy release app must warn");
        assert!(warning.contains("lean"), "{warning}");
        assert!(warning.contains("release_max_level_warn"), "{warning}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_release_keeps_lean_without_warning_for_declaring_app() {
        let dir = unique_dir("resolve-declaring");
        write_manifest(
            &dir,
            "[package]\nname = \"app\"\n\n[features]\nlean = [\"log/release_max_level_warn\"]\n",
        );
        let (features, warning) = resolve_release_features(&dir, BuildMode::Release, NO_EXTRA);
        assert_eq!(features, vec!["lean"]);
        assert!(warning.is_none(), "{warning:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_release_fails_open_keeps_lean_when_manifest_missing() {
        let dir = unique_dir("resolve-missing");
        let (features, warning) = resolve_release_features(&dir, BuildMode::Release, NO_EXTRA);
        assert_eq!(features, vec!["lean"]);
        assert!(warning.is_none(), "{warning:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_debug_passes_frust_crate_features_through_regardless_of_manifest() {
        // A legacy manifest (no `lean`) must not affect the debug/profile
        // arm — both `frust/perf-trace` and `frust/devtools` are frust-crate
        // features, always present in a generated app's dependency graph
        // whether or not the app declares anything of its own.
        let dir = unique_dir("resolve-debug");
        write_manifest(&dir, "[package]\nname = \"app\"\n");
        for mode in [BuildMode::Debug, BuildMode::Profile] {
            let (features, warning) = resolve_release_features(&dir, mode, NO_EXTRA);
            assert_eq!(
                features,
                vec!["frust/perf-trace", "frust/devtools"],
                "mode {mode:?}"
            );
            assert!(warning.is_none(), "mode {mode:?}: {warning:?}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extra_features_are_appended_after_the_modes_own_selection() {
        let dir = unique_dir("extra-after-mode");
        write_manifest(&dir, "[package]\nname = \"app\"\n");
        let (features, warning) =
            resolve_release_features(&dir, BuildMode::Profile, &extra(&["hybrid-tier"]));
        // Order is the contract: the mode's own instrumentation first, the
        // passthrough last, so an argv/CSV reads in selection order.
        assert_eq!(
            features,
            vec!["frust/perf-trace", "frust/devtools", "hybrid-tier"]
        );
        assert!(warning.is_none(), "{warning:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extra_features_survive_the_legacy_lean_drop() {
        // The legacy-app `lean` drop is a courtesy for a feature this funnel
        // selected itself; an explicitly requested passthrough is never
        // filtered, even when the same manifest lacks `lean`.
        let dir = unique_dir("extra-survives-lean-drop");
        write_manifest(&dir, "[package]\nname = \"app\"\n");
        let (features, warning) =
            resolve_release_features(&dir, BuildMode::Release, &extra(&["hybrid-tier"]));
        assert_eq!(features, vec!["hybrid-tier"]);
        assert!(warning.is_some(), "legacy release app must still warn");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_extra_is_byte_identical_to_the_modes_own_list() {
        let dir = unique_dir("extra-empty");
        write_manifest(
            &dir,
            "[package]\nname = \"app\"\n\n[features]\nlean = [\"log/release_max_level_warn\"]\n",
        );
        for mode in [BuildMode::Debug, BuildMode::Profile, BuildMode::Release] {
            let (features, _) = resolve_release_features(&dir, mode, NO_EXTRA);
            assert_eq!(features, mode.cargo_features(), "mode {mode:?}");
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
