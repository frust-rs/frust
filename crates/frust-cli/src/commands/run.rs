//! `frust run` (spec §12.4): Android drive pipeline, with a `cargo run`
//! desktop-preview fallback when no Android device is available.

use std::io::{BufRead, Write};

use anyhow::{Context, Result, bail};

use crate::build_args::BuildArgs;
use crate::cli::RenderTierArg;
use frust_drive::android_run::{self, DeviceSelection};
use frust_drive::build_info::{BuildInfo, BuildMode};
use frust_drive::devices::{self, Device, Kind, Platform};
use frust_drive::ios_run;
use frust_drive::process::ProcessRunner;

/// Env var `frust-render`'s `RenderContext` reads at startup
/// (`frust_render::RENDER_TIER_ENV_VAR`) — kept as a literal here rather
/// than importing `frust-render` (see `RenderTierArg`'s doc comment on
/// why `frust-cli` stays independent of the rendering stack).
const RENDER_TIER_ENV_VAR: &str = "FRUST_RENDER_TIER";

/// The testable core of `run`, taking an injected [`ProcessRunner`].
/// `commands::dispatch` constructs the real runner and calls this (the CLI's
/// one `Real` construction site).
pub fn run_in(
    runner: &dyn ProcessRunner,
    build_args: BuildArgs,
    device_id: Option<String>,
    render_tier: Option<RenderTierArg>,
    verbose: bool,
) -> Result<u8> {
    let info = BuildInfo::from_args(build_args.into_drive(), BuildMode::Debug)
        .map_err(|err| anyhow::anyhow!(err))?;

    let discoverers = devices::default_discoverers();
    let (found, notes) = devices::discover_all(runner, &discoverers);
    if verbose {
        for note in &notes {
            println!("[note] {note}");
        }
    }

    match android_run::select_device(&found, device_id.as_deref()) {
        DeviceSelection::Desktop => run_desktop_fallback(runner, &info, render_tier),
        DeviceSelection::Auto(device) => {
            warn_render_tier_not_plumbed(render_tier);
            run_on_device(runner, &device, &info)
        }
        DeviceSelection::Ambiguous(candidates) => {
            warn_render_tier_not_plumbed(render_tier);
            select_from_prompt(runner, &candidates, &info)
        }
        DeviceSelection::Error(message) => bail!(message),
    }
}

/// `--render-tier` is desktop-preview-only in v1 (see `Command::Run`'s doc
/// comment) — a device run still probes its own tier on-device, it just
/// can't be forced from here yet, so tell the caller rather than silently
/// dropping the flag.
fn warn_render_tier_not_plumbed(render_tier: Option<RenderTierArg>) {
    if let Some(tier) = render_tier {
        println!(
            "[note] --render-tier {} is not plumbed to on-device runs yet (desktop-preview only \
             in v1); the device still probes its own render tier.",
            tier.env_value()
        );
    }
}

/// Dispatches a resolved [`Device`] to its platform's mode/flavor-aware
/// drive pipeline (spec §12.4's Android path, task 66's mode-aware Android
/// and iOS-simulator paths, task 67's signed physical-iOS-device path).
fn run_on_device(runner: &dyn ProcessRunner, device: &Device, info: &BuildInfo) -> Result<u8> {
    match (device.platform, device.kind) {
        (Platform::Android, _) => run_android(runner, device, info),
        (Platform::Ios, Kind::Simulator) => {
            let cwd = std::env::current_dir().context("reading current directory")?;
            ios_run::run(runner, &cwd, device, info)
        }
        (Platform::Ios, Kind::PhysicalDevice) => {
            let cwd = std::env::current_dir().context("reading current directory")?;
            ios_run::run_physical(runner, &cwd, device, info)
        }
        (Platform::Ios, Kind::Emulator) => {
            unreachable!("iOS devices are never discovered as Kind::Emulator")
        }
    }
}

/// No Android device connected and no `-d`: run the desktop preview shell
/// exactly like a bare `cargo run` (spec §12.9's dev loop), streaming its
/// output rather than buffering it until exit.
///
/// Threads the resolved [`BuildInfo`] through to the spawn (verified
/// pre-extraction gap — the old fallback ignored `--profile` and dropped
/// every `--define`): the build mode selects the cargo profile arg
/// (`--release`/`--profile profile`) and every define is passed as an
/// environment variable to the launched process. A `--profile` run therefore
/// reaches the desktop preview with `FRUST_TRACE=1` set (the define
/// `BuildInfo::from_args` auto-injects), which is what makes desktop perf
/// tracing work. A `--render-tier` override adds [`RENDER_TIER_ENV_VAR`] on
/// top — the one path that flag is actually plumbed to in v1 (see
/// `Command::Run`'s doc comment).
fn run_desktop_fallback(
    runner: &dyn ProcessRunner,
    info: &BuildInfo,
    render_tier: Option<RenderTierArg>,
) -> Result<u8> {
    println!("No Android device connected; falling back to `cargo run` (desktop preview).");
    let mut on_line = |line: &str| println!("{line}");
    let args = desktop_cargo_run_args(info);
    let env = desktop_cargo_run_env(info, render_tier);
    let out = runner.run_streaming("cargo", &args, None, &env, &mut on_line)?;
    Ok(if out.success { 0 } else { 1 })
}

