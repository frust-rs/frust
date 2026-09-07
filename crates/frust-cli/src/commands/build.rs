//! `frust build apk|appbundle|ios|ipa|macos|windows|linux|web`: validates the
//! full `BuildTarget` flag surface into a [`BuildInfo`] + platform artifact
//! enum, resolves the project root (mirrors `run`'s `frust.toml`
//! detection), and dispatches to the `android_build`/`ios_build`/
//! `desktop_build`/`web_build` pipelines.

use std::path::Path;

use anyhow::{Result, bail};

use crate::cli::{BuildFlags, BuildTarget, validate_feature_token_charset};
use frust_drive::android_build::{self, AndroidArtifact};
use frust_drive::android_run;
use frust_drive::build_info::{BuildInfo, BuildMode};
use frust_drive::cargo_manifest;
use frust_drive::desktop_build::{
    self, BundleReport, DesktopBundleTarget, InstallerFormat, InstallerReport,
};
use frust_drive::ios_build::{self, IosArtifact};
use frust_drive::ios_run;
use frust_drive::process::ProcessRunner;
use frust_drive::web_build::{self, WebBuildReport};

/// `--target-platform` value -> Gradle ABI name.
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

/// The testable core of `build`, taking an injected [`ProcessRunner`] and
/// project directory so it can be exercised with a
/// [`frust_drive::process::FakeProcessRunner`] and a tempdir rather than the
/// real toolchain/cwd. `commands::dispatch` constructs the real runner and
/// current directory and calls this (the CLI's one `Real` construction
/// site).
pub fn run_in(runner: &dyn ProcessRunner, project_dir: &Path, target: BuildTarget) -> Result<u8> {
    match target {
        BuildTarget::Apk {
            build,
            split_per_abi,
            target_platform,
        } => {
            let extra_features = build.extra_features();
            validate_extra_features("build apk", &extra_features)?;
            refuse_devtools_features("build apk", project_dir, &extra_features)?;
            let info = BuildInfo::from_args(build.build.into_drive(), BuildMode::Release)
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
                &extra_features,
            )
        }
        BuildTarget::Appbundle {
            build,
            target_platform,
        } => {
            let extra_features = build.extra_features();
            validate_extra_features("build appbundle", &extra_features)?;
            refuse_devtools_features("build appbundle", project_dir, &extra_features)?;
            let info = BuildInfo::from_args(build.build.into_drive(), BuildMode::Release)
                .map_err(|err| anyhow::anyhow!(err))?;
            // Validated for a clear error even though `Appbundle` doesn't
            // carry the resolved ABI list itself (Gradle's bundle task
            // packages every ABI in one `.aab`).
            resolve_abis(target_platform.as_deref())?;
            build_android(
                runner,
                project_dir,
                &info,
                AndroidArtifact::Appbundle,
                &extra_features,
            )
        }
        BuildTarget::Ios {
            build,
            simulator,
            no_codesign,
        } => {
            require_macos_host("build ios")?;
            let extra_features = build.extra_features();
            validate_extra_features("build ios", &extra_features)?;
            refuse_devtools_features("build ios", project_dir, &extra_features)?;
            let info = BuildInfo::from_args(build.build.into_drive(), BuildMode::Release)
                .map_err(|err| anyhow::anyhow!(err))?;
            build_ios(
                runner,
                project_dir,
                &info,
                IosArtifact::App {
                    simulator,
                    codesign: !no_codesign,
                },
                &extra_features,
            )
        }
        BuildTarget::Ipa {
            build,
            export_method,
        } => {
            require_macos_host("build ipa")?;
            validate_export_method(&export_method)?;
            let extra_features = build.extra_features();
            validate_extra_features("build ipa", &extra_features)?;
            refuse_devtools_features("build ipa", project_dir, &extra_features)?;
            let info = BuildInfo::from_args(build.build.into_drive(), BuildMode::Release)
                .map_err(|err| anyhow::anyhow!(err))?;
            build_ios(
                runner,
                project_dir,
                &info,
                IosArtifact::Ipa { export_method },
                &extra_features,
            )
        }
        BuildTarget::Macos { build, installer } => {
            reject_unplumbed_features("build macos", &build)?;
            let info = BuildInfo::from_args(build.build.into_drive(), BuildMode::Release)
                .map_err(|err| anyhow::anyhow!(err))?;
            build_desktop(
                runner,
                project_dir,
                &info,
                DesktopBundleTarget::Macos,
                installer,
            )
        }
        BuildTarget::Windows { build, installer } => {
            reject_unplumbed_features("build windows", &build)?;
            let info = BuildInfo::from_args(build.build.into_drive(), BuildMode::Release)
                .map_err(|err| anyhow::anyhow!(err))?;
            build_desktop(
                runner,
                project_dir,
                &info,
                DesktopBundleTarget::Windows,
                installer,
            )
        }
        BuildTarget::Linux { build, installer } => {
            reject_unplumbed_features("build linux", &build)?;
            let info = BuildInfo::from_args(build.build.into_drive(), BuildMode::Release)
                .map_err(|err| anyhow::anyhow!(err))?;
            build_desktop(
                runner,
                project_dir,
                &info,
                DesktopBundleTarget::Linux,
                installer,
            )
        }
        BuildTarget::Web { build } => {
            reject_unplumbed_features("build web", &build)?;
            let info = BuildInfo::from_args(build.build.into_drive(), BuildMode::Release)
                .map_err(|err| anyhow::anyhow!(err))?;
            build_web(runner, project_dir, &info)
        }
    }
}

