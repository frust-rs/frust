//! `frust run`: Android drive pipeline, with a `cargo run`
//! desktop-preview fallback when no Android device is available, and a
//! `-d web` browser lane that builds for `wasm32` and serves the artifact
//! directory instead of installing/launching anything.

use std::collections::BTreeSet;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use notify::{RecursiveMode, Watcher};

use crate::cli::BuildFlags;
use frust_drive::android_run::{self, DeviceSelection};
use frust_drive::build_info::{BuildInfo, BuildMode};
use frust_drive::desktop_run::{self, DesktopPlan};
use frust_drive::devices::{self, Device, Kind, Platform};
use frust_drive::hotpatch::graph::WorkspaceGraph;
use frust_drive::hotpatch::session::{
    DesktopStart, HotSession, Outcome, SessionHost, StartError, start_desktop,
};
use frust_drive::ios_run;
use frust_drive::manifest;
use frust_drive::packages::CargoLocator;
use frust_drive::platform_wiring;
use frust_drive::process::{ProcessRunner, RealProcessRunner, StreamHandle, TryRecvError};
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
    no_hot: bool,
    verbose: bool,
    no_open: bool,
) -> Result<u8> {
    run_in_with_hooks(
        runner,
        build_args,
        device_id,
        watch,
        no_hot,
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
#[allow(clippy::too_many_arguments)]
fn run_in_with_hooks(
    runner: &dyn ProcessRunner,
    build_args: BuildFlags,
    device_id: Option<String>,
    watch: bool,
    no_hot: bool,
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
        return run_desktop_fallback(runner, &info, &extra_features, watch, no_hot, hooks.watch);
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
            run_desktop_fallback(runner, &info, &extra_features, watch, no_hot, hooks.watch)
        }
        DeviceSelection::Auto(device) => run_on_device(runner, &device, &info, &extra_features),
        DeviceSelection::Ambiguous(candidates) => {
            select_from_prompt(runner, &candidates, &info, &extra_features)
        }
        DeviceSelection::Error(message) => bail!(message),
    }
}

/// Dispatches a resolved [`Device`] to its platform's mode/flavor-aware
/// drive pipeline, after [`refresh_platform_wiring`] has brought the
/// project's Android/iOS embedding wiring up to date.
fn run_on_device(
    runner: &dyn ProcessRunner,
    device: &Device,
    info: &BuildInfo,
    extra_features: &[String],
) -> Result<u8> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    refresh_platform_wiring(runner, &cwd, &mut |line| println!("{line}"));
    match (device.platform, device.kind) {
        (Platform::Android, _) => run_android(runner, device, info, extra_features),
        (Platform::Ios, Kind::Simulator) => {
            ios_run::run(runner, &cwd, device, info, extra_features)
        }
        (Platform::Ios, Kind::PhysicalDevice) => {
            ios_run::run_physical(runner, &cwd, device, info, extra_features)
        }
        (Platform::Ios, Kind::Emulator) => {
            unreachable!("iOS devices are never discovered as Kind::Emulator")
        }
    }
}

/// Points `project_dir`'s `android/gradle.properties` and `ios/FrustEmbedding`
/// at the embedding modules of the shell crates cargo resolves through
/// `runner` — the step that keeps a project building after `cargo update`
/// moves those crates, or when it was created with `--no-sync`. Shared by
/// `frust run` and `frust build`'s Android/iOS lanes, ahead of Gradle/Xcode.
///
/// Emits one line per platform it rewrote (nothing when everything was
/// already current). A failure is a warning, never an error: the project
/// may still carry a valid wiring from an earlier run, and when it does not,
/// the Gradle/Xcode failure that follows names the unresolved path.
pub(crate) fn refresh_platform_wiring(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    on_line: &mut dyn FnMut(&str),
) {
    match platform_wiring::sync_with(&CargoLocator::new(runner), project_dir) {
        Ok(report) => {
            for line in report.lines() {
                on_line(&line);
            }
        }
        Err(err) => on_line(&format!(
            "warning: could not refresh the Android/iOS embedding wiring: {err}"
        )),
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
    no_hot: bool,
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
        // Hot is the default for a debug desktop `--watch`; `--no-hot`, a
        // Profile/Release build, a `--features` passthrough (the session's
        // fat build is the mode's own feature list) and a hook set with no
        // hot backend all keep today's relaunch loop exactly.
        let hot = !no_hot && info.mode == BuildMode::Debug && extra_features.is_empty();
        if hot && hooks.hot.is_some() {
            return run_desktop_hot(runner, info, &plan, &env, &cwd, hooks);
        }
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
    /// The hot-session seams; `None` keeps the plain relaunch loop (a test
    /// that drives the cold loop, or a build that cannot be hot).
    hot: Option<HotHooks>,
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
            hot: Some(HotHooks::real()),
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
            hot: None,
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

/// Debounce window for [`watch_loop`]: trailing-edge coalescing keeps consuming
/// raw change ticks arriving within this window before acting, so a save that fires
/// several raw filesystem events (an editor's rename-then-write, a formatter's
/// follow-up write, …) triggers one relaunch, not several. 100 ms covers
/// atomic-save and format-on-save bursts, and measured milestone-1 steady-state
/// save->`on_change` latency at 312–324 ms.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(100);

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
    // Judged relative to the package directory, so `src/build/` is source
    // and only the package's own `target/`/`build/` would be noise.
    let roots = vec![root.to_path_buf()];
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            tick_on_relevant_event(&roots, &event, &tx);
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
    // A caller that already put a live child in the slot (the hot loop falling
    // back after its session turned out restart-only) keeps it; the first
    // change replaces it like any other.
    if lock_slot(current).is_none() {
        let handle = spawn_preview(runner, plan, env)?;
        lock_slot(current).replace(handle);
    }

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

// ---------------------------------------------------------------------------
// Hot mode: `frust run --watch` on a debug desktop build drives a
// `frust_drive::hotpatch::session` instead of relaunching `cargo run`.
// ---------------------------------------------------------------------------

/// What the hot watcher registers, by the session graph's path classes. The
/// directories are watched recursively, the files individually; a path that
/// does not exist is skipped by the real watcher, not an error.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct WatchSet {
    /// `src/` of every workspace member: a thin build can patch these.
    replayable: Vec<PathBuf>,
    /// `src/` of every local path package outside the workspace: only a
    /// fat rebuild picks these up, so the session answers with a restart.
    local_non_member: Vec<PathBuf>,
    /// Manifests, build scripts, the lockfile, cargo config and toolchain
    /// files: any change is a restart.
    build_inputs: Vec<PathBuf>,
    /// What a changed path is judged relative to ([`is_relevant_path`]):
    /// the workspace root and every package directory — never a `src/`
    /// tree, whose own `build/` or `target/` module is source.
    roots: Vec<PathBuf>,
}

impl WatchSet {
    /// Every directory to watch recursively.
    fn dirs(&self) -> impl Iterator<Item = &PathBuf> {
        self.replayable.iter().chain(&self.local_non_member)
    }
}

/// Derives the [`WatchSet`] from the session's workspace graph: member and
/// non-member `src/` trees, each package's manifest and build script plus
/// the workspace-level build inputs, and the package directories paths are
/// judged relative to.
fn watch_set_from_graph(graph: &WorkspaceGraph) -> WatchSet {
    let mut set = WatchSet::default();
    let mut inputs = BTreeSet::new();
    set.roots.push(graph.workspace_root().to_path_buf());
    for package in graph.packages() {
        let src = package.dir.join("src");
        if package.member {
            set.replayable.push(src);
        } else {
            set.local_non_member.push(src);
        }
        set.roots.push(package.dir.clone());
        inputs.insert(package.dir.join("Cargo.toml"));
        inputs.insert(package.dir.join("build.rs"));
    }
    let root = graph.workspace_root();
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "frust.toml",
        "rust-toolchain",
        "rust-toolchain.toml",
        ".cargo/config",
        ".cargo/config.toml",
    ] {
        inputs.insert(root.join(name));
    }
    set.build_inputs = inputs.into_iter().collect();
    set
}

