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
use std::time::SystemTime;

use anyhow::{Result, bail};

use crate::android_build::AndroidArtifact;
use crate::build_dirs::BuildLayout;
use crate::build_info::BuildMode;

/// Where the migration recipe for a pre-migration project lives. Named in
/// the legacy-fallback warning/error [`resolve_output_dir`] emits — keep it
/// in step with the heading that recipe actually carries (pinned by
/// `tests::migration_recipe_doc_heading_exists_in_development_md`, which
/// reads the real `docs/DEVELOPMENT.md` off `CARGO_MANIFEST_DIR`). `pub(crate)`
/// so the build/run pipelines (`android_build::mod`, `android_run::mod`) can
/// reference the exact same text their own doc-drift-sensitive tests pin.
pub(crate) const MIGRATION_RECIPE_DOC: &str =
    "docs/DEVELOPMENT.md, `Migrating an already-scaffolded app to the build/ layout`";

/// The pre-migration, AGP-default jniLibs source set. The generated app
/// template and every in-repo example replace it with a `setSrcDirs`
/// redirect onto [`BuildLayout::android_jni_libs`] (r1-04) — but only a
/// project whose *own* `android/app/build.gradle.kts` carries that redirect
/// has stopped packaging from here. An unmigrated project (the population
/// [`legacy_output_dir`]'s fallback exists for) still has this directory as
/// its live packaging input, so [`warn_if_legacy_jni_libs`] reads the
/// project's Gradle config before deciding what to say, and never tells an
/// unmigrated project to delete it. [`crate::build_dirs::LEGACY_CLEAN_DIRS`]
/// is what `frust clean` removes it with.
const LEGACY_JNI_LIBS_DIR: &str = "android/app/src/main/jniLibs";

/// Whether `project_dir`'s `android/app/build.gradle.kts` carries the
/// migrated jniLibs redirect (`setSrcDirs` onto
/// [`BuildLayout::android_jni_libs`]) — the same read-the-project's-own-
/// generated-config technique `desktop_build::windows` uses for
/// `windows/build.rs`. A missing/unreadable Gradle file counts as
/// unmigrated (the safe direction: never claim a directory is dead).
fn app_gradle_redirects_jni_libs(project_dir: &Path) -> bool {
    let gradle = project_dir.join("android/app/build.gradle.kts");
    let Ok(contents) = std::fs::read_to_string(gradle) else {
        return false;
    };
    let jni_libs = BuildLayout::android_jni_libs();
    let jni_libs = jni_libs.to_string_lossy();
    contents.contains("setSrcDirs") && contents.contains(jni_libs.as_ref())
}

/// Emits a one-time [`on_line`] warning when `project_dir`'s legacy
/// `android/app/src/main/jniLibs` directory exists and is non-empty — see
/// [`LEGACY_JNI_LIBS_DIR`]. Read-only: nothing here deletes it. Called once
/// per invocation by both Android lanes (`android_build::build_with_env`,
/// `android_run::prepare_session`). The wording depends on the project's own
/// `android/app/build.gradle.kts` (see [`app_gradle_redirects_jni_libs`]):
/// a migrated project is told the leftover is inert and `frust clean`
/// removes it; an unmigrated project is told the directory is still its live
/// packaging input and pointed at the migration recipe — never at deletion.
pub fn warn_if_legacy_jni_libs(project_dir: &Path, on_line: &mut dyn FnMut(&str)) {
    let dir = project_dir.join(LEGACY_JNI_LIBS_DIR);
    let non_empty = std::fs::read_dir(&dir)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);
    if !non_empty {
        return;
    }
    let jni_libs = BuildLayout::android_jni_libs();
    if app_gradle_redirects_jni_libs(project_dir) {
        on_line(&format!(
            "{LEGACY_JNI_LIBS_DIR} is a pre-build/ layout leftover and is no longer packaged \
             (this project's android/app/build.gradle.kts redirects jniLibs to `{}`; see \
             {MIGRATION_RECIPE_DOC}); run `frust clean` to remove it",
            jni_libs.display(),
        ));
    } else {
        on_line(&format!(
            "Warning: {LEGACY_JNI_LIBS_DIR} is still this project's live native-library \
             packaging input — its android/app/build.gradle.kts does not redirect jniLibs to \
             `{}` yet, so do not delete it; migrate the project with the recipe in \
             {MIGRATION_RECIPE_DOC}",
            jni_libs.display(),
        ));
    }
}

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
///
/// Reproduces [`discover_since`] with no freshness gate (`legacy_not_before:
/// None`) — a caller that never installs/ships the artifact it finds (e.g.
/// `frust build`, which only reports it) has no staleness to guard against.
pub fn discover(
    project_dir: &Path,
    target: &AndroidArtifact,
    mode: BuildMode,
    flavor: Option<&str>,
    on_line: &mut dyn FnMut(&str),
) -> Result<Vec<PathBuf>> {
    discover_since(project_dir, target, mode, flavor, None, on_line)
}

