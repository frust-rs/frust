//! `frust run`: Android drive pipeline, with a `cargo run`
//! desktop-preview fallback when no Android device is available, and a
//! `-d web` browser lane that builds for `wasm32` and serves the artifact
//! directory instead of installing/launching anything.

use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use notify::{RecursiveMode, Watcher};

use crate::cli::BuildFlags;
use frust_drive::android_run::{self, DeviceSelection};
use frust_drive::build_info::{BuildInfo, BuildMode};
use frust_drive::desktop_run::{self, DesktopPlan};
use frust_drive::devices::{self, Device, Kind, Platform};
use frust_drive::ios_run;
use frust_drive::manifest;
use frust_drive::process::{ProcessRunner, StreamHandle, TryRecvError};
use frust_drive::web_build::{self, RequestLog};

/// The testable core of `run`, taking an injected [`ProcessRunner`].
/// `commands::dispatch` constructs the real runner and calls this (the CLI's
/// one `Real` construction site). A thin wrapper over [`run_in_with_hooks`],
/// always passing the real [`RunHooks`] — the production default.
pub fn run_in(
    runner: &dyn ProcessRunner,
    build_args: BuildFlags,
    device_id: Option<String>,
    watch: bool,
    verbose: bool,
    no_open: bool,
) -> Result<u8> {
    run_in_with_hooks(
        runner,
        build_args,
        device_id,
        watch,
        verbose,
        no_open,
        RunHooks::real(),
    )
}

/// [`run_in`]'s actual body, parameterized on [`RunHooks`] so a test can
/// reach every branch `run_in` reaches (including the `--watch` desktop-only
/// short-circuit and the `-d web` dev-server lane) without ever installing a
/// real, process-global Ctrl-C handler or filesystem watcher — see
/// [`RunHooks`]'s doc.
fn run_in_with_hooks(
    runner: &dyn ProcessRunner,
    build_args: BuildFlags,
    device_id: Option<String>,
    watch: bool,
    verbose: bool,
    no_open: bool,
    hooks: RunHooks,
) -> Result<u8> {
    // `--watch` is desktop-preview only — its
    // kill/rebuild/relaunch loop only knows how to drive a local `cargo
    // run` child, not an installed device app. Reject the combination up
    // front rather than silently ignoring `--watch` or `-d`.
    if watch && device_id.is_some() {
        bail!(
            "--watch is desktop-preview only and cannot be combined with -d/--device-id; \
             drop -d to run the desktop preview, or drop --watch to run on a device"
        );
    }

    // The `--features` passthrough is resolved before the funnel and travels
    // beside `info`, never inside it: `BuildInfo` validates what is being built
    // (mode/flavor/defines/version), while these features are appended to the
    // mode's own selection at each platform's cargo-argv site.
    let extra_features = build_args.extra_features();
    let info = BuildInfo::from_args(build_args.build.into_drive(), BuildMode::Debug)
        .map_err(|err| anyhow::anyhow!(err))?;

    // `--watch` always means the desktop preview (the bail above already
    // rejected the genuine `-d` + `--watch` contradiction) — short-circuit
    // here, before device discovery runs, so an attached-but-unselected
    // device (e.g. a charging phone) never makes `--watch`'s target
    // non-deterministic. Reaches the exact call the `Desktop` arm below
    // would make anyway, just one step earlier.
    if watch {
        return run_desktop_fallback(runner, &info, &extra_features, watch, hooks.watch);
    }

    // `-d web` is a reserved device id, not a discovered one: `frust-drive`'s
    // `devices::Platform` carries no browser variant (a wasm build has no
    // adb/simctl/devicectl counterpart to enumerate), so this is a CLI-side
    // sentinel checked before device discovery ever runs — mirroring
    // `--watch`'s own before-discovery short-circuit above, and for the same
    // reason: an attached-but-unselected device must never change what an
    // explicit `-d web` does.
    if device_id
        .as_deref()
        .is_some_and(|id| id.eq_ignore_ascii_case("web"))
    {
        let cwd = std::env::current_dir().context("reading current directory")?;
        return run_web(runner, &cwd, &info, &extra_features, no_open, hooks.web);
    }

    let discoverers = devices::default_discoverers();
    let (found, notes) = devices::discover_all(runner, &discoverers);
    if verbose {
        for note in &notes {
            println!("[note] {note}");
        }
    }

    match android_run::select_device(&found, device_id.as_deref()) {
        DeviceSelection::Desktop => {
            run_desktop_fallback(runner, &info, &extra_features, watch, hooks.watch)
        }
        DeviceSelection::Auto(device) => run_on_device(runner, &device, &info, &extra_features),
        DeviceSelection::Ambiguous(candidates) => {
            select_from_prompt(runner, &candidates, &info, &extra_features)
        }
        DeviceSelection::Error(message) => bail!(message),
    }
}