/// The session surface the hot loop drives: [`HotSession`] in production, a
/// scripted fake in tests.
trait HotSessionHandle {
    fn on_change(&mut self, paths: &[PathBuf]) -> Outcome;
    /// Why the session can only restart, when it can.
    fn restart_only_reason(&self) -> Option<String>;
}

impl HotSessionHandle for HotSession {
    fn on_change(&mut self, paths: &[PathBuf]) -> Outcome {
        HotSession::on_change(self, paths)
    }

    fn restart_only_reason(&self) -> Option<String> {
        HotSession::restart_only_reason(self).map(str::to_string)
    }
}

/// Why [`HotBackend::start`] produced no session.
enum HotStartError {
    /// This project cannot be hot-patched; use the relaunch loop.
    Unavailable(String),
    /// The fat build or launch failed; try again on the next change.
    Failed(Vec<String>),
}

/// A started hot session and the app child it patches.
type HotLaunch = (Box<dyn HotSessionHandle>, StreamHandle);

/// Starts fresh fat sessions and reports what to watch. Real:
/// `hotpatch::session::start_desktop` over the project; fake in tests.
trait HotBackend {
    fn watch_set(&mut self) -> Result<WatchSet>;
    /// Fat-builds and launches a new session. `on_line` receives the build
    /// diagnostics and the app's early output.
    fn start(&mut self, on_line: &mut dyn FnMut(&str)) -> Result<HotLaunch, HotStartError>;
}

/// [`HotBackend`] over the real `frust_drive` session.
struct DriveHotBackend {
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    root: PathBuf,
    info: BuildInfo,
    package: String,
}

impl HotBackend for DriveHotBackend {
    fn watch_set(&mut self) -> Result<WatchSet> {
        let graph = WorkspaceGraph::load(
            &*self.runner,
            &self.root.join("Cargo.toml"),
            None,
            &self.package,
            None,
        )
        .map_err(|err| anyhow::anyhow!("{err}"))?;
        Ok(watch_set_from_graph(&graph))
    }

    fn start(&mut self, on_line: &mut dyn FnMut(&str)) -> Result<HotLaunch, HotStartError> {
        let host = SessionHost::current(Arc::clone(&self.runner))
            .map_err(|err| HotStartError::Unavailable(err.to_string()))?;
        let start = DesktopStart {
            root: &self.root,
            info: &self.info,
            package: &self.package,
            bin: None,
        };
        match start_desktop(&host, &start, on_line) {
            Ok((session, handle)) => Ok((Box::new(session), handle)),
            Err(StartError::RestartRequired(reason)) => {
                Err(HotStartError::Unavailable(reason.to_string()))
            }
            Err(StartError::FatBuildFailed { diagnostics }) => {
                Err(HotStartError::Failed(diagnostics))
            }
            Err(err @ StartError::Launch { .. }) => {
                Err(HotStartError::Failed(vec![err.to_string()]))
            }
        }
    }
}

type HotBackendHook = Box<dyn FnOnce(&Path, &BuildInfo) -> Result<Box<dyn HotBackend>>>;
type SpawnPathWatcherHook =
    Box<dyn FnOnce(&WatchSet, mpsc::Sender<PathBuf>) -> Result<Box<dyn std::any::Any>>>;

/// The injectable seams of hot mode, beside [`WatchHooks`]' own: the session
/// backend and a watcher that reports changed *paths* (the session needs
/// them; the relaunch loop's watcher reports bare ticks).
struct HotHooks {
    backend: HotBackendHook,
    spawn_watcher: SpawnPathWatcherHook,
}

impl HotHooks {
    fn real() -> Self {
        Self {
            backend: Box::new(|root, info| {
                let package = read_package_name(root)?;
                Ok(Box::new(DriveHotBackend {
                    runner: Arc::new(RealProcessRunner),
                    root: root.to_path_buf(),
                    info: info.clone(),
                    package,
                }) as Box<dyn HotBackend>)
            }),
            spawn_watcher: Box::new(|set, tx| {
                spawn_path_watcher(set, tx).map(|w| Box::new(w) as Box<dyn std::any::Any>)
            }),
        }
    }
}

