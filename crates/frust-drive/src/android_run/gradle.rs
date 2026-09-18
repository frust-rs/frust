//! Drives `./gradlew assemble<Flavor><Mode>` in `<app>/android/`, mode/flavor
//! aware. The generated Gradle project's cargo-ndk task does the actual Rust
//! `.so` build; this module only shells out to Gradle and streams its
//! output.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::android_build::AndroidArtifact;
use crate::build_dirs::BuildLayout;
use crate::build_info::BuildMode;
use crate::process::{Output, ProcessRunner};

/// The Gradle CLI option that relocates Gradle's own per-project cache
/// (`.gradle/`), which Gradle otherwise creates next to `settings.gradle.kts`
/// — inside the app's `android/` source tree. `settings.gradle.kts`'
/// `buildDirectory` redirect cannot move it: the cache is written during
/// *configuration*, before any of that runs, so the CLI has to say where it
/// goes.
const PROJECT_CACHE_DIR_FLAG: &str = "--project-cache-dir";

/// The absolute `--project-cache-dir` value for the project rooted at
/// `project_dir`: its [`BuildLayout::android_gradle_cache`] directory, so
/// Gradle's cache joins every other Android build artifact under
/// `<project>/build/android/`.
///
/// Absolute on purpose. Gradle resolves a relative `--project-cache-dir`
/// against its own working directory (`<project>/android/`, not the project
/// root), which would silently put the cache back in the source tree; and
/// `frust clean` deletes the directory by the project-rooted path, so the two
/// sides must name the same place. Falls back to the lexical path, then the
/// unmodified join, if the process has no readable current directory —
/// defensive only, since the caller has already read files under
/// `project_dir`.
pub fn project_cache_dir(project_dir: &Path) -> PathBuf {
    let cache = project_dir.join(BuildLayout::android_gradle_cache());
    std::path::absolute(&cache).unwrap_or(cache)
}

/// The two leading `./gradlew` arguments every Frust-driven invocation
/// carries, ahead of the task name. Returned as owned `String`s because the
/// cache path is computed per project; both `assemble` here and
/// `android_build::build_with_env`'s own `run_streaming` call build their
/// argument vector from this, so the two lanes cannot drift.
pub fn leading_args(project_dir: &Path) -> [String; 2] {
    [
        PROJECT_CACHE_DIR_FLAG.to_string(),
        project_cache_dir(project_dir)
            .to_string_lossy()
            .into_owned(),
    ]
}

/// Runs `./gradlew --project-cache-dir <dir> <task> [-P...]` in `android_dir`
/// — `task` and `props` are `android_build::tasks::task_name`/
/// `gradle_properties`' output, reused rather than duplicated here so the run
/// and build pipelines compute a variant's task name and `-P` properties from
/// exactly one place. Streams each line of output through `on_line` (prefixed
/// `[gradle] `) and exports `JAVA_HOME=java_home` for the child process only.
///
/// `project_dir` is the Frust project root (`android_dir`'s parent); it is
/// taken explicitly rather than derived so a caller that already resolved it
/// — both do — cannot be given an `android_dir` whose parent is something
/// else. See [`leading_args`] for why the cache directory has to be passed on
/// the command line at all.
pub fn assemble(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    android_dir: &Path,
    java_home: &str,
    task: &str,
    props: &[String],
    on_line: &mut dyn FnMut(&str),
) -> Result<Output> {
    let leading = leading_args(project_dir);
    let mut args: Vec<&str> = Vec::with_capacity(leading.len() + 1 + props.len());
    for arg in &leading {
        args.push(arg.as_str());
    }
    args.push(task);
    for prop in props {
        args.push(prop.as_str());
    }

    let mut prefixed = |line: &str| on_line(&format!("[gradle] {line}"));
    runner
        .run_streaming(
            "./gradlew",
            &args,
            Some(android_dir),
            &[("JAVA_HOME", java_home)],
            &mut prefixed,
        )
        .with_context(|| format!("running `./gradlew {task}` in `{}`", android_dir.display()))
}