/// Dispatches a resolved [`Device`] to its platform's mode/flavor-aware
/// drive pipeline.
fn run_on_device(
    runner: &dyn ProcessRunner,
    device: &Device,
    info: &BuildInfo,
    extra_features: &[String],
) -> Result<u8> {
    match (device.platform, device.kind) {
        (Platform::Android, _) => run_android(runner, device, info, extra_features),
        (Platform::Ios, Kind::Simulator) => {
            let cwd = std::env::current_dir().context("reading current directory")?;
            ios_run::run(runner, &cwd, device, info, extra_features)
        }
        (Platform::Ios, Kind::PhysicalDevice) => {
            let cwd = std::env::current_dir().context("reading current directory")?;
            ios_run::run_physical(runner, &cwd, device, info, extra_features)
        }
        (Platform::Ios, Kind::Emulator) => {
            unreachable!("iOS devices are never discovered as Kind::Emulator")
        }
    }
}

/// Runs the desktop preview shell exactly like a bare `cargo run`,
/// streaming its output rather than buffering it until
/// exit. Reached either because no Android device is connected and no `-d`
/// was passed (the `DeviceSelection::Desktop` arm below), or because
/// `run_in` short-circuited here directly on seeing `--watch` — which
/// always means the desktop preview, skipping device discovery entirely so
/// an attached-but-unselected device never changes what `--watch` does.
///
/// The invocation itself is resolved by `frust_drive::desktop_run` — the one
/// desktop launch-plan construction site the workbench and the MCP server also
/// go through, so `frust run` and a `frust-tui` desktop session cannot drift
/// apart. This front-end contributes exactly one thing on top: the
/// release-lean preflight's resolved feature list
/// (`desktop_plan_with_features`).
///
/// What the plan carries is the resolved [`BuildInfo`] in full (verified
/// pre-extraction gap — the old fallback ignored `--profile` and dropped
/// every `--define`): the build mode selects the cargo profile arg
/// (`--release`/`--profile profile`) and every define is passed as an
/// environment variable to the launched process. A `--profile` run therefore
/// reaches the desktop preview with `FRUST_TRACE=1` set (the define
/// `BuildInfo::from_args` auto-injects), which is what makes desktop perf
/// tracing work.
fn run_desktop_fallback(
    runner: &dyn ProcessRunner,
    info: &BuildInfo,
    extra_features: &[String],
    watch: bool,
    hooks: WatchHooks,
) -> Result<u8> {
    println!("No Android device connected; falling back to `cargo run` (desktop preview).");
    let cwd = std::env::current_dir().context("reading current directory")?;
    // Release-lean preflight: a legacy app (no declared `lean`)
    // has it dropped here — with a one-time warning to stdout (the CLI
    // front-end owns printing) — so the desktop `cargo run` never carries an
    // undeclared `--features lean` and cargo's opaque hard error. The `frust`
    // crate's own `perf-trace`/`devtools` (debug/profile) are never filtered.
    let (features, warning) =
        frust_drive::cargo_manifest::resolve_release_features(&cwd, info.mode, extra_features);
    if let Some(warning) = warning {
        println!("{warning}");
    }
    let feature_refs: Vec<&str> = features.iter().map(String::as_str).collect();
    let plan = desktop_run::desktop_plan_with_features(&cwd, info, &feature_refs);
    let env = desktop_cargo_run_env(&plan);

    if watch {
        return run_desktop_watch(runner, &plan, &env, &cwd, hooks);
    }

    let mut on_line = |line: &str| println!("{line}");
    // `cwd` is `None` rather than `plan.cwd`: the plan's working directory IS
    // this process's current directory (that is what it was resolved from), so
    // inheriting it keeps the spawn byte-identical to the pre-convergence one.
    let out = runner.run_streaming(
        &plan.program,
        &arg_refs(&plan.args),
        None,
        &env_refs(&env),
        &mut on_line,
    )?;
    Ok(if out.success { 0 } else { 1 })
}