/// The `[package] name` of `root`'s `Cargo.toml`.
fn read_package_name(root: &Path) -> Result<String> {
    let manifest = root.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest)
        .with_context(|| format!("reading `{}`", manifest.display()))?;
    package_name_from_manifest(&text).with_context(|| {
        format!(
            "`{}` has no `[package] name` (hot mode needs a package manifest)",
            manifest.display()
        )
    })
}

fn package_name_from_manifest(text: &str) -> Option<String> {
    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(rest) = line.strip_prefix("name") {
            let value = rest.trim_start().strip_prefix('=')?.trim();
            let value = value.split('#').next()?.trim();
            return Some(value.trim_matches(['"', '\'']).to_string());
        }
    }
    None
}

/// The paths of `event` that mean "a source or build input changed", or
/// `None` when the event is not such a change. Mirrors the TUI watcher's
/// filter. inotify (notify's Linux backend) reports plain reads as
/// `Access(Open)` / `Access(Close(Read))`, and a build reads every watched
/// file, so only content-changing kinds count: `Access(Close(Write))`,
/// `Any`, `Create`, `Modify` (except `Metadata(AccessTime)`), `Remove` and
/// `Other`. A path-less event is a rescan (`Some` of an empty vec). Noise
/// paths (see [`is_relevant_path`]) are dropped.
fn relevant_paths(roots: &[PathBuf], event: &notify::Event) -> Option<Vec<PathBuf>> {
    use notify::EventKind;
    use notify::event::{AccessKind, AccessMode, MetadataKind, ModifyKind};
    let kind_counts = match event.kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime)) => false,
        EventKind::Any
        | EventKind::Create(_)
        | EventKind::Modify(_)
        | EventKind::Remove(_)
        | EventKind::Other => true,
    };
    if !kind_counts {
        return None;
    }
    if event.paths.is_empty() {
        return Some(Vec::new());
    }
    let paths: Vec<PathBuf> = event
        .paths
        .iter()
        .filter(|path| is_relevant_path(roots, path))
        .cloned()
        .collect();
    (!paths.is_empty()).then_some(paths)
}

/// Whether `path` is a source path rather than editor/build noise, judged
/// relative to the deepest of `roots` it lies under — the workspace root
/// and the package directories, never a `src/` tree (so a checkout living
/// under a hidden directory is not noise, and a `src/build/` module is
/// source): nothing under `target`/`build` there, no hidden component
/// (`.git`, `.#lib.rs` Emacs locks, `.foo.rs.swp`) except a cargo config
/// (`.cargo/config`, `.cargo/config.toml`, a build input), and no
/// `~`-suffixed backup.
fn is_relevant_path(roots: &[PathBuf], path: &Path) -> bool {
    use std::path::Component;
    let rel = roots
        .iter()
        .filter_map(|root| path.strip_prefix(root).ok())
        .min_by_key(|rel| rel.components().count())
        .unwrap_or(path);
    let names: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let Some(first) = names.first() else {
        // The watch root itself: nothing to judge by name.
        return true;
    };
    if first == "target" || first == "build" {
        return false;
    }
    if let [dir, file] = names.as_slice()
        && dir == ".cargo"
        && (file == "config" || file == "config.toml")
    {
        return true;
    }
    if names.iter().any(|name| name.starts_with('.')) {
        return false;
    }
    !names.last().is_some_and(|last| last.ends_with('~'))
}

/// The relaunch watcher's handler: one tick per relevant event.
fn tick_on_relevant_event(roots: &[PathBuf], event: &notify::Event, tx: &mpsc::Sender<()>) {
    if relevant_paths(roots, event).is_some() {
        // The receiver may already be gone (loop exited); a send failure
        // here is not this handler's problem to report.
        let _ = tx.send(());
    }
}

/// The hot watcher's handler: forwards the relevant paths of an event. A
/// path-less rescan forwards one empty path, which wakes the loop without
/// naming a file (the session classifies it as unaffected).
fn forward_relevant_paths(roots: &[PathBuf], event: &notify::Event, tx: &mpsc::Sender<PathBuf>) {
    match relevant_paths(roots, event) {
        Some(paths) if paths.is_empty() => {
            let _ = tx.send(PathBuf::new());
        }
        Some(paths) => {
            for path in paths {
                let _ = tx.send(path);
            }
        }
        None => {}
    }
}

/// A real [`notify`] watcher over a [`WatchSet`], sending every changed path.
fn spawn_path_watcher(
    set: &WatchSet,
    tx: mpsc::Sender<PathBuf>,
) -> Result<notify::RecommendedWatcher> {
    let roots = set.roots.clone();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            forward_relevant_paths(&roots, &event, &tx);
        }
    })
    .context("failed to create filesystem watcher")?;
    for dir in set.dirs().filter(|dir| dir.is_dir()) {
        watcher
            .watch(dir, RecursiveMode::Recursive)
            .with_context(|| format!("watching `{}`", dir.display()))?;
    }
    for file in set.build_inputs.iter().filter(|file| file.is_file()) {
        watcher
            .watch(file, RecursiveMode::NonRecursive)
            .with_context(|| format!("watching `{}`", file.display()))?;
    }
    Ok(watcher)
}