/// Resolves the single APK a non-split `assemble<Flavor><Mode>` build should
/// have produced, reusing `android_build::artifacts`' directory + glob +
/// expected-count discovery rather than guessing AGP's exact output
/// filename or re-checking the count here — the
/// output-naming/count-guard logic lives in exactly one place. `frust
/// run` always builds a single-ABI, non-split APK (it installs on one
/// connected device), so `discover` itself now errors on anything but
/// exactly one file (e.g. a stale split-build artifact left over in the
/// output directory) rather than this function re-deriving the same check.
pub fn apk_output_path(
    project_dir: &Path,
    mode: BuildMode,
    flavor: Option<&str>,
    on_line: &mut dyn FnMut(&str),
) -> Result<PathBuf> {
    let target = AndroidArtifact::Apk {
        split_per_abi: false,
        abis: Vec::new(),
    };
    let mut found =
        crate::android_build::artifacts::discover(project_dir, &target, mode, flavor, on_line)?;
    debug_assert_eq!(
        found.len(),
        1,
        "discover already guards a non-split APK build to exactly one file"
    );
    Ok(found
        .pop()
        .expect("discover errors on anything but exactly one file"))
}

/// Best-effort check for whether Gradle's wrapper distribution is already
/// cached under `<gradle_user_home>/wrapper/dists/` — used only to print a
/// one-time note that the first run downloads Gradle, never to gate the
/// build.
pub fn wrapper_dist_cached(gradle_user_home: &Path) -> bool {
    gradle_user_home
        .join("wrapper")
        .join("dists")
        .read_dir()
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{FakeProcessRunner, Output};
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// The full `./gradlew` argv this module produces, as the
    /// `FakeProcessRunner` keys it. Registering the WHOLE invocation (rather
    /// than a prefix) is what makes these tests assert the argv: the fake
    /// falls back to the longest registered key that is a *prefix* of the
    /// real one, so a missing or misplaced `--project-cache-dir` finds no
    /// fixture at all and fails the call.
    fn gradlew_argv(project_dir: &Path, task: &str, props: &[&str]) -> String {
        let mut key = format!(
            "./gradlew --project-cache-dir {} {task}",
            project_cache_dir(project_dir).display()
        );
        for prop in props {
            key.push(' ');
            key.push_str(prop);
        }
        key
    }

    #[test]
    fn assemble_runs_gradlew_with_task_props_java_home_env_and_prefixes_lines() {
        let project_dir = Path::new("/tmp/myapp");
        let runner = FakeProcessRunner::new().with(
            gradlew_argv(
                project_dir,
                "assembleDebug",
                &[
                    "-Pfrust.targetPlatforms=arm64-v8a",
                    "-Pfrust.splitPerAbi=false",
                ],
            ),
            Output {
                success: true,
                stdout: "> Task :app:assembleDebug\nBUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );
        let mut lines = Vec::new();
        let props = vec![
            "-Pfrust.targetPlatforms=arm64-v8a".to_string(),
            "-Pfrust.splitPerAbi=false".to_string(),
        ];
        let out = assemble(
            &runner,
            project_dir,
            &project_dir.join("android"),
            "/opt/jdk17",
            "assembleDebug",
            &props,
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();
        assert!(out.success);
        assert_eq!(
            lines,
            vec![
                "[gradle] > Task :app:assembleDebug",
                "[gradle] BUILD SUCCESSFUL",
            ]
        );
        // Gradle still runs *in* `android/` — only its cache moved.
        assert_eq!(
            runner.recorded_cwd().as_deref(),
            Some(&*project_dir.join("android"))
        );
    }

    /// Pins the exact leading argv and the exact cache directory: Gradle's
    /// per-project cache must land under `<project>/build/android/.gradle`,
    /// absolute, ahead of the task name — `frust clean` removes it by that
    /// same project-rooted path.
    #[test]
    fn assemble_leads_with_an_absolute_project_cache_dir_under_build_android() {
        let project_dir = Path::new("/tmp/myapp");
        let expected_cache = Path::new("/tmp/myapp/build/android/.gradle");
        assert_eq!(project_cache_dir(project_dir), expected_cache);
        assert_eq!(
            leading_args(project_dir),
            [
                "--project-cache-dir".to_string(),
                "/tmp/myapp/build/android/.gradle".to_string(),
            ]
        );

        let runner = FakeProcessRunner::new().with(
            "./gradlew --project-cache-dir /tmp/myapp/build/android/.gradle assembleRelease",
            Output {
                success: true,
                stdout: "BUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );
        let out = assemble(
            &runner,
            project_dir,
            &project_dir.join("android"),
            "/opt/jdk17",
            "assembleRelease",
            &[],
            &mut |_| {},
        )
        .unwrap();
        assert!(out.success);
    }

    /// A relative `project_dir` still yields an absolute cache path: Gradle
    /// would otherwise resolve it against `android/`, putting the cache back
    /// inside the source tree.
    #[test]
    fn project_cache_dir_is_absolute_even_for_a_relative_project_dir() {
        let cache = project_cache_dir(Path::new("myapp"));
        assert!(cache.is_absolute(), "{}", cache.display());
        assert!(
            cache.ends_with("myapp/build/android/.gradle"),
            "{}",
            cache.display()
        );
    }

    #[test]
    fn assemble_surfaces_failure() {
        let project_dir = Path::new("/tmp/failing-app");
        let runner = FakeProcessRunner::new().with(
            gradlew_argv(project_dir, "assembleRelease", &[]),
            Output {
                success: false,
                stdout: "> Task :app:compileReleaseKotlin FAILED".to_string(),
                stderr: "e: compile error".to_string(),
            },
        );
        let out = assemble(
            &runner,
            project_dir,
            &project_dir.join("android"),
            "/opt/jdk17",
            "assembleRelease",
            &[],
            &mut |_| {},
        )
        .unwrap();
        assert!(!out.success);
    }

    #[test]
    fn apk_output_path_finds_the_single_debug_apk_no_flavor() {
        let dir = unique_temp_dir("apk-output-debug");
        let out_dir = dir.join("build/android/app/outputs/apk/debug");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-debug.apk"), b"fake").unwrap();

        let path = apk_output_path(&dir, BuildMode::Debug, None, &mut |_| {}).unwrap();
        assert_eq!(path, out_dir.join("app-debug.apk"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apk_output_path_finds_flavored_release_apk() {
        let dir = unique_temp_dir("apk-output-flavored-release");
        let out_dir = dir.join("build/android/app/outputs/apk/paid/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-paid-release.apk"), b"fake").unwrap();

        let path = apk_output_path(&dir, BuildMode::Release, Some("paid"), &mut |_| {}).unwrap();
        assert_eq!(path, out_dir.join("app-paid-release.apk"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apk_output_path_errs_when_more_than_one_apk_present() {
        let dir = unique_temp_dir("apk-output-ambiguous");
        let out_dir = dir.join("build/android/app/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-arm64-v8a-release.apk"), b"fake").unwrap();
        fs::write(out_dir.join("app-x86_64-release.apk"), b"fake").unwrap();

        let err = apk_output_path(&dir, BuildMode::Release, None, &mut |_| {}).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("expected 1"), "{message}");
        assert!(message.contains("found 2"), "{message}");
        assert!(message.contains("frust clean"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-gradle-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn wrapper_dist_cached_false_when_dists_dir_absent() {
        let dir = unique_temp_dir("no-dists");
        fs::create_dir_all(&dir).unwrap();
        assert!(!wrapper_dist_cached(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrapper_dist_cached_true_when_dists_populated() {
        let dir = unique_temp_dir("with-dists");
        let dists = dir.join("wrapper").join("dists").join("gradle-9.5-bin");
        fs::create_dir_all(&dists).unwrap();
        fs::write(dists.join("marker"), b"x").unwrap();
        assert!(wrapper_dist_cached(&dir));
        let _ = fs::remove_dir_all(&dir);
    }
}