/// Borrow a resolved plan's owned argv as the `&[&str]` the
/// [`ProcessRunner`] seam takes.
fn arg_refs(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

/// Borrow resolved env pairs as the `&[(&str, &str)]` the [`ProcessRunner`]
/// seam takes.
fn env_refs(env: &[(String, String)]) -> Vec<(&str, &str)> {
    env.iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

/// Injectable seams for [`run_desktop_watch`]'s two process-wide side
/// effects — installing a Ctrl-C handler and starting a filesystem watcher —
/// each of which is a process-global, install-once resource: `ctrlc::set_handler`
/// outright errors on a second call anywhere in the same process, and a real
/// [`notify`] watcher touches the actual filesystem under the caller's cwd.
/// Production always uses [`WatchHooks::real`] (installed exactly once, by
/// `run_desktop_watch`, per `frust run --watch` process); a test injects a
/// no-op pair instead of ever reaching either real side effect — see
/// `run_in_with_hooks`.
type InstallCtrlcHook = Box<dyn FnOnce(Arc<Mutex<Option<StreamHandle>>>) -> Result<()>>;
type SpawnWatcherHook = Box<dyn FnOnce(&Path, mpsc::Sender<()>) -> Result<Box<dyn std::any::Any>>>;

struct WatchHooks {
    /// Installs the Ctrl-C handler that group-kills the shared `current`
    /// slot's live child before exiting.
    install_ctrlc: InstallCtrlcHook,
    /// Starts a filesystem watcher over `root`, forwarding a `()` tick into
    /// the given sender for every raw change. The returned box is a
    /// keep-alive handle only — the caller holds it for the watch loop's
    /// duration and never inspects it (dropping a real `notify` watcher
    /// stops it).
    spawn_watcher: SpawnWatcherHook,
}

impl WatchHooks {
    /// The production defaults: a real [`install_real_ctrlc_handler`] and a
    /// real [`spawn_fs_watcher`].
    fn real() -> Self {
        Self {
            install_ctrlc: Box::new(install_real_ctrlc_handler),
            spawn_watcher: Box::new(|root, tx| {
                spawn_fs_watcher(root, tx).map(|w| Box::new(w) as Box<dyn std::any::Any>)
            }),
        }
    }

    /// A no-op pair for tests: skips the real Ctrl-C install and hands back
    /// an inert keep-alive handle instead of starting a real filesystem
    /// watcher — reaching `run_desktop_watch` in a test must never touch
    /// either real process-global side effect.
    #[cfg(test)]
    fn fake() -> Self {
        Self {
            install_ctrlc: Box::new(|_current| Ok(())),
            spawn_watcher: Box::new(|_root, _tx| Ok(Box::new(()) as Box<dyn std::any::Any>)),
        }
    }
}

/// `run_in_with_hooks`'s combined injectable seam — [`WatchHooks`] for the
/// `--watch` desktop loop plus [`WebRunHooks`] for the `-d web` dev-server
/// wait, bundled so `run_in`/`run_in_with_hooks` carry one hooks parameter
/// rather than two unrelated ones that happen to always travel together.
struct RunHooks {
    watch: WatchHooks,
    web: WebRunHooks,
}

impl RunHooks {
    /// The production defaults: [`WatchHooks::real`] plus [`WebRunHooks::real`].
    fn real() -> Self {
        Self {
            watch: WatchHooks::real(),
            web: WebRunHooks::real(),
        }
    }

    /// A no-op pair for tests — see [`WatchHooks::fake`] and
    /// [`WebRunHooks::fake`].
    #[cfg(test)]
    fn fake() -> Self {
        Self {
            watch: WatchHooks::fake(),
            web: WebRunHooks::fake(),
        }
    }
}

/// Injectable seam for [`run_web`]'s "keep the dev server up until
/// interrupted" wait: `ctrlc::set_handler` is the same process-global,
/// install-once resource [`WatchHooks::install_ctrlc`] guards, so a test
/// reaching `run_web` must never install a real handler either — `-d web`
/// and `--watch` are mutually exclusive within one `frust run` invocation
/// (the combination bails before either hooks type is ever used), so this
/// never races [`WatchHooks`]'s own install.
type WaitForInterrupt = Box<dyn FnOnce() -> Result<()>>;

struct WebRunHooks {
    /// Blocks the calling thread until Ctrl-C (or an equivalent external
    /// signal) says the dev server should stop.
    wait_for_interrupt: WaitForInterrupt,
}

impl WebRunHooks {
    /// The production default: a real [`wait_for_ctrlc`].
    fn real() -> Self {
        Self {
            wait_for_interrupt: Box::new(wait_for_ctrlc),
        }
    }

    /// Returns immediately instead of blocking — reaching `run_web` in a
    /// test must never wait on a real Ctrl-C that will never arrive.
    #[cfg(test)]
    fn fake() -> Self {
        Self {
            wait_for_interrupt: Box::new(|| Ok(())),
        }
    }
}

/// Installs a process-wide Ctrl-C handler and blocks until it fires (or the
/// sending end is otherwise dropped), for [`run_web`]'s "serve until
/// interrupted" wait. Reached only through [`WebRunHooks::real`] — see that
/// type's doc for the one-handler-per-process rule this shares with
/// [`install_real_ctrlc_handler`].
fn wait_for_ctrlc() -> Result<()> {
    let (tx, rx) = mpsc::channel::<()>();
    ctrlc::set_handler(move || {
        let _ = tx.send(());
    })
    .context("failed to install Ctrl-C handler")?;
    let _ = rx.recv();
    Ok(())
}

/// Installs the real, process-wide Ctrl-C handler that group-kills
/// `current`'s live child before exiting `frust run --watch`.
///
/// **Warning: one handler per process.** `ctrlc::set_handler` can only be
/// installed once per process — a second call anywhere (e.g. a second
/// `--watch` invocation reached in the same process) errors outright. This
/// function is reached only through [`WatchHooks::real`], which
/// [`run_desktop_watch`] calls exactly once per production `--watch`
/// invocation; a test must go through [`WatchHooks::fake`] instead of ever
/// calling this directly (see
/// `run_in_with_watch_skips_device_discovery_even_when_a_device_is_present`).
fn install_real_ctrlc_handler(current: Arc<Mutex<Option<StreamHandle>>>) -> Result<()> {
    ctrlc::set_handler(move || {
        if let Some(mut handle) = current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            handle.kill();
        }
        std::process::exit(0);
    })
    .context("failed to install Ctrl-C handler")
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

/// `frust run --watch`'s desktop-only file-watch → rebuild → relaunch loop.
/// Watches `<root>/src` (recursive) and
/// `<root>/Cargo.toml` via `hooks.spawn_watcher`, wiring its every raw event
/// straight into [`watch_loop`], the testable core that owns debouncing,
/// spawning, and kill+relaunch; `hooks.install_ctrlc` installs the Ctrl-C
/// handler described below. Production always calls this with
/// [`WatchHooks::real`] (via [`run_in`]/`run_desktop_fallback`); a test
/// drives it with [`WatchHooks::fake`] instead so neither process-global
/// side effect is ever touched by `cargo test`.
fn run_desktop_watch(
    runner: &dyn ProcessRunner,
    plan: &DesktopPlan,
    env: &[(String, String)],
    root: &Path,
    hooks: WatchHooks,
) -> Result<u8> {
    let (raw_tx, raw_rx) = mpsc::channel();
    let _watcher = (hooks.spawn_watcher)(root, raw_tx)?;
    println!(
        "Watching `{}` for changes (Ctrl-C to exit)…",
        root.display()
    );

    // The streamed `cargo run` child is now spawned into its own process
    // group (`frust_drive::process::spawn_streaming`), so a kill reaches the
    // compiled preview binary it forks too — the orphaned-preview fix. That
    // same arrangement removes the child from this terminal's foreground
    // process group, so a bare Ctrl-C's SIGINT no longer reaches it: without
    // an explicit handler, exiting the watch loop would just orphan the live
    // preview (trading a kill-on-relaunch orphan for a Ctrl-C-to-exit orphan).
    // Share the live handle with a Ctrl-C handler that group-kills the current
    // child before exiting. Unlike `android_run`/`ios_run`'s handlers (whose
    // device-side child is unaffected by this group), this one MUST kill
    // before `exit`.
    let current: Arc<Mutex<Option<StreamHandle>>> = Arc::new(Mutex::new(None));
    (hooks.install_ctrlc)(Arc::clone(&current))?;

    let mut on_line = |line: &str| println!("{line}");
    watch_loop_with_slot(
        runner,
        plan,
        env,
        &raw_rx,
        WATCH_DEBOUNCE,
        &current,
        &mut on_line,
    )
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
/// before Ctrl-C tears down the whole process.
///
/// **Ctrl-C is no longer self-handling.** The streamed `cargo run` child now
/// runs in its own process group (`frust_drive::process::spawn_streaming`, so a
/// kill reaches the compiled preview binary it forks too), which also means the
/// terminal's SIGINT no longer reaches it — [`run_desktop_watch`] installs a
/// Ctrl-C handler over a shared handle slot to group-kill the current child
/// before exiting. This standalone `watch_loop` owns a private slot (no shared
/// handler); production drives [`watch_loop_with_slot`] with the shared one.
///
/// Test-only: it exists purely as the fixed-signature entry the loop's unit
/// tests drive; the shipped binary always goes through `watch_loop_with_slot`.
#[cfg(test)]
fn watch_loop(
    runner: &dyn ProcessRunner,
    plan: &DesktopPlan,
    env: &[(String, String)],
    raw_changes: &mpsc::Receiver<()>,
    debounce: Duration,
    on_line: &mut dyn FnMut(&str),
) -> Result<u8> {
    let current: Arc<Mutex<Option<StreamHandle>>> = Arc::new(Mutex::new(None));
    watch_loop_with_slot(runner, plan, env, raw_changes, debounce, &current, on_line)
}

/// [`watch_loop`]'s body, parameterized on a shared `current`-handle slot so a
/// Ctrl-C handler installed by [`run_desktop_watch`] can group-kill the live
/// child before the process exits (see [`watch_loop`]'s doc). Holds the slot's
/// lock only for the brief drain/kill/respawn steps — never across the blocking
/// `recv_timeout` — so the handler can always acquire it promptly.
fn watch_loop_with_slot(
    runner: &dyn ProcessRunner,
    plan: &DesktopPlan,
    env: &[(String, String)],
    raw_changes: &mpsc::Receiver<()>,
    debounce: Duration,
    current: &Arc<Mutex<Option<StreamHandle>>>,
    on_line: &mut dyn FnMut(&str),
) -> Result<u8> {
    lock_slot(current).replace(spawn_preview(runner, plan, env)?);

    loop {
        {
            let mut slot = lock_slot(current);
            if drain_available_lines(&mut slot, on_line) {
                let success = slot.take().expect("handle present in this arm").wait();
                if success {
                    on_line("`cargo run` exited; waiting for a source change to relaunch…");
                } else {
                    on_line("`cargo run` failed; watching for a source change to retry…");
                }
            }
        }

        match raw_changes.recv_timeout(WATCH_POLL_INTERVAL) {
            Ok(()) => {
                // Trailing-edge debounce: keep consuming ticks that arrive
                // within `debounce` of the previous one before acting.
                while raw_changes.recv_timeout(debounce).is_ok() {}
                let mut slot = lock_slot(current);
                // Flush whatever the about-to-be-killed process already
                // produced before killing it, so a burst of output right
                // before the kill isn't silently dropped.
                drain_available_lines(&mut slot, on_line);
                on_line("Change detected; rebuilding and relaunching…");
                if let Some(mut handle) = slot.take() {
                    handle.kill();
                }
                slot.replace(spawn_preview(runner, plan, env)?);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(mut handle) = lock_slot(current).take() {
                    handle.kill();
                }
                return Ok(0);
            }
        }
    }
}

/// Locks the shared `current`-handle slot, recovering from poisoning rather
/// than propagating a panic (a poisoned lock just means a prior holder panicked
/// mid-update; the loop can still drive the handle inside).
fn lock_slot(
    slot: &Arc<Mutex<Option<StreamHandle>>>,
) -> std::sync::MutexGuard<'_, Option<StreamHandle>> {
    slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
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
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => return true,
        }
    }
}

