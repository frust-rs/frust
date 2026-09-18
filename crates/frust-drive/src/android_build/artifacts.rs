//! Artifact output-path computation and existence verification: AGP's exact
//! output *file* naming varies across versions/flavors, so this module only
//! computes the *directory* Gradle writes a variant's artifact(s) into, then
//! glob-verifies what's actually there rather than trusting a guessed
//! filename.
//!
//! Every path here hangs off the **project** directory, not the `android/`
//! Gradle root. Since the build-directory migration the generated
//! `settings.gradle.kts` redirects `:app`'s `buildDirectory` to
//! `<project>/build/android/app` (see [`BuildLayout::android_app`]), so a
//! migrated project's outputs do not live under `android/` at all.
//! [`legacy_output_dir`] still names the pre-migration location, and
//! [`discover`] falls back to it — read-only, with a one-time warning — so a
//! project whose generated Gradle config predates the redirect keeps
//! building.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::android_build::AndroidArtifact;
use crate::build_dirs::BuildLayout;
use crate::build_info::BuildMode;

/// Where the migration recipe for a pre-migration project lives. Named in
/// the legacy-fallback warning [`discover`] emits — keep it in step with the
/// heading that recipe actually carries.
const MIGRATION_RECIPE_DOC: &str = "docs/DEVELOPMENT.md, `Migrating an already-scaffolded app`";

/// The directory Gradle writes `target`'s artifact(s) into, under
/// `project_dir`:
/// - APK: `build/android/app/outputs/apk/<flavor?>/<mode>/`
/// - App Bundle: `build/android/app/outputs/bundle/<variant>/`, where
///   `<variant>` is `<flavor><Mode>` (or just `<mode>` with no flavor).
pub fn expected_output_dir(
    project_dir: &Path,
    target: &AndroidArtifact,
    mode: BuildMode,
    flavor: Option<&str>,
) -> PathBuf {
    output_dir_under(
        &project_dir.join(BuildLayout::android_app()).join("outputs"),
        target,
        mode,
        flavor,
    )
}

/// The pre-migration location of [`expected_output_dir`]: AGP's own default,
/// `<project>/android/app/build/outputs/...`, which a generated project whose
/// `settings.gradle.kts` carries no `buildDirectory` redirect still writes
/// to. Read-only as far as Frust is concerned — nothing here ever puts an
/// artifact there; `frust clean` removes it via
/// [`LEGACY_CLEAN_DIRS`](crate::build_dirs::LEGACY_CLEAN_DIRS).
pub fn legacy_output_dir(
    project_dir: &Path,
    target: &AndroidArtifact,
    mode: BuildMode,
    flavor: Option<&str>,
) -> PathBuf {
    output_dir_under(
        &project_dir.join("android/app/build/outputs"),
        target,
        mode,
        flavor,
    )
}

/// The `apk/<flavor?>/<mode>` vs `bundle/<variant>` tail both
/// [`expected_output_dir`] and [`legacy_output_dir`] hang off their own
/// `outputs/` root: AGP's naming below `outputs/` is identical either side of
/// the migration — only the root moved.
fn output_dir_under(
    outputs_root: &Path,
    target: &AndroidArtifact,
    mode: BuildMode,
    flavor: Option<&str>,
) -> PathBuf {
    match target {
        AndroidArtifact::Apk { .. } => {
            let base = outputs_root.join("apk");
            match flavor {
                Some(flavor) => base.join(flavor).join(mode_dir_name(mode)),
                None => base.join(mode_dir_name(mode)),
            }
        }
        AndroidArtifact::Appbundle => outputs_root
            .join("bundle")
            .join(variant_dir_name(mode, flavor)),
    }
}

fn mode_dir_name(mode: BuildMode) -> &'static str {
    match mode {
        BuildMode::Debug => "debug",
        BuildMode::Profile => "profile",
        BuildMode::Release => "release",
    }
}

/// AGP's bundle-variant directory name: `<flavor><Mode>` (flavor lowercase
/// as given, mode capitalized) when a flavor is set, else just the lowercase
/// mode name — matches the naming `androidComponents.onVariants` reports.
fn variant_dir_name(mode: BuildMode, flavor: Option<&str>) -> String {
    match flavor {
        Some(flavor) => format!("{flavor}{}", mode.gradle_infix()),
        None => mode_dir_name(mode).to_string(),
    }
}

