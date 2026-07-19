//! iOS drive pipelines for `forgekit run`: [`run`] drives the Simulator
//! (preflight → xcodebuild → `simctl install` → `simctl launch --console-pty`,
//! streamed) and [`run_physical`] drives a signed physical device (iOS 17+
//! gate → `ios_build`'s signed device build → `devicectl device install app`
//! → `devicectl device process launch --console --terminate-existing`,
//! streamed — task 67). Both mirror `android_run`'s structure and testing
//! style.

pub mod devicectl;
pub mod preflight;
pub mod project;
pub mod simctl;
pub mod xcodebuild;

use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail};

use crate::build_info::BuildInfo;
use crate::devices::Device;
use crate::ios_build::{self, IosArtifact};
use crate::process::{ProcessRunner, RealProcessRunner, tail_lines};

/// Drives the full iOS simulator pipeline (preflight → xcodebuild →
/// `simctl install` → `simctl launch`) on `device`, an already-discovered
/// (and therefore already-booted) `Platform::Ios`/`Kind::Simulator` device,
/// against the ForgeKit project rooted at `root`. `info.mode` (task 66)
/// selects the `-configuration` xcodebuild builds and the matching
/// `<config>-iphonesimulator` products directory the app bundle is installed
/// from; flavor/defines/version aren't threaded here (unlike the Android
/// pipeline) — the iOS simulator run path stays scheme-fixed (`Runner`).
pub fn run(
    runner: &dyn ProcessRunner,
    root: &Path,
    device: &Device,
    info: &BuildInfo,
) -> Result<u8> {
    let project = project::detect(root)?;
    project::require_ios_dir(&project.root)?;

    let preflight_ctx = preflight::PreflightCtx {
        runner,
        udid: &device.id,
    };
    preflight::run(&preflight_ctx).map_err(|err| anyhow::anyhow!(err))?;

    let configuration = info.mode.xcode_configuration();

    println!("Building `{}`…", project.bundle_id);
    let build_start = Instant::now();
    let mut on_xcodebuild_line = |line: &str| println!("{line}");
    let build_out = xcodebuild::build(
        runner,
        &project.root,
        &device.id,
        configuration,
        &mut on_xcodebuild_line,
    )?;
    if !build_out.success {
        bail!("{}", xcodebuild_failure_message(&build_out));
    }
    println!(
        "Build finished in {:.1}s.",
        build_start.elapsed().as_secs_f32()
    );

    let app_path = xcodebuild::app_bundle_path(&project.root, configuration);
    if !app_path.exists() {
        bail!(
            "app bundle not found at `{}` after `xcodebuild build`",
            app_path.display()
        );
    }
    let app_path = app_path.to_string_lossy().into_owned();

    println!("Installing on {}…", device.name);
    let install_out = simctl::install(runner, &device.id, &app_path)?;
    if !install_out.success {
        bail!(
            "`xcrun simctl install` failed: {}",
            install_out.stderr.trim()
        );
    }

    println!("Launching {}…", project.bundle_id);
    // Best-effort cleanup on Ctrl-C: `simctl launch` is a foreground bridge
    // process, not the app itself, so killing it (the default SIGINT
    // disposition) does not terminate the app running in the simulator —
    // explicitly `simctl terminate` it for deterministic cleanup. Uses a
    // fresh `RealProcessRunner` (not the injected `runner`) since the
    // handler must be `'static` and this path only ever runs for real.
    let udid_for_handler = device.id.clone();
    let bundle_id_for_handler = project.bundle_id.clone();
    ctrlc::set_handler(move || {
        let _ = RealProcessRunner.run(
            "xcrun",
            &[
                "simctl",
                "terminate",
                &udid_for_handler,
                &bundle_id_for_handler,
            ],
        );
        std::process::exit(0);
    })
    .context("failed to install Ctrl-C handler")?;

    let mut on_launch_line = |line: &str| println!("{line}");
    let launch_out = simctl::launch(runner, &device.id, &project.bundle_id, &mut on_launch_line)?;

    // Best-effort cleanup on normal stream end too (app may already be gone).
    simctl::terminate(runner, &device.id, &project.bundle_id);

    Ok(if launch_out.success { 0 } else { 1 })
}