/// The `cargo run` argv the desktop fallback spawns: always `run`, plus the
/// build mode's cargo profile arg (`[]`/`--profile profile`/`--release`) so
/// a `frust run --release`/`--profile` desktop preview builds in the
/// requested profile instead of always debug.
fn desktop_cargo_run_args(info: &BuildInfo) -> Vec<&'static str> {
    let mut args = vec!["run"];
    args.extend_from_slice(info.mode.cargo_profile_arg());
    args
}

/// The env pairs [`run_desktop_fallback`]'s `cargo run` is spawned with:
/// every `--define KEY=VALUE` as `KEY=VALUE` (so e.g. a profile run's
/// auto-injected `FRUST_TRACE=1` reaches the preview process), plus a
/// `--render-tier` override as [`RENDER_TIER_ENV_VAR`]. Split out from the
/// spawning call so the mapping is unit-testable without a fake runner that
/// would otherwise ignore the `env` argument entirely (see
/// [`frust_drive::process::FakeProcessRunner::run_streaming`]).
fn desktop_cargo_run_env(
    info: &BuildInfo,
    render_tier: Option<RenderTierArg>,
) -> Vec<(&str, &str)> {
    let mut env: Vec<(&str, &str)> = info
        .defines
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    if let Some(tier) = render_tier {
        env.push((RENDER_TIER_ENV_VAR, tier.env_value()));
    }
    env
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
    use frust_drive::process::{FakeProcessRunner, Output};

    fn ios_physical_device() -> Device {
        Device {
            id: "00008110-000A2D3A3C68801E".to_string(),
            name: "Ed's iPhone".to_string(),
            platform: Platform::Ios,
            kind: Kind::PhysicalDevice,
            os_version: Some("17.5.1".to_string()),
            connection_state: None,
        }
    }

    fn debug_info() -> BuildInfo {
        BuildInfo::from_args(BuildArgs::default().into_drive(), BuildMode::Debug).unwrap()
    }

    fn profile_info() -> BuildInfo {
        BuildInfo::from_args(
            BuildArgs {
                profile: true,
                ..Default::default()
            }
            .into_drive(),
            BuildMode::Debug,
        )
        .unwrap()
    }

    /// The Phase-5 sentinel `run_on_device` used to bail a physical iOS
    /// device with is gone (task 67 wires `ios_run::run_physical` in
    /// instead). The test process's cwd is the `frust-cli` crate root,
    /// not a generated Frust project, so `run_on_device` now fails at
    /// `ios_run::run_physical`'s own `project::detect` step instead —
    /// proving dispatch reaches the real pipeline rather than the removed
    /// sentinel. The pipeline's own behavior (iOS-17+ gate, build/install/
    /// launch argv, failure hints) is covered by `ios_run::mod`'s tests
    /// against a fixture project directory.
    #[test]
    fn run_on_device_delegates_physical_ios_device_to_ios_run_run_physical() {
        let runner = FakeProcessRunner::new();
        let err = run_on_device(&runner, &ios_physical_device(), &debug_info()).unwrap_err();
        let message = err.to_string();
        assert!(!message.contains("lands in Phase 5"), "{message}");
        assert!(message.contains("frust.toml"), "{message}");
    }

    #[test]
    fn desktop_cargo_run_env_is_empty_without_defines_or_override() {
        assert_eq!(
            desktop_cargo_run_env(&debug_info(), None),
            Vec::<(&str, &str)>::new()
        );
    }

    #[test]
    fn desktop_cargo_run_env_sets_the_var_for_gpu() {
        assert_eq!(
            desktop_cargo_run_env(&debug_info(), Some(RenderTierArg::Gpu)),
            vec![(RENDER_TIER_ENV_VAR, "gpu")]
        );
    }

    #[test]
    fn desktop_cargo_run_env_sets_the_var_for_cpu() {
        assert_eq!(
            desktop_cargo_run_env(&debug_info(), Some(RenderTierArg::Cpu)),
            vec![(RENDER_TIER_ENV_VAR, "cpu")]
        );
    }

    /// Regression for the verified desktop-fallback gap (PLAN.md Phase 1
    /// step 1): a `--profile` desktop preview must (a) build in the profile
    /// cargo profile and (b) receive the auto-injected `FRUST_TRACE=1` as an
    /// environment variable, not silently drop both.
    #[test]
    fn desktop_fallback_threads_profile_mode_into_args_and_defines_into_env() {
        let info = profile_info();
        assert_eq!(
            desktop_cargo_run_args(&info),
            vec!["run", "--profile", "profile"]
        );
        let env = desktop_cargo_run_env(&info, None);
        assert!(env.contains(&("FRUST_TRACE", "1")), "{env:?}");
    }

    #[test]
    fn desktop_cargo_run_args_default_debug_is_bare_run() {
        assert_eq!(desktop_cargo_run_args(&debug_info()), vec!["run"]);
    }

    #[test]
    fn run_desktop_fallback_streams_cargo_run_regardless_of_render_tier() {
        // FakeProcessRunner's run_streaming ignores its env argument (keyed
        // only on cmd/args — see its doc comment), so this proves the
        // desktop fallback still reaches `cargo run` with a render-tier
        // override set; desktop_cargo_run_env's own tests above cover the
        // env-pair construction itself.
        let runner = FakeProcessRunner::new().with(
            "cargo run",
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let out = run_desktop_fallback(&runner, &debug_info(), Some(RenderTierArg::Cpu)).unwrap();
        assert_eq!(out, 0);
    }
}