/// Refuses `--features` on the three desktop-bundle targets, which are the
/// one build lane the passthrough does not reach: `desktop_build::build`'s
/// entry point carries no parameter for it, so the features would be silently
/// dropped and the bundle would be compiled without the very feature the
/// caller asked for — a wrong artifact that looks like a right one. Refusing
/// up front, before `frust.toml` is read or anything is compiled, is the same
/// stance `--watch` + `-d` takes on a combination the pipeline cannot honour.
fn reject_unplumbed_features(command: &str, build: &BuildFlags) -> Result<()> {
    let requested = build.extra_features();
    if requested.is_empty() {
        return Ok(());
    }
    bail!(
        "`frust {command}` does not support --features yet (requested: {}); \
         it is plumbed for `build apk`, `build appbundle`, `build ios`, `build ipa` and `run`",
        requested.join(", ")
    )
}

/// Rejects any `--features` passthrough token outside the strict charset
/// [`crate::cli::feature_token_charset_ok`] requires, naming the offending
/// token — see that function's doc comment for why a validated-charset
/// token still matters this far downstream. Runs on all four release lanes
/// (`build apk`/`appbundle`/`ios`/`ipa`) before any of them does anything
/// else with the tokens; `frust run` is unaffected (see `BuildFlags`'s own
/// doc comment on `features`).
fn validate_extra_features(command: &str, extra_features: &[String]) -> Result<()> {
    if let Err(bad) = validate_feature_token_charset(extra_features) {
        bail!(
            "`frust {command}` rejects --features token '{bad}': expected cargo's own \
             feature-name charset ([A-Za-z0-9_.-]+), optionally prefixed by '<pkg>/'"
        );
    }
    Ok(())
}

/// Whether `token` is one of the literal spellings that enable the in-app
/// devtools listener: the bare name, its `frust`-qualified form, or any
/// `<pkg>/devtools` spelling another crate might expose the same switch
/// under.
fn is_devtools_token(token: &str) -> bool {
    token == "devtools" || token == "frust/devtools" || token.ends_with("/devtools")
}

/// Refuses a `--features` token that would compile the in-app devtools
/// listener into a release-signed store artifact. `frust build`'s release
/// lanes (`apk`/`appbundle`/`ios`/`ipa`) forward `--features` straight
/// through `BuildMode::cargo_features`'s own devtools-off release
/// selection, so an explicit `--features frust/devtools` (or an app-defined
/// feature alias that itself enables it) would silently reopen the hole
/// `docs/DEVTOOLS_ARCHITECTURE.md`'s Trust model relies on release builds
/// closing ("a release build compiles the listener out entirely"). Checked
/// both literally ([`is_devtools_token`]) and — cheaply, one level, no
/// transitive feature-graph walk — against the app's own `[features]`
/// table (`cargo_manifest::feature_enables_devtools`), so an app-defined
/// alias for the switch is caught under its own name too. Mirrors
/// [`reject_unplumbed_features`]'s stance: refuse before any work, naming
/// every offending token.
fn refuse_devtools_features(
    command: &str,
    project_dir: &Path,
    extra_features: &[String],
) -> Result<()> {
    let offending: Vec<&str> = extra_features
        .iter()
        .map(String::as_str)
        .filter(|&token| {
            is_devtools_token(token) || cargo_manifest::feature_enables_devtools(project_dir, token)
        })
        .collect();
    if offending.is_empty() {
        return Ok(());
    }
    bail!(
        "`frust {command}` refuses --features that would enable the devtools listener on a \
         release artifact ({}); the in-app devtools RPC service must stay compiled out of a \
         store-distributed build (see docs/DEVTOOLS_ARCHITECTURE.md's Trust model)",
        offending.join(", ")
    )
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
    extra_features: &[String],
) -> Result<u8> {
    let project = android_run::project::detect(project_dir)?;
    android_run::project::require_android_dir(&project.root)?;
    println!(
        "Building `{}` — Gradle build type `{}`, cargo profile args {:?}…",
        project.app_id,
        info.mode.gradle_infix(),
        info.mode.cargo_profile_arg()
    );
    // The drive core is print-free (its `on_line` sink was added by the
    // tty-garbling fix); the CLI restores today's behavior by `println!`ing
    // each streamed line verbatim.
    let artifacts = android_build::build(
        runner,
        project_dir,
        info,
        &target,
        extra_features,
        &mut |line| println!("{line}"),
    )?;
    print_artifacts(&artifacts.paths);
    Ok(0)
}

