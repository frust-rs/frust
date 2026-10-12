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
use crate::facade_key::{FacadeKey, FacadeKeyError, KeySource};
use crate::process::{ProcessRunner, RealProcessRunner};

/// The one-line migration a legacy app needs to opt into release-lean log
/// parity — named in [`resolve_release_features`]'s warning so the fix is
/// copy-pasteable rather than a bare "feature missing" complaint.
const LEAN_MIGRATION_HINT: &str = "\
note: this app's Cargo.toml declares no `lean` feature, so `frust --release` builds without it \
(release log level not capped to warn, unlike Flutter's release parity). To opt in, add under [features]:\n    \
lean = [\"log/release_max_level_warn\"]\n\
and a `log` dependency under [dependencies].";

/// Reads and parses the app manifest at `<project_root>/Cargo.toml`, if it
/// exists and is well-formed. `None` on a missing, unreadable, or
/// unparseable manifest — every caller here picks its own fail-open/
/// fail-closed stance for that case rather than sharing one.
pub(crate) fn read_app_manifest(project_root: &Path) -> Option<DocumentMut> {
    let path = project_root.join("Cargo.toml");
    let text = std::fs::read_to_string(&path).ok()?;
    text.parse::<DocumentMut>().ok()
}

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
    let Some(doc) = read_app_manifest(project_root) else {
        return true; // fail open: missing/unreadable/unparseable
    };
    doc.get("features")
        .and_then(|features| features.as_table_like())
        .is_some_and(|table| table.contains_key(feature))
}

/// Whether the app's own `[features]` table defines `feature` as an alias
/// that directly compiles the facade's devtools listener in — e.g. `full =
/// ["frust/devtools"]`, or `full = ["frust-ui/devtools"]` in an app whose
/// facade dependency is keyed `frust-ui`. The facade's `hotpatch` counts as
/// devtools (it enables the shells' `hotpatch`, which each enable
/// `devtools`), and cargo's weak-dependency spelling `<dep>?/<feature>`
/// counts as `<dep>/<feature>` (the facade is always an active dependency).
///
/// So an entry of the enabled-list counts when, after stripping one `?`
/// before the slash, it is `<key>/devtools` or `<key>/hotpatch` with `<key>`
/// the canonical `frust` (always) or a key [`FacadeKey::resolve`] reports
/// through cargo (an ambiguous answer counts every key it names).
/// Deliberately cheap and non-transitive: only the feature's *own*
/// enabled-list is read, with no recursive feature-graph walk through other
/// app-defined aliases. The key is resolved only when the list names some
/// other `<dep>/devtools` or `<dep>/hotpatch`, so the common case spawns no
/// cargo.
///
/// Used by `frust build`'s release lanes
/// (`frust-cli::commands::build::refuse_devtools_features`) alongside the
/// literal `devtools`/`hotpatch`/`*/devtools`/`*/hotpatch` token checks, to
/// catch an app-defined feature name that itself turns the devtools listener
/// on.
///
/// **Fails closed** — the opposite of [`declares_feature`]'s courtesy
/// fail-open stance — since this function exists purely to widen a security
/// refusal. When the key came from the manifest fallback (cargo could not
/// answer, so a workspace-inherited rename is invisible) or the manifest
/// names the facade under a key no feature can spell, *any* `<dep>/devtools`
/// or `<dep>/hotpatch` spelling counts. A missing or unparseable manifest is
/// never read as "this token is safe"; it just means this extra
/// (best-effort, one-level) check has nothing to add, and the literal-spelling
/// checks alongside it remain the actual defense.
pub fn feature_enables_devtools(project_root: &Path, feature: &str) -> bool {
    feature_enables_devtools_with(&RealProcessRunner, project_root, feature)
}

/// The facade features that compile the devtools listener in: `devtools`
/// itself, and `hotpatch`, which enables it through the shells.
const DEVTOOLS_FEATURES: [&str; 2] = ["devtools", "hotpatch"];

/// `entry`'s dependency key when it spells `<dep>/devtools`,
/// `<dep>/hotpatch` or a weak `<dep>?/…` form of either; `None` otherwise.
fn devtools_dependency(entry: &str) -> Option<&str> {
    let (dep, feature) = entry.split_once('/')?;
    DEVTOOLS_FEATURES
        .contains(&feature)
        .then(|| dep.strip_suffix('?').unwrap_or(dep))
}