/// `frust run --watch`'s hot mode: resolves the backend and the watch set,
/// then hands both to [`hot_loop`]. A setup failure is printed and answered
/// with the relaunch loop, never an error — `--watch` always works.
fn run_desktop_hot(
    runner: &dyn ProcessRunner,
    info: &BuildInfo,
    plan: &DesktopPlan,
    env: &[(String, String)],
    root: &Path,
    hooks: WatchHooks,
) -> Result<u8> {
    let WatchHooks {
        install_ctrlc,
        spawn_watcher,
        hot,
    } = hooks;
    let hot = hot.expect("run_desktop_hot is reached only with hot hooks");
    let setup = (hot.backend)(root, info).and_then(|mut backend| {
        let set = backend.watch_set()?;
        Ok((backend, set))
    });
    let (mut backend, set) = match setup {
        Ok(ok) => ok,
        Err(err) => {
            println!("hot reload unavailable: {err:#}; relaunching on change instead");
            let cold = WatchHooks {
                install_ctrlc,
                spawn_watcher,
                hot: None,
            };
            return run_desktop_watch(runner, plan, env, root, cold);
        }
    };

    let (path_tx, path_rx) = mpsc::channel();
    let _watcher = (hot.spawn_watcher)(&set, path_tx)?;
    println!(
        "Watching `{}` for changes with hot reload (Ctrl-C to exit; --no-hot relaunches instead)…",
        root.display()
    );
    let current: Arc<Mutex<Option<StreamHandle>>> = Arc::new(Mutex::new(None));
    install_ctrlc(Arc::clone(&current))?;
    let mut on_line = |line: &str| println!("{line}");
    hot_loop(
        runner,
        plan,
        env,
        &mut *backend,
        path_rx,
        WATCH_DEBOUNCE,
        &current,
        &mut on_line,
    )
}

/// How a (re)launch of the hot session ended.
enum Launched {
    Live(Box<dyn HotSessionHandle>),
    /// The build failed; nothing runs until the next change.
    Failed,
    /// Hot reload is unavailable here; the relaunch loop takes over.
    Cold,
}

/// Starts a fresh fat session and parks its child in `current`. A
/// restart-only session keeps its child (the relaunch loop adopts it) and
/// prints its failed precondition once.
fn launch_hot(
    backend: &mut dyn HotBackend,
    current: &Arc<Mutex<Option<StreamHandle>>>,
    on_line: &mut dyn FnMut(&str),
) -> Launched {
    match backend.start(on_line) {
        Ok((session, handle)) => {
            lock_slot(current).replace(handle);
            match session.restart_only_reason() {
                Some(reason) => {
                    on_line(&format!(
                        "hot reload unavailable: {reason}; relaunching on change instead"
                    ));
                    Launched::Cold
                }
                None => Launched::Live(session),
            }
        }
        Err(HotStartError::Unavailable(reason)) => {
            on_line(&format!(
                "hot reload unavailable: {reason}; relaunching on change instead"
            ));
            Launched::Cold
        }
        Err(HotStartError::Failed(diagnostics)) => {
            for line in &diagnostics {
                on_line(line);
            }
            on_line("hot build failed; watching for a source change to retry…");
            Launched::Failed
        }
    }
}

