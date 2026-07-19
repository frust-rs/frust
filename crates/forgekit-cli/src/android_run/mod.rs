//! Android drive pipeline for `forgekit run` (spec §12.4): preflight →
//! gradle build → adb install → launch → pid-scoped logcat streaming.
//! Device selection (this module's top level) is decoupled from clap/stdin
//! so it's unit-testable without a terminal.

pub mod adb;
pub mod gradle;
pub mod preflight;
pub mod project;

use std::io::IsTerminal;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail};

use crate::android_build::AndroidArtifact;
use crate::build_info::BuildInfo;
use crate::devices::{Device, Platform};
use crate::doctor::{EnvLookup, RealEnv};
use crate::process::{ProcessRunner, tail_lines};

/// Outcome of matching discovered devices against `-d`/no-flag selection
/// (spec §12.4 step 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceSelection {
    /// A single device to run on: either `-d` matched exactly one Android
    /// device, or exactly one Android device was discovered with no `-d`.
    Auto(Device),
    /// No Android device connected and no `-d` given: desktop fallback
    /// (`cargo run` passthrough).
    Desktop,
    /// Multiple Android devices and no `-d`: caller prompts (TTY) or
    /// lists-and-exits (non-TTY).
    Ambiguous(Vec<Device>),
    /// `-d` matched zero/multiple devices, or matched a non-Android device.
    Error(String),
}