/// Minimum `devicectl`-drivable iOS major version (spec/RESEARCH.md §C):
/// devicectl only drives iOS 17+ devices; ios-deploy (the iOS 15-16
/// alternative) is dead.
const MIN_DEVICECTL_IOS_MAJOR: u32 = 17;

/// Drives the full physical-iOS-device pipeline (task 67): an iOS-17+ gate
/// (devicectl's minimum), a signed device build via `ios_build::build`, then
/// `devicectl device install app` → `devicectl device process launch
/// --console --terminate-existing` (streamed) on `device`, an
/// already-discovered `Platform::Ios`/`Kind::PhysicalDevice` device, against
/// the ForgeKit project rooted at `root`. `info.mode` (task 66) selects the
/// build configuration exactly like the simulator path; signing is always
/// requested (`codesign: true`) regardless of mode — `run` never skips
/// signing for a physical device.
pub fn run_physical(
    runner: &dyn ProcessRunner,
    root: &Path,
    device: &Device,
    info: &BuildInfo,
) -> Result<u8> {
    let project = project::detect(root)?;
    project::require_ios_dir(&project.root)?;

    match os_version_major(device.os_version.as_deref()) {
        Some(major) if major >= MIN_DEVICECTL_IOS_MAJOR => {}
        _ => bail!(
            "physical-device runs need iOS {MIN_DEVICECTL_IOS_MAJOR}+ (devicectl); this device reports {}",
            device.os_version.as_deref().unwrap_or("unknown")
        ),
    }

    println!("Building `{}`…", project.bundle_id);
    let build_start = Instant::now();
    let artifacts = ios_build::build(
        runner,
        &project.root,
        info,
        &IosArtifact::App {
            simulator: false,
            codesign: true,
        },
    )?;
    println!(
        "Build finished in {:.1}s.",
        build_start.elapsed().as_secs_f32()
    );

    let app_path = artifacts
        .paths
        .first()
        .ok_or_else(|| anyhow::anyhow!("ios_build::build returned no `.app` artifact"))?
        .to_string_lossy()
        .into_owned();

    println!("Installing on {}…", device.name);
    let install_out = devicectl::install(runner, &device.id, &app_path)?;
    if !install_out.success {
        bail!(
            "{}",
            with_developer_mode_hint(format!(
                "`xcrun devicectl device install app` failed: {}",
                install_out.stderr.trim()
            ))
        );
    }

    println!("Launching {}…", project.bundle_id);
    let mut on_launch_line = |line: &str| println!("{line}");
    let launch_out =
        devicectl::launch(runner, &device.id, &project.bundle_id, &mut on_launch_line)?;
    if !launch_out.success {
        bail!(
            "{}",
            with_developer_mode_hint(format!(
                "`xcrun devicectl device process launch` failed: {}",
                launch_out.stderr.trim()
            ))
        );
    }

    Ok(0)
}

/// Parses the major version number out of a `devicectl` `osVersionNumber`
/// string (e.g. `"17.5.1"` -> `Some(17)`); `None` for a missing or
/// unparseable version, which the iOS-17+ gate treats as unsupported.
fn os_version_major(version: Option<&str>) -> Option<u32> {
    version?.split('.').next()?.parse().ok()
}

/// Appends the physical-device hardware hint `devicectl install`/`launch`
/// failures need (task 67): these commands fail opaquely when the device is
/// locked, not paired/trusted, or lacks Developer Mode, none of which
/// `xcodebuild`'s own signing errors cover.
fn with_developer_mode_hint(message: String) -> String {
    format!(
        "{message}\nhint: make sure the device is unlocked, paired and trusted with this Mac, \
         and has Developer Mode enabled (Settings → Privacy & Security → Developer Mode — the \
         device reboots after enabling it)."
    )
}

