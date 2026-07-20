//! `frust run` (spec §12.4): Android drive pipeline, with a `cargo run`
//! desktop-preview fallback when no Android device is available.

use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use notify::{RecursiveMode, Watcher};

use crate::build_args::BuildArgs;
use crate::cli::RenderTierArg;
use frust_drive::android_run::{self, DeviceSelection};
use frust_drive::build_info::{BuildInfo, BuildMode};
use frust_drive::devices::{self, Device, Kind, Platform};
use frust_drive::ios_run;
use frust_drive::process::{ProcessRunner, StreamHandle};

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
    watch: bool,
    verbose: bool,
) -> Result<u8> {
    // `--watch` (PLAN.md Phase 9.D step 2) is desktop-preview only — its
    // kill/rebuild/relaunch loop only knows how to drive a local `cargo
    // run` child, not an installed device app. Reject the combination up
    // front rather than silently ignoring `--watch` or `-d`.
    if watch && device_id.is_some() {
        bail!(
            "--watch is desktop-preview only and cannot be combined with -d/--device-id; \
             drop -d to run the desktop preview, or drop --watch to run on a device"
        );
    }

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
        DeviceSelection::Desktop => run_desktop_fallback(runner, &info, render_tier, watch),
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
    watch: bool,
) -> Result<u8> {
    println!("No Android device connected; falling back to `cargo run` (desktop preview).");
    let args = desktop_cargo_run_args(info);
    let env = desktop_cargo_run_env(info, render_tier);

    if watch {
        let cwd = std::env::current_dir().context("reading current directory")?;
        return run_desktop_watch(runner, &args, &env, &cwd);
    }

    let mut on_line = |line: &str| println!("{line}");
    let out = runner.run_streaming("cargo", &args, None, &env, &mut on_line)?;
    Ok(if out.success { 0 } else { 1 })
}

/// Poll cadence for [`watch_loop`]'s inner loop — bounds both output latency
/// (how promptly a streamed `cargo run` line is printed) and spontaneous-exit
/// detection latency (how promptly a build failure is noticed) without
/// busy-spinning between polls.
const WATCH_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Debounce window for [`watch_loop`]: further raw change ticks arriving
/// within this window of the previous one collapse into a single
/// kill+relaunch, so a save that fires several raw filesystem events (an
/// editor's rename-then-write, a formatter's follow-up write, …) triggers one
/// relaunch, not several.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(300);

/// `frust run --watch`'s desktop-only file-watch → rebuild → relaunch loop
/// (PLAN.md Phase 9.D step 2). Watches `<root>/src` (recursive) and
/// `<root>/Cargo.toml`; wires the real filesystem watcher's every raw event
/// straight into [`watch_loop`], the testable core that owns debouncing,
/// spawning, and kill+relaunch.
fn run_desktop_watch(
    runner: &dyn ProcessRunner,
    args: &[&str],
    env: &[(&str, &str)],
    root: &Path,
) -> Result<u8> {
    let (raw_tx, raw_rx) = mpsc::channel();
    let _watcher = spawn_fs_watcher(root, raw_tx)?;
    println!(
        "Watching `{}` for changes (Ctrl-C to exit)…",
        root.display()
    );
    let mut on_line = |line: &str| println!("{line}");
    watch_loop(runner, args, env, &raw_rx, WATCH_DEBOUNCE, &mut on_line)
}

/// Starts a real [`notify`] watcher over `root`'s `src/` tree (recursive) and
/// `Cargo.toml`, sending a `()` tick into `tx` for every raw filesystem event
/// (kind/path are irrelevant to [`watch_loop`], which only cares that
/// *something* changed). Returns the watcher itself — dropping it stops
/// watching, so the caller must keep it alive for the loop's duration.
///
/// Neither path existing is a hard error (a project mid-scaffold, or one
/// with no `src/` yet) — `--watch` still runs, just with nothing to watch
/// until the missing path is created (a re-run picks it up).
fn spawn_fs_watcher(root: &Path, tx: mpsc::Sender<()>) -> Result<notify::RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            // The receiver may already be gone (loop exited); a send
            // failure here is not this closure's problem to report.
            let _ = tx.send(());
        }
    })
    .context("failed to create filesystem watcher")?;

    let src = root.join("src");
    if src.is_dir() {
        watcher
            .watch(&src, RecursiveMode::Recursive)
            .with_context(|| format!("watching `{}`", src.display()))?;
    }
    let cargo_toml = root.join("Cargo.toml");
    if cargo_toml.is_file() {
        watcher
            .watch(&cargo_toml, RecursiveMode::NonRecursive)
            .with_context(|| format!("watching `{}`", cargo_toml.display()))?;
    }
    Ok(watcher)
}

