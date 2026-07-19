//! `frust build apk|appbundle|ios|ipa` (spec §12.5/12.6): validates the
//! full `BuildTarget` flag surface into a [`BuildInfo`] + platform artifact
//! enum, resolves the project root (mirrors `run`'s `frust.toml`
//! detection), and dispatches to the `android_build`/`ios_build` pipelines.

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::android_build::{self, AndroidArtifact};
use crate::android_run;
use crate::build_info::{BuildInfo, BuildMode};
use crate::cli::BuildTarget;
use crate::ios_build::{self, IosArtifact};
use crate::ios_run;
use crate::process::{ProcessRunner, RealProcessRunner};

/// `--target-platform` value -> Gradle ABI name (spec §12.5).
const TARGET_PLATFORMS: &[(&str, &str)] = &[
    ("android-arm64", "arm64-v8a"),
    ("android-arm", "armeabi-v7a"),
    ("android-x64", "x86_64"),
];

/// Valid `--export-method` values for `frust build ipa`.
const EXPORT_METHODS: &[&str] = &[
    "app-store-connect",
    "release-testing",
    "debugging",
    "enterprise",
];

/// Entry point `commands::dispatch` calls: resolves the real process runner
/// and current directory, then hands off to [`run_in`].
pub fn run(target: BuildTarget) -> Result<u8> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    run_in(&RealProcessRunner, &cwd, target)
}

/// The testable core of `build`, taking an injected [`ProcessRunner`] and
/// project directory so it can be exercised with a [`crate::process::FakeProcessRunner`]
/// and a tempdir rather than the real toolchain/cwd.
pub fn run_in(runner: &dyn ProcessRunner, project_dir: &Path, target: BuildTarget) -> Result<u8> {
    match target {
        BuildTarget::Apk {
            build,
            split_per_abi,
            target_platform,
        } => {
            let info = BuildInfo::from_args(build, BuildMode::Release)
                .map_err(|err| anyhow::anyhow!(err))?;
            let abis = resolve_abis(target_platform.as_deref())?;
            build_android(
                runner,
                project_dir,
                &info,
                AndroidArtifact::Apk {
                    split_per_abi,
                    abis,
                },
            )
        }
        BuildTarget::Appbundle {
            build,
            target_platform,
        } => {
            let info = BuildInfo::from_args(build, BuildMode::Release)
                .map_err(|err| anyhow::anyhow!(err))?;
            // Validated for a clear error even though `Appbundle` doesn't
            // carry the resolved ABI list itself (spec §12.5: Gradle's
            // bundle task packages every ABI in one `.aab`).
            resolve_abis(target_platform.as_deref())?;
            build_android(runner, project_dir, &info, AndroidArtifact::Appbundle)
        }
        BuildTarget::Ios {
            build,
            simulator,
            no_codesign,
        } => {
            require_macos_host("build ios")?;
            let info = BuildInfo::from_args(build, BuildMode::Release)
                .map_err(|err| anyhow::anyhow!(err))?;
            build_ios(
                runner,
                project_dir,
                &info,
                IosArtifact::App {
                    simulator,
                    codesign: !no_codesign,
                },
            )
        }
        BuildTarget::Ipa {
            build,
            export_method,
        } => {
            require_macos_host("build ipa")?;
            validate_export_method(&export_method)?;
            let info = BuildInfo::from_args(build, BuildMode::Release)
                .map_err(|err| anyhow::anyhow!(err))?;
            build_ios(
                runner,
                project_dir,
                &info,
                IosArtifact::Ipa { export_method },
            )
        }
    }
}

fn require_macos_host(command: &str) -> Result<()> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        bail!("`frust {command}` requires a macOS host (Xcode)")
    }
}

fn build_android(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    info: &BuildInfo,
    target: AndroidArtifact,
) -> Result<u8> {
    let project = android_run::project::detect(project_dir)?;
    android_run::project::require_android_dir(&project.root)?;
    println!(
        "Building `{}` — Gradle build type `{}`, cargo profile args {:?}…",
        project.app_id,
        info.mode.gradle_infix(),
        info.mode.cargo_profile_arg()
    );
    let artifacts = android_build::build(runner, project_dir, info, &target)?;
    print_artifacts(&artifacts.paths);
    Ok(0)
}