fn build_ios(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    info: &BuildInfo,
    target: IosArtifact,
    extra_features: &[String],
) -> Result<u8> {
    let project = ios_run::project::detect(project_dir)?;
    ios_run::project::require_ios_dir(&project.root)?;
    println!(
        "Building `{}` — Xcode configuration `{}`, cargo profile args {:?}…",
        project.bundle_id,
        info.mode.xcode_configuration(),
        info.mode.cargo_profile_arg()
    );
    // Print-free drive core (see `build_android` above); the CLI `println!`s
    // each streamed line to keep its stdout verbatim.
    let artifacts = ios_build::build(
        runner,
        project_dir,
        info,
        &target,
        extra_features,
        &mut |line| println!("{line}"),
    )?;
    print_artifacts(&artifacts.paths);
    Ok(0)
}

/// Drives [`desktop_build::build`] (plus [`desktop_build::build_installer`]
/// once per format for `target`, when `installer` is set) and renders the
/// result. The host-lock check happens inside `desktop_build::build` itself,
/// as the very first thing it does — before `frust.toml` is even read — so
/// a foreign-host invocation is refused before any work, with the typed
/// [`frust_drive::desktop_build::DesktopBuildError::HostMismatch`] message
/// naming the required OS (the desktop mirror of `build ios`'s "macOS host
/// only" refusal).
fn build_desktop(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    info: &BuildInfo,
    target: DesktopBundleTarget,
    installer: bool,
) -> Result<u8> {
    // Print-free drive core (see `build_android`/`build_ios` above); the CLI
    // `println!`s each streamed line to keep its stdout verbatim.
    let report = desktop_build::build(runner, project_dir, info, target, &mut |line| {
        println!("{line}")
    })?;
    print_bundle_report(&report);

    if installer {
        for format in InstallerFormat::for_target(target) {
            let installer_report = desktop_build::build_installer(
                runner,
                project_dir,
                info,
                &report,
                *format,
                &mut |line| println!("{line}"),
            )?;
            print_installer_report(&installer_report);
        }
    }

    Ok(0)
}

/// Drives [`web_build::build`] and renders the resulting artifact
/// directory. No host lock (unlike `build_desktop`) — every desktop OS can
/// cross-compile to `wasm32-unknown-unknown`. Print-free drive core (see
/// `build_android`/`build_ios`/`build_desktop` above); the CLI `println!`s
/// each streamed line to keep its stdout verbatim.
fn build_web(runner: &dyn ProcessRunner, project_dir: &Path, info: &BuildInfo) -> Result<u8> {
    let report = web_build::build(runner, project_dir, info, &mut |line| println!("{line}"))?;
    print_web_report(&report);
    Ok(0)
}

fn print_web_report(report: &WebBuildReport) {
    println!("Bundle: {}", report.root.display());
    for artifact in &report.artifacts {
        println!("Built: {}", artifact.display());
    }
    for note in &report.notes {
        println!("Note: {note}");
    }
}

fn print_artifacts(paths: &[std::path::PathBuf]) {
    for path in paths {
        println!("Built: {}", path.display());
    }
}

fn print_bundle_report(report: &BundleReport) {
    println!("Bundle: {}", report.root.display());
    for artifact in &report.artifacts {
        println!("Built: {}", artifact.display());
    }
    for note in &report.notes {
        println!("Note: {note}");
    }
}