/// Like [`discover`], but when the pre-migration legacy directory is the one
/// used, rejects it unless it holds an artifact whose mtime is at or after
/// `legacy_not_before` — the Gradle invocation's own start instant, so a
/// stale leftover the legacy Gradle config failed to actually rebuild is
/// never reported as this build's output. `None` reproduces [`discover`]'s
/// behavior exactly; the migrated-path fast path
/// ([`expected_output_dir`] existing) is never touched by this gate either
/// way. Used by `android_run`'s pipeline (via
/// [`discover_single_apk_since`]), which installs what it finds here onto a
/// device and so cannot afford a stale APK; `android_build`'s pipeline keeps
/// calling [`discover`] since it only reports the path.
pub fn discover_since(
    project_dir: &Path,
    target: &AndroidArtifact,
    mode: BuildMode,
    flavor: Option<&str>,
    legacy_not_before: Option<SystemTime>,
    on_line: &mut dyn FnMut(&str),
) -> Result<Vec<PathBuf>> {
    let extension = artifact_extension(target);
    let dir = resolve_output_dir(
        project_dir,
        target,
        mode,
        flavor,
        legacy_not_before,
        on_line,
    )?;
    let expected = expected_count(target)?;
    discover_in_dir(&dir, extension, expected)
}

/// The single non-split APK `android_run`'s pipeline always builds (one
/// connected device, so no split-per-ABI, no App Bundle) — the
/// freshness-aware counterpart of `android_run::gradle::apk_output_path`
/// (which calls plain [`discover`], no freshness gate), reachable directly
/// from this module so `legacy_not_before` can gate a stale pre-migration
/// APK without changing that read-only dependency's frozen signature.
pub fn discover_single_apk_since(
    project_dir: &Path,
    mode: BuildMode,
    flavor: Option<&str>,
    legacy_not_before: Option<SystemTime>,
    on_line: &mut dyn FnMut(&str),
) -> Result<PathBuf> {
    let target = AndroidArtifact::Apk {
        split_per_abi: false,
        abis: Vec::new(),
    };
    let mut found = discover_since(
        project_dir,
        &target,
        mode,
        flavor,
        legacy_not_before,
        on_line,
    )?;
    debug_assert_eq!(
        found.len(),
        1,
        "discover_since already guards a non-split APK build to exactly one file"
    );
    Ok(found
        .pop()
        .expect("discover_since errors on anything but exactly one file"))
}

/// The `.apk`/`.aab` extension (no leading dot) `target` produces —
/// shared by [`discover_since`] (glob filter) and [`resolve_output_dir`]
/// (freshness peek) so the two never drift on what counts as "this
/// target's file".
fn artifact_extension(target: &AndroidArtifact) -> &'static str {
    match target {
        AndroidArtifact::Apk { .. } => "apk",
        AndroidArtifact::Appbundle => "aab",
    }
}

