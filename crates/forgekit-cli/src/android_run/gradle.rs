//! Drives `./gradlew assemble<Flavor><Mode>` in `<app>/android/` (spec
//! §12.4 step 4, mode-aware since task 66). The generated Gradle project's
//! cargo-ndk task does the actual Rust `.so` build; this module only shells
//! out to Gradle and streams its output.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::android_build::AndroidArtifact;
use crate::build_info::BuildMode;
use crate::process::{Output, ProcessRunner};

/// Runs `./gradlew <task> [-P...]` in `android_dir` — `task` and `props` are
/// `android_build::tasks::task_name`/`gradle_properties`' output (task 64),
/// reused rather than duplicated here so the run and build pipelines compute
/// a variant's task name and `-P` properties from exactly one place.
/// Streams each line of output through `on_line` (prefixed `[gradle] `) and
/// exports `JAVA_HOME=java_home` for the child process only.
pub fn assemble(
    runner: &dyn ProcessRunner,
    android_dir: &Path,
    java_home: &str,
    task: &str,
    props: &[String],
    on_line: &mut dyn FnMut(&str),
) -> Result<Output> {
    let mut args: Vec<&str> = Vec::with_capacity(1 + props.len());
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
/// expected-count discovery (tasks 64, followup F2) rather than guessing
/// AGP's exact output filename or re-checking the count here — the
/// output-naming/count-guard logic lives in exactly one place. `forgekit
/// run` always builds a single-ABI, non-split APK (it installs on one
/// connected device), so `discover` itself now errors on anything but
/// exactly one file (e.g. a stale split-build artifact left over in the
/// output directory) rather than this function re-deriving the same check.
pub fn apk_output_path(
    android_dir: &Path,
    mode: BuildMode,
    flavor: Option<&str>,
) -> Result<PathBuf> {
    let target = AndroidArtifact::Apk {
        split_per_abi: false,
        abis: Vec::new(),
    };
    let mut found = crate::android_build::artifacts::discover(android_dir, &target, mode, flavor)?;
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
/// one-time note that the first run downloads Gradle (spec §12.4 step 4),
/// never to gate the build.
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

    #[test]
    fn assemble_runs_gradlew_with_task_props_java_home_env_and_prefixes_lines() {
        let runner = FakeProcessRunner::new().with(
            "./gradlew assembleDebug -Pforgekit.targetPlatforms=arm64-v8a -Pforgekit.splitPerAbi=false",
            Output {
                success: true,
                stdout: "> Task :app:assembleDebug\nBUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );
        let mut lines = Vec::new();
        let props = vec![
            "-Pforgekit.targetPlatforms=arm64-v8a".to_string(),
            "-Pforgekit.splitPerAbi=false".to_string(),
        ];
        let out = assemble(
            &runner,
            Path::new("/tmp/myapp/android"),
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
    }

    #[test]
    fn assemble_surfaces_failure() {
        let runner = FakeProcessRunner::new().with(
            "./gradlew assembleRelease",
            Output {
                success: false,
                stdout: "> Task :app:compileReleaseKotlin FAILED".to_string(),
                stderr: "e: compile error".to_string(),
            },
        );
        let out = assemble(
            &runner,
            Path::new("android"),
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
        let out_dir = dir.join("app/build/outputs/apk/debug");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-debug.apk"), b"fake").unwrap();

        let path = apk_output_path(&dir, BuildMode::Debug, None).unwrap();
        assert_eq!(path, out_dir.join("app-debug.apk"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apk_output_path_finds_flavored_release_apk() {
        let dir = unique_temp_dir("apk-output-flavored-release");
        let out_dir = dir.join("app/build/outputs/apk/paid/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-paid-release.apk"), b"fake").unwrap();

        let path = apk_output_path(&dir, BuildMode::Release, Some("paid")).unwrap();
        assert_eq!(path, out_dir.join("app-paid-release.apk"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn apk_output_path_errs_when_more_than_one_apk_present() {
        let dir = unique_temp_dir("apk-output-ambiguous");
        let out_dir = dir.join("app/build/outputs/apk/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-arm64-v8a-release.apk"), b"fake").unwrap();
        fs::write(out_dir.join("app-x86_64-release.apk"), b"fake").unwrap();

        let err = apk_output_path(&dir, BuildMode::Release, None).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("expected 1"), "{message}");
        assert!(message.contains("found 2"), "{message}");
        assert!(message.contains("forgekit clean"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "forgekit-cli-gradle-test-{tag}-{}-{n}",
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