fn print_installer_report(report: &InstallerReport) {
    if report.artifacts.is_empty() {
        println!(
            "Installer ({}): no artifact found in {}",
            report.format,
            report.out_dir.display()
        );
    }
    for artifact in &report.artifacts {
        println!("Built ({}): {}", report.format, artifact.display());
    }
    for note in &report.notes {
        println!("Note ({}): {note}", report.format);
    }
}

/// Maps `--target-platform`'s comma-separated `android-*` values to Gradle
/// ABI names; `None` defaults to all three.
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
    use crate::build_args::BuildArgs;
    use crate::cli::BuildFlags;
    use frust_drive::process::{FakeProcessRunner, Output};
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
        // `android_build::build` is real, not a stub — with no
        // `ProcessRunner` fixtures registered at all, the pipeline reaches
        // (and fails at) its own preflight, proving `run_in` dispatches into
        // it rather than stopping at project detection.
        let dir = android_project_dir("apk-stub");
        let runner = FakeProcessRunner::new();
        let target = BuildTarget::Apk {
            build: BuildFlags::default(),
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
            build: BuildFlags::default(),
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
            build: BuildFlags {
                build: BuildArgs {
                    debug: true,
                    release: true,
                    ..Default::default()
                },
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
            build: BuildFlags {
                build: BuildArgs {
                    build_number: Some(0),
                    ..Default::default()
                },
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
            build: BuildFlags::default(),
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
            build: BuildFlags::default(),
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
        // (Xcode presence) — the pipeline is real now, not a stub.
        let dir = ios_project_dir("ios-pipeline");
        let runner = FakeProcessRunner::new();
        let target = BuildTarget::Ios {
            build: BuildFlags::default(),
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
            build: BuildFlags::default(),
            simulator: true,
            no_codesign: true,
        };
        let err = run_in(&runner, &dir, target).unwrap_err();
        assert!(err.to_string().contains("macOS host"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// `BuildTarget::{Macos,Windows,Linux}` for this host's own target reach
    /// the real `desktop_build::build` pipeline and default to release mode:
    /// the fake runner only has the *release* cargo invocation registered
    /// (`RELEASE_BUILD`, matching `desktop_build`'s own tests) — a debug
    /// default would hit that invocation's `--features frust/perf-trace
    /// --features frust/devtools` sibling instead, an unregistered call the
    /// fake runner would reject outright. No `--debug`/`--profile`/`--release`
    /// flag is passed, so a `BuildInfo` default other than
    /// `BuildMode::Release` would fail this test.
    const RELEASE_BUILD: &str = "cargo build --release --features lean";

    fn desktop_target_for(host: DesktopBundleTarget) -> BuildTarget {
        match host {
            DesktopBundleTarget::Macos => BuildTarget::Macos {
                build: BuildFlags::default(),
                installer: false,
            },
            DesktopBundleTarget::Windows => BuildTarget::Windows {
                build: BuildFlags::default(),
                installer: false,
            },
            DesktopBundleTarget::Linux => BuildTarget::Linux {
                build: BuildFlags::default(),
                installer: false,
            },
        }
    }

    #[test]
    fn build_desktop_target_matching_this_host_reaches_the_pipeline_and_defaults_to_release() {
        let Some(host) = DesktopBundleTarget::host() else {
            eprintln!("no desktop bundle target for this OS — skipping");
            return;
        };
        let dir = unique_project_dir("desktop-own-host");
        let runner = FakeProcessRunner::new().with(
            RELEASE_BUILD,
            Output {
                success: true,
                stdout: "    Finished `release` profile [optimized]".to_string(),
                stderr: String::new(),
            },
        );
        let err = run_in(&runner, &dir, desktop_target_for(host)).unwrap_err();
        // A green compile with nothing at the expected binary path is
        // `DesktopBuildError::BinaryNotFound` — proof the release
        // invocation above was the one actually run.
        assert!(err.to_string().contains("no `release`"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// `--features` is refused on the three desktop-bundle targets rather
    /// than accepted and dropped: `desktop_build::build` has no parameter to
    /// carry it, and a bundle compiled WITHOUT a feature the caller asked for
    /// is a wrong artifact that looks like a right one. The runner holds no
    /// registrations at all, so a `cargo` invocation would itself error — the
    /// refusal message proves the check runs before any work.
    #[test]
    fn build_desktop_rejects_the_features_passthrough_before_any_work() {
        let Some(host) = DesktopBundleTarget::host() else {
            eprintln!("no desktop bundle target for this OS — skipping");
            return;
        };
        let dir = unique_project_dir("desktop-features-rejected");
        let runner = FakeProcessRunner::new();
        let flags = BuildFlags {
            features: vec!["perf-trace".to_string()],
            ..Default::default()
        };
        let target = match host {
            DesktopBundleTarget::Macos => BuildTarget::Macos {
                build: flags,
                installer: false,
            },
            DesktopBundleTarget::Windows => BuildTarget::Windows {
                build: flags,
                installer: false,
            },
            DesktopBundleTarget::Linux => BuildTarget::Linux {
                build: flags,
                installer: false,
            },
        };
        let err = run_in(&runner, &dir, target).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("--features"), "{message}");
        assert!(message.contains("perf-trace"), "{message}");
        assert!(message.contains("build apk"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The passthrough reaches the Android pipeline: `build apk --features`
    /// gets past dispatch into `android_build`'s own preflight (the same
    /// "reaches the pipeline" proof the flagless case above makes), so the
    /// flag is threaded rather than rejected or ignored at the CLI boundary.
    #[test]
    fn build_apk_with_features_reaches_the_android_build_pipeline() {
        let dir = android_project_dir("apk-features");
        let runner = FakeProcessRunner::new();
        let target = BuildTarget::Apk {
            build: BuildFlags {
                features: vec!["perf-trace".to_string()],
                ..Default::default()
            },
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

    /// `build appbundle --features frust/devtools` — the exact release
    /// artifact the finding names (a store-bound `.aab`) — is refused
    /// outright, before Gradle/`android_build` ever runs (the fake runner
    /// holds no registrations at all, so a real invocation would itself
    /// error with something unrelated).
    #[test]
    fn build_appbundle_refuses_frust_devtools_feature() {
        let dir = android_project_dir("appbundle-devtools-refused");
        let runner = FakeProcessRunner::new();
        let target = BuildTarget::Appbundle {
            build: BuildFlags {
                features: vec!["frust/devtools".to_string()],
                ..Default::default()
            },
            target_platform: None,
        };
        let err = run_in(&runner, &dir, target).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("devtools"), "{message}");
        assert!(message.contains("frust/devtools"), "{message}");
        assert!(message.contains("build appbundle"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The bare `devtools` spelling and any `<pkg>/devtools` spelling are
    /// refused the same way as the fully-qualified `frust/devtools` above.
    #[test]
    fn build_apk_refuses_bare_and_pkg_qualified_devtools_spellings() {
        for token in ["devtools", "some-plugin/devtools"] {
            let dir = android_project_dir("apk-devtools-spelling");
            let runner = FakeProcessRunner::new();
            let target = BuildTarget::Apk {
                build: BuildFlags {
                    features: vec![token.to_string()],
                    ..Default::default()
                },
                split_per_abi: false,
                target_platform: None,
            };
            let err = run_in(&runner, &dir, target).unwrap_err();
            let message = err.to_string();
            assert!(message.contains("devtools"), "token {token}: {message}");
            let _ = fs::remove_dir_all(&dir);
        }
    }

    /// An app-defined feature alias whose own `[features]` entry names
    /// `frust/devtools` is refused under its own name too — the "cheap,
    /// one-level" app-manifest check alongside the literal-spelling ones.
    #[test]
    fn build_apk_refuses_app_defined_alias_that_enables_devtools() {
        let dir = android_project_dir("apk-devtools-alias");
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"app\"\n\n[features]\nfull = [\"frust/devtools\"]\n",
        )
        .unwrap();
        let runner = FakeProcessRunner::new();
        let target = BuildTarget::Apk {
            build: BuildFlags {
                features: vec!["full".to_string()],
                ..Default::default()
            },
            split_per_abi: false,
            target_platform: None,
        };
        let err = run_in(&runner, &dir, target).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("devtools"), "{message}");
        assert!(message.contains("full"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A passthrough token carrying a shell metacharacter is rejected before
    /// any work, naming the offending token — this is what stands between an
    /// unvalidated CSV and the iOS release lane's unquoted `cargo build ...
    /// $CARGO_FEATURES` shell expansion.
    #[test]
    fn build_apk_rejects_features_token_with_shell_metacharacters() {
        let dir = android_project_dir("apk-shell-metacharacters");
        let runner = FakeProcessRunner::new();
        let target = BuildTarget::Apk {
            build: BuildFlags {
                features: vec!["evil;touch /tmp/pwned".to_string()],
                ..Default::default()
            },
            split_per_abi: false,
            target_platform: None,
        };
        let err = run_in(&runner, &dir, target).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("evil;touch"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A desktop target foreign to this host is refused before any work —
    /// the fake runner holds no registrations at all, so a `cargo`
    /// invocation would itself be an "unregistered call" error; the actual
    /// error is the typed host-lock message instead, proving the refusal
    /// happens first.
    #[test]
    fn build_desktop_target_foreign_to_this_host_is_host_locked_before_any_work() {
        let Some(host) = DesktopBundleTarget::host() else {
            eprintln!("no desktop bundle target for this OS — skipping");
            return;
        };
        let foreign = match host {
            DesktopBundleTarget::Macos => DesktopBundleTarget::Windows,
            DesktopBundleTarget::Windows => DesktopBundleTarget::Linux,
            DesktopBundleTarget::Linux => DesktopBundleTarget::Macos,
        };
        let dir = unique_project_dir("desktop-foreign-host");
        let runner = FakeProcessRunner::new();
        let err = run_in(&runner, &dir, desktop_target_for(foreign)).unwrap_err();
        assert!(err.to_string().contains("host-locked"), "{err}");
        assert!(err.to_string().contains(foreign.as_str()), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// `--installer` without `cargo-packager` on `PATH` surfaces the typed
    /// tool-missing error, doctor hint included — after a real bundle
    /// assembly (the fake runner's only registration is the release
    /// compile), never a bundle-assembly failure masking it.
    #[test]
    fn build_installer_flag_surfaces_a_typed_tool_missing_error() {
        let Some(host) = DesktopBundleTarget::host() else {
            eprintln!("no desktop bundle target for this OS — skipping");
            return;
        };
        let dir = unique_project_dir("desktop-installer-tool-missing");
        let binary_name = if host == DesktopBundleTarget::Windows {
            "myapp.exe"
        } else {
            "myapp"
        };
        fs::create_dir_all(dir.join("target/release")).unwrap();
        fs::write(
            dir.join("target/release").join(binary_name),
            b"#!/bin/sh\ntrue\n",
        )
        .unwrap();

        let runner = FakeProcessRunner::new()
            .with(
                RELEASE_BUILD,
                Output {
                    success: true,
                    stdout: "    Finished `release` profile [optimized]".to_string(),
                    stderr: String::new(),
                },
            )
            .missing("cargo packager --version");

        let target = match host {
            DesktopBundleTarget::Macos => BuildTarget::Macos {
                build: BuildFlags::default(),
                installer: true,
            },
            DesktopBundleTarget::Windows => BuildTarget::Windows {
                build: BuildFlags::default(),
                installer: true,
            },
            DesktopBundleTarget::Linux => BuildTarget::Linux {
                build: BuildFlags::default(),
                installer: true,
            },
        };
        let err = run_in(&runner, &dir, target).unwrap_err();
        assert!(
            err.to_string().contains("cargo install cargo-packager"),
            "{err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// `web_build::build` is real, not a stub — the fixture project carries a
    /// `frust.toml` (from `unique_project_dir`) but no `Cargo.toml`, so
    /// dispatch reaches (and fails at) the pipeline's own package-name read,
    /// proving `run_in` dispatches into it rather than stopping earlier.
    #[test]
    fn build_web_reaches_the_web_build_pipeline() {
        let dir = unique_project_dir("web-stub");
        let runner = FakeProcessRunner::new();
        let target = BuildTarget::Web {
            build: BuildFlags::default(),
        };
        let err = run_in(&runner, &dir, target).unwrap_err();
        assert!(err.to_string().contains("Cargo.toml"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// `--features` is refused outright before anything is read from disk —
    /// `web_build::build`'s entry point carries no parameter for it, the same
    /// reason `build macos|windows|linux` refuse it
    /// ([`reject_unplumbed_features`]'s own doc comment).
    #[test]
    fn build_web_rejects_features_passthrough() {
        let dir = unique_project_dir("web-features-rejected");
        let runner = FakeProcessRunner::new();
        let target = BuildTarget::Web {
            build: BuildFlags {
                features: vec!["devtools".to_string()],
                ..Default::default()
            },
        };
        let err = run_in(&runner, &dir, target).unwrap_err();
        assert!(
            err.to_string().contains("does not support --features"),
            "{err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