/// Picks the directory to read artifacts from: [`expected_output_dir`]
/// whenever it exists, else [`legacy_output_dir`] when *that* does (and,
/// with `legacy_not_before: Some(_)`, holds a fresh-enough artifact — see
/// below), else [`expected_output_dir`] again so the "directory missing"
/// error names the path a correctly-configured project would have written.
///
/// New-first, not legacy-first: a project that has been migrated must never
/// be distracted by stale artifacts still sitting under `android/app/build/`
/// from before the move. The warning is emitted only on the step that
/// actually falls back, so a migrated project never sees it.
///
/// `legacy_not_before` adds a freshness *warning* to the legacy branch: when
/// set and the legacy directory holds no `target`-extension file whose mtime
/// is at or after that instant, a second warning says the artifact predates
/// the Gradle invocation that just ran. It is deliberately not an error:
/// Gradle leaves the APK untouched when its packaging task is UP-TO-DATE, so
/// a source-unchanged rerun of a pre-migration project legitimately produces
/// nothing new and must still install what the last successful build made.
/// The warning tells the user how to tell the two cases apart (`frust clean`
/// + rebuild) without breaking the run loop for unmigrated apps.
fn resolve_output_dir(
    project_dir: &Path,
    target: &AndroidArtifact,
    mode: BuildMode,
    flavor: Option<&str>,
    legacy_not_before: Option<SystemTime>,
    on_line: &mut dyn FnMut(&str),
) -> Result<PathBuf> {
    let current = expected_output_dir(project_dir, target, mode, flavor);
    if current.is_dir() {
        return Ok(current);
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
        if let Some(not_before) = legacy_not_before {
            let extension = artifact_extension(target);
            if !has_fresh_artifact(&legacy, extension, not_before) {
                on_line(&format!(
                    "Warning: the `.{extension}` under `{}` predates this build — the Gradle \
                     invocation that just ran did not rewrite it (its packaging task was \
                     up-to-date, or the legacy config wrote elsewhere). If the installed app \
                     looks stale, run `frust clean` and rebuild.",
                    legacy.display(),
                ));
            }
        }
        return Ok(legacy);
    }

    Ok(current)
}