/// The number of `.apk`/`.aab` files `target`'s build variant should have
/// produced: `abis.len()` for a `--split-per-abi` APK build (one APK per
/// requested ABI), `1` for a fat APK or an App Bundle. `discover` checks the
/// glob result against this count so a leftover artifact from a *different*
/// build shape sitting in the (shared, non-split-aware) AGP output directory
/// — e.g. a stale split APK next to a freshly built fat one — is caught as
/// an error instead of silently reported as "Built".
fn expected_count(target: &AndroidArtifact) -> Result<usize> {
    match target {
        AndroidArtifact::Apk {
            split_per_abi: true,
            abis,
        } => {
            if abis.is_empty() {
                bail!(
                    "internal error: a split-per-ABI APK build was requested with no ABIs \
                     resolved — this should have been caught earlier"
                );
            }
            Ok(abis.len())
        }
        AndroidArtifact::Apk {
            split_per_abi: false,
            ..
        } => Ok(1),
        AndroidArtifact::Appbundle => Ok(1),
    }
}

/// Lists every file with `extension` (no leading dot, e.g. `"apk"`/`"aab"`)
/// directly inside `target`/`mode`/`flavor`'s output directory, erroring if
/// the directory is missing, contains none, or contains a different count
/// than `target` should have produced — the build is trusted to have run only
/// once this returns a result, since AGP writes every APK shape into the same
/// directory and a leftover from a prior, differently-shaped build would
/// otherwise be reported as freshly built.
///
/// Resolves the directory new-then-legacy (see [`resolve_output_dir`]),
/// routing the one-time pre-migration warning through `on_line` rather than
/// printing it — this sits inside a print-free core, so the warning has to
/// reach the caller's sink like every other line the pipeline emits.
pub fn discover(
    project_dir: &Path,
    target: &AndroidArtifact,
    mode: BuildMode,
    flavor: Option<&str>,
    on_line: &mut dyn FnMut(&str),
) -> Result<Vec<PathBuf>> {
    let extension = match target {
        AndroidArtifact::Apk { .. } => "apk",
        AndroidArtifact::Appbundle => "aab",
    };
    let dir = resolve_output_dir(project_dir, target, mode, flavor, on_line);
    let expected = expected_count(target)?;
    discover_in_dir(&dir, extension, expected)
}

/// Picks the directory to read artifacts from: [`expected_output_dir`]
/// whenever it exists, else [`legacy_output_dir`] when *that* does, else
/// [`expected_output_dir`] again so the "directory missing" error names the
/// path a correctly-configured project would have written.
///
/// New-first, not legacy-first: a project that has been migrated must never
/// be distracted by stale artifacts still sitting under `android/app/build/`
/// from before the move. The warning is emitted only on the step that
/// actually falls back, so a migrated project never sees it.
fn resolve_output_dir(
    project_dir: &Path,
    target: &AndroidArtifact,
    mode: BuildMode,
    flavor: Option<&str>,
    on_line: &mut dyn FnMut(&str),
) -> PathBuf {
    let current = expected_output_dir(project_dir, target, mode, flavor);
    if current.is_dir() {
        return current;
    }

    let legacy = legacy_output_dir(project_dir, target, mode, flavor);
    if legacy.is_dir() {
        on_line(&format!(
            "Warning: reading Android build output from the pre-migration path `{}`. \
             This project's generated Gradle config still writes build output into its \
             own source tree instead of `{}`; migrate it with the recipe in \
             {MIGRATION_RECIPE_DOC}.",
            legacy.display(),
            current.display(),
        ));
        return legacy;
    }

    current
}

