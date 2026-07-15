//! `forgekit run` (spec §12.4): Android drive pipeline, with a `cargo run`
//! desktop-preview fallback when no Android device is available.

use std::io::{BufRead, Write};
use std::time::Instant;

use anyhow::{Context, Result, bail};

use crate::android_run::{self, DeviceSelection};
use crate::build_info::{BuildArgs, BuildInfo, BuildMode};
use crate::devices::{self, Device};
use crate::doctor::RealEnv;
use crate::process::{ProcessRunner, RealProcessRunner};

pub fn run(build_args: BuildArgs, device_id: Option<String>, verbose: bool) -> Result<u8> {
    let info =
        BuildInfo::from_args(build_args, BuildMode::Debug).map_err(|err| anyhow::anyhow!(err))?;
    if info.mode != BuildMode::Debug {
        bail!("release/profile runs land in Phase 5; `forgekit run` currently only builds debug");
    }

    let runner = RealProcessRunner;
    let discoverers = devices::default_discoverers();
    let (found, notes) = devices::discover_all(&runner, &discoverers);
    if verbose {
        for note in &notes {
            println!("[note] {note}");
        }
    }

    match android_run::select_device(&found, device_id.as_deref()) {
        DeviceSelection::Desktop => run_desktop_fallback(&runner),
        DeviceSelection::Auto(device) => run_android(&runner, &device),
        DeviceSelection::Ambiguous(candidates) => select_from_prompt(&runner, &candidates),
        DeviceSelection::Error(message) => bail!(message),
    }
}

/// No Android device connected and no `-d`: run the desktop preview shell
/// exactly like a bare `cargo run` (spec §12.9's dev loop), streaming its
/// output rather than buffering it until exit.
fn run_desktop_fallback(runner: &dyn ProcessRunner) -> Result<u8> {
    println!("No Android device connected; falling back to `cargo run` (desktop preview).");
    let mut on_line = |line: &str| println!("{line}");
    let out = runner.run_streaming("cargo", &["run"], None, &[], &mut on_line)?;
    Ok(if out.success { 0 } else { 1 })
}

fn select_from_prompt(runner: &dyn ProcessRunner, candidates: &[Device]) -> Result<u8> {
    if !android_run::stdout_is_tty() {
        println!("Multiple Android devices connected; pass -d <id> to select one:");
        for device in candidates {
            println!("  {} ({})", device.name, device.id);
        }
        return Ok(1);
    }

    for (i, device) in candidates.iter().enumerate() {
        println!("{}. {} ({})", i + 1, device.name, device.id);
    }
    print!("Select a device [1-{}]: ", candidates.len());
    std::io::stdout().flush().ok();

    let mut input = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut input)
        .context("reading device selection")?;
    let index = android_run::parse_prompt_selection(&input, candidates.len())
        .map_err(|err| anyhow::anyhow!(err))?;

    run_android(runner, &candidates[index])
}

/// Drives the full Android pipeline (spec §12.4 steps 3-7) on `device`.
fn run_android(runner: &dyn ProcessRunner, device: &Device) -> Result<u8> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    let project = android_run::project::detect(&cwd)?;
    let android_dir = android_run::project::require_android_dir(&project.root)?;

    let env = RealEnv;
    let preflight_ctx = android_run::preflight::PreflightCtx {
        runner,
        env: &env,
        is_macos: cfg!(target_os = "macos"),
    };
    let outcome =
        android_run::preflight::run(&preflight_ctx).map_err(|err| anyhow::anyhow!(err))?;

    if let Some(gradle_user_home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        let gradle_user_home = gradle_user_home.join(".gradle");
        if !android_run::gradle::wrapper_dist_cached(&gradle_user_home) {
            println!("Note: first Gradle run downloads the wrapper distribution (~1-2 min).");
        }
    }

    println!("Building `{}`…", project.app_id);
    let build_start = Instant::now();
    let mut on_gradle_line = |line: &str| println!("{line}");
    let build_out = android_run::gradle::assemble_debug(
        runner,
        &android_dir,
        &outcome.java_home,
        &mut on_gradle_line,
    )?;
    if !build_out.success {
        bail!("`./gradlew assembleDebug` failed");
    }
    println!(
        "Build finished in {:.1}s.",
        build_start.elapsed().as_secs_f32()
    );

    let apk_path = android_dir.join(android_run::gradle::DEBUG_APK_PATH);
    let apk_path = apk_path.to_string_lossy().into_owned();

    println!("Installing on {}…", device.name);
    let install_start = Instant::now();
    let install_out = android_run::adb::install(runner, &device.id, &apk_path)?;
    if !install_out.success {
        bail!("`adb install` failed: {}", install_out.stderr.trim());
    }
    println!(
        "Installed in {:.1}s.",
        install_start.elapsed().as_secs_f32()
    );

    println!("Launching {}…", project.app_id);
    let launch_out = android_run::adb::launch(runner, &device.id, &project.app_id)?;
    if !launch_out.success {
        bail!("`adb shell am start` failed: {}", launch_out.stderr.trim());
    }

    let mut sleep = || std::thread::sleep(android_run::adb::PID_RETRY_DELAY);
    let pid = android_run::adb::resolve_pid(
        runner,
        &device.id,
        &project.app_id,
        android_run::adb::PID_RETRY_ATTEMPTS,
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
    android_run::adb::stream_logcat(runner, &device.id, &pid, &mut on_log_line)?;

    Ok(0)
}