/// Whether `dir` contains at least one `extension` file whose mtime is at or
/// after `not_before` — a coarse freshness peek for
/// [`resolve_output_dir`]'s legacy-fallback warning; the full listing, count
/// validation, and per-target naming stay in [`discover_in_dir`]. An
/// unreadable directory or unreadable mtime counts as "not fresh" (the
/// caller already checked `dir.is_dir()`, so a read error here is the rarer
/// permissions/race case, not the common "doesn't exist" one) — favoring the
/// stale-artifact warning over silently treating an artifact as fresh.
fn has_fresh_artifact(dir: &Path, extension: &str, not_before: SystemTime) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        if entry.path().extension().and_then(|e| e.to_str()) != Some(extension) {
            return false;
        }
        entry
            .metadata()
            .and_then(|meta| meta.modified())
            .map(|mtime| mtime >= not_before)
            .unwrap_or(false)
    })
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
        assert!(warning.contains(MIGRATION_RECIPE_DOC), "{warning}");
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

    /// The doc heading `MIGRATION_RECIPE_DOC` names must actually exist in
    /// `docs/DEVELOPMENT.md` — removing the heading line (as a docs-side
    /// rewrite could) fails this test with a message naming both the
    /// expected heading and the doc path, instead of the two silently
    /// drifting apart.
    #[test]
    fn migration_recipe_doc_heading_exists_in_development_md() {
        let doc_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/DEVELOPMENT.md");
        let doc = fs::read_to_string(&doc_path)
            .unwrap_or_else(|err| panic!("reading `{}`: {err}", doc_path.display()));
        let heading = MIGRATION_RECIPE_DOC
            .split('`')
            .nth(1)
            .expect("MIGRATION_RECIPE_DOC names a `backtick-quoted` heading");
        let expected_line = format!("### {heading}");
        assert!(
            doc.lines().any(|line| line == expected_line),
            "`{}` must carry the heading `{expected_line}` — MIGRATION_RECIPE_DOC and the doc \
             have drifted",
            doc_path.display(),
        );
    }

    /// A legacy artifact whose mtime is at/after the Gradle invocation's
    /// start instant is accepted, with the same one-time warning the
    /// no-freshness-gate path emits.
    #[test]
    fn discover_since_accepts_a_fresh_legacy_artifact_and_warns_once() {
        let dir = unique_temp_dir("legacy-fresh");
        let out_dir = dir.join("android/app/build/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        let not_before = SystemTime::now();
        let apk_path = out_dir.join("app-release.apk");
        fs::write(&apk_path, b"fake").unwrap();
        // Pin the mtime explicitly to `not_before` rather than relying on
        // write-then-check ordering, so the test can't flake on a coarse
        // filesystem timestamp.
        fs::File::open(&apk_path)
            .unwrap()
            .set_modified(not_before)
            .unwrap();

        let mut lines = Vec::new();
        let found = discover_since(
            &dir,
            &apk(&["arm64-v8a"]),
            BuildMode::Release,
            None,
            Some(not_before),
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();
        assert_eq!(found, vec![apk_path]);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains(MIGRATION_RECIPE_DOC), "{lines:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A legacy artifact whose mtime predates the Gradle invocation's start
    /// instant is still returned (Gradle's UP-TO-DATE packaging task leaves
    /// the APK untouched on a source-unchanged rerun, which must keep
    /// working), but a second warning says it predates this build.
    #[test]
    fn discover_since_accepts_a_stale_legacy_artifact_with_a_second_warning() {
        let dir = unique_temp_dir("legacy-stale");
        let out_dir = dir.join("android/app/build/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        let apk_path = out_dir.join("app-release.apk");
        fs::write(&apk_path, b"fake").unwrap();
        let stale_mtime = SystemTime::now() - std::time::Duration::from_secs(3600);
        fs::File::open(&apk_path)
            .unwrap()
            .set_modified(stale_mtime)
            .unwrap();
        let build_start = SystemTime::now();

        let mut lines = Vec::new();
        let paths = discover_since(
            &dir,
            &apk(&["arm64-v8a"]),
            BuildMode::Release,
            None,
            Some(build_start),
            &mut |l| lines.push(l.to_string()),
        )
        .unwrap();
        assert_eq!(paths, vec![apk_path]);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains(MIGRATION_RECIPE_DOC), "{lines:?}");
        assert!(lines[1].contains("predates this build"), "{lines:?}");
        assert!(lines[1].contains("frust clean"), "{lines:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The migrated-path fast path is unaffected by the freshness gate: with
    /// the current directory present, `legacy_not_before` never even reaches
    /// the (absent) legacy directory.
    #[test]
    fn discover_since_migrated_path_unaffected_by_freshness_gate() {
        let dir = unique_temp_dir("migrated-with-gate");
        let out_dir = dir.join("build/android/app/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-release.apk"), b"fake").unwrap();

        let mut lines = Vec::new();
        let found = discover_since(
            &dir,
            &apk(&["arm64-v8a"]),
            BuildMode::Release,
            None,
            // Far in the future: would reject any legacy fallback outright,
            // proving the migrated path never even consults it.
            Some(SystemTime::now() + std::time::Duration::from_secs(3600)),
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();
        assert_eq!(found, vec![out_dir.join("app-release.apk")]);
        assert!(lines.is_empty(), "{lines:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_single_apk_since_finds_the_migrated_apk() {
        let dir = unique_temp_dir("single-apk");
        let out_dir = dir.join("build/android/app/outputs/apk/debug");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-debug.apk"), b"fake").unwrap();

        let path =
            discover_single_apk_since(&dir, BuildMode::Debug, None, None, &mut silent()).unwrap();
        assert_eq!(path, out_dir.join("app-debug.apk"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn warn_if_legacy_jni_libs_warns_once_for_a_non_empty_dir() {
        let dir = unique_temp_dir("legacy-jnilibs-nonempty");
        let jni_dir = dir.join("android/app/src/main/jniLibs");
        fs::create_dir_all(jni_dir.join("arm64-v8a")).unwrap();
        fs::write(jni_dir.join("arm64-v8a/libapp.so"), b"fake").unwrap();
        write_migrated_app_gradle(&dir);

        let mut lines = Vec::new();
        warn_if_legacy_jni_libs(&dir, &mut |line| lines.push(line.to_string()));
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("android/app/src/main/jniLibs is a pre-build/ layout leftover"),
            "{lines:?}"
        );
        assert!(lines[0].contains(MIGRATION_RECIPE_DOC), "{lines:?}");
        assert!(lines[0].contains("frust clean"), "{lines:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// An UNMIGRATED project (no `setSrcDirs` redirect in its own
    /// `android/app/build.gradle.kts`) still packages from
    /// `src/main/jniLibs`, so the warning must say so and must never advise
    /// deleting the directory.
    #[test]
    fn warn_if_legacy_jni_libs_never_advises_deletion_for_an_unmigrated_project() {
        let dir = unique_temp_dir("legacy-jnilibs-unmigrated");
        let jni_dir = dir.join("android/app/src/main/jniLibs");
        fs::create_dir_all(jni_dir.join("arm64-v8a")).unwrap();
        fs::write(jni_dir.join("arm64-v8a/libapp.so"), b"live").unwrap();
        fs::create_dir_all(dir.join("android/app")).unwrap();
        fs::write(
            dir.join("android/app/build.gradle.kts"),
            "android {\n    namespace = \"x\"\n}\n",
        )
        .unwrap();

        let mut lines = Vec::new();
        warn_if_legacy_jni_libs(&dir, &mut |line| lines.push(line.to_string()));
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("live native-library packaging input"),
            "{lines:?}"
        );
        assert!(lines[0].contains("do not delete it"), "{lines:?}");
        assert!(lines[0].contains(MIGRATION_RECIPE_DOC), "{lines:?}");
        assert!(!lines[0].contains("no longer packaged"), "{lines:?}");

        // No Gradle file at all is treated the same way (never "dead").
        fs::remove_file(dir.join("android/app/build.gradle.kts")).unwrap();
        lines.clear();
        warn_if_legacy_jni_libs(&dir, &mut |line| lines.push(line.to_string()));
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("do not delete it"), "{lines:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Writes the migrated `setSrcDirs` redirect the app template carries
    /// since r1-04, derived from `BuildLayout` exactly like the template's
    /// tripwire test does.
    fn write_migrated_app_gradle(dir: &Path) {
        fs::create_dir_all(dir.join("android/app")).unwrap();
        fs::write(
            dir.join("android/app/build.gradle.kts"),
            format!(
                "android {{\n    sourceSets.getByName(\"main\").jniLibs.setSrcDirs(listOf(\"../../{}\"))\n}}\n",
                BuildLayout::android_jni_libs().display()
            ),
        )
        .unwrap();
    }

    #[test]
    fn warn_if_legacy_jni_libs_silent_when_absent_or_empty() {
        let dir = unique_temp_dir("legacy-jnilibs-absent");
        let mut lines = Vec::new();
        warn_if_legacy_jni_libs(&dir, &mut |line| lines.push(line.to_string()));
        assert!(lines.is_empty(), "{lines:?}");

        fs::create_dir_all(dir.join("android/app/src/main/jniLibs")).unwrap();
        warn_if_legacy_jni_libs(&dir, &mut |line| lines.push(line.to_string()));
        assert!(lines.is_empty(), "empty dir must not warn: {lines:?}");

        let _ = fs::remove_dir_all(&dir);
    }
}
