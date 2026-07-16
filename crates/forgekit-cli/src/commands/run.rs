//! `forgekit run` (spec §12.4): Android drive pipeline, with a `cargo run`
//! desktop-preview fallback when no Android device is available.

use std::io::{BufRead, Write};

use anyhow::{Context, Result, bail};

use crate::android_run::{self, DeviceSelection};
use crate::build_info::{BuildArgs, BuildInfo, BuildMode};
use crate::devices::{self, Device, Kind, Platform};
use crate::ios_run;
use crate::process::{ProcessRunner, RealProcessRunner};

pub fn run(build_args: BuildArgs, device_id: Option<String>, verbose: bool) -> Result<u8> {
    let info =
        BuildInfo::from_args(build_args, BuildMode::Debug).map_err(|err| anyhow::anyhow!(err))?;

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
        DeviceSelection::Auto(device) => run_on_device(&runner, &device, &info),
        DeviceSelection::Ambiguous(candidates) => select_from_prompt(&runner, &candidates, &info),
        DeviceSelection::Error(message) => bail!(message),
    }
}

/// Dispatches a resolved [`Device`] to its platform's mode/flavor-aware
/// drive pipeline (spec §12.4's Android path, task 66's mode-aware Android
/// and iOS-simulator paths); a physical iOS device has no signing pipeline
/// yet, so it errors with a Phase 5 note instead (task 67).
fn run_on_device(runner: &dyn ProcessRunner, device: &Device, info: &BuildInfo) -> Result<u8> {
    match (device.platform, device.kind) {
        (Platform::Android, _) => run_android(runner, device, info),
        (Platform::Ios, Kind::Simulator) => {
            let cwd = std::env::current_dir().context("reading current directory")?;
            ios_run::run(runner, &cwd, device, info)
        }
        (Platform::Ios, Kind::PhysicalDevice) => {
            bail!("iOS physical-device run lands in Phase 5 (requires the signing pipeline)")
        }
        (Platform::Ios, Kind::Emulator) => {
            unreachable!("iOS devices are never discovered as Kind::Emulator")
        }
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

fn select_from_prompt(
    runner: &dyn ProcessRunner,
    candidates: &[Device],
    info: &BuildInfo,
) -> Result<u8> {
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

    run_on_device(runner, &candidates[index], info)
}

/// Drives the full, mode/flavor-aware Android pipeline (spec §12.4 steps
/// 3-7; task 66) on `device` — delegates to `android_run::run`, which owns
/// the pipeline body (mirroring `ios_run::run`'s shape).
fn run_android(runner: &dyn ProcessRunner, device: &Device, info: &BuildInfo) -> Result<u8> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    android_run::run(runner, &cwd, device, info)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::FakeProcessRunner;

    fn ios_physical_device() -> Device {
        Device {
            id: "00008110-000A2D3A3C68801E".to_string(),
            name: "Ed's iPhone".to_string(),
            platform: Platform::Ios,
            kind: Kind::PhysicalDevice,
        }
    }

    fn debug_info() -> BuildInfo {
        BuildInfo::from_args(BuildArgs::default(), BuildMode::Debug).unwrap()
    }

    #[test]
    fn run_on_device_bails_with_phase5_note_for_physical_ios_device() {
        let runner = FakeProcessRunner::new();
        let err = run_on_device(&runner, &ios_physical_device(), &debug_info()).unwrap_err();
        assert!(err.to_string().contains("lands in Phase 5"), "{err}");
        assert!(err.to_string().contains("signing pipeline"), "{err}");
    }

    #[test]
    fn physical_ios_sentinel_fires_regardless_of_build_mode() {
        // The physical-iOS sentinel (task 67 removes it) is independent of
        // `--release`/`--profile`/`--debug` — task 66 only unblocks the
        // Android and iOS-*simulator* run paths.
        let runner = FakeProcessRunner::new();
        let info = BuildInfo::from_args(
            BuildArgs {
                release: true,
                ..Default::default()
            },
            BuildMode::Debug,
        )
        .unwrap();
        let err = run_on_device(&runner, &ios_physical_device(), &info).unwrap_err();
        assert!(err.to_string().contains("lands in Phase 5"), "{err}");
        assert!(err.to_string().contains("signing pipeline"), "{err}");
    }
}
