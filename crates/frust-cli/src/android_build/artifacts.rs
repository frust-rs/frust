//! Artifact output-path computation and existence verification (spec
//! §12.5): AGP's exact output *file* naming varies across versions/flavors,
//! so this module only computes the *directory* Gradle writes a variant's
//! artifact(s) into, then glob-verifies what's actually there rather than
//! trusting a guessed filename.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::android_build::AndroidArtifact;
use crate::build_info::BuildMode;

/// The directory Gradle writes `target`'s artifact(s) into, under
/// `android_dir` (spec §12.5):
/// - APK: `app/build/outputs/apk/<flavor?>/<mode>/`
/// - App Bundle: `app/build/outputs/bundle/<variant>/`, where `<variant>` is
///   `<flavor><Mode>` (or just `<mode>` with no flavor).
pub fn expected_output_dir(
    android_dir: &Path,
    target: &AndroidArtifact,
    mode: BuildMode,
    flavor: Option<&str>,
) -> PathBuf {
    match target {
        AndroidArtifact::Apk { .. } => {
            let base = android_dir.join("app/build/outputs/apk");
            match flavor {
                Some(flavor) => base.join(flavor).join(mode_dir_name(mode)),
                None => base.join(mode_dir_name(mode)),
            }
        }
        AndroidArtifact::Appbundle => android_dir
            .join("app/build/outputs/bundle")
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
/// an error instead of silently reported as "Built" (followup F2).
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
/// directly inside `target`/`mode`/`flavor`'s expected output directory,
/// erroring if the directory is missing, contains none, or contains a
/// different count than `target` should have produced — the build is
/// trusted to have run only once this returns a result, since AGP writes
/// every APK shape into the same directory and a leftover from a prior,
/// differently-shaped build would otherwise be reported as freshly built.
pub fn discover(
    android_dir: &Path,
    target: &AndroidArtifact,
    mode: BuildMode,
    flavor: Option<&str>,
) -> Result<Vec<PathBuf>> {
    let extension = match target {
        AndroidArtifact::Apk { .. } => "apk",
        AndroidArtifact::Appbundle => "aab",
    };
    let dir = expected_output_dir(android_dir, target, mode, flavor);
    let expected = expected_count(target)?;
    discover_in_dir(&dir, extension, expected)
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

    #[test]
    fn expected_output_dir_apk_no_flavor() {
        let android_dir = Path::new("/proj/android");
        let dir = expected_output_dir(android_dir, &apk(&["arm64-v8a"]), BuildMode::Release, None);
        assert_eq!(
            dir,
            Path::new("/proj/android/app/build/outputs/apk/release")
        );
    }

    #[test]
    fn expected_output_dir_apk_with_flavor() {
        let android_dir = Path::new("/proj/android");
        let dir = expected_output_dir(
            android_dir,
            &apk(&["arm64-v8a"]),
            BuildMode::Release,
            Some("paid"),
        );
        assert_eq!(
            dir,
            Path::new("/proj/android/app/build/outputs/apk/paid/release")
        );
    }

    #[test]
    fn expected_output_dir_appbundle_no_flavor() {
        let android_dir = Path::new("/proj/android");
        let dir = expected_output_dir(
            android_dir,
            &AndroidArtifact::Appbundle,
            BuildMode::Release,
            None,
        );
        assert_eq!(
            dir,
            Path::new("/proj/android/app/build/outputs/bundle/release")
        );
    }

    #[test]
    fn expected_output_dir_appbundle_with_flavor() {
        let android_dir = Path::new("/proj/android");
        let dir = expected_output_dir(
            android_dir,
            &AndroidArtifact::Appbundle,
            BuildMode::Release,
            Some("paid"),
        );
        assert_eq!(
            dir,
            Path::new("/proj/android/app/build/outputs/bundle/paidRelease")
        );
    }

    #[test]
    fn discover_finds_planted_apk_files() {
        let dir = unique_temp_dir("apk-found");
        let out_dir = dir.join("app/build/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-release.apk"), b"fake").unwrap();
        fs::write(out_dir.join("output-metadata.json"), b"{}").unwrap();

        let found = discover(&dir, &apk(&["arm64-v8a"]), BuildMode::Release, None).unwrap();
        assert_eq!(found, vec![out_dir.join("app-release.apk")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_finds_multiple_split_apks_sorted() {
        let dir = unique_temp_dir("apk-split");
        let out_dir = dir.join("app/build/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-x86_64-release.apk"), b"fake").unwrap();
        fs::write(out_dir.join("app-arm64-v8a-release.apk"), b"fake").unwrap();

        let found = discover(
            &dir,
            &split_apk(&["arm64-v8a", "x86_64"]),
            BuildMode::Release,
            None,
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
        let out_dir = dir.join("app/build/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-arm64-v8a-release.apk"), b"fake").unwrap();
        fs::write(out_dir.join("app-x86_64-release.apk"), b"fake").unwrap();
        fs::write(out_dir.join("app-armeabi-v7a-release.apk"), b"stale").unwrap();

        let err = discover(
            &dir,
            &split_apk(&["arm64-v8a", "x86_64"]),
            BuildMode::Release,
            None,
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
        let out_dir = dir.join("app/build/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-release.apk"), b"fake").unwrap();
        fs::write(out_dir.join("app-arm64-v8a-release.apk"), b"stale").unwrap();

        let err = discover(&dir, &apk(&["arm64-v8a"]), BuildMode::Release, None).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("expected 1"), "{message}");
        assert!(message.contains("found 2"), "{message}");
        assert!(message.contains("frust clean"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_errs_when_appbundle_has_stale_extra_aab() {
        let dir = unique_temp_dir("aab-stale");
        let out_dir = dir.join("app/build/outputs/bundle/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-release.aab"), b"fake").unwrap();
        fs::write(out_dir.join("app-old-release.aab"), b"stale").unwrap();

        let err =
            discover(&dir, &AndroidArtifact::Appbundle, BuildMode::Release, None).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("expected 1"), "{message}");
        assert!(message.contains("found 2"), "{message}");
        assert!(message.contains("frust clean"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_finds_planted_aab() {
        let dir = unique_temp_dir("aab-found");
        let out_dir = dir.join("app/build/outputs/bundle/paidRelease");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-paid-release.aab"), b"fake").unwrap();

        let found = discover(
            &dir,
            &AndroidArtifact::Appbundle,
            BuildMode::Release,
            Some("paid"),
        )
        .unwrap();
        assert_eq!(found, vec![out_dir.join("app-paid-release.aab")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_errs_when_directory_missing() {
        let dir = unique_temp_dir("dir-missing");
        let err = discover(&dir, &apk(&["arm64-v8a"]), BuildMode::Release, None).unwrap_err();
        assert!(err.to_string().contains("outputs/apk/release"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_errs_when_directory_empty() {
        let dir = unique_temp_dir("dir-empty");
        let out_dir = dir.join("app/build/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        let err = discover(&dir, &apk(&["arm64-v8a"]), BuildMode::Release, None).unwrap_err();
        assert!(err.to_string().contains("no `.apk` artifacts"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }
}