/// Spawn one streamed desktop-preview child from a resolved plan — the single
/// spawn shape both the plain fallback and the `--watch` loop's relaunches use.
///
/// `cwd` is `None` for the same reason [`run_desktop_fallback`]'s own spawn
/// passes it: the plan's working directory is this process's current
/// directory.
fn spawn_preview(
    runner: &dyn ProcessRunner,
    plan: &DesktopPlan,
    env: &[(String, String)],
) -> Result<StreamHandle> {
    runner.spawn_streaming(&plan.program, &arg_refs(&plan.args), None, &env_refs(env))
}

/// The env pairs [`run_desktop_fallback`]'s `cargo run` is spawned with: the
/// resolved plan's own environment (every `--define KEY=VALUE`, plus the
/// profile build's auto-injected `FRUST_TRACE=1` — see
/// `frust_drive::desktop_run`) and nothing else. This front-end used to append
/// a render-tier override of its own here; the renderer is no longer a choice,
/// so the plan's environment is the whole of it.
///
/// Split out from the spawning call so the mapping is unit-testable without a
/// fake runner that would otherwise ignore the `env` argument entirely (see
/// [`frust_drive::process::FakeProcessRunner::run_streaming`]).
fn desktop_cargo_run_env(plan: &DesktopPlan) -> Vec<(String, String)> {
    plan.env.clone()
}