fn build_ios(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    info: &BuildInfo,
    target: IosArtifact,
) -> Result<u8> {
    let project = ios_run::project::detect(project_dir)?;
    ios_run::project::require_ios_dir(&project.root)?;
    println!(
        "Building `{}` — Xcode configuration `{}`, cargo profile args {:?}…",
        project.bundle_id,
        info.mode.xcode_configuration(),
        info.mode.cargo_profile_arg()
    );
    let artifacts = ios_build::build(runner, project_dir, info, &target)?;
    print_artifacts(&artifacts.paths);
    Ok(0)
}

fn print_artifacts(paths: &[std::path::PathBuf]) {
    for path in paths {
        println!("Built: {}", path.display());
    }
}

/// Maps `--target-platform`'s comma-separated `android-*` values to Gradle
/// ABI names (spec §12.5); `None` defaults to all three.
fn resolve_abis(target_platform: Option<&str>) -> Result<Vec<String>> {
    let Some(csv) = target_platform else {
        return Ok(TARGET_PLATFORMS
            .iter()
            .map(|(_, abi)| abi.to_string())
            .collect());
    };

    let mut abis = Vec::new();
    for token in csv.split(',') {
        let token = token.trim();
        match TARGET_PLATFORMS.iter().find(|(name, _)| *name == token) {
            Some((_, abi)) => abis.push((*abi).to_string()),
            None => bail!(
                "invalid --target-platform value '{token}'; valid values: {}",
                TARGET_PLATFORMS
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
    Ok(abis)
}

fn validate_export_method(method: &str) -> Result<()> {
    if EXPORT_METHODS.contains(&method) {
        Ok(())
    } else {
        bail!(
            "invalid --export-method '{method}'; valid values: {}",
            EXPORT_METHODS.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_info::BuildArgs;
    use crate::process::FakeProcessRunner;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_project_dir(tag: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-build-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("frust.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n",
        )
        .unwrap();
        dir
    }

    fn android_project_dir(tag: &str) -> std::path::PathBuf {
        let dir = unique_project_dir(tag);
        fs::create_dir_all(dir.join("android")).unwrap();
        fs::write(dir.join("android/gradlew"), "#!/bin/sh\n").unwrap();
        dir
    }

    fn ios_project_dir(tag: &str) -> std::path::PathBuf {
        let dir = unique_project_dir(tag);
        fs::create_dir_all(dir.join("ios/Runner.xcodeproj")).unwrap();
        dir
    }

    #[test]
    fn resolve_abis_defaults_to_all_three() {
        let abis = resolve_abis(None).unwrap();
        assert_eq!(abis, vec!["arm64-v8a", "armeabi-v7a", "x86_64"]);
    }

    #[test]
    fn resolve_abis_maps_requested_platforms() {
        let abis = resolve_abis(Some("android-arm64,android-x64")).unwrap();
        assert_eq!(abis, vec!["arm64-v8a", "x86_64"]);
    }

    #[test]
    fn resolve_abis_rejects_unknown_value_listing_valid_ones() {
        let err = resolve_abis(Some("android-arm64,bogus")).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("bogus"), "{message}");
        assert!(message.contains("android-arm64"), "{message}");
        assert!(message.contains("android-arm"), "{message}");
        assert!(message.contains("android-x64"), "{message}");
    }

    #[test]
    fn validate_export_method_accepts_every_valid_value() {
        for method in EXPORT_METHODS {
            validate_export_method(method).unwrap();
        }
    }

    #[test]
    fn validate_export_method_rejects_bogus_value_listing_valid_ones() {
        let err = validate_export_method("bogus").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("bogus"), "{message}");
        for method in EXPORT_METHODS {
            assert!(message.contains(method), "{message}");
        }
    }

    #[test]
    fn build_apk_reaches_the_android_build_pipeline() {
        // `android_build::build` (task 64) is real now, not a stub — with no
        // `ProcessRunner` fixtures registered at all, the pipeline reaches
        // (and fails at) its own preflight, proving `run_in` dispatches into
        // it rather than stopping at project detection.
        let dir = android_project_dir("apk-stub");
        let runner = FakeProcessRunner::new();
        let target = BuildTarget::Apk {
            build: BuildArgs::default(),
            split_per_abi: false,
            target_platform: None,
        };
        let err = run_in(&runner, &dir, target).unwrap_err();
        assert!(
            err.to_string()
                .contains("rustup target add aarch64-linux-android"),
            "{err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_apk_without_android_dir_errs_before_reaching_the_stub() {
        let dir = unique_project_dir("apk-no-android-dir");
        let runner = FakeProcessRunner::new();
        let target = BuildTarget::Apk {
            build: BuildArgs::default(),
            split_per_abi: false,
            target_platform: None,
        };
        let err = run_in(&runner, &dir, target).unwrap_err();
        assert!(err.to_string().contains("android/"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_apk_rejects_conflicting_mode_flags() {
        let dir = android_project_dir("apk-conflicting-modes");
        let runner = FakeProcessRunner::new();
        let target = BuildTarget::Apk {
            build: BuildArgs {
                debug: true,
                release: true,
                ..Default::default()
            },
            split_per_abi: false,
            target_platform: None,
        };
        let err = run_in(&runner, &dir, target).unwrap_err();
        assert!(err.to_string().contains("mutually exclusive"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_apk_rejects_zero_build_number() {
        let dir = android_project_dir("apk-bad-build-number");
        let runner = FakeProcessRunner::new();
        let target = BuildTarget::Apk {
            build: BuildArgs {
                build_number: Some(0),
                ..Default::default()
            },
            split_per_abi: false,
            target_platform: None,
        };
        let err = run_in(&runner, &dir, target).unwrap_err();
        assert!(err.to_string().contains(">= 1"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_appbundle_reaches_the_android_build_pipeline() {
        let dir = android_project_dir("aab-stub");
        let runner = FakeProcessRunner::new();
        let target = BuildTarget::Appbundle {
            build: BuildArgs::default(),
            target_platform: None,
        };
        let err = run_in(&runner, &dir, target).unwrap_err();
        assert!(
            err.to_string()
                .contains("rustup target add aarch64-linux-android"),
            "{err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg_attr(
        not(target_os = "macos"),
        ignore = "asserts export-method validation order past the macOS host gate; on other \
                  hosts the gate fires first and the error is the host rejection instead"
    )]
    fn build_ipa_rejects_bad_export_method_before_touching_the_project() {
        // No `frust.toml`/`ios/` fixtures at all: proves export-method
        // validation runs before project detection.
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-build-test-ipa-bad-method-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let runner = FakeProcessRunner::new();
        let target = BuildTarget::Ipa {
            build: BuildArgs::default(),
            export_method: "bogus".to_string(),
        };
        let err = run_in(&runner, &dir, target).unwrap_err();
        assert!(err.to_string().contains("bogus"), "{err}");
    }

    #[test]
    #[cfg_attr(
        not(target_os = "macos"),
        ignore = "asserts the macOS-only gate is bypassed on macOS; on other hosts see \
                  `build_ios_rejected_on_non_macos_host`"
    )]
    fn build_ios_reaches_the_ios_pipeline_on_macos() {
        // With the macOS gate bypassed and an empty runner, dispatch reaches
        // `ios_build`'s pipeline, which fails at its first preflight check
        // (Xcode presence) rather than at the removed task-65 stub.
        let dir = ios_project_dir("ios-pipeline");
        let runner = FakeProcessRunner::new();
        let target = BuildTarget::Ios {
            build: BuildArgs::default(),
            simulator: true,
            no_codesign: true,
        };
        let err = run_in(&runner, &dir, target).unwrap_err();
        assert!(err.to_string().contains("Xcode not found"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "asserts the macOS-only gate rejects a non-macOS host; on macOS see \
                  `build_ios_reaches_the_ios_pipeline_on_macos`"
    )]
    fn build_ios_rejected_on_non_macos_host() {
        let dir = ios_project_dir("ios-non-macos");
        let runner = FakeProcessRunner::new();
        let target = BuildTarget::Ios {
            build: BuildArgs::default(),
            simulator: true,
            no_codesign: true,
        };
        let err = run_in(&runner, &dir, target).unwrap_err();
        assert!(err.to_string().contains("macOS host"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }
}