/// The testable core of `frust run --watch`'s rebuild-relaunch loop: drives
/// `cargo run` through [`ProcessRunner::spawn_streaming`], killing and
/// relaunching it every time `raw_changes` reports a source change (after
/// debouncing bursts within `debounce` of each other). `on_line` receives
/// every line of the running child's stdout plus the loop's own status
/// lines, exactly like [`ProcessRunner::run_streaming`]'s callback.
///
/// `raw_changes` is deliberately raw/undebounced — production wires a real
/// filesystem watcher's every event straight through ([`spawn_fs_watcher`]);
/// tests send synthetic ticks directly, exercising the debounce behavior at
/// this seam without touching the filesystem.
///
/// A build/run failure (`cargo run` exiting non-zero, e.g. a compile error)
/// is reported through `on_line` but never ends the loop — it just leaves no
/// process running until the next change ticks a fresh attempt. The loop
/// itself only returns when `raw_changes` disconnects (the real watcher
/// dropped, or a test drops its sender) — in real usage that never happens
/// before Ctrl-C tears down the whole process (the default SIGINT
/// disposition reaches the foreground process group, killing the streamed
/// `cargo run` child right along with this process — no extra `ctrlc`
/// handler is needed here, unlike `android_run`/`ios_run`'s device-side
/// processes which live outside this terminal's process group).
fn watch_loop(
    runner: &dyn ProcessRunner,
    args: &[&str],
    env: &[(&str, &str)],
    raw_changes: &mpsc::Receiver<()>,
    debounce: Duration,
    on_line: &mut dyn FnMut(&str),
) -> Result<u8> {
    let mut current: Option<StreamHandle> = Some(runner.spawn_streaming("cargo", args, None, env)?);

    loop {
        if drain_available_lines(&mut current, on_line) {
            let success = current.take().expect("handle present in this arm").wait();
            if success {
                on_line("`cargo run` exited; waiting for a source change to relaunch…");
            } else {
                on_line("`cargo run` failed; watching for a source change to retry…");
            }
        }

        match raw_changes.recv_timeout(WATCH_POLL_INTERVAL) {
            Ok(()) => {
                // Trailing-edge debounce: keep consuming ticks that arrive
                // within `debounce` of the previous one before acting.
                while raw_changes.recv_timeout(debounce).is_ok() {}
                // Flush whatever the about-to-be-killed process already
                // produced before killing it, so a burst of output right
                // before the kill isn't silently dropped.
                drain_available_lines(&mut current, on_line);
                on_line("Change detected; rebuilding and relaunching…");
                if let Some(mut handle) = current.take() {
                    handle.kill();
                }
                current = Some(runner.spawn_streaming("cargo", args, None, env)?);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(mut handle) = current.take() {
                    handle.kill();
                }
                return Ok(0);
            }
        }
    }
}