/// The hot counterpart of [`watch_loop_with_slot`]: each debounced change set
/// goes to the session. `Patched` prints its one line and leaves the app
/// running; a compile error prints its diagnostics and leaves the app
/// untouched; `RestartRequired` prints its reason verbatim, kills the child
/// and starts a fresh fat session. When hot reload turns out to be
/// unavailable the loop becomes the relaunch loop over the same watcher.
#[allow(clippy::too_many_arguments)]
fn hot_loop(
    runner: &dyn ProcessRunner,
    plan: &DesktopPlan,
    env: &[(String, String)],
    backend: &mut dyn HotBackend,
    changes: mpsc::Receiver<PathBuf>,
    debounce: Duration,
    current: &Arc<Mutex<Option<StreamHandle>>>,
    on_line: &mut dyn FnMut(&str),
) -> Result<u8> {
    let mut session = match launch_hot(backend, current, on_line) {
        Launched::Live(session) => Some(session),
        Launched::Failed => None,
        Launched::Cold => {
            return relaunch_loop(runner, plan, env, changes, debounce, current, on_line);
        }
    };

    loop {
        {
            let mut slot = lock_slot(current);
            if drain_available_lines(&mut slot, on_line) {
                let success = slot.take().expect("handle present in this arm").wait();
                session = None;
                if success {
                    on_line("the app exited; waiting for a source change to relaunch…");
                } else {
                    on_line("the app failed; watching for a source change to retry…");
                }
            }
        }

        match changes.recv_timeout(WATCH_POLL_INTERVAL) {
            Ok(first) => {
                let mut paths = BTreeSet::from([first]);
                while let Ok(path) = changes.recv_timeout(debounce) {
                    paths.insert(path);
                }
                let paths: Vec<PathBuf> = paths.into_iter().collect();

                let outcome = match session.as_mut() {
                    Some(live) => {
                        drain_available_lines(&mut lock_slot(current), on_line);
                        live.on_change(&paths)
                    }
                    None => {
                        // No live app: a change is a retry, whatever it touched.
                        on_line("Change detected; rebuilding and relaunching…");
                        session = match launch_hot(backend, current, on_line) {
                            Launched::Live(next) => Some(next),
                            Launched::Failed => None,
                            Launched::Cold => {
                                return relaunch_loop(
                                    runner, plan, env, changes, debounce, current, on_line,
                                );
                            }
                        };
                        continue;
                    }
                };
                match outcome {
                    Outcome::Patched { .. } => on_line(&outcome.to_string()),
                    Outcome::NoChange => {}
                    Outcome::CompileFailed { diagnostics } => {
                        for line in &diagnostics {
                            on_line(line);
                        }
                        on_line("compile failed; the running app is untouched");
                    }
                    Outcome::RestartRequired(reason) => {
                        on_line(&format!("restart required: {reason}"));
                        if let Some(mut handle) = lock_slot(current).take() {
                            handle.kill();
                        }
                        session = match launch_hot(backend, current, on_line) {
                            Launched::Live(next) => Some(next),
                            Launched::Failed => None,
                            Launched::Cold => {
                                return relaunch_loop(
                                    runner, plan, env, changes, debounce, current, on_line,
                                );
                            }
                        };
                    }
                }
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

/// Hands the hot loop's change stream to the plain relaunch loop: a thread
/// folds each changed path into the bare tick that loop takes, and ends (so
/// the loop does) when the watcher drops. The slot may already hold a live
/// child, which the loop keeps until the first change.
fn relaunch_loop(
    runner: &dyn ProcessRunner,
    plan: &DesktopPlan,
    env: &[(String, String)],
    changes: mpsc::Receiver<PathBuf>,
    debounce: Duration,
    current: &Arc<Mutex<Option<StreamHandle>>>,
    on_line: &mut dyn FnMut(&str),
) -> Result<u8> {
    let (tick_tx, tick_rx) = mpsc::channel();
    std::thread::spawn(move || {
        for _ in changes {
            if tick_tx.send(()).is_err() {
                break;
            }
        }
    });
    watch_loop_with_slot(runner, plan, env, &tick_rx, debounce, current, on_line)
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
    fn ev(kind: notify::EventKind, path: Option<&str>) -> notify::Event {
        let event = notify::Event::new(kind);
        match path {
            Some(path) => event.add_path(PathBuf::from(path)),
            None => event,
        }
    }

    /// The package directory, as the watchers judge against.
    fn roots() -> Vec<PathBuf> {
        vec![PathBuf::from("/w")]
    }

    fn watcher_cases() -> Vec<(notify::Event, Option<Vec<PathBuf>>)> {
        use notify::EventKind as K;
        use notify::event::{
            AccessKind, AccessMode, CreateKind, DataChange, MetadataKind, ModifyKind, RemoveKind,
        };
        let p = |s: &str| Some(vec![PathBuf::from(s)]);
        vec![
            (
                ev(
                    K::Access(AccessKind::Open(AccessMode::Any)),
                    Some("/w/Cargo.toml"),
                ),
                None,
            ),
            (
                ev(
                    K::Access(AccessKind::Close(AccessMode::Read)),
                    Some("/w/src/lib.rs"),
                ),
                None,
            ),
            (
                ev(
                    K::Modify(ModifyKind::Metadata(MetadataKind::AccessTime)),
                    Some("/w/src/lib.rs"),
                ),
                None,
            ),
            (
                ev(
                    K::Modify(ModifyKind::Data(DataChange::Content)),
                    Some("/w/src/lib.rs"),
                ),
                p("/w/src/lib.rs"),
            ),
            (
                ev(K::Create(CreateKind::File), Some("/w/src/.#lib.rs")),
                None,
            ),
            (
                ev(K::Create(CreateKind::File), Some("/w/target/debug/x")),
                None,
            ),
            // Modules merely named like build output are source.
            (
                ev(K::Create(CreateKind::File), Some("/w/src/target/x.rs")),
                p("/w/src/target/x.rs"),
            ),
            (
                ev(
                    K::Modify(ModifyKind::Data(DataChange::Content)),
                    Some("/w/src/build/mod.rs"),
                ),
                p("/w/src/build/mod.rs"),
            ),
            (
                ev(K::Remove(RemoveKind::File), Some("/w/src/x.rs")),
                p("/w/src/x.rs"),
            ),
            (
                ev(
                    K::Access(AccessKind::Close(AccessMode::Write)),
                    Some("/w/src/lib.rs"),
                ),
                p("/w/src/lib.rs"),
            ),
            (ev(K::Any, None), Some(Vec::new())),
        ]
    }

    #[test]
    fn relevant_paths_keeps_content_changes_and_drops_reads_and_noise() {
        for (event, expected) in watcher_cases() {
            assert_eq!(relevant_paths(&roots(), &event), expected, "{event:?}");
        }
    }

    #[test]
    fn hot_path_watcher_handler_forwards_only_relevant_paths() {
        let (tx, rx) = mpsc::channel();
        for (event, _) in watcher_cases() {
            forward_relevant_paths(&roots(), &event, &tx);
        }
        drop(tx);
        let got: Vec<PathBuf> = rx.iter().collect();
        assert_eq!(
            got,
            vec![
                PathBuf::from("/w/src/lib.rs"),
                PathBuf::from("/w/src/target/x.rs"),
                PathBuf::from("/w/src/build/mod.rs"),
                PathBuf::from("/w/src/x.rs"),
                PathBuf::from("/w/src/lib.rs"),
                PathBuf::new(),
            ]
        );
    }

    #[test]
    fn relaunch_watcher_handler_ticks_only_for_relevant_events() {
        let (tx, rx) = mpsc::channel();
        for (event, expected) in watcher_cases() {
            tick_on_relevant_event(&roots(), &event, &tx);
            assert_eq!(
                rx.try_iter().count(),
                usize::from(expected.is_some()),
                "{event:?}"
            );
        }
    }

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

    fn wiring_project(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-run-wiring-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("android")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
        std::fs::write(
            dir.join("android/gradle.properties"),
            "frust.embedding.dir=unresolved\n",
        )
        .unwrap();
        dir.canonicalize().unwrap()
    }

    fn metadata_key(dir: &Path) -> String {
        format!(
            "cargo metadata --format-version 1 --manifest-path {}",
            dir.join("Cargo.toml").display()
        )
    }

    /// The refresh asks cargo through the injected runner and emits one line
    /// for the rewrite — then nothing on a second, already-current run.
    #[test]
    fn refresh_platform_wiring_reports_a_rewrite_once() {
        let dir = wiring_project("rewrite");
        let shell = dir.join("shells/frust-shell-android");
        std::fs::create_dir_all(shell.join(platform_wiring::ANDROID_EMBEDDING_REL)).unwrap();
        let runner = FakeProcessRunner::new().with(
            metadata_key(&dir),
            Output {
                success: true,
                stdout: format!(
                    r#"{{"packages":[{{"name":"frust-shell-android","manifest_path":"{}"}}]}}"#,
                    shell.join("Cargo.toml").display()
                ),
                stderr: String::new(),
            },
        );

        let mut lines = Vec::new();
        refresh_platform_wiring(&runner, &dir, &mut |line| lines.push(line.to_string()));
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("Android embedding: android/gradle.properties -> "),
            "{lines:?}"
        );

        lines.clear();
        refresh_platform_wiring(&runner, &dir, &mut |line| lines.push(line.to_string()));
        assert!(lines.is_empty(), "{lines:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cargo failure becomes one warning line, and the project is left as
    /// it was for Gradle to report on.
    #[test]
    fn refresh_platform_wiring_warns_instead_of_failing() {
        let dir = wiring_project("warns");
        let runner = FakeProcessRunner::new().with(
            metadata_key(&dir),
            Output {
                success: false,
                stdout: String::new(),
                stderr: "error: no matching package named `frust-ui` found".to_string(),
            },
        );
        let mut lines = Vec::new();
        refresh_platform_wiring(&runner, &dir, &mut |line| lines.push(line.to_string()));
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("warning: "), "{lines:?}");
        assert!(lines[0].contains("no matching package"), "{lines:?}");
        assert_eq!(
            std::fs::read_to_string(dir.join("android/gradle.properties")).unwrap(),
            "frust.embedding.dir=unresolved\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
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
        let out = run_desktop_fallback(
            &runner,
            &debug_info(),
            NO_EXTRA,
            false,
            false,
            WatchHooks::fake(),
        )
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
    /// `frust-cli` crate root), `run_web` fails at its embedder lookup (the
    /// `frust-shell-web` package is located through `cargo metadata`, which
    /// the fake runner has no answer for) rather than ever reaching
    /// `devices::discover_all` (no responses are registered for `adb`/`xcrun`
    /// here, so a discovery call would fail loudly and differently).
    #[test]
    fn run_in_with_device_web_skips_device_discovery() {
        let runner = FakeProcessRunner::new();
        let err = run_in_with_hooks(
            &runner,
            BuildFlags::default(),
            Some("web".to_string()),
            false,
            false,
            false,
            true,
            RunHooks::fake(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("frust-shell-web"), "{err}");
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
            false,
            true,
            RunHooks::fake(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("frust-shell-web"), "{err}");
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

    // ---- hot mode -------------------------------------------------------

    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use frust_drive::hotpatch::session::RestartReason;

    /// One scripted start: a session's outcomes (and restart-only reason), or a failure.
    type ScriptedStart = Result<(Vec<Outcome>, Option<String>), HotStartError>;

    /// A scripted [`HotSessionHandle`]: answers each `on_change` with the next
    /// outcome and records the paths it was given.
    struct FakeSession {
        outcomes: VecDeque<Outcome>,
        calls: Arc<Mutex<Vec<Vec<PathBuf>>>>,
        restart_only: Option<String>,
    }

    impl HotSessionHandle for FakeSession {
        fn on_change(&mut self, paths: &[PathBuf]) -> Outcome {
            self.calls.lock().unwrap().push(paths.to_vec());
            self.outcomes.pop_front().expect("scripted outcome")
        }

        fn restart_only_reason(&self) -> Option<String> {
            self.restart_only.clone()
        }
    }

    /// One scripted `HotBackend::start`: `Ok(outcomes)` is a session answering
    /// them in order; `Err` is a start failure.
    struct FakeBackend {
        runner: FakeProcessRunner,
        starts: VecDeque<ScriptedStart>,
        started: Arc<AtomicUsize>,
        calls: Arc<Mutex<Vec<Vec<PathBuf>>>>,
        set: WatchSet,
    }

    impl FakeBackend {
        fn new(starts: Vec<ScriptedStart>) -> Self {
            Self {
                runner: FakeProcessRunner::new().with_hanging_stream("app", ["up"]),
                starts: starts.into(),
                started: Arc::new(AtomicUsize::new(0)),
                calls: Arc::new(Mutex::new(Vec::new())),
                set: WatchSet::default(),
            }
        }
    }

    impl HotBackend for FakeBackend {
        fn watch_set(&mut self) -> Result<WatchSet> {
            Ok(self.set.clone())
        }

        fn start(&mut self, _on_line: &mut dyn FnMut(&str)) -> Result<HotLaunch, HotStartError> {
            self.started.fetch_add(1, Ordering::SeqCst);
            let (outcomes, restart_only) = self.starts.pop_front().expect("scripted start")?;
            let handle = self.runner.spawn_streaming("app", &[], None, &[]).unwrap();
            let session = FakeSession {
                outcomes: outcomes.into(),
                calls: Arc::clone(&self.calls),
                restart_only,
            };
            Ok((Box::new(session), handle))
        }
    }

    /// Drives [`hot_loop`] over `backend`, sending each `(delay, path)` tick,
    /// then lets the loop end by dropping the sender. Returns the printed lines.
    fn drive_hot(backend: &mut FakeBackend, ticks: &[&str]) -> Vec<String> {
        let runner = FakeProcessRunner::new().with_hanging_stream("cargo run", ["cold"]);
        let (tx, rx) = mpsc::channel();
        let ticks: Vec<PathBuf> = ticks.iter().map(PathBuf::from).collect();
        let sender = std::thread::spawn(move || {
            for path in ticks {
                std::thread::sleep(Duration::from_millis(120));
                tx.send(path).unwrap();
            }
            std::thread::sleep(Duration::from_millis(250));
        });
        let current = Arc::new(Mutex::new(None));
        let mut lines = Vec::new();
        let mut on_line = |line: &str| lines.push(line.to_string());
        let out = hot_loop(
            &runner,
            &bare_run_plan(),
            &[],
            backend,
            rx,
            Duration::from_millis(10),
            &current,
            &mut on_line,
        )
        .unwrap();
        sender.join().unwrap();
        assert_eq!(out, 0);
        lines
    }

    fn restart(reason: RestartReason) -> Outcome {
        Outcome::RestartRequired(reason)
    }

    /// `Patched` prints its one line and the app is neither killed nor
    /// relaunched: a single start, the session saw the changed path.
    #[test]
    fn hot_patched_prints_one_line_and_does_not_relaunch() {
        let mut backend = FakeBackend::new(vec![Ok((
            vec![Outcome::Patched {
                ms: 41,
                components: 3,
            }],
            None,
        ))]);
        let lines = drive_hot(&mut backend, &["/p/src/lib.rs"]);
        assert_eq!(backend.started.load(Ordering::SeqCst), 1, "{lines:?}");
        assert_eq!(
            lines
                .iter()
                .filter(|l| l.starts_with("patched in 41 ms (3 components rebuilt)"))
                .count(),
            1,
            "{lines:?}"
        );
        assert!(!lines.iter().any(|l| l.contains("rebuilding")), "{lines:?}");
        assert_eq!(
            *backend.calls.lock().unwrap(),
            vec![vec![PathBuf::from("/p/src/lib.rs")]]
        );
    }

    /// `RestartRequired` prints its reason verbatim, kills the child and
    /// starts a fresh fat session through the same backend.
    #[test]
    fn hot_restart_required_prints_the_reason_and_relaunches() {
        let reason = RestartReason::StateTypeChanged {
            changes: vec!["Counter".to_string()],
        };
        let verbatim = format!("restart required: {reason}");
        let mut backend = FakeBackend::new(vec![
            Ok((vec![restart(reason)], None)),
            Ok((Vec::new(), None)),
        ]);
        let lines = drive_hot(&mut backend, &["/p/src/state.rs"]);
        assert_eq!(backend.started.load(Ordering::SeqCst), 2, "{lines:?}");
        assert!(lines.contains(&verbatim), "{lines:?}");
    }

    /// A compile error prints the diagnostics and leaves the child running:
    /// one start, no relaunch.
    #[test]
    fn hot_compile_error_keeps_the_running_app() {
        let mut backend = FakeBackend::new(vec![Ok((
            vec![Outcome::CompileFailed {
                diagnostics: vec!["error[E0425]: cannot find value".to_string()],
            }],
            None,
        ))]);
        let lines = drive_hot(&mut backend, &["/p/src/lib.rs"]);
        assert_eq!(backend.started.load(Ordering::SeqCst), 1, "{lines:?}");
        assert!(
            lines.iter().any(|l| l.contains("error[E0425]")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("running app is untouched")),
            "{lines:?}"
        );
        assert!(!lines.iter().any(|l| l.contains("rebuilding")), "{lines:?}");
    }

    /// `NoChange` prints nothing and keeps the app.
    #[test]
    fn hot_no_change_is_silent() {
        let mut backend = FakeBackend::new(vec![Ok((vec![Outcome::NoChange], None))]);
        let lines = drive_hot(&mut backend, &["/p/README.md"]);
        assert_eq!(backend.started.load(Ordering::SeqCst), 1);
        assert!(
            !lines
                .iter()
                .any(|l| l.contains("patched") || l.contains("restart")),
            "{lines:?}"
        );
    }

    /// A restart-only session prints its failed precondition once, then the
    /// relaunch loop takes over: the first change kills the fat child and
    /// relaunches `cargo run`, and the session is never asked to patch.
    #[test]
    fn hot_restart_only_session_prints_the_precondition_once_and_relaunches_cold() {
        let mut backend = FakeBackend::new(vec![Ok((
            Vec::new(),
            Some("the app does not advertise the HotPatch capability".to_string()),
        ))]);
        let lines = drive_hot(&mut backend, &["/p/src/lib.rs"]);
        assert_eq!(backend.started.load(Ordering::SeqCst), 1, "{lines:?}");
        assert_eq!(
            lines
                .iter()
                .filter(|l| l.contains("hot reload unavailable"))
                .count(),
            1,
            "{lines:?}"
        );
        assert!(backend.calls.lock().unwrap().is_empty());
        assert!(
            lines.iter().any(|l| l.contains("Change detected")),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l == "cold"), "{lines:?}");
    }

    /// A project the session cannot build for hot use behaves as today.
    #[test]
    fn hot_unavailable_at_start_falls_back_to_the_relaunch_loop() {
        let mut backend = FakeBackend::new(vec![Err(HotStartError::Unavailable(
            "restart required: no debuginfo".to_string(),
        ))]);
        let lines = drive_hot(&mut backend, &["/p/src/lib.rs"]);
        assert!(
            lines.iter().any(|l| l.contains("no debuginfo")),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l == "cold"), "{lines:?}");
    }

    /// A failed fat build prints its diagnostics, runs nothing, and the next
    /// change starts a session again.
    #[test]
    fn hot_fat_build_failure_retries_on_the_next_change() {
        let mut backend = FakeBackend::new(vec![
            Err(HotStartError::Failed(vec!["error: boom".to_string()])),
            Ok((Vec::new(), None)),
        ]);
        let lines = drive_hot(&mut backend, &["/p/src/lib.rs"]);
        assert_eq!(backend.started.load(Ordering::SeqCst), 2, "{lines:?}");
        assert!(lines.iter().any(|l| l == "error: boom"), "{lines:?}");
    }

    /// Debounced bursts reach the session as one deduplicated change set.
    #[test]
    fn hot_changes_in_one_burst_reach_the_session_together() {
        let runner = FakeProcessRunner::new();
        let mut backend = FakeBackend::new(vec![Ok((vec![Outcome::NoChange], None))]);
        let (tx, rx) = mpsc::channel();
        let sender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            for path in ["/p/b.rs", "/p/a.rs", "/p/b.rs"] {
                tx.send(PathBuf::from(path)).unwrap();
            }
            std::thread::sleep(Duration::from_millis(250));
        });
        let current = Arc::new(Mutex::new(None));
        hot_loop(
            &runner,
            &bare_run_plan(),
            &[],
            &mut backend,
            rx,
            Duration::from_millis(50),
            &current,
            &mut |_| {},
        )
        .unwrap();
        sender.join().unwrap();
        assert_eq!(
            *backend.calls.lock().unwrap(),
            vec![vec![PathBuf::from("/p/a.rs"), PathBuf::from("/p/b.rs")]]
        );
    }

    /// A workspace at `/w` whose member `app` depends on the non-member path
    /// package `frust-material`.
    const METADATA: &str = r#"{
        "packages": [
            {"id": "path+file:///w/app#0.1.0", "name": "app", "source": null,
             "manifest_path": "/w/app/Cargo.toml",
             "targets": [
                {"name": "app", "kind": ["lib"], "crate_types": ["lib"], "src_path": "/w/app/src/lib.rs"},
                {"name": "app", "kind": ["bin"], "crate_types": ["bin"], "src_path": "/w/app/src/main.rs"}]},
            {"id": "path+file:///x/material#0.6.0", "name": "frust-material", "source": null,
             "manifest_path": "/x/material/Cargo.toml",
             "targets": [
                {"name": "frust_material", "kind": ["lib"], "crate_types": ["lib"], "src_path": "/x/material/src/lib.rs"}]}
        ],
        "workspace_members": ["path+file:///w/app#0.1.0"],
        "resolve": {"nodes": [
            {"id": "path+file:///w/app#0.1.0", "deps": [
                {"name": "frust_material", "pkg": "path+file:///x/material#0.6.0",
                 "dep_kinds": [{"kind": null, "target": null}]}]},
            {"id": "path+file:///x/material#0.6.0", "deps": []}],
         "root": "path+file:///w/app#0.1.0"},
        "workspace_root": "/w"
    }"#;

    /// The watch set classifies the graph into the session's three path
    /// classes: replayable members, local non-member path packages, and
    /// build inputs.
    #[test]
    fn the_watch_set_has_the_three_path_classes() {
        let graph = WorkspaceGraph::from_metadata(METADATA, "app", None).unwrap();
        let set = watch_set_from_graph(&graph);
        assert_eq!(set.replayable, vec![PathBuf::from("/w/app/src")]);
        assert_eq!(set.local_non_member, vec![PathBuf::from("/x/material/src")]);
        for input in [
            "/w/app/Cargo.toml",
            "/w/app/build.rs",
            "/x/material/Cargo.toml",
            "/w/Cargo.toml",
            "/w/Cargo.lock",
            "/w/.cargo/config.toml",
            "/w/rust-toolchain.toml",
        ] {
            assert!(
                set.build_inputs.contains(&PathBuf::from(input)),
                "{input} in {:?}",
                set.build_inputs
            );
        }
        let dirs: Vec<_> = set.dirs().cloned().collect();
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/w/app/src"),
                PathBuf::from("/x/material/src")
            ]
        );
        let mut roots = set.roots.clone();
        roots.sort();
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/w"),
                PathBuf::from("/w/app"),
                PathBuf::from("/x/material")
            ]
        );
        // Judged against those roots, a member's `src/build/` module is
        // source while its `target/` is not.
        assert!(is_relevant_path(
            &set.roots,
            Path::new("/w/app/src/build/mod.rs")
        ));
        assert!(!is_relevant_path(
            &set.roots,
            Path::new("/w/app/target/debug/app")
        ));
        assert!(!is_relevant_path(
            &set.roots,
            Path::new("/w/target/debug/app")
        ));
    }

    /// The hot watcher hook receives the backend's whole watch set, and
    /// `run_desktop_hot` drives the session from the paths it forwards.
    #[test]
    fn run_desktop_hot_registers_the_watch_set_and_feeds_changes_to_the_session() {
        let set = WatchSet {
            replayable: vec![PathBuf::from("/w/app/src")],
            local_non_member: vec![PathBuf::from("/x/material/src")],
            build_inputs: vec![PathBuf::from("/w/Cargo.toml")],
            roots: vec![
                PathBuf::from("/w"),
                PathBuf::from("/w/app"),
                PathBuf::from("/x/material"),
            ],
        };
        let mut backend = FakeBackend::new(vec![Ok((
            vec![Outcome::Patched {
                ms: 5,
                components: 1,
            }],
            None,
        ))]);
        backend.set = set.clone();
        let calls = Arc::clone(&backend.calls);
        let registered: Arc<Mutex<Option<WatchSet>>> = Arc::new(Mutex::new(None));
        let seen = Arc::clone(&registered);
        let hooks = WatchHooks {
            install_ctrlc: Box::new(|_| Ok(())),
            spawn_watcher: Box::new(|_, _| unreachable!("the tick watcher is for the cold loop")),
            hot: Some(HotHooks {
                backend: Box::new(move |_, _| Ok(Box::new(backend) as Box<dyn HotBackend>)),
                spawn_watcher: Box::new(move |set, tx| {
                    *seen.lock().unwrap() = Some(set.clone());
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(150));
                        tx.send(PathBuf::from("/w/app/src/lib.rs")).unwrap();
                        std::thread::sleep(Duration::from_millis(400));
                    });
                    Ok(Box::new(()) as Box<dyn std::any::Any>)
                }),
            }),
        };
        let runner = FakeProcessRunner::new();
        // The sender thread's `tx` clone is the only one: the loop ends when
        // that thread finishes and drops it.
        let out = run_desktop_hot(
            &runner,
            &debug_info(),
            &bare_run_plan(),
            &[],
            Path::new("/w/app"),
            hooks,
        )
        .unwrap();
        assert_eq!(out, 0);
        assert_eq!(registered.lock().unwrap().as_ref(), Some(&set));
        assert_eq!(
            *calls.lock().unwrap(),
            vec![vec![PathBuf::from("/w/app/src/lib.rs")]]
        );
    }

    /// `--no-hot` (and profile/release) never touch the hot hooks.
    #[test]
    fn no_hot_keeps_the_relaunch_loop_and_never_builds_a_backend() {
        // No scripted `cargo run`: reaching the relaunch loop is what fails.
        let runner = FakeProcessRunner::new();
        let hooks = RunHooks {
            watch: WatchHooks {
                install_ctrlc: Box::new(|_| Ok(())),
                spawn_watcher: Box::new(|_, _tx| {
                    // Dropping the sender ends the loop immediately.
                    Ok(Box::new(()) as Box<dyn std::any::Any>)
                }),
                hot: Some(HotHooks {
                    backend: Box::new(|_, _| panic!("--no-hot must not build a hot backend")),
                    spawn_watcher: Box::new(|_, _| panic!("--no-hot must not watch paths")),
                }),
            },
            web: WebRunHooks::fake(),
        };
        let err = run_in_with_hooks(
            &runner,
            BuildFlags::default(),
            None,
            true,
            true,
            false,
            false,
            hooks,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("cargo"), "{err:#}");
    }

    #[test]
    fn the_package_name_is_read_from_the_package_table() {
        let text =
            "[workspace]\nname = \"nope\"\n\n[package]\nversion = \"1\"\nname = \"my-app\" # c\n";
        assert_eq!(package_name_from_manifest(text).as_deref(), Some("my-app"));
        assert_eq!(
            package_name_from_manifest("[workspace]\nmembers = []\n"),
            None
        );
    }
}
