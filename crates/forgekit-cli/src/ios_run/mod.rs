//! iOS simulator drive pipeline for `forgekit run`: preflight → xcodebuild
//! → `simctl install` → `simctl launch --console-pty` (streamed). Mirrors
//! `android_run`'s structure and testing style. A physical iOS device has
//! no signing pipeline yet — `commands::run` routes those to a Phase 5
//! sentinel before ever reaching this module.

pub mod preflight;
pub mod project;
pub mod simctl;
pub mod xcodebuild;

use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail};

use crate::build_info::BuildInfo;
use crate::devices::Device;
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
                "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Debug -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios build",
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
                "xcrun xcodebuild -project ios/Runner.xcodeproj -scheme Runner -configuration Profile -sdk iphonesimulator -destination id=AAAA -derivedDataPath build/ios build",
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
}
