//! Android release build pipeline (spec §12.5, PLAN.md decision 2):
//! preflight (reusing `android_run`'s checks, minus the device-only `adb`
//! one) → merge-write `android/local.properties` → gate release builds on a
//! keystore → compute the `assemble<Flavor><Mode>`/`bundle<Flavor><Mode>`
//! Gradle task and its `-P` properties → run `./gradlew` → glob-verify and
//! report the produced artifact(s).

// `pub(crate)`, not private: task 66's `android_run::run` reuses these
// helpers directly (task-name/`-P`-property computation, the
// local.properties merge-write, the release-signing gate, artifact
// discovery) rather than duplicating the naming/path logic for `frust
// run`'s Android pipeline.
pub(crate) mod artifacts;
pub(crate) mod local_properties;
pub(crate) mod signing;
pub(crate) mod tasks;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::android_run::preflight::{self, PreflightCtx};
use crate::build_info::BuildInfo;
use crate::doctor::{EnvLookup, RealEnv};
use crate::process::{ProcessRunner, tail_lines};

/// The artifact `frust build apk`/`appbundle` requests (spec §12.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AndroidArtifact {
    /// A `--split-per-abi` (one APK per ABI) or fat APK build, for the
    /// already-mapped Gradle ABI names (`arm64-v8a`, `armeabi-v7a`,
    /// `x86_64`) requested via `--target-platform`.
    Apk {
        split_per_abi: bool,
        abis: Vec<String>,
    },
    /// An Android App Bundle (`.aab`) for Play Store distribution.
    Appbundle,
}

/// Paths to the artifact(s) a successful build produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuiltArtifacts {
    pub paths: Vec<PathBuf>,
}

/// Drives the Android release pipeline (Gradle `assemble<Flavor><Mode>` /
/// `bundle<Flavor><Mode>`, `cargo ndk` under the hood) for `target` in the
/// Frust project rooted at `project_dir`.
///
/// **Frozen signature** (task 61) — do not change without updating both
/// `commands::build` and this doc comment.
pub fn build(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    info: &BuildInfo,
    target: &AndroidArtifact,
) -> Result<BuiltArtifacts> {
    build_with_env(runner, project_dir, info, target, &RealEnv)
}

/// The testable core of [`build`], taking an injected [`EnvLookup`] so
/// `JAVA_HOME` resolution (`preflight::run_without_device_checks`) can be
/// exercised with a `crate::doctor::FakeEnv` in tests instead of the real
/// process environment — kept private since [`build`]'s frozen signature
/// (task 61) has no room for a fifth parameter.
fn build_with_env(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    info: &BuildInfo,
    target: &AndroidArtifact,
    env: &dyn EnvLookup,
) -> Result<BuiltArtifacts> {
    let android_dir = project_dir.join("android");

    let preflight_ctx = PreflightCtx {
        runner,
        env,
        is_macos: cfg!(target_os = "macos"),
    };
    let outcome =
        preflight::run_without_device_checks(&preflight_ctx).map_err(|err| anyhow::anyhow!(err))?;

    let version_name = info.build_name.clone().unwrap_or_else(|| "1.0".to_string());
    let version_code = info
        .build_number
        .map(|n| n.to_string())
        .unwrap_or_else(|| "1".to_string());
    local_properties::write(&android_dir, &version_name, &version_code)
        .context("writing android/local.properties")?;

    signing::check_release_signing(&android_dir, info.mode)?;

    let task = tasks::task_name(target, info.mode, info.flavor.as_deref());
    let props = tasks::gradle_properties(target, &info.defines);

    let mut args: Vec<&str> = Vec::with_capacity(1 + props.len());
    args.push(task.as_str());
    for prop in &props {
        args.push(prop.as_str());
    }

    let mut on_line = |line: &str| println!("[gradle] {line}");
    let out = runner
        .run_streaming(
            "./gradlew",
            &args,
            Some(&android_dir),
            &[("JAVA_HOME", &outcome.java_home)],
            &mut on_line,
        )
        .with_context(|| format!("running `./gradlew {task}` in `{}`", android_dir.display()))?;

    if !out.success {
        let combined = format!("{}\n{}", out.stdout, out.stderr);
        if let Some(flavor) = info.flavor.as_deref()
            && task_not_found(&combined, &task)
        {
            bail!(
                "flavor '{flavor}' has no Gradle product flavor — declare it in \
                 android/app/build.gradle.kts (see the commented example)"
            );
        }
        let tail = tail_lines(&out.stderr, 50);
        if tail.is_empty() {
            bail!("`./gradlew {task}` failed");
        }
        bail!("`./gradlew {task}` failed:\n{tail}");
    }

    let paths = artifacts::discover(&android_dir, target, info.mode, info.flavor.as_deref())?;
    for path in &paths {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        println!("{} ({size} bytes)", path.display());
    }

    Ok(BuiltArtifacts { paths })
}