/// Resolves device selection from all discovered devices and an optional
/// `-d` pattern (prefix match on id or name, case-insensitive).
pub fn select_device(devices: &[Device], pattern: Option<&str>) -> DeviceSelection {
    if let Some(pattern) = pattern {
        let matches: Vec<&Device> = devices
            .iter()
            .filter(|d| matches_pattern(d, pattern))
            .collect();
        return match matches.as_slice() {
            [] => DeviceSelection::Error(format!("no device matching `{pattern}`")),
            // Platform/kind-specific handling (Android drive pipeline, iOS
            // simulator drive pipeline, or a Phase 5 sentinel for physical
            // iOS devices) is the caller's job — `commands::run::run_on_device`
            // dispatches on `Device::platform`/`Device::kind`.
            [device] => DeviceSelection::Auto((*device).clone()),
            many => DeviceSelection::Error(format!(
                "ambiguous device id `{pattern}`; matches: {}",
                many.iter()
                    .map(|d| d.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        };
    }

    let android: Vec<Device> = devices
        .iter()
        .filter(|d| d.platform == Platform::Android)
        .cloned()
        .collect();
    match android.len() {
        0 => DeviceSelection::Desktop,
        1 => DeviceSelection::Auto(android.into_iter().next().expect("len checked")),
        _ => DeviceSelection::Ambiguous(android),
    }
}

fn matches_pattern(device: &Device, pattern: &str) -> bool {
    let pattern = pattern.to_lowercase();
    device.id.to_lowercase().starts_with(&pattern)
        || device.name.to_lowercase().starts_with(&pattern)
}

/// Parses a 1-based numbered-prompt answer (e.g. `"2"`) against `count`
/// choices, returning a 0-based index. Pure so the prompt's parsing is
/// unit-testable without stdin.
pub fn parse_prompt_selection(input: &str, count: usize) -> Result<usize, String> {
    let trimmed = input.trim();
    let choice: usize = trimmed
        .parse()
        .map_err(|_| format!("`{trimmed}` is not a number"))?;
    if choice == 0 || choice > count {
        return Err(format!("enter a number between 1 and {count}"));
    }
    Ok(choice - 1)
}

/// Whether stdout is attached to a terminal — decides numbered-prompt vs
/// list-and-exit for [`DeviceSelection::Ambiguous`].
pub fn stdout_is_tty() -> bool {
    std::io::stdout().is_terminal()
}

/// Drives the full, mode/flavor-aware Android pipeline (spec §12.4 steps
/// 3-7, task 66): preflight → local.properties version write → (release
/// only) signing gate → variant-aware `./gradlew assemble<Flavor><Mode>` →
/// variant-aware APK install → launch → pid-scoped logcat streaming.
/// Mirrors `ios_run::run`'s `(runner, root, device)` shape, plus `info` for
/// the mode/flavor/defines/version funnel.
pub fn run(
    runner: &dyn ProcessRunner,
    root: &Path,
    device: &Device,
    info: &BuildInfo,
) -> Result<u8> {
    run_with_env(runner, root, device, info, &RealEnv)
}

/// The testable core of [`run`], taking an injected [`EnvLookup`] so
/// `JAVA_HOME` resolution can be exercised with a `crate::doctor::FakeEnv`
/// in tests instead of the real process environment.
fn run_with_env(
    runner: &dyn ProcessRunner,
    root: &Path,
    device: &Device,
    info: &BuildInfo,
    env: &dyn EnvLookup,
) -> Result<u8> {
    let project = project::detect(root)?;
    let android_dir = project::require_android_dir(&project.root)?;

    let preflight_ctx = preflight::PreflightCtx {
        runner,
        env,
        is_macos: cfg!(target_os = "macos"),
    };
    let outcome = preflight::run(&preflight_ctx).map_err(|err| anyhow::anyhow!(err))?;

    if let Some(gradle_user_home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        let gradle_user_home = gradle_user_home.join(".gradle");
        if !gradle::wrapper_dist_cached(&gradle_user_home) {
            println!("Note: first Gradle run downloads the wrapper distribution (~1-2 min).");
        }
    }

    let version_name = info.build_name.clone().unwrap_or_else(|| "1.0".to_string());
    let version_code = info
        .build_number
        .map(|n| n.to_string())
        .unwrap_or_else(|| "1".to_string());
    crate::android_build::local_properties::write(&android_dir, &version_name, &version_code)
        .context("writing android/local.properties")?;

    crate::android_build::signing::check_release_signing(&android_dir, info.mode)?;

    let abi = adb::device_abi(runner, &device.id);
    let target = AndroidArtifact::Apk {
        split_per_abi: false,
        abis: vec![abi],
    };
    let task = crate::android_build::tasks::task_name(&target, info.mode, info.flavor.as_deref());
    let props = crate::android_build::tasks::gradle_properties(&target, &info.defines);

    println!("Building `{}`…", project.app_id);
    let build_start = Instant::now();
    let mut on_gradle_line = |line: &str| println!("{line}");
    let build_out = gradle::assemble(
        runner,
        &android_dir,
        &outcome.java_home,
        &task,
        &props,
        &mut on_gradle_line,
    )?;
    if !build_out.success {
        let tail = tail_lines(&build_out.stderr, 50);
        if tail.is_empty() {
            bail!("`./gradlew {task}` failed");
        }
        bail!("`./gradlew {task}` failed:\n{tail}");
    }
    println!(
        "Build finished in {:.1}s.",
        build_start.elapsed().as_secs_f32()
    );

    let apk_path = gradle::apk_output_path(&android_dir, info.mode, info.flavor.as_deref())?;
    let apk_path = apk_path.to_string_lossy().into_owned();

    println!("Installing on {}…", device.name);
    let install_start = Instant::now();
    let install_out = adb::install(runner, &device.id, &apk_path)?;
    if !install_out.success {
        bail!("`adb install` failed: {}", install_out.stderr.trim());
    }
    println!(
        "Installed in {:.1}s.",
        install_start.elapsed().as_secs_f32()
    );

    println!("Launching {}…", project.app_id);
    let launch_out = adb::launch(runner, &device.id, &project.app_id)?;
    if !launch_out.success {
        bail!("`adb shell am start` failed: {}", launch_out.stderr.trim());
    }

    let mut sleep = || std::thread::sleep(adb::PID_RETRY_DELAY);
    let pid = adb::resolve_pid(
        runner,
        &device.id,
        &project.app_id,
        adb::PID_RETRY_ATTEMPTS,
        &mut sleep,
    )?;

    println!("Streaming logs (pid {pid}); press Ctrl-C to stop.");
    // Default SIGINT disposition would exit 130; spec §12.4 wants Ctrl-C to
    // stop the (already-SIGINT'd, same-process-group) `adb logcat` child
    // and exit 0.
    ctrlc::set_handler(|| {
        std::process::exit(0);
    })
    .context("failed to install Ctrl-C handler")?;

    let mut on_log_line = |line: &str| println!("{line}");
    adb::stream_logcat(runner, &device.id, &pid, &mut on_log_line)?;

    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::Kind;

    fn android(id: &str, name: &str) -> Device {
        Device {
            id: id.to_string(),
            name: name.to_string(),
            platform: Platform::Android,
            kind: Kind::Emulator,
            os_version: None,
            connection_state: None,
        }
    }

    fn ios(id: &str, name: &str) -> Device {
        Device {
            id: id.to_string(),
            name: name.to_string(),
            platform: Platform::Ios,
            kind: Kind::Simulator,
            os_version: None,
            connection_state: None,
        }
    }

    #[test]
    fn no_devices_and_no_pattern_falls_back_to_desktop() {
        assert_eq!(select_device(&[], None), DeviceSelection::Desktop);
    }

    #[test]
    fn only_ios_devices_and_no_pattern_falls_back_to_desktop() {
        let devices = vec![ios("sim1", "iPhone 15")];
        assert_eq!(select_device(&devices, None), DeviceSelection::Desktop);
    }

    #[test]
    fn exactly_one_android_device_auto_picked() {
        let devices = vec![android("emulator-5554", "Pixel 7")];
        assert_eq!(
            select_device(&devices, None),
            DeviceSelection::Auto(devices[0].clone())
        );
    }

    #[test]
    fn multiple_android_devices_are_ambiguous() {
        let devices = vec![
            android("emulator-5554", "Pixel 7"),
            android("R58N90ABCDE", "Pixel 8"),
        ];
        assert_eq!(
            select_device(&devices, None),
            DeviceSelection::Ambiguous(devices.clone())
        );
    }

    #[test]
    fn dash_d_prefix_matches_id() {
        let devices = vec![android("emulator-5554", "Pixel 7")];
        assert_eq!(
            select_device(&devices, Some("emulator")),
            DeviceSelection::Auto(devices[0].clone())
        );
    }

    #[test]
    fn dash_d_prefix_matches_name_case_insensitively() {
        let devices = vec![android("emulator-5554", "Pixel 7")];
        assert_eq!(
            select_device(&devices, Some("pixel")),
            DeviceSelection::Auto(devices[0].clone())
        );
    }

    #[test]
    fn dash_d_no_match_errs() {
        let devices = vec![android("emulator-5554", "Pixel 7")];
        match select_device(&devices, Some("nope")) {
            DeviceSelection::Error(msg) => assert!(msg.contains("no device matching")),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn dash_d_ambiguous_match_errs() {
        let devices = vec![
            android("emulator-5554", "Pixel 7"),
            android("emulator-5556", "Pixel 8"),
        ];
        match select_device(&devices, Some("emulator")) {
            DeviceSelection::Error(msg) => assert!(msg.contains("ambiguous")),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn dash_d_matching_ios_device_is_auto_selected() {
        // `select_device` no longer special-cases iOS: platform/kind
        // dispatch (iOS simulator drive pipeline vs. a Phase 5 sentinel for
        // physical devices) is `commands::run::run_on_device`'s job.
        let devices = vec![ios("sim1", "iPhone 15")];
        assert_eq!(
            select_device(&devices, Some("sim1")),
            DeviceSelection::Auto(devices[0].clone())
        );
    }

    #[test]
    fn parse_prompt_selection_accepts_in_range_number() {
        assert_eq!(parse_prompt_selection("2", 3), Ok(1));
        assert_eq!(parse_prompt_selection(" 1 \n", 3), Ok(0));
    }

    #[test]
    fn parse_prompt_selection_rejects_out_of_range() {
        assert!(parse_prompt_selection("0", 3).is_err());
        assert!(parse_prompt_selection("4", 3).is_err());
    }

    #[test]
    fn parse_prompt_selection_rejects_non_numeric() {
        assert!(parse_prompt_selection("abc", 3).is_err());
    }

    mod run_pipeline {
        use super::*;
        use crate::build_info::{BuildArgs, BuildMode};
        use crate::doctor::FakeEnv;
        use crate::process::{FakeProcessRunner, Output};
        use std::fs;
        use std::sync::atomic::{AtomicU32, Ordering};

        const JAVA_HOME: &str = "/opt/jdk17";

        fn unique_project_dir(tag: &str) -> std::path::PathBuf {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "forgekit-cli-android-run-pipeline-test-{tag}-{}-{n}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("android")).unwrap();
            fs::write(dir.join("android/gradlew"), "#!/bin/sh\n").unwrap();
            fs::write(
                dir.join("forgekit.toml"),
                "[app]\nname = \"myapp\"\norg = \"dev.f0x\"\n",
            )
            .unwrap();
            dir
        }

        fn fake_env() -> FakeEnv {
            FakeEnv::new().set("JAVA_HOME", JAVA_HOME)
        }

        fn ok(stdout: &str) -> Output {
            Output {
                success: true,
                stdout: stdout.to_string(),
                stderr: String::new(),
            }
        }

        /// Every preflight check `preflight::run` performs (including the
        /// device-only `adb version` one this module's pipeline needs, unlike
        /// `android_build`'s device-less preflight).
        fn preflight_ok_runner() -> FakeProcessRunner {
            FakeProcessRunner::new()
                .with(
                    "rustup target list --installed",
                    ok("aarch64-linux-android\n"),
                )
                .with("cargo ndk --version", ok("cargo-ndk 3.5.4\n"))
                .with(
                    format!("{JAVA_HOME}/bin/java -version"),
                    Output {
                        success: true,
                        stdout: String::new(),
                        stderr: "openjdk version \"17.0.9\" 2023-10-17\n".to_string(),
                    },
                )
                .with("adb version", ok("Android Debug Bridge version 1.0.41\n"))
        }

        fn device() -> Device {
            Device {
                id: "emulator-5554".to_string(),
                name: "Pixel 7".to_string(),
                platform: Platform::Android,
                kind: Kind::Emulator,
                os_version: None,
                connection_state: None,
            }
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

        /// Registering the exact `adb install` invocation to *fail* (rather
        /// than succeed) stops the pipeline right after it, before
        /// `resolve_pid`'s real-clock retry loop / `ctrlc::set_handler`
        /// (which can only be installed once per test process) — a
        /// deliberately-failing fixture that only matches on the *exact*
        /// apk path proves the gradle task/props/apk-path computation was
        /// correct up to that point (any mismatch earlier would instead
        /// error on an unregistered `./gradlew`/`adb` invocation).
        fn stop_after_install(runner: FakeProcessRunner, apk_path: &str) -> FakeProcessRunner {
            runner.with(
                format!("adb -s emulator-5554 install -r {apk_path}"),
                Output {
                    success: false,
                    stdout: String::new(),
                    stderr: "INSTALL_FAILED_TEST_STOP".to_string(),
                },
            )
        }

        #[test]
        fn debug_default_assembles_debug_and_installs_debug_apk_unchanged() {
            let dir = unique_project_dir("debug-default");
            let android_dir = dir.join("android");
            let out_dir = android_dir.join("app/build/outputs/apk/debug");
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-debug.apk"), b"fake").unwrap();
            let apk_path = out_dir.join("app-debug.apk").to_string_lossy().into_owned();

            let runner = stop_after_install(
                preflight_ok_runner().with(
                    "./gradlew assembleDebug -Pforgekit.targetPlatforms=arm64-v8a -Pforgekit.splitPerAbi=false",
                    ok("BUILD SUCCESSFUL"),
                ),
                &apk_path,
            );

            let build_info = info(BuildMode::Debug, None);
            let err = run_with_env(&runner, &dir, &device(), &build_info, &fake_env()).unwrap_err();
            assert!(err.to_string().contains("adb install"), "{err}");

            let local_props = fs::read_to_string(android_dir.join("local.properties")).unwrap();
            assert!(local_props.contains("forgekit.versionName=1.0"));
            assert!(local_props.contains("forgekit.versionCode=1"));

            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn release_assembles_release_and_installs_release_apk() {
            let dir = unique_project_dir("release-default");
            let android_dir = dir.join("android");
            fs::write(android_dir.join("key.properties"), "keyAlias=upload\n").unwrap();
            let out_dir = android_dir.join("app/build/outputs/apk/release");
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-release.apk"), b"fake").unwrap();
            let apk_path = out_dir
                .join("app-release.apk")
                .to_string_lossy()
                .into_owned();

            let runner = stop_after_install(
                preflight_ok_runner().with(
                    "./gradlew assembleRelease -Pforgekit.targetPlatforms=arm64-v8a -Pforgekit.splitPerAbi=false",
                    ok("BUILD SUCCESSFUL"),
                ),
                &apk_path,
            );

            let build_info = info(BuildMode::Release, None);
            let err = run_with_env(&runner, &dir, &device(), &build_info, &fake_env()).unwrap_err();
            assert!(err.to_string().contains("adb install"), "{err}");

            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn profile_assembles_profile_and_installs_profile_apk() {
            let dir = unique_project_dir("profile-default");
            let android_dir = dir.join("android");
            let out_dir = android_dir.join("app/build/outputs/apk/profile");
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-profile.apk"), b"fake").unwrap();
            let apk_path = out_dir
                .join("app-profile.apk")
                .to_string_lossy()
                .into_owned();

            let runner = stop_after_install(
                preflight_ok_runner().with(
                    "./gradlew assembleProfile -Pforgekit.targetPlatforms=arm64-v8a -Pforgekit.splitPerAbi=false",
                    ok("BUILD SUCCESSFUL"),
                ),
                &apk_path,
            );

            let build_info = info(BuildMode::Profile, None);
            let err = run_with_env(&runner, &dir, &device(), &build_info, &fake_env()).unwrap_err();
            assert!(err.to_string().contains("adb install"), "{err}");

            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn flavor_and_release_assembles_flavored_task_and_installs_flavored_apk() {
            let dir = unique_project_dir("flavor-release");
            let android_dir = dir.join("android");
            fs::write(android_dir.join("key.properties"), "keyAlias=upload\n").unwrap();
            let out_dir = android_dir.join("app/build/outputs/apk/paid/release");
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-paid-release.apk"), b"fake").unwrap();
            let apk_path = out_dir
                .join("app-paid-release.apk")
                .to_string_lossy()
                .into_owned();

            let runner = stop_after_install(
                preflight_ok_runner().with(
                    "./gradlew assemblePaidRelease -Pforgekit.targetPlatforms=arm64-v8a -Pforgekit.splitPerAbi=false",
                    ok("BUILD SUCCESSFUL"),
                ),
                &apk_path,
            );

            let build_info = info(BuildMode::Release, Some("paid"));
            let err = run_with_env(&runner, &dir, &device(), &build_info, &fake_env()).unwrap_err();
            assert!(err.to_string().contains("adb install"), "{err}");

            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn release_without_key_properties_errs_before_invoking_gradle() {
            let dir = unique_project_dir("release-no-keystore");
            // No `./gradlew`/`adb` fixtures registered at all: an unexpected
            // call would itself error, proving Gradle is never invoked.
            let runner = preflight_ok_runner();

            let build_info = info(BuildMode::Release, None);
            let err = run_with_env(&runner, &dir, &device(), &build_info, &fake_env()).unwrap_err();
            assert!(err.to_string().contains("keytool"), "{err}");

            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn device_abi_flows_into_target_platforms_property() {
            let dir = unique_project_dir("device-abi-x86-64");
            let android_dir = dir.join("android");
            let out_dir = android_dir.join("app/build/outputs/apk/debug");
            fs::create_dir_all(&out_dir).unwrap();
            fs::write(out_dir.join("app-debug.apk"), b"fake").unwrap();
            let apk_path = out_dir.join("app-debug.apk").to_string_lossy().into_owned();

            let runner = stop_after_install(
                preflight_ok_runner()
                    .with(
                        "adb -s emulator-5554 shell getprop ro.product.cpu.abi",
                        ok("x86_64\n"),
                    )
                    .with(
                        "./gradlew assembleDebug -Pforgekit.targetPlatforms=x86_64 -Pforgekit.splitPerAbi=false",
                        ok("BUILD SUCCESSFUL"),
                    ),
                &apk_path,
            );

            let build_info = info(BuildMode::Debug, None);
            let err = run_with_env(&runner, &dir, &device(), &build_info, &fake_env()).unwrap_err();
            assert!(err.to_string().contains("adb install"), "{err}");

            let _ = fs::remove_dir_all(&dir);
        }
    }
}