fn select_from_prompt(
    runner: &dyn ProcessRunner,
    candidates: &[Device],
    info: &BuildInfo,
    extra_features: &[String],
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

    run_on_device(runner, &candidates[index], info, extra_features)
}

/// Drives the full, mode/flavor-aware Android pipeline on `device` —
/// delegates to `android_run::run`, which owns
/// the pipeline body (mirroring `ios_run::run`'s shape).
fn run_android(
    runner: &dyn ProcessRunner,
    device: &Device,
    info: &BuildInfo,
    extra_features: &[String],
) -> Result<u8> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    android_run::run(runner, &cwd, device, info, extra_features)
}

/// `frust run -d web`'s dev-server lane: builds through
/// [`web_build::build`] (the same pipeline `frust build web` drives), serves
/// the resulting artifact directory, optionally opens a browser at it, then
/// blocks (via `hooks.wait_for_interrupt`) until told to stop — the browser
/// counterpart of `run_android`/`ios_run::run` staying up until Ctrl-C, just
/// serving files instead of streaming device logs.
fn run_web(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    info: &BuildInfo,
    extra_features: &[String],
    no_open: bool,
    hooks: WebRunHooks,
) -> Result<u8> {
    // `web_build::build`'s entry point carries no parameter for it — the
    // same reason `build macos|windows|linux` refuse it
    // (`commands::build::reject_unplumbed_features`).
    if !extra_features.is_empty() {
        bail!(
            "`frust run -d web` does not support --features yet (requested: {}); \
             the browser pipeline (`frust_drive::web_build::build`) carries no parameter for it",
            extra_features.join(", ")
        );
    }

    let project_manifest = manifest::load_optional(project_dir).context("reading `frust.toml`")?;

    let mut on_line = |line: &str| println!("{line}");
    let report = web_build::build(runner, project_dir, info, &mut on_line)?;
    for note in &report.notes {
        println!("Note: {note}");
    }
    println!("Built: {}", report.root.display());

    let options = web_build::ServeOptions::from_manifest(project_manifest.as_ref());
    let on_request: RequestLog = Arc::new(|line: &str| println!("{line}"));
    let server = web_build::serve(&report.root, options, Some(on_request))?;
    println!(
        "Serving `{}` at {} (Ctrl-C to stop)…",
        server.root().display(),
        server.url()
    );

    if !no_open && let Err(err) = open_browser(runner, &server.url()) {
        println!("Note: could not open a browser automatically: {err:#}");
    }

    (hooks.wait_for_interrupt)()?;
    server.shutdown();
    Ok(0)
}