/// Drains every line currently available from `current`'s child (if any),
/// forwarding each to `on_line`. Returns `true` if the drain ended because
/// the child's stdout disconnected (the process has exited, spontaneously or
/// via a prior [`StreamHandle::kill`]) rather than simply having nothing more
/// buffered right now — the caller reads the exit status via
/// [`StreamHandle::wait`] in that case. A no-op (returns `false`) when
/// `current` is `None`.
fn drain_available_lines(
    current: &mut Option<StreamHandle>,
    on_line: &mut dyn FnMut(&str),
) -> bool {
    let Some(handle) = current.as_mut() else {
        return false;
    };
    loop {
        match handle.lines.try_recv() {
            Ok(line) => on_line(&line),
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => return true,
        }
    }
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
        let out =
            run_desktop_fallback(&runner, &debug_info(), Some(RenderTierArg::Cpu), false).unwrap();
        assert_eq!(out, 0);
    }

    /// `--watch` combined with an explicit `-d <device>` is a hard error,
    /// before any device discovery/build happens (`FakeProcessRunner::new()`
    /// has no registered responses, so any discovery/build call would fail
    /// loudly and prove this check didn't run first).
    #[test]
    fn run_in_rejects_watch_combined_with_device_id() {
        let runner = FakeProcessRunner::new();
        let err = run_in(
            &runner,
            BuildArgs::default(),
            Some("emulator-5554".to_string()),
            None,
            true,
            false,
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("--watch"), "{message}");
        assert!(message.contains("device"), "{message}");
    }

    /// A raw change tick kills the running child and relaunches a fresh
    /// `cargo run` — the core change→kill→rebuild→relaunch sequence
    /// (acceptance criterion 2). The hanging stream's one scripted line is
    /// replayed from scratch on every spawn, so seeing it exactly twice
    /// proves two genuinely separate spawns happened (the initial one, and
    /// a real relaunch after the kill) rather than the first process simply
    /// continuing to stream.
    #[test]
    fn watch_loop_relaunches_on_a_change_tick() {
        let runner = FakeProcessRunner::new().with_hanging_stream("cargo run", ["hello"]);
        let (raw_tx, raw_rx) = mpsc::channel();
        let mut lines = Vec::new();
        let mut on_line = |line: &str| lines.push(line.to_string());

        let handle_thread = std::thread::spawn(move || {
            // Send one change tick shortly after start, then drop the
            // sender so `watch_loop` exits (a real run never exits this
            // way; the test needs a deterministic end).
            std::thread::sleep(Duration::from_millis(30));
            raw_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(150));
            drop(raw_tx);
        });

        let out = watch_loop(
            &runner,
            &["run"],
            &[],
            &raw_rx,
            Duration::from_millis(10),
            &mut on_line,
        )
        .unwrap();
        handle_thread.join().unwrap();

        assert_eq!(out, 0);
        let hello_count = lines.iter().filter(|l| l.as_str() == "hello").count();
        assert_eq!(hello_count, 2, "{lines:?}");
        assert!(
            lines.iter().any(|l| l.contains("Change detected")),
            "{lines:?}"
        );
    }

    /// Several raw ticks arriving within the debounce window collapse into a
    /// single relaunch (acceptance criterion 2's debounce case) — only one
    /// "Change detected" status line for a burst of 5 ticks sent 5ms apart
    /// under a 50ms debounce window.
    #[test]
    fn watch_loop_debounces_a_burst_of_ticks_into_one_relaunch() {
        let runner = FakeProcessRunner::new().with_hanging_stream("cargo run", Vec::<&str>::new());
        let (raw_tx, raw_rx) = mpsc::channel();
        let mut lines = Vec::new();
        let mut on_line = |line: &str| lines.push(line.to_string());

        let handle_thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            for _ in 0..5 {
                raw_tx.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(5));
            }
            // Let the debounce window (50ms) elapse with no further ticks,
            // then end the test deterministically.
            std::thread::sleep(Duration::from_millis(100));
            drop(raw_tx);
        });

        let out = watch_loop(
            &runner,
            &["run"],
            &[],
            &raw_rx,
            Duration::from_millis(50),
            &mut on_line,
        )
        .unwrap();
        handle_thread.join().unwrap();

        assert_eq!(out, 0);
        let relaunches = lines
            .iter()
            .filter(|l| l.contains("Change detected"))
            .count();
        assert_eq!(relaunches, 1, "{lines:?}");
    }

    /// A `cargo run` that exits non-zero on its own (a build failure) is
    /// reported but keeps the loop watching rather than ending it
    /// (acceptance criterion 2's build-failure case) — a subsequent change
    /// tick still triggers a fresh relaunch attempt.
    #[test]
    fn watch_loop_survives_a_build_failure_and_keeps_watching() {
        let runner = FakeProcessRunner::new().with_stream("cargo run", ["error[E0000]"], false);
        let (raw_tx, raw_rx) = mpsc::channel();
        let mut lines = Vec::new();
        let mut on_line = |line: &str| lines.push(line.to_string());

        let handle_thread = std::thread::spawn(move || {
            // Give the failed stream time to exit and be noticed before
            // ending the test.
            std::thread::sleep(Duration::from_millis(150));
            drop(raw_tx);
        });

        let out = watch_loop(
            &runner,
            &["run"],
            &[],
            &raw_rx,
            Duration::from_millis(10),
            &mut on_line,
        )
        .unwrap();
        handle_thread.join().unwrap();

        assert_eq!(out, 0);
        assert!(
            lines.iter().any(|l| l.contains("error[E0000]")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("failed") && l.contains("retry")),
            "{lines:?}"
        );
    }
}