/// [`feature_enables_devtools`] resolving the facade key through `runner`.
fn feature_enables_devtools_with(
    runner: &dyn ProcessRunner,
    project_root: &Path,
    feature: &str,
) -> bool {
    const CANONICAL_KEY: &str = "frust";
    let Some(doc) = read_app_manifest(project_root) else {
        return false; // fail closed: missing/unreadable/unparseable
    };
    let Some(enabled) = doc
        .get("features")
        .and_then(|features| features.as_table_like())
        .and_then(|table| table.get(feature))
        .and_then(|item| item.as_array())
    else {
        return false;
    };
    let deps: Vec<&str> = enabled
        .iter()
        .filter_map(|value| value.as_str())
        .filter_map(devtools_dependency)
        .collect();
    if deps.is_empty() {
        return false;
    }
    if deps.contains(&CANONICAL_KEY) {
        return true;
    }
    let keys = match FacadeKey::resolve_with_source(runner, project_root) {
        (_, KeySource::ManifestFallback) | (Err(FacadeKeyError::InvalidKey { .. }), _) => {
            return true; // fail closed: the key may be wrong or unspellable
        }
        (Ok(key), _) => vec![key],
        (Err(FacadeKeyError::Ambiguous { keys }), _) => {
            keys.into_iter().map(FacadeKey::new).collect()
        }
    };
    keys.iter().any(|key| deps.contains(&key.as_str()))
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
    use crate::process::{FakeProcessRunner, Output};
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
    fn feature_enables_devtools_true_when_alias_names_frust_devtools() {
        let dir = unique_dir("enables-devtools-true");
        write_manifest(
            &dir,
            "[package]\nname = \"app\"\n\n[features]\nfull = [\"frust/devtools\"]\n",
        );
        assert!(feature_enables_devtools(&dir, "full"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn feature_enables_devtools_false_when_array_lacks_it() {
        let dir = unique_dir("enables-devtools-false");
        write_manifest(
            &dir,
            "[package]\nname = \"app\"\n\n[features]\nfull = [\"frust/perf-trace\"]\n",
        );
        assert!(!feature_enables_devtools(&dir, "full"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn feature_enables_devtools_false_when_feature_absent() {
        let dir = unique_dir("enables-devtools-absent");
        write_manifest(
            &dir,
            "[package]\nname = \"app\"\n\n[features]\nother = []\n",
        );
        assert!(!feature_enables_devtools(&dir, "full"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A runner whose `cargo metadata --no-deps` for `dir` names `deps` as
    /// the root package's dependencies — the cargo-resolved key path.
    fn metadata_runner(dir: &Path, deps: &str) -> FakeProcessRunner {
        let manifest = serde_json::to_string(&dir.join("Cargo.toml")).unwrap();
        FakeProcessRunner::new().with(
            format!(
                "cargo metadata --format-version 1 --no-deps --manifest-path {}",
                dir.join("Cargo.toml").display()
            ),
            Output {
                success: true,
                stdout: format!(
                    r#"{{"packages":[{{"name":"app","version":"0.1.0","id":"app",
                        "manifest_path":{manifest},"dependencies":[{deps}]}}],
                        "workspace_members":["app"],"resolve":null,"version":1}}"#
                ),
                stderr: String::new(),
            },
        )
    }

    const FRUST_UI_UNRENAMED: &str =
        r#"{"name":"frust-ui","rename":null,"kind":null,"target":null}"#;
    const FRUST_UI_AS_FRUST: &str =
        r#"{"name":"frust-ui","rename":"frust","kind":null,"target":null}"#;
    const FRUST_UI_AS_UI: &str = r#"{"name":"frust-ui","rename":"ui","kind":null,"target":null}"#;

    #[test]
    fn feature_enables_devtools_follows_the_facade_key() {
        let dir = unique_dir("enables-devtools-frust-ui");
        write_manifest(
            &dir,
            "[package]\nname = \"app\"\n\n[dependencies]\nfrust-ui = \"0.6.0\"\n\n\
             [features]\nfull = [\"frust-ui/devtools\"]\nlegacy = [\"frust/devtools\"]\n\
             other = [\"frust-material/devtools\"]\n",
        );
        let runner = metadata_runner(&dir, FRUST_UI_UNRENAMED);
        assert!(feature_enables_devtools_with(&runner, &dir, "full"));
        assert!(feature_enables_devtools_with(&runner, &dir, "legacy"));
        assert!(!feature_enables_devtools_with(&runner, &dir, "other"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// cargo's weak-dependency spelling enables the feature whenever the
    /// dependency is active — and the facade always is.
    #[test]
    fn feature_enables_devtools_counts_the_weak_dependency_spelling() {
        let dir = unique_dir("enables-devtools-weak");
        write_manifest(
            &dir,
            "[package]\nname = \"app\"\n\n[dependencies]\nfrust-ui = \"0.6.0\"\n\n\
             [features]\ncanonical = [\"frust?/devtools\"]\nkeyed = [\"frust-ui?/devtools\"]\n\
             other = [\"frust-material?/devtools\"]\n",
        );
        let runner = metadata_runner(&dir, FRUST_UI_UNRENAMED);
        assert!(feature_enables_devtools_with(&runner, &dir, "canonical"));
        assert!(feature_enables_devtools_with(&runner, &dir, "keyed"));
        assert!(!feature_enables_devtools_with(&runner, &dir, "other"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// The facade's `hotpatch` enables the shells' `hotpatch`, which each
    /// enable `devtools`, so it compiles the listener in too.
    #[test]
    fn feature_enables_devtools_counts_hotpatch() {
        let dir = unique_dir("enables-devtools-hotpatch");
        write_manifest(
            &dir,
            "[package]\nname = \"app\"\n\n[dependencies]\nui = { package = \"frust-ui\", version = \"0.6\" }\n\n\
             [features]\nfull = [\"frust/hotpatch\"]\nkeyed = [\"ui?/hotpatch\"]\n\
             other = [\"frust-material/hotpatch\"]\nperf = [\"ui/perf-trace\"]\n",
        );
        let runner = metadata_runner(&dir, FRUST_UI_AS_UI);
        assert!(feature_enables_devtools_with(&runner, &dir, "full"));
        assert!(feature_enables_devtools_with(&runner, &dir, "keyed"));
        assert!(!feature_enables_devtools_with(&runner, &dir, "other"));
        assert!(!feature_enables_devtools_with(&runner, &dir, "perf"));
        // The canonical spelling needs no resolution at all.
        assert!(feature_enables_devtools_with(
            &FakeProcessRunner::new(),
            &dir,
            "full"
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A workspace-inherited rename is invisible to the manifest fallback, so
    /// when cargo cannot answer, any `<dep>/devtools` counts (fail closed);
    /// when cargo can, only the key it names does.
    #[test]
    fn feature_enables_devtools_fails_closed_on_the_manifest_fallback() {
        let dir = unique_dir("enables-devtools-fallback");
        write_manifest(
            &dir,
            "[package]\nname = \"app\"\n\n[dependencies]\nui = { workspace = true }\n\n\
             [features]\nfull = [\"ui/devtools\"]\nperf = [\"ui/perf-trace\"]\n",
        );
        // No scripted cargo: the key comes from the manifest fallback.
        let fallback = FakeProcessRunner::new();
        assert!(feature_enables_devtools_with(&fallback, &dir, "full"));
        assert!(!feature_enables_devtools_with(&fallback, &dir, "perf"));
        // cargo sees the inherited rename `ui`.
        let renamed = metadata_runner(&dir, FRUST_UI_AS_UI);
        assert!(feature_enables_devtools_with(&renamed, &dir, "full"));
        // cargo resolves the facade as `frust`, so `ui` is another crate.
        let other = metadata_runner(
            &dir,
            &format!(
                r#"{FRUST_UI_AS_FRUST},{{"name":"ui","rename":null,"kind":null,"target":null}}"#
            ),
        );
        assert!(!feature_enables_devtools_with(&other, &dir, "full"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A facade key no feature can spell is not trusted to exclude any
    /// `<dep>/devtools` spelling.
    #[test]
    fn feature_enables_devtools_fails_closed_on_an_invalid_key() {
        let dir = unique_dir("enables-devtools-invalid-key");
        write_manifest(
            &dir,
            "[package]\nname = \"app\"\n\n[features]\nfull = [\"frust-material/devtools\"]\n",
        );
        let runner = metadata_runner(
            &dir,
            r#"{"name":"frust-ui","rename":"fr ust","kind":null,"target":null}"#,
        );
        assert!(feature_enables_devtools_with(&runner, &dir, "full"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A `frust`-keyed app does not get `frust-ui/devtools` for free: cargo
    /// would reject that spelling, so it is not this app's devtools alias.
    #[test]
    fn feature_enables_devtools_ignores_another_keys_spelling() {
        let dir = unique_dir("enables-devtools-wrong-key");
        write_manifest(
            &dir,
            "[package]\nname = \"app\"\n\n\
             [dependencies]\nfrust = { package = \"frust-ui\", version = \"0.6.0\" }\n\n\
             [features]\nfull = [\"frust-ui/devtools\"]\n",
        );
        assert!(!feature_enables_devtools_with(
            &metadata_runner(&dir, FRUST_UI_AS_FRUST),
            &dir,
            "full"
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    /// An app keying the facade twice counts either key's devtools — the
    /// refusal this widens must not be defeated by an ambiguous manifest.
    #[test]
    fn feature_enables_devtools_counts_every_key_of_an_ambiguous_manifest() {
        let dir = unique_dir("enables-devtools-ambiguous");
        write_manifest(
            &dir,
            "[package]\nname = \"app\"\n\n\
             [dependencies]\nfrust = { package = \"frust-ui\", version = \"0.6.0\" }\n\n\
             [target.'cfg(unix)'.dependencies]\nui = { package = \"frust-ui\", version = \"0.6.0\" }\n\n\
             [features]\nfull = [\"ui/devtools\"]\n",
        );
        assert!(feature_enables_devtools_with(
            &FakeProcessRunner::new(),
            &dir,
            "full"
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn feature_enables_devtools_fails_closed_on_missing_manifest() {
        let dir = unique_dir("enables-devtools-missing");
        // No Cargo.toml written — unlike `declares_feature`, this must NOT
        // fail open, since it exists to widen a security refusal.
        assert!(!feature_enables_devtools(&dir, "full"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn feature_enables_devtools_fails_closed_on_unparseable_manifest() {
        let dir = unique_dir("enables-devtools-garbage");
        write_manifest(&dir, "this is { not ]= valid toml");
        assert!(!feature_enables_devtools(&dir, "full"));
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