/// Best-effort opens `url` in the host's default browser through the
/// injected [`ProcessRunner`] — the seam every other child `run` starts goes
/// through, so the launch is scriptable in tests like the rest of the
/// command. The opener is [`browser_command`]'s: `open` on macOS, `cmd /C
/// start` on Windows, `xdg-open` elsewhere.
///
/// Spawned, never waited for: [`ProcessRunner::spawn_streaming`] returns as
/// soon as the opener is running — in its own process group, with its stdio
/// piped rather than inherited, so it can neither block this process nor
/// receive its Ctrl-C — and the [`StreamHandle`] it returns is dropped on the
/// spot. Dropping a handle neither kills nor joins the child (only an
/// explicit `kill`/`wait` does), so the opener runs to completion on its own
/// while `run_web` moves straight on to its interrupt wait. The one failure
/// this can observe is the spawn itself (no `xdg-open` on PATH, say), and
/// that is returned for the caller to render as a note: the dev server is
/// already up and its URL already printed, so a missing browser is a
/// degraded convenience, not a failed `run`.
fn open_browser(runner: &dyn ProcessRunner, url: &str) -> Result<()> {
    let (program, args) = browser_command(url);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let handle = runner
        .spawn_streaming(program, &args, None, &[])
        .with_context(|| format!("spawning `{program}` to open {url}"))?;
    drop(handle);
    Ok(())
}