fn discover_in_dir(dir: &Path, extension: &str, expected: usize) -> Result<Vec<PathBuf>> {
    let entries = std::fs::read_dir(dir).map_err(|err| {
        anyhow::anyhow!(
            "reading `{}` for built `.{extension}` artifacts: {err}",
            dir.display()
        )
    })?;

    let mut found = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|err| anyhow::anyhow!("reading an entry in `{}`: {err}", dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some(extension) {
            found.push(path);
        }
    }

    if found.is_empty() {
        bail!(
            "no `.{extension}` artifacts found in `{}` — the Gradle build may have written its \
             output somewhere else (AGP output paths vary by version)",
            dir.display()
        );
    }

    found.sort();

    if found.len() != expected {
        bail!(
            "expected {expected} `.{extension}` artifact{} in `{}`, found {}: {:?} — AGP writes \
             every APK shape into the same output directory, so this is likely a stale artifact \
             left over from a previous build; run `frust clean` and rebuild",
            if expected == 1 { "" } else { "s" },
            dir.display(),
            found.len(),
            found,
        );
    }

    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-artifacts-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn apk(abis: &[&str]) -> AndroidArtifact {
        AndroidArtifact::Apk {
            split_per_abi: false,
            abis: abis.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn split_apk(abis: &[&str]) -> AndroidArtifact {
        AndroidArtifact::Apk {
            split_per_abi: true,
            abis: abis.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// A no-op `on_line` sink for the discovery tests that assert nothing
    /// about the warning.
    fn silent() -> impl FnMut(&str) {
        |_| {}
    }

    #[test]
    fn expected_output_dir_apk_no_flavor() {
        let project = Path::new("/proj");
        let dir = expected_output_dir(project, &apk(&["arm64-v8a"]), BuildMode::Release, None);
        assert_eq!(
            dir,
            Path::new("/proj/build/android/app/outputs/apk/release")
        );
    }

    #[test]
    fn expected_output_dir_apk_with_flavor() {
        let project = Path::new("/proj");
        let dir = expected_output_dir(
            project,
            &apk(&["arm64-v8a"]),
            BuildMode::Release,
            Some("paid"),
        );
        assert_eq!(
            dir,
            Path::new("/proj/build/android/app/outputs/apk/paid/release")
        );
    }

    #[test]
    fn expected_output_dir_appbundle_no_flavor() {
        let project = Path::new("/proj");
        let dir = expected_output_dir(
            project,
            &AndroidArtifact::Appbundle,
            BuildMode::Release,
            None,
        );
        assert_eq!(
            dir,
            Path::new("/proj/build/android/app/outputs/bundle/release")
        );
    }

    #[test]
    fn expected_output_dir_appbundle_with_flavor() {
        let project = Path::new("/proj");
        let dir = expected_output_dir(
            project,
            &AndroidArtifact::Appbundle,
            BuildMode::Release,
            Some("paid"),
        );
        assert_eq!(
            dir,
            Path::new("/proj/build/android/app/outputs/bundle/paidRelease")
        );
    }

    /// The pre-migration path is still computable — `discover` falls back to
    /// it, and this pins the exact shape that fallback looks for.
    #[test]
    fn legacy_output_dir_names_the_pre_migration_agp_default() {
        let project = Path::new("/proj");
        assert_eq!(
            legacy_output_dir(project, &apk(&["arm64-v8a"]), BuildMode::Release, None),
            Path::new("/proj/android/app/build/outputs/apk/release")
        );
        assert_eq!(
            legacy_output_dir(
                project,
                &AndroidArtifact::Appbundle,
                BuildMode::Release,
                Some("paid")
            ),
            Path::new("/proj/android/app/build/outputs/bundle/paidRelease")
        );
    }

    #[test]
    fn discover_finds_planted_apk_files() {
        let dir = unique_temp_dir("apk-found");
        let out_dir = dir.join("build/android/app/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-release.apk"), b"fake").unwrap();
        fs::write(out_dir.join("output-metadata.json"), b"{}").unwrap();

        let found = discover(
            &dir,
            &apk(&["arm64-v8a"]),
            BuildMode::Release,
            None,
            &mut silent(),
        )
        .unwrap();
        assert_eq!(found, vec![out_dir.join("app-release.apk")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_finds_multiple_split_apks_sorted() {
        let dir = unique_temp_dir("apk-split");
        let out_dir = dir.join("build/android/app/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-x86_64-release.apk"), b"fake").unwrap();
        fs::write(out_dir.join("app-arm64-v8a-release.apk"), b"fake").unwrap();

        let found = discover(
            &dir,
            &split_apk(&["arm64-v8a", "x86_64"]),
            BuildMode::Release,
            None,
            &mut silent(),
        )
        .unwrap();
        assert_eq!(
            found,
            vec![
                out_dir.join("app-arm64-v8a-release.apk"),
                out_dir.join("app-x86_64-release.apk"),
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_errs_when_split_build_has_extra_stale_apk() {
        let dir = unique_temp_dir("apk-split-stale");
        let out_dir = dir.join("build/android/app/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-arm64-v8a-release.apk"), b"fake").unwrap();
        fs::write(out_dir.join("app-x86_64-release.apk"), b"fake").unwrap();
        fs::write(out_dir.join("app-armeabi-v7a-release.apk"), b"stale").unwrap();

        let err = discover(
            &dir,
            &split_apk(&["arm64-v8a", "x86_64"]),
            BuildMode::Release,
            None,
            &mut silent(),
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("expected 2"), "{message}");
        assert!(message.contains("found 3"), "{message}");
        assert!(message.contains("app-arm64-v8a-release.apk"), "{message}");
        assert!(message.contains("app-x86_64-release.apk"), "{message}");
        assert!(message.contains("app-armeabi-v7a-release.apk"), "{message}");
        assert!(message.contains("frust clean"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_errs_when_fat_build_has_stale_extra_apk() {
        let dir = unique_temp_dir("apk-fat-stale");
        let out_dir = dir.join("build/android/app/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-release.apk"), b"fake").unwrap();
        fs::write(out_dir.join("app-arm64-v8a-release.apk"), b"stale").unwrap();

        let err = discover(
            &dir,
            &apk(&["arm64-v8a"]),
            BuildMode::Release,
            None,
            &mut silent(),
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("expected 1"), "{message}");
        assert!(message.contains("found 2"), "{message}");
        assert!(message.contains("frust clean"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_errs_when_appbundle_has_stale_extra_aab() {
        let dir = unique_temp_dir("aab-stale");
        let out_dir = dir.join("build/android/app/outputs/bundle/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-release.aab"), b"fake").unwrap();
        fs::write(out_dir.join("app-old-release.aab"), b"stale").unwrap();

        let err = discover(
            &dir,
            &AndroidArtifact::Appbundle,
            BuildMode::Release,
            None,
            &mut silent(),
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("expected 1"), "{message}");
        assert!(message.contains("found 2"), "{message}");
        assert!(message.contains("frust clean"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_finds_planted_aab() {
        let dir = unique_temp_dir("aab-found");
        let out_dir = dir.join("build/android/app/outputs/bundle/paidRelease");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-paid-release.aab"), b"fake").unwrap();

        let found = discover(
            &dir,
            &AndroidArtifact::Appbundle,
            BuildMode::Release,
            Some("paid"),
            &mut silent(),
        )
        .unwrap();
        assert_eq!(found, vec![out_dir.join("app-paid-release.aab")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_errs_when_directory_missing() {
        let dir = unique_temp_dir("dir-missing");
        let err = discover(
            &dir,
            &apk(&["arm64-v8a"]),
            BuildMode::Release,
            None,
            &mut silent(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("outputs/apk/release"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Pre-migration project: nothing under `build/android/`, an APK sitting
    /// where AGP's own default put it. `discover` finds it and warns once,
    /// naming both paths and the migration recipe.
    #[test]
    fn discover_falls_back_to_the_legacy_output_dir_and_warns_once() {
        let dir = unique_temp_dir("legacy-apk");
        let out_dir = dir.join("android/app/build/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-release.apk"), b"fake").unwrap();

        let mut lines = Vec::new();
        let found = discover(
            &dir,
            &apk(&["arm64-v8a"]),
            BuildMode::Release,
            None,
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();
        assert_eq!(found, vec![out_dir.join("app-release.apk")]);

        assert_eq!(lines.len(), 1, "{lines:?}");
        let warning = &lines[0];
        assert!(warning.contains("android/app/build/outputs"), "{warning}");
        assert!(warning.contains("build/android/app/outputs"), "{warning}");
        assert!(warning.contains("docs/DEVELOPMENT.md"), "{warning}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The legacy fallback covers App Bundles on the same terms as APKs —
    /// both lanes moved, so both need the fallback.
    #[test]
    fn discover_falls_back_to_the_legacy_bundle_dir() {
        let dir = unique_temp_dir("legacy-aab");
        let out_dir = dir.join("android/app/build/outputs/bundle/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-release.aab"), b"fake").unwrap();

        let mut lines = Vec::new();
        let found = discover(
            &dir,
            &AndroidArtifact::Appbundle,
            BuildMode::Release,
            None,
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();
        assert_eq!(found, vec![out_dir.join("app-release.aab")]);
        assert_eq!(lines.len(), 1, "{lines:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A migrated project is never distracted by leftovers: with BOTH
    /// directories populated the new one wins outright and nothing is
    /// warned, so a stale pre-migration APK can't be reported as freshly
    /// built.
    #[test]
    fn discover_prefers_the_migrated_dir_over_a_stale_legacy_one_without_warning() {
        let dir = unique_temp_dir("legacy-and-current");
        let current = dir.join("build/android/app/outputs/apk/release");
        let legacy = dir.join("android/app/build/outputs/apk/release");
        fs::create_dir_all(&current).unwrap();
        fs::create_dir_all(&legacy).unwrap();
        fs::write(current.join("app-release.apk"), b"fresh").unwrap();
        fs::write(legacy.join("app-release.apk"), b"stale").unwrap();

        let mut lines = Vec::new();
        let found = discover(
            &dir,
            &apk(&["arm64-v8a"]),
            BuildMode::Release,
            None,
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();
        assert_eq!(found, vec![current.join("app-release.apk")]);
        assert!(lines.is_empty(), "{lines:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Neither directory exists: the error names the *migrated* path, so a
    /// correctly-configured project's failure never points at a location it
    /// was never going to write to.
    #[test]
    fn discover_errs_naming_the_migrated_dir_when_neither_exists() {
        let dir = unique_temp_dir("neither-layout");
        let err = discover(
            &dir,
            &apk(&["arm64-v8a"]),
            BuildMode::Release,
            None,
            &mut silent(),
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("build/android/app/outputs/apk/release"),
            "{message}"
        );
        assert!(!message.contains("android/app/build/outputs"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_errs_when_directory_empty() {
        let dir = unique_temp_dir("dir-empty");
        let out_dir = dir.join("build/android/app/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        let err = discover(
            &dir,
            &apk(&["arm64-v8a"]),
            BuildMode::Release,
            None,
            &mut silent(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("no `.apk` artifacts"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }
}