/// Heuristically detects Gradle's "task not found" failure for `task`
/// (e.g. `Task 'assemblePaidRelease' not found in root project '…'`) in the
/// build's combined stdout+stderr, so a missing `--flavor` product flavor
/// can be re-explained instead of surfacing Gradle's generic message.
fn task_not_found(combined_output: &str, task: &str) -> bool {
    combined_output.contains(&format!("'{task}'")) && combined_output.contains("not found")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_info::{BuildArgs, BuildMode};
    use crate::doctor::FakeEnv;
    use crate::process::{FakeProcessRunner, Output};
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    const JAVA_HOME: &str = "/opt/jdk17";

    fn unique_project_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-android-build-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("android")).unwrap();
        dir
    }

    fn fake_env() -> FakeEnv {
        FakeEnv::new().set("JAVA_HOME", JAVA_HOME)
    }

    fn preflight_ok_runner() -> FakeProcessRunner {
        FakeProcessRunner::new()
            .with(
                "rustup target list --installed",
                Output {
                    success: true,
                    stdout: "aarch64-linux-android\n".to_string(),
                    stderr: String::new(),
                },
            )
            .with(
                "cargo ndk --version",
                Output {
                    success: true,
                    stdout: "cargo-ndk 3.5.4\n".to_string(),
                    stderr: String::new(),
                },
            )
            .with(
                format!("{JAVA_HOME}/bin/java -version"),
                Output {
                    success: true,
                    stdout: String::new(),
                    stderr: "openjdk version \"17.0.9\" 2023-10-17\n".to_string(),
                },
            )
    }

    fn info(mode: BuildMode, flavor: Option<&str>) -> BuildInfo {
        BuildInfo::from_args(
            BuildArgs {
                flavor: flavor.map(str::to_string),
                ..BuildArgs::default()
            },
            mode,
        )
        .unwrap()
    }

    #[test]
    fn full_pipeline_debug_apk_no_flavor_asserts_exact_gradlew_invocation_and_artifacts() {
        let dir = unique_project_dir("full-debug-apk");
        let android_dir = dir.join("android");
        let out_dir = android_dir.join("app/build/outputs/apk/debug");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-debug.apk"), b"fake-apk-bytes").unwrap();

        let runner = preflight_ok_runner().with(
            "./gradlew assembleDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
            Output {
                success: true,
                stdout: "BUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );

        let target = AndroidArtifact::Apk {
            split_per_abi: false,
            abis: vec!["arm64-v8a".to_string()],
        };
        let build_info = info(BuildMode::Debug, None);
        let result = build_with_env(&runner, &dir, &build_info, &target, &fake_env()).unwrap();
        assert_eq!(result.paths, vec![out_dir.join("app-debug.apk")]);

        // local.properties merge-write happened as a side effect.
        let local_props = fs::read_to_string(android_dir.join("local.properties")).unwrap();
        assert!(local_props.contains("frust.versionName=1.0"));
        assert!(local_props.contains("frust.versionCode=1"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_leftover_artifact_errors_instead_of_being_reported_as_built() {
        // Followup F2: AGP writes every APK shape into the same output
        // directory, so a leftover from a previous, differently-shaped
        // build (e.g. a stale split APK) sitting next to the fresh fat APK
        // this build produced must be caught as an error, never silently
        // printed as "Built" alongside/instead of the real artifact.
        let dir = unique_project_dir("stale-leftover");
        let android_dir = dir.join("android");
        let out_dir = android_dir.join("app/build/outputs/apk/debug");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-debug.apk"), b"fresh-apk-bytes").unwrap();
        fs::write(
            out_dir.join("app-arm64-v8a-debug.apk"),
            b"stale-leftover-from-a-prior-split-build",
        )
        .unwrap();

        let runner = preflight_ok_runner().with(
            "./gradlew assembleDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
            Output {
                success: true,
                stdout: "BUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );

        let target = AndroidArtifact::Apk {
            split_per_abi: false,
            abis: vec!["arm64-v8a".to_string()],
        };
        let build_info = info(BuildMode::Debug, None);
        let err = build_with_env(&runner, &dir, &build_info, &target, &fake_env()).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("expected 1"), "{message}");
        assert!(message.contains("found 2"), "{message}");
        assert!(message.contains("frust clean"), "{message}");
        assert!(
            !message.contains("Built"),
            "error must not itself echo a stale-path 'Built' report: {message}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn release_apk_with_flavor_and_defines_asserts_exact_argv() {
        let dir = unique_project_dir("full-release-flavor-defines");
        let android_dir = dir.join("android");
        fs::write(android_dir.join("key.properties"), "keyAlias=upload\n").unwrap();
        let out_dir = android_dir.join("app/build/outputs/apk/paid/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-paid-release.apk"), b"fake").unwrap();

        let runner = preflight_ok_runner().with(
            "./gradlew assemblePaidRelease -Pfrust.targetPlatforms=arm64-v8a,x86_64 \
-Pfrust.splitPerAbi=false -Pfrust.defines=QT0xO0I9Mg==",
            Output {
                success: true,
                stdout: "BUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );

        let target = AndroidArtifact::Apk {
            split_per_abi: false,
            abis: vec!["arm64-v8a".to_string(), "x86_64".to_string()],
        };
        let build_args = BuildArgs {
            flavor: Some("paid".to_string()),
            release: true,
            defines: vec!["A=1".to_string(), "B=2".to_string()],
            ..BuildArgs::default()
        };
        let build_info = BuildInfo::from_args(build_args, BuildMode::Release).unwrap();

        let result = build_with_env(&runner, &dir, &build_info, &target, &fake_env()).unwrap();
        assert_eq!(result.paths, vec![out_dir.join("app-paid-release.apk")]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn release_without_key_properties_errs_before_invoking_gradle() {
        let dir = unique_project_dir("release-no-keystore");
        // No fixture registered for `./gradlew ...` at all: an unexpected
        // call would itself error, proving Gradle is never invoked.
        let runner = preflight_ok_runner();

        let target = AndroidArtifact::Apk {
            split_per_abi: false,
            abis: vec!["arm64-v8a".to_string()],
        };
        let build_info = info(BuildMode::Release, None);
        let err = build_with_env(&runner, &dir, &build_info, &target, &fake_env()).unwrap_err();
        assert!(err.to_string().contains("keytool"), "{err}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_flavor_gradle_failure_is_reexplained() {
        let dir = unique_project_dir("missing-flavor");
        let runner = preflight_ok_runner().with(
            "./gradlew assemblePaidRelease -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
            Output {
                success: false,
                stdout: String::new(),
                stderr: "FAILURE: Build failed with an exception.\n\n\
* What went wrong:\nTask 'assemblePaidRelease' not found in root project 'myapp'."
                    .to_string(),
            },
        );
        fs::write(dir.join("android/key.properties"), "keyAlias=upload\n").unwrap();

        let target = AndroidArtifact::Apk {
            split_per_abi: false,
            abis: vec!["arm64-v8a".to_string()],
        };
        let build_info = info(BuildMode::Release, Some("paid"));
        let err = build_with_env(&runner, &dir, &build_info, &target, &fake_env()).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("flavor 'paid'"), "{message}");
        assert!(message.contains("build.gradle.kts"), "{message}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn other_gradle_failure_passes_through_stderr_tail() {
        let dir = unique_project_dir("other-failure");
        let runner = preflight_ok_runner().with(
            "./gradlew assembleDebug -Pfrust.targetPlatforms=arm64-v8a -Pfrust.splitPerAbi=false",
            Output {
                success: false,
                stdout: String::new(),
                stderr: "e: compile error in MainActivity.kt".to_string(),
            },
        );

        let target = AndroidArtifact::Apk {
            split_per_abi: false,
            abis: vec!["arm64-v8a".to_string()],
        };
        let build_info = info(BuildMode::Debug, None);
        let err = build_with_env(&runner, &dir, &build_info, &target, &fake_env()).unwrap_err();
        assert!(
            err.to_string().contains("compile error in MainActivity.kt"),
            "{err}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn appbundle_build_asserts_bundle_task_and_all_abis() {
        let dir = unique_project_dir("appbundle");
        let android_dir = dir.join("android");
        let out_dir = android_dir.join("app/build/outputs/bundle/release");
        fs::create_dir_all(&out_dir).unwrap();
        fs::write(out_dir.join("app-release.aab"), b"fake").unwrap();

        let runner = preflight_ok_runner().with(
            "./gradlew bundleRelease -Pfrust.targetPlatforms=arm64-v8a,armeabi-v7a,x86_64",
            Output {
                success: true,
                stdout: "BUILD SUCCESSFUL".to_string(),
                stderr: String::new(),
            },
        );

        let build_info = info(BuildMode::Release, None);
        fs::write(android_dir.join("key.properties"), "keyAlias=upload\n").unwrap();
        let result = build_with_env(
            &runner,
            &dir,
            &build_info,
            &AndroidArtifact::Appbundle,
            &fake_env(),
        )
        .unwrap();
        assert_eq!(result.paths, vec![out_dir.join("app-release.aab")]);

        let _ = fs::remove_dir_all(&dir);
    }
}