/// The host's URL opener and its argv for `url`: `open` on macOS, `cmd /C
/// start "" <url>` on Windows (`start` is a `cmd.exe` builtin, not a
/// standalone executable — the empty title argument keeps a URL containing
/// `&` from being misparsed as a second `start` argument), `xdg-open`
/// elsewhere. Split out of [`open_browser`] so a test can register the exact
/// invocation with a [`frust_drive::process::FakeProcessRunner`].
fn browser_command(url: &str) -> (&'static str, Vec<String>) {
    if cfg!(target_os = "macos") {
        ("open", vec![url.to_string()])
    } else if cfg!(target_os = "windows") {
        (
            "cmd",
            vec![
                "/C".to_string(),
                "start".to_string(),
                String::new(),
                url.to_string(),
            ],
        )
    } else {
        ("xdg-open", vec![url.to_string()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The clap mirror `BuildFlags` wraps — still the funnel input every
    // `BuildInfo` fixture below is built from.
    use crate::build_args::BuildArgs;
    use frust_drive::process::{FakeProcessRunner, Output};

    /// No `--features` passthrough — byte-identical to the pre-passthrough
    /// invocation, which is what every case but an explicit passthrough test
    /// asserts against.
    const NO_EXTRA: &[String] = &[];

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

    fn release_info() -> BuildInfo {
        BuildInfo::from_args(
            BuildArgs {
                release: true,
                ..Default::default()
            }
            .into_drive(),
            BuildMode::Debug,
        )
        .unwrap()
    }

    /// A physical iOS device dispatches to `ios_run::run_physical` rather
    /// than bailing out. The test process's cwd is the `frust-cli` crate root,
    /// not a generated Frust project, so `run_on_device` fails at
    /// `ios_run::run_physical`'s own `project::detect` step instead —
    /// proving dispatch reaches the real pipeline rather than bailing early.
    /// The pipeline's own behavior (iOS-17+ gate, build/install/
    /// launch argv, failure hints) is covered by `ios_run::mod`'s tests
    /// against a fixture project directory.
    #[test]
    fn run_on_device_delegates_physical_ios_device_to_ios_run_run_physical() {
        let runner = FakeProcessRunner::new();
        let err =
            run_on_device(&runner, &ios_physical_device(), &debug_info(), NO_EXTRA).unwrap_err();
        let message = err.to_string();
        assert!(!message.contains("lands in Phase 5"), "{message}");
        assert!(message.contains("frust.toml"), "{message}");
    }

    /// The plan the desktop fallback resolves for `info`, with the mode's own
    /// (unfiltered) feature list — the declaring-app case every expectation
    /// below is written against.
    fn plan_for(info: &BuildInfo) -> DesktopPlan {
        desktop_run::desktop_plan(Path::new("/tmp/project"), info)
    }

    fn env_pairs(env: &[(String, String)]) -> Vec<(&str, &str)> {
        env_refs(env)
    }

    /// A bare `cargo run` plan (no features, no env) — the minimal shape the
    /// watch-loop tests script their [`FakeProcessRunner`] against, so those
    /// tests stay about the loop rather than about argv resolution (which
    /// `frust_drive::desktop_run`'s own tests own).
    fn bare_run_plan() -> DesktopPlan {
        desktop_run::desktop_plan_with_features(Path::new("/tmp/project"), &debug_info(), &[])
    }

    #[test]
    fn desktop_cargo_run_env_is_empty_without_defines() {
        let env = desktop_cargo_run_env(&plan_for(&debug_info()));
        assert_eq!(env_pairs(&env), Vec::<(&str, &str)>::new());
    }

    /// Regression for the verified desktop-fallback gap: a `--profile`
    /// desktop preview must (a) build in the profile
    /// cargo profile and (b) receive the auto-injected `FRUST_TRACE=1` as an
    /// environment variable, not silently drop both.
    ///
    /// Asserted against the plan `run_desktop_fallback` now resolves through
    /// `frust_drive::desktop_run` — the expectations themselves are the
    /// pre-convergence ones, unchanged.
    #[test]
    fn desktop_fallback_threads_profile_mode_into_args_and_defines_into_env() {
        let info = profile_info();
        let plan = plan_for(&info);
        assert_eq!(
            plan.args,
            vec![
                "run",
                "--profile",
                "profile",
                "--features",
                "frust/perf-trace",
                "--features",
                "frust/devtools"
            ]
        );
        let env = desktop_cargo_run_env(&plan);
        assert!(env_pairs(&env).contains(&("FRUST_TRACE", "1")), "{env:?}");
    }

    #[test]
    fn desktop_cargo_run_args_default_debug_carries_perf_trace_and_devtools_features() {
        // Debug desktop preview compiles instrumentation AND the in-app
        // devtools service in — one `--features` pair per selected feature.
        assert_eq!(
            plan_for(&debug_info()).args,
            vec![
                "run",
                "--features",
                "frust/perf-trace",
                "--features",
                "frust/devtools"
            ]
        );
    }

    #[test]
    fn desktop_cargo_run_args_release_carries_lean_not_perf_trace_or_devtools() {
        let info = release_info();
        assert_eq!(
            plan_for(&info).args,
            vec!["run", "--release", "--features", "lean"]
        );
    }

    /// Legacy direction: a release desktop preview whose
    /// preflight resolved to an EMPTY feature list (a legacy app that dropped
    /// `lean`) must produce argv with `--release` but no `--features` at all —
    /// never an undeclared `--features lean` cargo would reject. This is the
    /// preflight-resolved list the fallback threads into
    /// `desktop_plan_with_features`.
    #[test]
    fn desktop_cargo_run_args_with_dropped_lean_omits_features() {
        let plan = desktop_run::desktop_plan_with_features(
            Path::new("/tmp/project"),
            &release_info(),
            &[],
        );
        assert_eq!(plan.args, vec!["run", "--release"]);
    }

    /// The `--features` passthrough reaches the desktop `cargo run` argv as
    /// its own `--features <name>` pair, AFTER the mode's own pair(s) — the
    /// same append order the Android/iOS CSVs use, so one reading of the argv
    /// tells which features came from the mode and which from the flag.
    #[test]
    fn desktop_cargo_run_args_append_passthrough_features_after_the_modes_own() {
        let plan = desktop_run::desktop_plan_with_features(
            Path::new("/tmp/project"),
            &debug_info(),
            &["frust/perf-trace", "frust/devtools", "lean"],
        );
        assert_eq!(
            plan.args,
            vec![
                "run",
                "--features",
                "frust/perf-trace",
                "--features",
                "frust/devtools",
                "--features",
                "lean",
            ]
        );
    }

    /// Declaring direction: a declaring app resolves to
    /// `["lean"]`, giving byte-identical argv to the pure mode → argv
    /// mapping above.
    #[test]
    fn desktop_cargo_run_args_with_declared_lean_matches_pure_mapping() {
        let info = release_info();
        let plan =
            desktop_run::desktop_plan_with_features(Path::new("/tmp/project"), &info, &["lean"]);
        assert_eq!(plan.args, plan_for(&info).args);
        assert_eq!(plan.args, vec!["run", "--release", "--features", "lean"]);
    }

    #[test]
    fn run_desktop_fallback_streams_cargo_run() {
        // FakeProcessRunner's run_streaming ignores its env argument (keyed
        // only on cmd/args — see its doc comment), so this proves the desktop
        // fallback reaches `cargo run` at all; desktop_cargo_run_env's own
        // test above covers the env-pair construction itself.
        let runner = FakeProcessRunner::new().with(
            "cargo run --features frust/perf-trace --features frust/devtools",
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let out = run_desktop_fallback(&runner, &debug_info(), NO_EXTRA, false, WatchHooks::fake())
            .unwrap();
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
            BuildFlags::default(),
            Some("emulator-5554".to_string()),
            true,
            false,
            false,
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("--watch"), "{message}");
        assert!(message.contains("device"), "{message}");
    }

    /// `--watch` must short-circuit to the desktop preview
    /// *before* device discovery runs, so an attached-but-unselected device
    /// never changes what `--watch` does. `adb devices -l` is scripted to
    /// report a connected emulator — device discovery WOULD select it if it
    /// ran — while `cargo run` is deliberately left unregistered; reaching
    /// the desktop/watch path therefore fails with `spawn_streaming`'s
    /// distinct "No such file or directory ... cargo" error, not the Android
    /// pipeline's very different first-failure message ("missing Rust
    /// target ...", from `android_run`'s preflight `rustup target list
    /// --installed` check) — proving discovery was never consulted.
    ///
    /// Drives [`run_in_with_hooks`] directly (the exact dispatch logic
    /// [`run_in`] delegates to) with [`RunHooks::fake`] instead of calling
    /// public `run_in` — this test genuinely reaches `run_desktop_watch`
    /// (`--watch` always does), and a real `ctrlc::set_handler`/`notify`
    /// watcher has no place running during `cargo test` (the former is also
    /// process-global and install-once, so a second such test would error).
    #[test]
    fn run_in_with_watch_skips_device_discovery_even_when_a_device_is_present() {
        let runner = FakeProcessRunner::new().with(
            "adb devices -l",
            Output {
                success: true,
                stdout: "List of devices attached\n\
                         emulator-5554  device product:sdk_gphone64_arm64 model:sdk_gphone64_arm64 device:emulator64_arm64 transport_id:1\n"
                    .to_string(),
                stderr: String::new(),
            },
        );

        let err = run_in_with_hooks(
            &runner,
            BuildFlags::default(),
            None,
            true,
            false,
            false,
            RunHooks::fake(),
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("cargo"), "{message}");
        assert!(!message.contains("Rust target"), "{message}");
    }

    /// `-d web` is checked before device discovery too — with no
    /// `Cargo.toml`/`frust.toml` fixture in the test process's own cwd (the
    /// `frust-cli` crate root), `run_web` fails at its own `package_name`
    /// read rather than ever reaching `devices::discover_all` (no responses
    /// are registered for `adb`/`xcrun` here, so a discovery call would fail
    /// loudly and differently).
    #[test]
    fn run_in_with_device_web_skips_device_discovery() {
        let runner = FakeProcessRunner::new();
        let err = run_in_with_hooks(
            &runner,
            BuildFlags::default(),
            Some("web".to_string()),
            false,
            false,
            true,
            RunHooks::fake(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("Cargo.toml"), "{err}");
    }

    /// `-d web` is matched case-insensitively — `-d Web`/`-d WEB` reach the
    /// same lane as `-d web`.
    #[test]
    fn run_in_with_device_web_is_case_insensitive() {
        let runner = FakeProcessRunner::new();
        let err = run_in_with_hooks(
            &runner,
            BuildFlags::default(),
            Some("WEB".to_string()),
            false,
            false,
            true,
            RunHooks::fake(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("Cargo.toml"), "{err}");
    }

    /// `--watch -d web` is refused by the same pre-discovery bail every other
    /// `--watch` + `-d <device>` combination hits — the watch loop has no
    /// browser-side kill/rebuild/relaunch story either.
    #[test]
    fn run_in_rejects_watch_combined_with_device_web() {
        let runner = FakeProcessRunner::new();
        let err = run_in(
            &runner,
            BuildFlags::default(),
            Some("web".to_string()),
            true,
            false,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("--watch"), "{err}");
    }

    /// `-d web` refuses `--features` before touching the filesystem at all —
    /// mirrors `build_web_rejects_features_passthrough` in `commands::build`.
    /// The browser opener goes through the runner like every other child of
    /// `run`: the platform's exact invocation, registered as a scripted
    /// stream, is what gets spawned — and the handle is dropped without
    /// waiting, so the call returns while the fake "opener" is still alive.
    #[test]
    fn open_browser_spawns_the_platform_opener_through_the_runner() {
        let url = "http://127.0.0.1:8000/";
        let (program, args) = browser_command(url);
        let key = std::iter::once(program.to_string())
            .chain(args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        let runner = FakeProcessRunner::new().with_stream(key, Vec::<String>::new(), true);
        open_browser(&runner, url).expect("a registered opener spawns");
    }

    /// A spawn failure (no opener on PATH) is the caller's to report — it
    /// surfaces as the `Err` `run_web` renders as its "could not open a
    /// browser" note, instead of being swallowed inside the opener.
    #[test]
    fn open_browser_reports_a_spawn_failure_instead_of_swallowing_it() {
        let runner = FakeProcessRunner::new();
        let err = open_browser(&runner, "http://127.0.0.1:8000/").unwrap_err();
        let rendered = format!("{err:#}");
        assert!(rendered.contains("spawning `"), "{rendered}");
        assert!(rendered.contains("http://127.0.0.1:8000/"), "{rendered}");
    }

    #[test]
    fn run_web_rejects_features_passthrough() {
        let runner = FakeProcessRunner::new();
        let build = BuildFlags {
            features: vec!["devtools".to_string()],
            ..Default::default()
        };
        let err = run_in_with_hooks(
            &runner,
            build,
            Some("web".to_string()),
            false,
            false,
            true,
            RunHooks::fake(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("does not support --features"),
            "{err}"
        );
    }

    /// A raw change tick kills the running child and relaunches a fresh
    /// `cargo run` — the core change→kill→rebuild→relaunch sequence.
    /// The hanging stream's one scripted line is
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
            &bare_run_plan(),
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
    /// single relaunch — only one
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
            &bare_run_plan(),
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
    /// reported but keeps the loop watching rather than ending it — a
    /// subsequent change tick still triggers a fresh relaunch attempt.
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
            &bare_run_plan(),
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