/// Builds the `xcodebuild` failure message, surfacing the last 50 non-empty
/// lines of both stdout AND stderr — xcodebuild reports most errors on
/// stdout, unlike `gradlew`, which is why this extends (rather than
/// verbatim-copies) the phase-2 `tail_lines` approach.
fn xcodebuild_failure_message(build_out: &crate::process::Output) -> String {
    let stdout_tail = tail_lines(&build_out.stdout, 50);
    let stderr_tail = tail_lines(&build_out.stderr, 50);
    let mut message = "`xcodebuild` failed".to_string();
    if !stdout_tail.is_empty() {
        message.push_str(&format!("\n--- stdout (tail) ---\n{stdout_tail}"));
    }
    if !stderr_tail.is_empty() {
        message.push_str(&format!("\n--- stderr (tail) ---\n{stderr_tail}"));
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_info::{BuildArgs, BuildMode};
    use crate::devices::{Kind, Platform};
    use crate::ios_build::xcodebuild::host_sim_arch;
    use crate::process::{FakeProcessRunner, Output};
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn unique_project_dir(tag: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "forgekit-cli-ios-run-mod-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("ios/Runner.xcodeproj")).unwrap();
        fs::write(
            dir.join("forgekit.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n",
        )
        .unwrap();
        dir
    }

    fn device() -> Device {
        Device {
            id: "AAAA".to_string(),
            name: "iPhone 15".to_string(),
            platform: Platform::Ios,
            kind: Kind::Simulator,
            os_version: None,
            connection_state: None,
        }
    }

    fn ok(stdout: &str) -> Output {
        Output {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    fn debug_info() -> BuildInfo {
        BuildInfo::from_args(BuildArgs::default(), BuildMode::Debug).unwrap()
    }

    fn profile_info() -> BuildInfo {
        BuildInfo::from_args(
            BuildArgs {
                profile: true,
                ..BuildArgs::default()
            },
            BuildMode::Debug,
        )
        .unwrap()
    }

    const BOOTED_JSON: &str = r#"{
        "devices": {
            "com.apple.CoreSimulator.SimRuntime.iOS-17-5": [
                {"udid": "AAAA", "name": "iPhone 15", "state": "Booted", "isAvailable": true}
            ]
        }
    }"#;

    /// Failure exits before the pipeline ever reaches `simctl launch` (and
    /// therefore before `ctrlc::set_handler` is called), so this is safe to
    /// run alongside other tests in the same process.
    #[test]
    fn xcodebuild_failure_surfaces_tail_of_both_stdout_and_stderr() {
        let dir = unique_project_dir("build-fail");
        let runner = FakeProcessRunner::new()
            .with(
                "xcode-select -p",
                ok("/Applications/Xcode.app/Contents/Developer\n"),
            )
            .with(
                "rustup target list --installed",
                ok("aarch64-apple-ios-sim\n"),
            )
            .with("xcrun simctl list devices --json", ok(BOOTED_JSON))
            .with(
                format!(
                    "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Debug -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios ARCHS={} build",
                    host_sim_arch()
                ),
                Output {
                    success: false,
                    stdout: "note: Compiling Runner\nerror: build input file cannot be found"
                        .to_string(),
                    stderr: "xcodebuild: error: Scheme Runner is not currently configured"
                        .to_string(),
                },
            );

        let err = run(&runner, &dir, &device(), &debug_info()).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("xcodebuild"), "{message}");
        assert!(message.contains("--- stdout (tail) ---"), "{message}");
        assert!(
            message.contains("build input file cannot be found"),
            "{message}"
        );
        assert!(message.contains("--- stderr (tail) ---"), "{message}");
        assert!(
            message.contains("Scheme Runner is not currently configured"),
            "{message}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn preflight_failure_surfaces_before_any_xcodebuild_call() {
        let dir = unique_project_dir("preflight-fail");
        let runner = FakeProcessRunner::new().missing("xcode-select -p");
        // No `xcodebuild` fixture registered: if preflight didn't stop the
        // pipeline early, the next unmatched invocation would itself error
        // via `FakeProcessRunner`'s "missing" path, which would also fail
        // this test — either way this proves xcodebuild was never reached.
        let err = run(&runner, &dir, &device(), &debug_info()).unwrap_err();
        assert!(err.to_string().contains("Xcode not found"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Failure exits right after `simctl install` (a deliberately-failing
    /// fixture matched only on the *exact* Profile-configuration app path),
    /// before `ctrlc::set_handler` (which can only be installed once per
    /// test process) — proving `--profile` selects `-configuration Profile`
    /// and the matching `Profile-iphonesimulator` products directory (task
    /// 66), rather than the hardcoded `Debug` this pipeline used before.
    #[test]
    fn profile_mode_uses_profile_configuration_and_products_dir() {
        let dir = unique_project_dir("profile-mode");
        let app_dir = dir.join("build/ios/Build/Products/Profile-iphonesimulator/Runner.app");
        fs::create_dir_all(&app_dir).unwrap();
        let app_path = app_dir.to_string_lossy().into_owned();

        let runner = FakeProcessRunner::new()
            .with(
                "xcode-select -p",
                ok("/Applications/Xcode.app/Contents/Developer\n"),
            )
            .with(
                "rustup target list --installed",
                ok("aarch64-apple-ios-sim\n"),
            )
            .with("xcrun simctl list devices --json", ok(BOOTED_JSON))
            .with(
                format!(
                    "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Profile -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios ARCHS={} build",
                    host_sim_arch()
                ),
                ok("Build succeeded"),
            )
            .with(
                format!("xcrun simctl install AAAA {app_path}"),
                Output {
                    success: false,
                    stdout: String::new(),
                    stderr: "INSTALL_FAILED_TEST_STOP".to_string(),
                },
            );

        let err = run(&runner, &dir, &device(), &profile_info()).unwrap_err();
        assert!(err.to_string().contains("simctl install"), "{err}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_xcodeproj_errs_before_any_process_call() {
        let dir = std::env::temp_dir().join(format!(
            "forgekit-cli-ios-run-mod-test-no-xcodeproj-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("forgekit.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n",
        )
        .unwrap();
        // No fixtures registered at all: proves `require_ios_dir` rejects
        // before any `ProcessRunner` call is made.
        let runner = FakeProcessRunner::new();
        let err = run(&runner, &dir, &device(), &debug_info()).unwrap_err();
        assert!(err.to_string().contains("ios/Runner.xcodeproj"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    // --- `run_physical` (task 67) ---

    const PHYSICAL_LIST_JSON: &str = r#"{"project":{"name":"Runner","schemes":["Runner"],"configurations":["Debug","Profile","Release"]}}"#;

    /// A project fixture with `[ios] team` set explicitly so `ios_build::build`'s
    /// team resolution (env → toml → `security find-identity` auto-detect)
    /// settles on a known, deterministic team id without registering a
    /// `security find-identity` fixture.
    fn unique_physical_project_dir(tag: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "forgekit-cli-ios-run-physical-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("ios/Runner.xcodeproj")).unwrap();
        fs::write(
            dir.join("forgekit.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n\n[ios]\nteam = \"TEAMID1234\"\n",
        )
        .unwrap();
        dir
    }

    fn physical_device() -> Device {
        Device {
            id: "00008110-000A2D3A3C68801E".to_string(),
            name: "Ed's iPhone".to_string(),
            platform: Platform::Ios,
            kind: crate::devices::Kind::PhysicalDevice,
            os_version: Some("17.5.1".to_string()),
            connection_state: None,
        }
    }

    fn plant_physical_app(dir: &std::path::Path) -> String {
        let app_dir = dir.join("build/ios/Build/Products/Debug-iphoneos/Runner.app");
        fs::create_dir_all(&app_dir).unwrap();
        app_dir.to_string_lossy().into_owned()
    }

    /// Registers the preflight/scheme-listing fixtures every `run_physical`
    /// happy/failure-path test needs (mirrors `ios_build::mod`'s `base_runner`).
    fn physical_base_runner() -> FakeProcessRunner {
        FakeProcessRunner::new()
            .with("xcode-select -p", ok("/Applications/Xcode.app\n"))
            .with(
                "rustup target list --installed",
                ok("aarch64-apple-ios\n"),
            )
            .with(
                "xcrun xcodebuild -list -json -project ios/Runner.xcodeproj",
                ok(PHYSICAL_LIST_JSON),
            )
            .with(
                "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Debug -sdk iphoneos -destination generic/platform=iOS -derivedDataPath build/ios DEVELOPMENT_TEAM=TEAMID1234 CODE_SIGN_STYLE=Automatic CODE_SIGNING_ALLOWED=YES CODE_SIGNING_REQUIRED=YES -allowProvisioningUpdates -allowProvisioningDeviceRegistration build",
                ok("Build succeeded"),
            )
    }

    #[test]
    fn run_physical_happy_path_builds_installs_and_launches() {
        let dir = unique_physical_project_dir("happy");
        let app_path = plant_physical_app(&dir);

        let runner = physical_base_runner()
            .with(
                format!(
                    "xcrun devicectl device install app --device 00008110-000A2D3A3C68801E {app_path}"
                ),
                ok(""),
            )
            .with(
                "xcrun devicectl device process launch --device 00008110-000A2D3A3C68801E --console --terminate-existing dev.f0x.myapp",
                ok("hello from device\n"),
            );

        let code = run_physical(&runner, &dir, &physical_device(), &debug_info()).unwrap();
        assert_eq!(code, 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_physical_rejects_ios16_device_before_any_build_call() {
        let dir = unique_physical_project_dir("ios16-gate");
        let mut device = physical_device();
        device.os_version = Some("16.7.2".to_string());
        // No fixtures registered at all: proves the iOS-17+ gate stops the
        // pipeline before any `xcodebuild`/`devicectl` call is attempted.
        let runner = FakeProcessRunner::new();
        let err = run_physical(&runner, &dir, &device, &debug_info()).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("iOS 17+"), "{message}");
        assert!(message.contains("16.7.2"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_physical_rejects_device_with_unknown_os_version() {
        let dir = unique_physical_project_dir("unknown-version");
        let mut device = physical_device();
        device.os_version = None;
        let runner = FakeProcessRunner::new();
        let err = run_physical(&runner, &dir, &device, &debug_info()).unwrap_err();
        assert!(err.to_string().contains("unknown"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_physical_install_failure_surfaces_developer_mode_hint() {
        let dir = unique_physical_project_dir("install-fail");
        let app_path = plant_physical_app(&dir);

        // No `devicectl device process launch` fixture registered: proves
        // an install failure stops the pipeline before launch is attempted.
        let runner = physical_base_runner().with(
            format!(
                "xcrun devicectl device install app --device 00008110-000A2D3A3C68801E {app_path}"
            ),
            Output {
                success: false,
                stdout: String::new(),
                stderr: "Unable to install: device locked".to_string(),
            },
        );

        let err = run_physical(&runner, &dir, &physical_device(), &debug_info()).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("devicectl device install app"),
            "{message}"
        );
        assert!(message.contains("device locked"), "{message}");
        assert!(message.contains("Developer Mode"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_physical_launch_failure_surfaces_developer_mode_hint() {
        let dir = unique_physical_project_dir("launch-fail");
        let app_path = plant_physical_app(&dir);

        let runner = physical_base_runner()
            .with(
                format!(
                    "xcrun devicectl device install app --device 00008110-000A2D3A3C68801E {app_path}"
                ),
                ok(""),
            )
            .with(
                "xcrun devicectl device process launch --device 00008110-000A2D3A3C68801E --console --terminate-existing dev.f0x.myapp",
                Output {
                    success: false,
                    stdout: String::new(),
                    stderr: "Unable to launch: Developer Mode disabled".to_string(),
                },
            );

        let err = run_physical(&runner, &dir, &physical_device(), &debug_info()).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("devicectl device process launch"),
            "{message}"
        );
        assert!(message.contains("Developer Mode disabled"), "{message}");
        assert!(message.contains("Developer Mode enabled"), "{message}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_physical_missing_xcodeproj_errs_before_any_process_call() {
        let dir = std::env::temp_dir().join(format!(
            "forgekit-cli-ios-run-physical-test-no-xcodeproj-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("forgekit.toml"),
            "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n",
        )
        .unwrap();
        // No fixtures registered at all: proves `require_ios_dir` rejects
        // before any `ProcessRunner` call is made.
        let runner = FakeProcessRunner::new();
        let err = run_physical(&runner, &dir, &physical_device(), &debug_info()).unwrap_err();
        assert!(err.to_string().contains("ios/Runner.xcodeproj"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }
}
