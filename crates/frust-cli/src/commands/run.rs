//! `frust run`: Android drive pipeline, with a `cargo run`
//! desktop-preview fallback when no Android device is available, and a
//! `-d web` browser lane that builds for `wasm32` and serves the artifact
//! directory instead of installing/launching anything.

use std::collections::BTreeSet;
use std::io::{BufRead, Write};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use notify::{RecursiveMode, Watcher};

use crate::cli::BuildFlags;
use frust_drive::android_run::{self, AndroidLaunch, DeviceSelection, StoppedShort, force_stop};
use frust_drive::build_info::{BuildInfo, BuildMode};
use frust_drive::desktop_run::{self, DesktopPlan};
use frust_drive::devices::{self, Device, Kind, Platform};
use frust_drive::devtools_client::adb_forward_remove;
use frust_drive::hotpatch::android::{AndroidHotStart, AndroidStart, start_android};
use frust_drive::hotpatch::graph::WorkspaceGraph;
use frust_drive::hotpatch::ios_sim::{IosSimHotStart, IosSimStart, start_ios_sim};
use frust_drive::hotpatch::session::{
    DesktopStart, HotSession, Outcome, RestartReason, SessionHost, StartError,
    redact_discovery_line, start_desktop,
};
use frust_drive::hotpatch::watch::{WATCH_DEBOUNCE, WatchSet, watch_set};
use frust_drive::ios_run::{self, IosLaunch, simctl};
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
    // `--watch -d web` has no watch loop at all (the browser lane serves
    // files; nothing relaunches it), so it is refused up front, before
    // anything runs. Every other `-d` is refused after discovery unless it
    // names an Android device or a booted iOS simulator
    // (`run_watch_on_device`).
    if watch
        && device_id
            .as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case("web"))
    {
        bail!(WATCH_DEVICE_REJECTION);
    }

    // The `--features` passthrough is resolved before the funnel and travels
    // beside `info`, never inside it: `BuildInfo` validates what is being built
    // (mode/flavor/defines/version), while these features are appended to the
    // mode's own selection at each platform's cargo-argv site.
    let extra_features = build_args.extra_features();
    let info = BuildInfo::from_args(build_args.build.into_drive(), BuildMode::Debug)
        .map_err(|err| anyhow::anyhow!(err))?;

    // `--watch` without `-d` always means the desktop preview —
    // short-circuit here, before device discovery runs, so an
    // attached-but-unselected device (e.g. a charging phone) never makes
    // `--watch`'s target non-deterministic. Reaches the exact call the
    // `Desktop` arm below would make anyway, just one step earlier. With an
    // explicit `-d`, the device is resolved and only an Android device or an
    // iOS simulator is watched (hot by default, the device relaunch loop
    // under `--no-hot`).
    if watch {
        return match device_id.as_deref() {
            None => {
                run_desktop_fallback(runner, &info, &extra_features, watch, no_hot, hooks.watch)
            }
            Some(id) => {
                let device = resolve_watch_device(runner, id, verbose)?;
                run_watch_on_device(runner, &device, &info, &extra_features, no_hot, hooks.watch)
            }
        };
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

/// The refusal for `--watch` with a `-d` that is neither an Android device
/// nor an iOS simulator (a physical iOS device, or `-d web`): their
/// pipelines have no watch loop (a patch a physical iOS device loaded would
/// need code signing).
const WATCH_DEVICE_REJECTION: &str = "--watch runs the desktop preview, an Android device, or an iOS \
     simulator in a debug build; web, a physical iOS device and other devices are not \
     watched; drop -d to run the desktop preview, or drop --watch to run on a device";

/// The one device `--watch -d <id>` names, through the same discovery and
/// `-d` matching a plain `frust run -d` uses (discovery already counts a
/// booted simulator `devicectl` also lists once).
fn resolve_watch_device(runner: &dyn ProcessRunner, id: &str, verbose: bool) -> Result<Device> {
    let discoverers = devices::default_discoverers();
    let (found, notes) = devices::discover_all(runner, &discoverers);
    if verbose {
        for note in &notes {
            println!("[note] {note}");
        }
    }
    match android_run::select_device(&found, Some(id)) {
        DeviceSelection::Auto(device) => Ok(device),
        DeviceSelection::Error(message) => bail!(message),
        // `select_device` answers a `-d` pattern with one device or an
        // error; these two arms are for the no-`-d` case.
        DeviceSelection::Desktop | DeviceSelection::Ambiguous(_) => {
            bail!("no single device matches `{id}`")
        }
    }
}

/// A device `--watch -d` drives: what the device kind decides, never the
/// `-d` flag itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchedDevice {
    Android,
    /// A booted iOS simulator: the app shares the host's loopback and loads
    /// an unsigned patch from its own data container.
    Simulator,
}

impl WatchedDevice {
    /// `None` for a device `--watch` refuses: a physical iOS device.
    fn of(device: &Device) -> Option<Self> {
        match (device.platform, device.kind) {
            (Platform::Android, _) => Some(Self::Android),
            (Platform::Ios, Kind::Simulator) => Some(Self::Simulator),
            (Platform::Ios, Kind::PhysicalDevice | Kind::Emulator) => None,
        }
    }

    /// How the refusals name the device.
    fn label(self) -> &'static str {
        match self {
            Self::Android => "an Android device",
            Self::Simulator => "an iOS simulator",
        }
    }

    /// The cold session seam the device relaunch loop drives.
    fn session_seam(self) -> &'static str {
        match self {
            Self::Android => "android_run::spawn_session",
            Self::Simulator => "ios_run::spawn_session",
        }
    }
}

/// `frust run --watch -d <device>`: an Android device or an iOS simulator
/// is watched — a hot session in a debug build ([`run_android_watch`],
/// [`run_simulator_watch`]), the device relaunch loop under `--no-hot` —
/// and every other device is refused with [`WATCH_DEVICE_REJECTION`] before
/// anything runs. Only a debug build without `--features` is accepted: the
/// device session seams build what the mode selects and nothing else, and
/// hot patching needs a debug build.
fn run_watch_on_device(
    runner: &dyn ProcessRunner,
    device: &Device,
    info: &BuildInfo,
    extra_features: &[String],
    no_hot: bool,
    hooks: WatchHooks,
) -> Result<u8> {
    let Some(watched) = WatchedDevice::of(device) else {
        bail!(WATCH_DEVICE_REJECTION);
    };
    if info.mode != BuildMode::Debug {
        bail!(
            "--watch on {} needs a debug build, not {:?}; drop --profile/--release, \
             or drop --watch to run the build once",
            watched.label(),
            info.mode
        );
    }
    if !extra_features.is_empty() {
        bail!(
            "--watch on {} does not support --features yet (requested: {}); \
             the device session seam (`{}`) carries no parameter for it",
            watched.label(),
            extra_features.join(", "),
            watched.session_seam()
        );
    }
    let cwd = std::env::current_dir().context("reading current directory")?;
    refresh_platform_wiring(runner, &cwd, &mut |line| println!("{line}"));
    match watched {
        WatchedDevice::Android => run_android_watch(&cwd, device, info, no_hot, hooks),
        WatchedDevice::Simulator => run_simulator_watch(&cwd, device, info, no_hot, hooks),
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

/// Injectable seams for the watch loops' two process-wide side
/// effects — installing a Ctrl-C handler and starting a filesystem watcher —
/// each of which is a process-global, install-once resource: `ctrlc::set_handler`
/// outright errors on a second call anywhere in the same process, and a real
/// [`notify`] watcher touches the actual filesystem under the caller's cwd.
/// Production always uses [`WatchHooks::real`] (installed exactly once per
/// `frust run --watch` process); a test injects a
/// no-op pair instead of ever reaching either real side effect — see
/// `run_in_with_hooks`.
type InstallCtrlcHook = Box<dyn FnOnce(AppSlot) -> Result<()>>;
type SpawnWatcherHook = Box<dyn FnOnce(&Path, mpsc::Sender<()>) -> Result<Box<dyn std::any::Any>>>;
type DeviceRelauncherHook =
    Box<dyn FnOnce(&Path, &BuildInfo, &Device, WatchedDevice) -> Box<dyn Relauncher>>;

struct WatchHooks {
    /// Installs the Ctrl-C handler over the shared slot ([`on_ctrlc`]):
    /// it stops the live app and exits, or cancels a start in flight.
    install_ctrlc: InstallCtrlcHook,
    /// Starts a filesystem watcher over `root`, forwarding a `()` tick into
    /// the given sender for every raw change. The returned box is a
    /// keep-alive handle only — the caller holds it for the watch loop's
    /// duration and never inspects it (dropping a real `notify` watcher
    /// stops it).
    spawn_watcher: SpawnWatcherHook,
    /// The hot-session seams; `None` keeps the plain relaunch loop (a test
    /// that drives the relaunch loop, or a build that cannot be hot).
    hot: Option<HotHooks>,
    /// Builds a watched device's relaunch target from the kind
    /// [`run_watch_on_device`] resolved ([`DeviceRelauncher`] on Android,
    /// [`SimulatorRelauncher`] on an iOS simulator, in production): what
    /// `--watch -d <device> --no-hot` relaunches, and what a hot session
    /// that cannot patch falls back to. A device `--watch` refuses never
    /// reaches it.
    device_relauncher: DeviceRelauncherHook,
}

impl WatchHooks {
    /// The production defaults: a real [`install_real_ctrlc_handler`], a
    /// real [`spawn_fs_watcher`] and the real device pipeline.
    fn real() -> Self {
        Self {
            install_ctrlc: Box::new(install_real_ctrlc_handler),
            spawn_watcher: Box::new(|root, tx| {
                spawn_fs_watcher(root, tx).map(|w| Box::new(w) as Box<dyn std::any::Any>)
            }),
            hot: Some(HotHooks::real()),
            device_relauncher: Box::new(|root, info, device, watched| {
                let runner: Arc<dyn ProcessRunner + Send + Sync> = Arc::new(RealProcessRunner);
                let (root, device, info) = (root.to_path_buf(), device.clone(), info.clone());
                match watched {
                    WatchedDevice::Simulator => Box::new(SimulatorRelauncher {
                        runner,
                        root,
                        device,
                        info,
                    }) as Box<dyn Relauncher>,
                    WatchedDevice::Android => Box::new(DeviceRelauncher {
                        runner,
                        root,
                        device,
                        info,
                    }),
                }
            }),
        }
    }

    /// A no-op pair for tests: skips the real Ctrl-C install and hands back
    /// an inert keep-alive handle instead of starting a real filesystem
    /// watcher — reaching `run_desktop_watch` in a test must never touch
    /// either real process-global side effect. The device relauncher fails
    /// every launch instead of running a device pipeline.
    #[cfg(test)]
    fn fake() -> Self {
        Self {
            install_ctrlc: Box::new(|_current| Ok(())),
            spawn_watcher: Box::new(|_root, _tx| Ok(Box::new(()) as Box<dyn std::any::Any>)),
            hot: None,
            device_relauncher: Box::new(|_, _, _, _| {
                Box::new(NoDevicePipeline) as Box<dyn Relauncher>
            }),
        }
    }
}

/// [`WatchHooks::fake`]'s device relauncher: every launch is an error.
#[cfg(test)]
struct NoDevicePipeline;

#[cfg(test)]
impl Relauncher for NoDevicePipeline {
    fn label(&self) -> &str {
        "the device app"
    }

    fn relaunch(
        &mut self,
        _current: &AppSlot,
        _on_line: &mut dyn FnMut(&str),
    ) -> Result<ControlFlow<()>> {
        bail!("no device pipeline in this test")
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

/// Installs the real, process-wide Ctrl-C handler over `current`
/// ([`on_ctrlc`]): it stops the live app (group-kills a desktop child; kills
/// the logcat stream, removes the `adb forward` and force-stops an Android
/// app; kills the console stream and `simctl terminate`s a simulator app)
/// and exits `frust run --watch` — except during a cancellable start,
/// which it cancels instead, leaving the exit to the loop.
///
/// **Warning: one handler per process.** `ctrlc::set_handler` can only be
/// installed once per process — a second call anywhere (e.g. a second
/// `--watch` invocation reached in the same process) errors outright. This
/// function is reached only through [`WatchHooks::real`], whose hook the
/// watch loop calls exactly once per production `--watch` invocation; a
/// test must go through [`WatchHooks::fake`] instead of ever
/// calling this directly (see
/// `run_in_with_watch_skips_device_discovery_even_when_a_device_is_present`).
fn install_real_ctrlc_handler(current: AppSlot) -> Result<()> {
    ctrlc::set_handler(move || {
        if on_ctrlc(&current) {
            std::process::exit(0);
        }
        println!("Stopping the launch in progress (Ctrl-C again to exit now)…");
    })
    .context("failed to install Ctrl-C handler")
}

/// A Ctrl-C's effect on `current`, the exit aside: raise the cancel flag
/// and stop the live app. Answers whether to exit now: `false` only for the
/// first Ctrl-C during a cancellable start ([`WatchSlot::begin_start`]),
/// which then tears down what it launched and ends the loop itself; a
/// second Ctrl-C exits at once. Decided under the slot's lock, so it cannot
/// fall between a start parking its app and ending ([`StartInFlight::end`]).
fn on_ctrlc(current: &WatchSlot) -> bool {
    let mut slot = lock_slot(current);
    let repeated = current.cancel.swap(true, Ordering::SeqCst);
    if let Some(app) = slot.take() {
        app.stop();
    }
    repeated || !current.starting.load(Ordering::SeqCst)
}

/// The live app a watch loop owns: the stream its output arrives on, plus —
/// for a device app — what stopping it means beyond killing that stream.
struct RunningApp {
    handle: StreamHandle,
    /// Run once, after the stream is killed or has ended: `adb forward
    /// --remove` for a hot session's devtools port and `am force-stop` for
    /// the app. `None` for a desktop child, which killing already stops.
    teardown: Option<Teardown>,
}

/// A device app's teardown (see [`RunningApp::teardown`]).
type Teardown = Box<dyn FnOnce() + Send>;

/// What a watch loop shares with its Ctrl-C handler ([`on_ctrlc`]): the
/// live app, and the cancel flag a start in flight observes.
#[derive(Default)]
struct WatchSlot {
    app: Mutex<Option<RunningApp>>,
    /// Raised by Ctrl-C, never lowered. A cancellable start (the device
    /// pipeline, the Android hot start) observes it, tears down what it
    /// launched and returns; the loop then ends.
    cancel: AtomicBool,
    /// Whether a cancellable start is in flight. Written and read only
    /// under `app`'s lock.
    starting: AtomicBool,
}

/// The slot a watch loop shares with its Ctrl-C handler.
type AppSlot = Arc<WatchSlot>;

impl WatchSlot {
    /// Marks a cancellable start in flight: one that observes
    /// [`Self::cancel`] and tears down what it launched. Until the returned
    /// mark ends ([`StartInFlight::end`]), a first Ctrl-C cancels the start
    /// instead of exiting; only a start that took the mark can end it.
    fn begin_start(&self) -> StartInFlight<'_> {
        let _slot = lock_slot(self);
        self.starting.store(true, Ordering::SeqCst);
        StartInFlight { slot: self }
    }
}

/// A cancellable start in flight ([`WatchSlot::begin_start`]). Dropped
/// without [`Self::end`] (an early return, a panic), it only clears the
/// mark.
#[must_use = "a cancellable start ends through `StartInFlight::end`"]
struct StartInFlight<'a> {
    slot: &'a WatchSlot,
}

impl StartInFlight<'_> {
    /// Ends the start: parks the app it launched (if any) and answers
    /// `Continue`, or — when a Ctrl-C landed meanwhile — answers `Break`
    /// with that app, unparked, for the caller to stop once whatever links
    /// to it is gone ([`launch_hot`] drops the session first).
    fn end(self, app: Option<RunningApp>) -> ControlFlow<Option<RunningApp>> {
        let mut slot = lock_slot(self.slot);
        self.slot.starting.store(false, Ordering::SeqCst);
        if self.slot.cancel.load(Ordering::SeqCst) {
            return ControlFlow::Break(app);
        }
        if let Some(app) = app {
            slot.replace(app);
        }
        ControlFlow::Continue(())
    }

    /// [`Self::end`] for an app nothing else links to: on `Break` it is
    /// stopped at once.
    fn end_or_stop(self, app: Option<RunningApp>) -> ControlFlow<()> {
        match self.end(app) {
            ControlFlow::Continue(()) => ControlFlow::Continue(()),
            ControlFlow::Break(app) => {
                if let Some(app) = app {
                    app.stop();
                }
                ControlFlow::Break(())
            }
        }
    }
}

impl Drop for StartInFlight<'_> {
    fn drop(&mut self) {
        let _slot = lock_slot(self.slot);
        self.slot.starting.store(false, Ordering::SeqCst);
    }
}

impl RunningApp {
    /// Kill the stream, then run the teardown.
    fn stop(mut self) {
        self.handle.kill();
        self.finish();
    }

    /// Wait for the (already ended) stream's exit status, then run the
    /// teardown.
    fn wait(mut self) -> bool {
        let success = self.handle.wait();
        self.finish();
        success
    }

    fn finish(&mut self) {
        if let Some(teardown) = self.teardown.take() {
            teardown();
        }
    }
}

impl From<StreamHandle> for RunningApp {
    fn from(handle: StreamHandle) -> Self {
        Self {
            handle,
            teardown: None,
        }
    }
}

/// An Android app launched for a watch loop: its logcat stream, torn down by
/// removing `forward_port`'s `adb forward` (a hot session's devtools port,
/// when one was allocated) and force-stopping the launched package. The
/// serial is the resolved [`Device`]'s; nothing here derives it.
fn device_app(
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    serial: &str,
    launch: AndroidLaunch,
    forward_port: Option<u16>,
) -> RunningApp {
    let serial = serial.to_string();
    let package = launch.package;
    RunningApp {
        handle: launch.stream,
        teardown: Some(Box::new(move || {
            // Best-effort, like the TUI supervisor's stop: a device that went
            // away or an app that already exited is a normal end.
            if let Some(port) = forward_port {
                let _ = adb_forward_remove(&*runner, &serial, port);
            }
            force_stop(&*runner, &serial, &package);
        })),
    }
}

/// What a relaunch loop restarts on every change: `cargo run` on the
/// desktop ([`DesktopRelauncher`]), the whole device pipeline on Android
/// ([`DeviceRelauncher`]) or on an iOS simulator ([`SimulatorRelauncher`]).
trait Relauncher {
    /// How the status lines name the app's stream (`` `cargo run` ``).
    fn label(&self) -> &str;
    /// Stop the app in `current` (if any) and launch a fresh one into it. An
    /// error ends the loop; a launch the next change may fix (a device
    /// pipeline failure) is reported through `on_line` and leaves the slot
    /// empty. `Break` ends the loop too: a Ctrl-C cancelled the launch.
    fn relaunch(
        &mut self,
        current: &AppSlot,
        on_line: &mut dyn FnMut(&str),
    ) -> Result<ControlFlow<()>>;
}

/// The desktop preview: kill and respawn `cargo run` from `plan`.
struct DesktopRelauncher<'a> {
    runner: &'a dyn ProcessRunner,
    plan: &'a DesktopPlan,
    env: &'a [(String, String)],
}

impl Relauncher for DesktopRelauncher<'_> {
    fn label(&self) -> &str {
        "`cargo run`"
    }

    /// Kill and respawn under one hold of the slot's lock (the spawn is
    /// instant), so a Ctrl-C can never land between the two and orphan the
    /// new child.
    fn relaunch(
        &mut self,
        current: &AppSlot,
        _on_line: &mut dyn FnMut(&str),
    ) -> Result<ControlFlow<()>> {
        let mut slot = lock_slot(current);
        if let Some(app) = slot.take() {
            app.stop();
        }
        slot.replace(spawn_preview(self.runner, self.plan, self.env)?.into());
        Ok(ControlFlow::Continue(()))
    }
}

/// An Android device: stop the app, then rerun the non-hot device pipeline
/// (`android_run::spawn_session`: Gradle build, install, launch, logcat).
struct DeviceRelauncher {
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    root: PathBuf,
    device: Device,
    info: BuildInfo,
}

impl Relauncher for DeviceRelauncher {
    fn label(&self) -> &str {
        "the app's log stream"
    }

    /// The pipeline runs for minutes, so the slot's lock is not held across
    /// it. It is a cancellable start ([`WatchSlot::begin_start`]): a Ctrl-C
    /// meanwhile cancels it at its next phase boundary, the app it launched
    /// by then is stopped, and the loop ends.
    fn relaunch(
        &mut self,
        current: &AppSlot,
        on_line: &mut dyn FnMut(&str),
    ) -> Result<ControlFlow<()>> {
        let previous = lock_slot(current).take();
        if let Some(app) = previous {
            app.stop();
        }
        let start = current.begin_start();
        let launched = android_run::spawn_session_outcome(
            &*self.runner,
            &self.root,
            &self.device,
            &self.info,
            on_line,
            &current.cancel,
        );
        Ok(settle_device_launch(
            Arc::clone(&self.runner),
            &self.device.id,
            launched,
            start,
            on_line,
        ))
    }
}

/// Ends [`DeviceRelauncher`]'s start over the pipeline's answer: a launched
/// app is parked, or stopped when a Ctrl-C landed meanwhile. A run that
/// ended without a stream is reported as a failure the next change may fix
/// — unless a Ctrl-C landed, which ends the loop after force-stopping the
/// package the run issued `am start` for ([`StoppedShort::stop_launched`]),
/// whether it answered a cancel or a failure (the Ctrl-C's SIGINT also
/// reaches an in-flight `adb shell am start`).
fn settle_device_launch(
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    serial: &str,
    launched: Result<AndroidLaunch, StoppedShort>,
    start: StartInFlight<'_>,
    on_line: &mut dyn FnMut(&str),
) -> ControlFlow<()> {
    let stopped = match launched {
        Ok(launch) => return start.end_or_stop(Some(device_app(runner, serial, launch, None))),
        Err(stopped) => stopped,
    };
    if start.end(None).is_break() {
        stopped.stop_launched(&*runner, serial);
        return ControlFlow::Break(());
    }
    if let Some(err) = stopped.error {
        on_line(&format!("error: {err:#}"));
        on_line("the device pipeline failed; watching for a source change to retry…");
    }
    ControlFlow::Continue(())
}

/// An app launched on the iOS simulator `udid` for a watch loop: its
/// `simctl launch --console-pty` stream, torn down with `simctl terminate`
/// (killing the stream stops only the `simctl` bridge, not the app). The
/// simulator shares the host's loopback, so there is no forward to remove.
fn simulator_app(
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    udid: &str,
    launch: IosLaunch,
) -> RunningApp {
    let udid = udid.to_string();
    let bundle_id = launch.bundle_id;
    RunningApp {
        handle: launch.stream,
        teardown: Some(Box::new(move || {
            // Best-effort: an app that already exited is a normal end.
            simctl::terminate(&*runner, &udid, &bundle_id);
        })),
    }
}

/// An iOS simulator: stop the app, then rerun the simulator pipeline
/// (`ios_run::spawn_session`: preflight, `xcodebuild`, `simctl install`,
/// `simctl launch --console-pty`).
struct SimulatorRelauncher {
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    root: PathBuf,
    device: Device,
    info: BuildInfo,
}

impl Relauncher for SimulatorRelauncher {
    fn label(&self) -> &str {
        "the app's console"
    }

    /// A cancellable start like [`DeviceRelauncher::relaunch`]'s: a Ctrl-C
    /// during the pipeline cancels it at its next phase boundary, and an app
    /// it launched by then is terminated before the loop ends.
    fn relaunch(
        &mut self,
        current: &AppSlot,
        on_line: &mut dyn FnMut(&str),
    ) -> Result<ControlFlow<()>> {
        let previous = lock_slot(current).take();
        if let Some(app) = previous {
            app.stop();
        }
        let start = current.begin_start();
        let launched = ios_run::spawn_session(
            &*self.runner,
            &self.root,
            &self.device,
            &self.info,
            on_line,
            &current.cancel,
        );
        Ok(settle_simulator_launch(
            Arc::clone(&self.runner),
            &self.device.id,
            launched,
            start,
            on_line,
        ))
    }
}

/// Ends [`SimulatorRelauncher`]'s start over the pipeline's answer. A
/// launched app is parked — or, when a Ctrl-C landed meanwhile (the
/// pipeline checks the flag only before `simctl launch`, so one landing
/// after it still hands the app back), terminated and the loop ends. No
/// launch (a cancel at a phase boundary, or a failure before `simctl
/// launch`) left nothing running: it ends the loop after a Ctrl-C, and a
/// failure without one is reported for the next change to retry.
fn settle_simulator_launch(
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    udid: &str,
    launched: Result<Option<IosLaunch>>,
    start: StartInFlight<'_>,
    on_line: &mut dyn FnMut(&str),
) -> ControlFlow<()> {
    let error = match launched {
        Ok(Some(launch)) => {
            return start.end_or_stop(Some(simulator_app(runner, udid, launch)));
        }
        Ok(None) => None,
        Err(err) => Some(err),
    };
    if start.end(None).is_break() {
        return ControlFlow::Break(());
    }
    if let Some(err) = error {
        on_line(&format!("error: {err:#}"));
        on_line("the simulator pipeline failed; watching for a source change to retry…");
    }
    ControlFlow::Continue(())
}

/// Poll cadence for [`watch_loop`]'s inner loop — bounds both output latency
/// (how promptly a streamed `cargo run` line is printed) and spontaneous-exit
/// detection latency (how promptly a build failure is noticed) without
/// busy-spinning between polls.
const WATCH_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// `frust run --watch`'s desktop file-watch → rebuild → relaunch loop.
/// Watches `<root>/src` (recursive) and
/// `<root>/Cargo.toml` via `hooks.spawn_watcher`, wiring its every raw event
/// straight into [`relaunch_loop_with`], the testable core that owns
/// debouncing, spawning, and kill+relaunch; `hooks.install_ctrlc` installs
/// the Ctrl-C handler described in [`run_relaunch_watch`]. Production always
/// calls this with [`WatchHooks::real`] (via [`run_in`]/`run_desktop_fallback`);
/// a test drives it with [`WatchHooks::fake`] instead so neither
/// process-global side effect is ever touched by `cargo test`.
fn run_desktop_watch(
    runner: &dyn ProcessRunner,
    plan: &DesktopPlan,
    env: &[(String, String)],
    root: &Path,
    hooks: WatchHooks,
) -> Result<u8> {
    let mut relauncher = DesktopRelauncher { runner, plan, env };
    run_relaunch_watch(
        &mut relauncher,
        root,
        hooks.install_ctrlc,
        hooks.spawn_watcher,
    )
}

/// The relaunch watch over any [`Relauncher`]: the tick watcher over `root`,
/// the Ctrl-C handler, then [`relaunch_loop_with`].
fn run_relaunch_watch(
    relauncher: &mut dyn Relauncher,
    root: &Path,
    install_ctrlc: InstallCtrlcHook,
    spawn_watcher: SpawnWatcherHook,
) -> Result<u8> {
    let (raw_tx, raw_rx) = mpsc::channel();
    let _watcher = spawn_watcher(root, raw_tx)?;
    println!(
        "Watching `{}` for changes (Ctrl-C to exit)…",
        root.display()
    );

    // The app's stream — the desktop `cargo run` child, a device app's `adb
    // logcat` — is spawned into its own process group
    // (`frust_drive::process::spawn_streaming`), so a kill reaches the
    // compiled preview binary `cargo run` forks too — the orphaned-preview
    // fix. That same arrangement removes the child from this terminal's
    // foreground process group, so a bare Ctrl-C's SIGINT no longer reaches
    // it: without an explicit handler, exiting the watch loop would just
    // orphan the live app. Share the slot with a Ctrl-C handler that stops
    // the app before exiting, or cancels a device pipeline in flight.
    let current = AppSlot::default();
    install_ctrlc(Arc::clone(&current))?;

    let mut on_line = |line: &str| println!("{line}");
    relaunch_loop_with(relauncher, &raw_rx, WATCH_DEBOUNCE, &current, &mut on_line)
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

/// The desktop entry of `frust run --watch`'s rebuild-relaunch loop: drives
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
/// terminal's SIGINT no longer reaches it — [`run_relaunch_watch`] installs a
/// Ctrl-C handler over a shared app slot to stop the current app before
/// exiting. This standalone `watch_loop` owns a private slot (no shared
/// handler); production drives [`relaunch_loop_with`] with the shared one,
/// over the desktop or the device [`Relauncher`].
///
/// Test-only: it exists purely as the fixed-signature entry the loop's unit
/// tests drive; the shipped binary always goes through `relaunch_loop_with`.
#[cfg(test)]
fn watch_loop(
    runner: &dyn ProcessRunner,
    plan: &DesktopPlan,
    env: &[(String, String)],
    raw_changes: &mpsc::Receiver<()>,
    debounce: Duration,
    on_line: &mut dyn FnMut(&str),
) -> Result<u8> {
    let current = AppSlot::default();
    let mut relauncher = DesktopRelauncher { runner, plan, env };
    relaunch_loop_with(&mut relauncher, raw_changes, debounce, &current, on_line)
}

/// [`watch_loop`]'s body over any [`Relauncher`], parameterized on a shared
/// `current` slot so a Ctrl-C handler installed by [`run_relaunch_watch`]
/// can stop the live app before the process exits (see [`watch_loop`]'s
/// doc). Holds the slot's lock only for the brief drain steps — never across
/// the blocking `recv_timeout` — so the handler can always acquire it
/// promptly; how a relaunch locks is the [`Relauncher`]'s own business. A
/// relaunch a Ctrl-C cancelled ends the loop.
fn relaunch_loop_with(
    relauncher: &mut dyn Relauncher,
    raw_changes: &mpsc::Receiver<()>,
    debounce: Duration,
    current: &AppSlot,
    on_line: &mut dyn FnMut(&str),
) -> Result<u8> {
    // A caller that already put a live app in the slot (the hot loop falling
    // back after its session turned out restart-only) keeps it; the first
    // change replaces it like any other.
    if lock_slot(current).is_none() && relauncher.relaunch(current, on_line)?.is_break() {
        return Ok(0);
    }

    loop {
        {
            let mut slot = lock_slot(current);
            if drain_available_lines(&mut slot, on_line) {
                let success = slot.take().expect("app present in this arm").wait();
                let label = relauncher.label();
                if success {
                    on_line(&format!(
                        "{label} exited; waiting for a source change to relaunch…"
                    ));
                } else {
                    on_line(&format!(
                        "{label} failed; watching for a source change to retry…"
                    ));
                }
            }
        }

        match raw_changes.recv_timeout(WATCH_POLL_INTERVAL) {
            Ok(()) => {
                // Trailing-edge debounce: keep consuming ticks that arrive
                // within `debounce` of the previous one before acting.
                while raw_changes.recv_timeout(debounce).is_ok() {}
                // Flush whatever the about-to-be-stopped app already
                // produced before stopping it, so a burst of output right
                // before the stop isn't silently dropped.
                drain_available_lines(&mut lock_slot(current), on_line);
                on_line("Change detected; rebuilding and relaunching…");
                if relauncher.relaunch(current, on_line)?.is_break() {
                    return Ok(0);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let app = lock_slot(current).take();
                if let Some(app) = app {
                    app.stop();
                }
                return Ok(0);
            }
        }
    }
}

/// Locks the shared `current`-app slot, recovering from poisoning rather
/// than propagating a panic (a poisoned lock just means a prior holder panicked
/// mid-update; the loop can still drive the app inside).
fn lock_slot(slot: &WatchSlot) -> std::sync::MutexGuard<'_, Option<RunningApp>> {
    slot.app
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Drains every line currently available from `current`'s app (if any),
/// forwarding each to `on_line` with a devtools discovery line's token
/// redacted. Returns `true` if the drain ended because the app's stream
/// disconnected (the process has exited, spontaneously or via a prior
/// [`StreamHandle::kill`]) rather than simply having nothing more
/// buffered right now — the caller reads the exit status via
/// [`RunningApp::wait`] in that case. A no-op (returns `false`) when
/// `current` is `None`.
fn drain_available_lines(current: &mut Option<RunningApp>, on_line: &mut dyn FnMut(&str)) -> bool {
    let Some(app) = current.as_mut() else {
        return false;
    };
    loop {
        match app.handle.lines.try_recv() {
            Ok(line) => on_line(&redact_discovery_line(&line)),
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
    /// This project cannot be hot-patched (the start's own
    /// `RestartRequired` reason); use the relaunch loop.
    Unavailable(RestartReason),
    /// The fat build or launch failed; try again on the next change.
    Failed(Vec<String>),
}

impl From<StartError> for HotStartError {
    fn from(err: StartError) -> Self {
        match err {
            StartError::RestartRequired(reason) => Self::Unavailable(reason),
            StartError::FatBuildFailed { diagnostics } => Self::Failed(diagnostics),
            err @ StartError::Launch { .. } => Self::Failed(vec![err.to_string()]),
        }
    }
}

/// A started hot session and the app it patches.
type HotLaunch = (Box<dyn HotSessionHandle>, RunningApp);

/// Starts fresh fat sessions and reports what to watch. Real:
/// `hotpatch::session::start_desktop` over the project ([`DriveHotBackend`]),
/// `hotpatch::android::start_android` on an Android device
/// ([`AndroidHotBackend`]) or `hotpatch::ios_sim::start_ios_sim` on an iOS
/// simulator ([`IosSimHotBackend`]); fake in tests.
trait HotBackend {
    fn watch_set(&mut self) -> Result<WatchSet>;
    /// Fat-builds and launches a new session. `on_line` receives the build
    /// diagnostics and the app's early output. A [`Self::cancellable`]
    /// start observes `cancel` and tears down what it launched.
    fn start(
        &mut self,
        on_line: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<HotLaunch, HotStartError>;
    /// Whether [`Self::start`] honours `cancel`, so a Ctrl-C during it
    /// cancels it rather than exiting ([`WatchSlot::begin_start`]).
    fn cancellable(&self) -> bool {
        false
    }
}

/// The [`WatchSet`] of `root`'s workspace graph, tipped at `package`.
fn load_watch_set(runner: &dyn ProcessRunner, root: &Path, package: &str) -> Result<WatchSet> {
    let graph = WorkspaceGraph::load(runner, &root.join("Cargo.toml"), None, package, None)
        .map_err(|err| anyhow::anyhow!("{err}"))?;
    Ok(watch_set(&graph))
}

/// [`HotBackend`] over the real `frust_drive` desktop session.
struct DriveHotBackend {
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    root: PathBuf,
    info: BuildInfo,
    package: String,
}

impl HotBackend for DriveHotBackend {
    fn watch_set(&mut self) -> Result<WatchSet> {
        load_watch_set(&*self.runner, &self.root, &self.package)
    }

    /// `start_desktop` has no cancel seam: a Ctrl-C during it exits at once.
    fn start(
        &mut self,
        on_line: &mut dyn FnMut(&str),
        _cancel: &AtomicBool,
    ) -> Result<HotLaunch, HotStartError> {
        let host = SessionHost::current(Arc::clone(&self.runner)).map_err(StartError::from)?;
        let start = DesktopStart {
            root: &self.root,
            info: &self.info,
            package: &self.package,
            bin: None,
        };
        let (session, handle) = start_desktop(&host, &start, on_line)?;
        Ok((Box::new(session), handle.into()))
    }
}

/// [`HotBackend`] over the real `frust_drive` Android session: the fat
/// build outside Gradle, `assembleDebug -x cargoNdkBuild`, install, launch
/// and an `adb forward` to the app's devtools endpoint. The app it hands
/// back tears down by removing that forward and force-stopping the
/// package ([`device_app`]).
struct AndroidHotBackend {
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    root: PathBuf,
    info: BuildInfo,
    package: String,
    device: Device,
}

impl HotBackend for AndroidHotBackend {
    fn watch_set(&mut self) -> Result<WatchSet> {
        load_watch_set(&*self.runner, &self.root, &self.package)
    }

    fn start(
        &mut self,
        on_line: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<HotLaunch, HotStartError> {
        let host = SessionHost::current(Arc::clone(&self.runner)).map_err(StartError::from)?;
        let start = AndroidStart {
            root: &self.root,
            info: &self.info,
            package: &self.package,
            device: &self.device,
        };
        match start_android(&host, &start, on_line, cancel)? {
            Some(AndroidHotStart {
                session,
                launch,
                forward_port,
            }) => {
                let app = device_app(
                    Arc::clone(&self.runner),
                    &self.device.id,
                    launch,
                    forward_port,
                );
                Ok((Box::new(session), app))
            }
            // A Ctrl-C cancelled it; `start_android` has torn down what it
            // launched, and the loop ends ([`launch_hot`]).
            None => Err(HotStartError::Failed(vec![
                "the hot start was cancelled".to_string(),
            ])),
        }
    }

    fn cancellable(&self) -> bool {
        true
    }
}

/// [`HotBackend`] over the real `frust_drive` iOS simulator session: the
/// fat build through Xcode, `simctl install`, `simctl launch --console-pty`
/// and a devtools connection straight to the app on the host's loopback.
/// The app it hands back tears down with `simctl terminate`
/// ([`simulator_app`]); a restart starts the whole simulator pipeline again.
struct IosSimHotBackend {
    runner: Arc<dyn ProcessRunner + Send + Sync>,
    root: PathBuf,
    info: BuildInfo,
    package: String,
    device: Device,
}

impl HotBackend for IosSimHotBackend {
    fn watch_set(&mut self) -> Result<WatchSet> {
        load_watch_set(&*self.runner, &self.root, &self.package)
    }

    fn start(
        &mut self,
        on_line: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<HotLaunch, HotStartError> {
        let host = SessionHost::current(Arc::clone(&self.runner)).map_err(StartError::from)?;
        let start = IosSimStart {
            root: &self.root,
            info: &self.info,
            package: &self.package,
            device: &self.device,
        };
        match start_ios_sim(&host, &start, on_line, cancel)? {
            Some(IosSimHotStart { session, launch }) => {
                let app = simulator_app(Arc::clone(&self.runner), &self.device.id, launch);
                Ok((Box::new(session), app))
            }
            // A Ctrl-C cancelled it; `start_ios_sim` has killed the console
            // stream and terminated what it launched, and the loop ends
            // ([`launch_hot`]).
            None => Err(HotStartError::Failed(vec![
                "the hot start was cancelled".to_string(),
            ])),
        }
    }

    fn cancellable(&self) -> bool {
        true
    }
}

type HotBackendHook = Box<dyn FnOnce(&Path, &BuildInfo) -> Result<Box<dyn HotBackend>>>;
type DeviceBackendHook = Box<dyn FnOnce(&Path, &BuildInfo, &Device) -> Result<Box<dyn HotBackend>>>;
type SpawnPathWatcherHook =
    Box<dyn FnOnce(&WatchSet, mpsc::Sender<PathBuf>) -> Result<Box<dyn std::any::Any>>>;

/// The injectable seams of hot mode, beside [`WatchHooks`]' own: the session
/// backends (desktop, Android device, iOS simulator) and a watcher that
/// reports changed *paths* (the session needs them; the relaunch loop's
/// watcher reports bare ticks).
struct HotHooks {
    backend: HotBackendHook,
    android: DeviceBackendHook,
    ios_sim: DeviceBackendHook,
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
            android: Box::new(|root, info, device| {
                let package = read_package_name(root)?;
                Ok(Box::new(AndroidHotBackend {
                    runner: Arc::new(RealProcessRunner),
                    root: root.to_path_buf(),
                    info: info.clone(),
                    package,
                    device: device.clone(),
                }) as Box<dyn HotBackend>)
            }),
            ios_sim: Box::new(|root, info, device| {
                let package = read_package_name(root)?;
                Ok(Box::new(IosSimHotBackend {
                    runner: Arc::new(RealProcessRunner),
                    root: root.to_path_buf(),
                    info: info.clone(),
                    package,
                    device: device.clone(),
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

/// `frust run --watch`'s desktop hot mode: resolves the backend and the
/// watch set, then hands both to [`hot_loop_with`] over the `cargo run`
/// relaunch target. A setup failure is printed and answered with the
/// relaunch loop, never an error — `--watch` always works.
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
        device_relauncher: _,
    } = hooks;
    let hot = hot.expect("run_desktop_hot is reached only with hot hooks");
    let backend = (hot.backend)(root, info);
    let mut relauncher = DesktopRelauncher { runner, plan, env };
    run_hot_watch(
        &mut relauncher,
        backend,
        root,
        install_ctrlc,
        spawn_watcher,
        hot.spawn_watcher,
    )
}

/// `frust run --watch -d <android>` (a debug build, checked by
/// [`run_watch_on_device`]): hot by default — the Android session backend
/// over the device relaunch target, so a session that cannot patch falls
/// back to rerunning the device pipeline — and the plain device relaunch
/// loop under `--no-hot` (or a hook set with no hot backend). Hot mode is
/// never reached without the explicit `-d`.
fn run_android_watch(
    root: &Path,
    device: &Device,
    info: &BuildInfo,
    no_hot: bool,
    hooks: WatchHooks,
) -> Result<u8> {
    let WatchHooks {
        install_ctrlc,
        spawn_watcher,
        hot,
        device_relauncher,
    } = hooks;
    let mut relauncher = device_relauncher(root, info, device, WatchedDevice::Android);
    match hot.filter(|_| !no_hot) {
        Some(hot) => {
            let backend = (hot.android)(root, info, device);
            run_hot_watch(
                &mut *relauncher,
                backend,
                root,
                install_ctrlc,
                spawn_watcher,
                hot.spawn_watcher,
            )
        }
        None => run_relaunch_watch(&mut *relauncher, root, install_ctrlc, spawn_watcher),
    }
}

/// `frust run --watch -d <simulator>` (a booted iOS simulator, a debug
/// build, checked by [`run_watch_on_device`]): [`run_android_watch`]'s
/// shape over the simulator — the iOS simulator session backend over the
/// simulator relaunch target, so a session that cannot patch falls back to
/// rerunning the simulator pipeline, and that relaunch loop alone under
/// `--no-hot`.
fn run_simulator_watch(
    root: &Path,
    device: &Device,
    info: &BuildInfo,
    no_hot: bool,
    hooks: WatchHooks,
) -> Result<u8> {
    let WatchHooks {
        install_ctrlc,
        spawn_watcher,
        hot,
        device_relauncher,
    } = hooks;
    let mut relauncher = device_relauncher(root, info, device, WatchedDevice::Simulator);
    match hot.filter(|_| !no_hot) {
        Some(hot) => {
            let backend = (hot.ios_sim)(root, info, device);
            run_hot_watch(
                &mut *relauncher,
                backend,
                root,
                install_ctrlc,
                spawn_watcher,
                hot.spawn_watcher,
            )
        }
        None => run_relaunch_watch(&mut *relauncher, root, install_ctrlc, spawn_watcher),
    }
}

/// The hot watch over any backend and relaunch target: resolve the watch
/// set, start the path watcher and the Ctrl-C handler, then
/// [`hot_loop_with`]. A setup failure is printed and answered with the
/// relaunch loop over `relauncher`.
fn run_hot_watch(
    relauncher: &mut dyn Relauncher,
    backend: Result<Box<dyn HotBackend>>,
    root: &Path,
    install_ctrlc: InstallCtrlcHook,
    spawn_watcher: SpawnWatcherHook,
    spawn_path_watcher: SpawnPathWatcherHook,
) -> Result<u8> {
    let setup = backend.and_then(|mut backend| {
        let set = backend.watch_set()?;
        Ok((backend, set))
    });
    let (mut backend, set) = match setup {
        Ok(ok) => ok,
        Err(err) => {
            println!("hot reload unavailable: {err:#}; relaunching on change instead");
            return run_relaunch_watch(relauncher, root, install_ctrlc, spawn_watcher);
        }
    };

    let (path_tx, path_rx) = mpsc::channel();
    let _watcher = spawn_path_watcher(&set, path_tx)?;
    println!(
        "Watching `{}` for changes with hot reload (Ctrl-C to exit; --no-hot relaunches instead)…",
        root.display()
    );
    let current = AppSlot::default();
    install_ctrlc(Arc::clone(&current))?;
    let mut on_line = |line: &str| println!("{line}");
    hot_loop_with(
        relauncher,
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
    /// A Ctrl-C landed during the start: what it launched is stopped, and
    /// the loop ends.
    Cancelled,
}

/// The one line a session that cannot patch prints before the relaunch loop
/// takes over: `restart required: <reason>`, the reason naming the failed
/// precondition.
fn cold_fallback_line(reason: &RestartReason) -> String {
    format!(
        "{}; hot reload unavailable, relaunching on change instead",
        Outcome::RestartRequired(reason.clone())
    )
}

/// Starts a fresh fat session and parks its app in `current`. A
/// restart-only session (no `Capability::HotPatch`, or no devtools link)
/// keeps its app (the relaunch loop adopts it) and prints its failed
/// precondition once, as `restart required: hot-patch builder unsupported:
/// <precondition>`. A cancellable start is bracketed by
/// [`WatchSlot::begin_start`]/[`StartInFlight::end`], so a Ctrl-C during
/// it ends in [`Launched::Cancelled`]; one that ignores the cancel flag
/// (the desktop start) parks its app whatever the flag says.
fn launch_hot(
    backend: &mut dyn HotBackend,
    current: &AppSlot,
    on_line: &mut dyn FnMut(&str),
) -> Launched {
    let start = backend.cancellable().then(|| current.begin_start());
    let (started, app) = match backend.start(on_line, &current.cancel) {
        Ok((session, app)) => (Ok(session), Some(app)),
        Err(err) => (Err(err), None),
    };
    match start {
        Some(start) => {
            if let ControlFlow::Break(app) = start.end(app) {
                // The session (and its devtools socket) goes before its app
                // and forward do, as on a restart.
                drop(started);
                if let Some(app) = app {
                    app.stop();
                }
                return Launched::Cancelled;
            }
        }
        None => {
            if let Some(app) = app {
                lock_slot(current).replace(app);
            }
        }
    }
    match started {
        Ok(session) => match session.restart_only_reason() {
            Some(reason) => {
                on_line(&cold_fallback_line(&RestartReason::BuilderUnsupported {
                    detail: reason,
                }));
                Launched::Cold
            }
            None => Launched::Live(session),
        },
        Err(HotStartError::Unavailable(reason)) => {
            on_line(&cold_fallback_line(&reason));
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

/// [`hot_loop_with`] over the desktop `cargo run` relaunch target — the
/// fixed-signature entry the desktop hot tests drive.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn hot_loop(
    runner: &dyn ProcessRunner,
    plan: &DesktopPlan,
    env: &[(String, String)],
    backend: &mut dyn HotBackend,
    changes: mpsc::Receiver<PathBuf>,
    debounce: Duration,
    current: &AppSlot,
    on_line: &mut dyn FnMut(&str),
) -> Result<u8> {
    let mut relauncher = DesktopRelauncher { runner, plan, env };
    hot_loop_with(
        &mut relauncher,
        backend,
        changes,
        debounce,
        current,
        on_line,
    )
}

/// The hot counterpart of [`relaunch_loop_with`]: each debounced change set
/// goes to the session. `Patched` prints its one line and leaves the app
/// running; a compile error prints its diagnostics and leaves the app
/// untouched; `RestartRequired` prints its reason verbatim, stops the app
/// (an Android app's forward is removed and its package force-stopped, a
/// simulator app is `simctl terminate`d) and starts a fresh fat session — on
/// a device the whole device or simulator pipeline again.
/// When hot reload turns out to be unavailable the loop becomes the relaunch
/// loop over `relauncher` and the same watcher. A start a Ctrl-C cancelled
/// ends the loop.
fn hot_loop_with(
    relauncher: &mut dyn Relauncher,
    backend: &mut dyn HotBackend,
    changes: mpsc::Receiver<PathBuf>,
    debounce: Duration,
    current: &AppSlot,
    on_line: &mut dyn FnMut(&str),
) -> Result<u8> {
    let mut session = match launch_hot(backend, current, on_line) {
        Launched::Live(session) => Some(session),
        Launched::Failed => None,
        Launched::Cold => {
            return relaunch_loop(relauncher, changes, debounce, current, on_line);
        }
        Launched::Cancelled => return Ok(0),
    };

    loop {
        {
            let mut slot = lock_slot(current);
            if drain_available_lines(&mut slot, on_line) {
                let success = slot.take().expect("app present in this arm").wait();
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
                                    relauncher, changes, debounce, current, on_line,
                                );
                            }
                            Launched::Cancelled => return Ok(0),
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
                        // The old session (and its devtools socket) goes
                        // before its app and forward do.
                        drop(session.take());
                        let app = lock_slot(current).take();
                        if let Some(app) = app {
                            app.stop();
                        }
                        session = match launch_hot(backend, current, on_line) {
                            Launched::Live(next) => Some(next),
                            Launched::Failed => None,
                            Launched::Cold => {
                                return relaunch_loop(
                                    relauncher, changes, debounce, current, on_line,
                                );
                            }
                            Launched::Cancelled => return Ok(0),
                        };
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let app = lock_slot(current).take();
                if let Some(app) = app {
                    app.stop();
                }
                return Ok(0);
            }
        }
    }
}

/// Hands the hot loop's change stream to the plain relaunch loop: a thread
/// folds each changed path into the bare tick that loop takes, and ends (so
/// the loop does) when the watcher drops. The slot may already hold a live
/// app, which the loop keeps until the first change.
fn relaunch_loop(
    relauncher: &mut dyn Relauncher,
    changes: mpsc::Receiver<PathBuf>,
    debounce: Duration,
    current: &AppSlot,
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
    relaunch_loop_with(relauncher, &tick_rx, debounce, current, on_line)
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

    /// `--watch` combined with an explicit `-d <device>` that is a physical
    /// iOS device is a hard error with the original wording, before any
    /// build or watch happens (`FakeProcessRunner::new()` has no registered
    /// responses, so any build call would fail loudly and prove this check
    /// didn't run first) — the check is on the device kind, so the same
    /// refusal holds whatever the `-d` pattern was. An Android `-d`
    /// (`run_in_with_watch_and_an_android_device_starts_a_hot_session`) and
    /// an iOS simulator
    /// (`run_in_with_watch_and_an_ios_simulator_starts_a_hot_session`) are
    /// accepted instead.
    #[test]
    fn run_in_rejects_watch_combined_with_device_id() {
        let runner = FakeProcessRunner::new();
        for no_hot in [false, true] {
            let device = ios_physical_device();
            let err = run_watch_on_device(
                &runner,
                &device,
                &debug_info(),
                NO_EXTRA,
                no_hot,
                WatchHooks::fake(),
            )
            .unwrap_err();
            let message = err.to_string();
            assert_eq!(message, WATCH_DEVICE_REJECTION, "{device:?}");
            assert!(message.contains("--watch"), "{message}");
            assert!(message.contains("device"), "{message}");
            assert!(message.contains("Android"), "{message}");
            assert!(message.contains("iOS simulator"), "{message}");
            assert!(message.contains("physical iOS device"), "{message}");
            assert!(!message.contains("desktop-preview only"), "{message}");
            assert!(message.contains("drop -d"), "{message}");
            assert!(message.contains("drop --watch"), "{message}");
        }
        assert_eq!(WatchedDevice::of(&ios_physical_device()), None);
        assert_eq!(
            WatchedDevice::of(&fake_simulator()),
            Some(WatchedDevice::Simulator)
        );
        assert_eq!(
            WatchedDevice::of(&fake_android()),
            Some(WatchedDevice::Android)
        );
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
        assert_eq!(err.to_string(), WATCH_DEVICE_REJECTION);
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
        /// Which start (1-based) opened it, and the backend's event log its
        /// drop is recorded in.
        start: usize,
        events: Arc<Mutex<Vec<String>>>,
    }

    impl Drop for FakeSession {
        fn drop(&mut self) {
            let event = format!("session {} dropped", self.start);
            self.events.lock().unwrap().push(event);
        }
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
        /// Set for a device backend: every app it starts counts its
        /// teardown (forward removal + force-stop) here.
        teardowns: Option<Arc<AtomicUsize>>,
        /// Whether its starts honour `cancel` (a device backend's do).
        cancellable: bool,
        /// A Ctrl-C one start fires mid-way.
        interrupt: Option<Interrupt>,
        /// Session drops and (device) app stops, in order.
        events: Arc<Mutex<Vec<String>>>,
    }

    /// A Ctrl-C a scripted start fires mid-way, through [`on_ctrlc`] over
    /// the slot the fake `install_ctrlc` hook captured.
    struct Interrupt {
        /// Which start (1-based) it lands in.
        at_start: usize,
        slot: Arc<Mutex<Option<AppSlot>>>,
        /// Whether the start observes it and tears its app down itself
        /// (`start_android`'s cancel), or it lands after the start's last
        /// check and the start hands the app back.
        unwound_by_start: bool,
    }

    impl FakeBackend {
        fn new(starts: Vec<ScriptedStart>) -> Self {
            Self {
                runner: FakeProcessRunner::new().with_hanging_stream("app", ["up"]),
                starts: starts.into(),
                started: Arc::new(AtomicUsize::new(0)),
                calls: Arc::new(Mutex::new(Vec::new())),
                set: WatchSet::default(),
                teardowns: None,
                cancellable: false,
                interrupt: None,
                events: Arc::new(Mutex::new(Vec::new())),
            }
        }

        /// The backend of an Android device: its apps have a teardown, and
        /// its starts honour `cancel`.
        fn on_device(starts: Vec<ScriptedStart>) -> Self {
            Self {
                teardowns: Some(Arc::new(AtomicUsize::new(0))),
                cancellable: true,
                ..Self::new(starts)
            }
        }

        fn teardowns(&self) -> usize {
            self.teardowns
                .as_ref()
                .expect("a device backend")
                .load(Ordering::SeqCst)
        }
    }

    impl HotBackend for FakeBackend {
        fn watch_set(&mut self) -> Result<WatchSet> {
            Ok(self.set.clone())
        }

        fn start(
            &mut self,
            _on_line: &mut dyn FnMut(&str),
            cancel: &AtomicBool,
        ) -> Result<HotLaunch, HotStartError> {
            let n = self.started.fetch_add(1, Ordering::SeqCst) + 1;
            let (outcomes, restart_only) = self.starts.pop_front().expect("scripted start")?;
            let handle = self.runner.spawn_streaming("app", &[], None, &[]).unwrap();
            let session = FakeSession {
                outcomes: outcomes.into(),
                calls: Arc::clone(&self.calls),
                restart_only,
                start: n,
                events: Arc::clone(&self.events),
            };
            let mut app = RunningApp::from(handle);
            if let Some(count) = &self.teardowns {
                let count = Arc::clone(count);
                let events = Arc::clone(&self.events);
                app.teardown = Some(Box::new(move || {
                    count.fetch_add(1, Ordering::SeqCst);
                    events.lock().unwrap().push(format!("app {n} stopped"));
                }));
            }
            if let Some(interrupt) = self.interrupt.as_ref().filter(|i| i.at_start == n) {
                let slot = interrupt
                    .slot
                    .lock()
                    .unwrap()
                    .clone()
                    .expect("handler installed");
                assert!(
                    !on_ctrlc(&slot),
                    "a first Ctrl-C during a cancellable start leaves the exit to the loop"
                );
                assert!(cancel.load(Ordering::SeqCst), "the start sees the cancel");
                if interrupt.unwound_by_start {
                    app.stop();
                    return Err(HotStartError::Failed(vec!["cancelled".to_string()]));
                }
            }
            Ok((Box::new(session), app))
        }

        fn cancellable(&self) -> bool {
            self.cancellable
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
        let current = AppSlot::default();
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
        assert!(
            lines.iter().any(|l| l.starts_with(
                "restart required: hot-patch builder unsupported: the app does not advertise \
                 the HotPatch capability"
            )),
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
            RestartReason::BuilderUnsupported {
                detail: "debuginfo off: no debuginfo".to_string(),
            },
        ))]);
        let lines = drive_hot(&mut backend, &["/p/src/lib.rs"]);
        assert!(
            lines.iter().any(|l| l.starts_with(
                "restart required: hot-patch builder unsupported: debuginfo off: no debuginfo"
            )),
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
        let current = AppSlot::default();
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
        let set = watch_set(&graph);
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
            spawn_watcher: Box::new(|_, _| {
                unreachable!("the tick watcher is for the relaunch loop")
            }),
            hot: Some(HotHooks {
                backend: Box::new(move |_, _| Ok(Box::new(backend) as Box<dyn HotBackend>)),
                android: Box::new(|_, _, _| unreachable!("a desktop run builds no device backend")),
                ios_sim: Box::new(|_, _, _| unreachable!("a desktop run builds no device backend")),
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
            device_relauncher: Box::new(|_, _, _, _| {
                unreachable!("a desktop run relaunches no device")
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
                    android: Box::new(|_, _, _| panic!("--no-hot must not build a hot backend")),
                    ios_sim: Box::new(|_, _, _| panic!("--no-hot must not build a hot backend")),
                    spawn_watcher: Box::new(|_, _| panic!("--no-hot must not watch paths")),
                }),
                device_relauncher: Box::new(|_, _, _, _| {
                    panic!("a desktop run relaunches no device")
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

    // ---- Android `--watch -d` -------------------------------------------

    /// An Android phone as discovery reports it. The serial is a fake.
    fn fake_android() -> Device {
        Device {
            id: "FAKE-SERIAL".to_string(),
            name: "Fake Phone".to_string(),
            platform: Platform::Android,
            kind: Kind::PhysicalDevice,
            os_version: None,
            connection_state: None,
        }
    }

    /// A runner whose `adb devices -l` lists [`fake_android`].
    fn adb_lists_fake_android() -> FakeProcessRunner {
        FakeProcessRunner::new().with(
            "adb devices -l",
            Output {
                success: true,
                stdout: "List of devices attached\n\
                         FAKE-SERIAL  device product:fake model:Fake_Phone device:fake transport_id:1\n"
                    .to_string(),
                stderr: String::new(),
            },
        )
    }

    /// A scripted device relaunch target: each relaunch stops the slot's app
    /// and parks a fresh one whose teardown is counted.
    struct FakeDeviceRelauncher {
        runner: FakeProcessRunner,
        relaunches: Arc<AtomicUsize>,
        teardowns: Arc<AtomicUsize>,
        ctrl_c_at: Option<usize>,
    }

    impl FakeDeviceRelauncher {
        fn new() -> Self {
            Self {
                runner: FakeProcessRunner::new().with_hanging_stream("logcat", ["cold app"]),
                relaunches: Arc::new(AtomicUsize::new(0)),
                teardowns: Arc::new(AtomicUsize::new(0)),
                ctrl_c_at: None,
            }
        }
    }

    impl Relauncher for FakeDeviceRelauncher {
        fn label(&self) -> &str {
            "the app's log stream"
        }

        /// Shaped like [`DeviceRelauncher::relaunch`]: a cancellable start
        /// that parks its app through [`StartInFlight::end`]. With
        /// `ctrl_c_at` set, that relaunch (1-based) is interrupted by a
        /// Ctrl-C landing after its app launched.
        fn relaunch(
            &mut self,
            current: &AppSlot,
            _on_line: &mut dyn FnMut(&str),
        ) -> Result<ControlFlow<()>> {
            let previous = lock_slot(current).take();
            if let Some(app) = previous {
                app.stop();
            }
            let start = current.begin_start();
            let n = self.relaunches.fetch_add(1, Ordering::SeqCst) + 1;
            let teardowns = Arc::clone(&self.teardowns);
            let app = RunningApp {
                handle: self.runner.spawn_streaming("logcat", &[], None, &[])?,
                teardown: Some(Box::new(move || {
                    teardowns.fetch_add(1, Ordering::SeqCst);
                })),
            };
            if self.ctrl_c_at == Some(n) {
                assert!(!on_ctrlc(current), "the exit is left to the loop");
            }
            Ok(start.end_or_stop(Some(app)))
        }
    }

    /// Drives [`hot_loop_with`] over a device backend and `relauncher`, sending
    /// each path tick, then lets the loop end. Returns the printed lines.
    fn drive_device_hot(
        backend: &mut FakeBackend,
        relauncher: &mut FakeDeviceRelauncher,
        ticks: &[&str],
    ) -> Vec<String> {
        let (tx, rx) = mpsc::channel();
        let ticks: Vec<PathBuf> = ticks.iter().map(PathBuf::from).collect();
        let sender = std::thread::spawn(move || {
            for path in ticks {
                std::thread::sleep(Duration::from_millis(120));
                tx.send(path).unwrap();
            }
            std::thread::sleep(Duration::from_millis(250));
        });
        let current = AppSlot::default();
        let mut lines = Vec::new();
        let mut on_line = |line: &str| lines.push(line.to_string());
        let out = hot_loop_with(
            relauncher,
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

    /// Criterion 1: `frust run -d <serial> --watch` (debug) resolves the
    /// Android device through discovery and starts a hot session on it — the
    /// Android backend is built for exactly that device, the desktop one never
    /// is, and the changed path reaches the session. The device relaunch
    /// loop is never run, and the app is torn down (forward removed,
    /// force-stopped) when the loop ends.
    #[test]
    fn run_in_with_watch_and_an_android_device_starts_a_hot_session() {
        let mut backend = FakeBackend::on_device(vec![Ok((
            vec![Outcome::Patched {
                ms: 5,
                components: 1,
            }],
            None,
        ))]);
        backend.set.roots = vec![PathBuf::from("/w/app")];
        let calls = Arc::clone(&backend.calls);
        let teardowns = Arc::clone(backend.teardowns.as_ref().unwrap());
        let seen_device: Arc<Mutex<Option<Device>>> = Arc::new(Mutex::new(None));
        let seen = Arc::clone(&seen_device);
        let relauncher = FakeDeviceRelauncher::new();
        let relaunches = Arc::clone(&relauncher.relaunches);
        let hooks = RunHooks {
            watch: WatchHooks {
                install_ctrlc: Box::new(|_| Ok(())),
                spawn_watcher: Box::new(|_, _| unreachable!("the tick watcher is for --no-hot")),
                hot: Some(HotHooks {
                    backend: Box::new(|_, _| panic!("a device run builds no desktop backend")),
                    android: Box::new(move |_, info, device| {
                        assert_eq!(info.mode, BuildMode::Debug);
                        *seen.lock().unwrap() = Some(device.clone());
                        Ok(Box::new(backend) as Box<dyn HotBackend>)
                    }),
                    ios_sim: Box::new(|_, _, _| {
                        panic!("an Android run builds no simulator backend")
                    }),
                    spawn_watcher: Box::new(|_, tx| {
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(150));
                            tx.send(PathBuf::from("/w/app/src/lib.rs")).unwrap();
                            std::thread::sleep(Duration::from_millis(400));
                        });
                        Ok(Box::new(()) as Box<dyn std::any::Any>)
                    }),
                }),
                device_relauncher: Box::new(move |_, _, device, watched| {
                    assert_eq!(device.id, "FAKE-SERIAL");
                    assert_eq!(watched, WatchedDevice::Android);
                    Box::new(relauncher) as Box<dyn Relauncher>
                }),
            },
            web: WebRunHooks::fake(),
        };

        let out = run_in_with_hooks(
            &adb_lists_fake_android(),
            BuildFlags::default(),
            Some("FAKE-SERIAL".to_string()),
            true,
            false,
            false,
            false,
            hooks,
        )
        .unwrap();

        assert_eq!(out, 0);
        assert_eq!(seen_device.lock().unwrap().clone(), Some(fake_android()));
        assert_eq!(
            *calls.lock().unwrap(),
            vec![vec![PathBuf::from("/w/app/src/lib.rs")]]
        );
        assert_eq!(relaunches.load(Ordering::SeqCst), 0);
        assert_eq!(teardowns.load(Ordering::SeqCst), 1, "the app is torn down");
    }

    /// Criterion 1: `--no-hot` on Android never builds a hot backend and runs
    /// the device relaunch loop instead: the pipeline at start, then again on
    /// a change, the previous app torn down each time.
    #[test]
    fn no_hot_on_android_runs_the_device_relaunch_loop_and_never_builds_a_backend() {
        let relauncher = FakeDeviceRelauncher::new();
        let relaunches = Arc::clone(&relauncher.relaunches);
        let teardowns = Arc::clone(&relauncher.teardowns);
        let hooks = RunHooks {
            watch: WatchHooks {
                install_ctrlc: Box::new(|_| Ok(())),
                spawn_watcher: Box::new(|_, tx| {
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(150));
                        tx.send(()).unwrap();
                        std::thread::sleep(Duration::from_millis(300));
                    });
                    Ok(Box::new(()) as Box<dyn std::any::Any>)
                }),
                hot: Some(HotHooks {
                    backend: Box::new(|_, _| panic!("--no-hot must not build a hot backend")),
                    android: Box::new(|_, _, _| panic!("--no-hot must not build a hot backend")),
                    ios_sim: Box::new(|_, _, _| panic!("--no-hot must not build a hot backend")),
                    spawn_watcher: Box::new(|_, _| panic!("--no-hot must not watch paths")),
                }),
                device_relauncher: Box::new(move |_, _, _, _| {
                    Box::new(relauncher) as Box<dyn Relauncher>
                }),
            },
            web: WebRunHooks::fake(),
        };

        let out = run_in_with_hooks(
            &adb_lists_fake_android(),
            BuildFlags::default(),
            Some("FAKE".to_string()),
            true,
            true,
            false,
            false,
            hooks,
        )
        .unwrap();

        assert_eq!(out, 0);
        assert_eq!(relaunches.load(Ordering::SeqCst), 2, "start + one change");
        assert_eq!(
            teardowns.load(Ordering::SeqCst),
            2,
            "the replaced app and the last one are both torn down"
        );
    }

    /// Criterion 1: an Android app without `Capability::HotPatch` (a
    /// restart-only session) prints `restart required: hot-patch builder
    /// unsupported: <precondition>` once, is never asked to patch, and the
    /// device relaunch loop takes over: the first change tears the adopted
    /// hot app down (its forward included) and reruns the device pipeline.
    #[test]
    fn an_android_app_without_hot_patch_prints_builder_unsupported_once_and_relaunch_loops() {
        let mut backend = FakeBackend::on_device(vec![Ok((
            Vec::new(),
            Some("the app does not advertise the HotPatch capability".to_string()),
        ))]);
        let mut relauncher = FakeDeviceRelauncher::new();
        let lines = drive_device_hot(&mut backend, &mut relauncher, &["/w/app/src/lib.rs"]);

        let expected = "restart required: hot-patch builder unsupported: the app does not \
                        advertise the HotPatch capability; hot reload unavailable, relaunching \
                        on change instead";
        assert_eq!(
            lines
                .iter()
                .filter(|l| l.starts_with("restart required"))
                .count(),
            1,
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l == expected), "{lines:?}");
        assert!(backend.calls.lock().unwrap().is_empty());
        assert_eq!(backend.started.load(Ordering::SeqCst), 1);
        assert_eq!(relauncher.relaunches.load(Ordering::SeqCst), 1, "{lines:?}");
        assert_eq!(backend.teardowns(), 1, "the adopted hot app is torn down");
        assert!(lines.iter().any(|l| l == "cold app"), "{lines:?}");
    }

    /// Criterion 2: a `RestartRequired` prints its reason verbatim, tears the
    /// app down (forward removed, package force-stopped) and reruns the full
    /// device pipeline through the backend (a fresh fat session), never the
    /// cold relaunch.
    #[test]
    fn an_android_restart_required_reruns_the_device_pipeline() {
        let reason = RestartReason::LayoutChanged {
            records: vec!["HomeState changed layout (4 → 8 bytes)".to_string()],
        };
        let mut backend = FakeBackend::on_device(vec![
            Ok((vec![restart(reason)], None)),
            Ok((Vec::new(), None)),
        ]);
        let mut relauncher = FakeDeviceRelauncher::new();
        let lines = drive_device_hot(&mut backend, &mut relauncher, &["/w/app/src/home.rs"]);

        assert!(
            lines.iter().any(|l| l
                == "restart required: HomeState changed layout (4 → 8 bytes); restarting to \
                    keep memory safe"),
            "{lines:?}"
        );
        assert_eq!(backend.started.load(Ordering::SeqCst), 2, "{lines:?}");
        assert_eq!(
            backend.teardowns(),
            2,
            "the restarted app at the restart, the fresh one at the end"
        );
        assert_eq!(relauncher.relaunches.load(Ordering::SeqCst), 0);
    }

    /// Criterion 2: `patched in N ms (...)` is printed only for the
    /// session's own `Patched` answer — a patch with no seam hit comes back
    /// as `RestartRequired(NoSeamHit)` and prints `restart required: ...`,
    /// relaunching through the device pipeline.
    #[test]
    fn an_android_session_prints_patched_only_for_a_patched_answer() {
        let mut backend = FakeBackend::on_device(vec![
            Ok((
                vec![
                    Outcome::Patched {
                        ms: 41,
                        components: 3,
                    },
                    restart(RestartReason::NoSeamHit),
                ],
                None,
            )),
            Ok((Vec::new(), None)),
        ]);
        let mut relauncher = FakeDeviceRelauncher::new();
        let lines = drive_device_hot(
            &mut backend,
            &mut relauncher,
            &["/w/app/src/a.rs", "/w/app/src/b.rs"],
        );

        assert_eq!(
            lines.iter().filter(|l| l.starts_with("patched in")).count(),
            1,
            "{lines:?}"
        );
        assert!(
            lines.contains(&"patched in 41 ms (3 components rebuilt)".to_string()),
            "{lines:?}"
        );
        assert!(
            lines.contains(&format!("restart required: {}", RestartReason::NoSeamHit)),
            "{lines:?}"
        );
        assert_eq!(backend.started.load(Ordering::SeqCst), 2, "{lines:?}");
    }

    /// `--watch -d <android>` accepts only a debug build without
    /// `--features`, refusing before anything runs.
    #[test]
    fn android_watch_refuses_a_non_debug_build_and_a_features_passthrough() {
        let runner = FakeProcessRunner::new();
        for info in [profile_info(), release_info()] {
            let err = run_watch_on_device(
                &runner,
                &fake_android(),
                &info,
                NO_EXTRA,
                false,
                WatchHooks::fake(),
            )
            .unwrap_err();
            assert!(err.to_string().contains("needs a debug build"), "{err}");
        }
        let err = run_watch_on_device(
            &runner,
            &fake_android(),
            &debug_info(),
            &["extra".to_string()],
            false,
            WatchHooks::fake(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("--features"), "{err}");
    }

    /// Records every `run` invocation (answered by the inner fake) in
    /// `runs`, and every invocation of any kind, in order, in `all` (a
    /// streamed one prefixed `stream `, a spawned one `spawn `).
    struct RecordingRunner {
        inner: FakeProcessRunner,
        runs: Mutex<Vec<String>>,
        all: Mutex<Vec<String>>,
        /// A Ctrl-C ([`on_ctrlc`]) fired over this slot right after a
        /// spawn: one landing after a pipeline's last cancel check.
        ctrl_c_on_spawn: Mutex<Option<AppSlot>>,
    }

    fn invocation(cmd: &str, args: &[&str]) -> String {
        std::iter::once(cmd)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ")
    }

    impl RecordingRunner {
        fn new(inner: FakeProcessRunner) -> Arc<Self> {
            Arc::new(Self {
                inner,
                runs: Mutex::new(Vec::new()),
                all: Mutex::new(Vec::new()),
                ctrl_c_on_spawn: Mutex::new(None),
            })
        }
    }

    impl ProcessRunner for RecordingRunner {
        fn run(&self, cmd: &str, args: &[&str]) -> Result<Output> {
            let key = invocation(cmd, args);
            self.runs.lock().unwrap().push(key.clone());
            self.all.lock().unwrap().push(key);
            self.inner.run(cmd, args)
        }

        fn run_streaming(
            &self,
            cmd: &str,
            args: &[&str],
            cwd: Option<&Path>,
            env: &[(&str, &str)],
            on_line: &mut dyn FnMut(&str),
        ) -> Result<Output> {
            let key = format!("stream {}", invocation(cmd, args));
            self.all.lock().unwrap().push(key);
            self.inner.run_streaming(cmd, args, cwd, env, on_line)
        }

        fn spawn_streaming(
            &self,
            cmd: &str,
            args: &[&str],
            cwd: Option<&Path>,
            env: &[(&str, &str)],
        ) -> Result<StreamHandle> {
            let key = format!("spawn {}", invocation(cmd, args));
            self.all.lock().unwrap().push(key);
            let handle = self.inner.spawn_streaming(cmd, args, cwd, env)?;
            if let Some(slot) = self.ctrl_c_on_spawn.lock().unwrap().take() {
                assert!(!on_ctrlc(&slot), "the start in flight is cancelled");
            }
            Ok(handle)
        }
    }

    /// A device app's teardown removes the hot session's `adb forward` (when
    /// one was allocated) and force-stops the launched package, addressed by
    /// the resolved device's serial; a cold app (no forward) only
    /// force-stops.
    #[test]
    fn a_device_app_teardown_removes_the_forward_and_force_stops_the_package() {
        for (port, expected) in [
            (
                Some(41234),
                vec![
                    "adb -s FAKE-SERIAL forward --remove tcp:41234",
                    "adb -s FAKE-SERIAL shell am force-stop it.example.fake",
                ],
            ),
            (
                None,
                vec!["adb -s FAKE-SERIAL shell am force-stop it.example.fake"],
            ),
        ] {
            let runner = RecordingRunner::new(
                FakeProcessRunner::new().with_hanging_stream("logcat", ["log"]),
            );
            let launch = AndroidLaunch {
                stream: runner.spawn_streaming("logcat", &[], None, &[]).unwrap(),
                package: "it.example.fake".to_string(),
            };
            let app = device_app(runner.clone(), "FAKE-SERIAL", launch, port);
            app.stop();
            assert_eq!(*runner.runs.lock().unwrap(), expected);
        }
    }

    /// The real device relauncher stops the previous app before rerunning
    /// the pipeline, and a pipeline failure (here: no project at the root)
    /// is reported and leaves the loop running with nothing launched.
    #[test]
    fn the_device_relauncher_stops_the_previous_app_and_survives_a_pipeline_failure() {
        let runner = FakeProcessRunner::new().with_hanging_stream("app", ["up"]);
        let stopped = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&stopped);
        let current = AppSlot::default();
        lock_slot(&current).replace(RunningApp {
            handle: runner.spawn_streaming("app", &[], None, &[]).unwrap(),
            teardown: Some(Box::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
            })),
        });
        let mut relauncher = DeviceRelauncher {
            runner: Arc::new(FakeProcessRunner::new()),
            root: PathBuf::from("/nonexistent/frust-cli-device-watch"),
            device: fake_android(),
            info: debug_info(),
        };
        let mut lines = Vec::new();
        let flow = relauncher
            .relaunch(&current, &mut |line| lines.push(line.to_string()))
            .unwrap();
        assert_eq!(flow, ControlFlow::Continue(()));
        assert_eq!(stopped.load(Ordering::SeqCst), 1);
        assert!(lock_slot(&current).is_none());
        assert!(!current.starting.load(Ordering::SeqCst), "the start ended");
        assert!(
            lines.iter().any(|l| l.contains("device pipeline failed")),
            "{lines:?}"
        );
    }

    /// Criterion 2: the real device relauncher runs the pipeline under the
    /// slot's shared cancel flag — a Ctrl-C raised it, so the relaunch ends
    /// the loop (`Break`) with nothing parked, and a failure the cancel
    /// caused is not reported as a retry.
    #[test]
    fn a_cancelled_device_relaunch_ends_the_loop() {
        let current = AppSlot::default();
        assert!(on_ctrlc(&current), "no start in flight: exit at once");
        let mut relauncher = DeviceRelauncher {
            runner: Arc::new(FakeProcessRunner::new()),
            root: PathBuf::from("/nonexistent/frust-cli-device-watch"),
            device: fake_android(),
            info: debug_info(),
        };
        let mut lines = Vec::new();
        let flow = relauncher
            .relaunch(&current, &mut |line| lines.push(line.to_string()))
            .unwrap();
        assert_eq!(flow, ControlFlow::Break(()));
        assert!(lock_slot(&current).is_none());
        assert!(
            !lines
                .iter()
                .any(|l| l.contains("watching for a source change")),
            "{lines:?}"
        );
    }

    /// Criterion 1/2 (`--no-hot`): with a Ctrl-C landed during the start, a
    /// device pipeline that issued `am start` and then answered a cancel
    /// or a failure (the SIGINT also killed the in-flight `adb shell am
    /// start`) force-stops the package it launched and ends the loop,
    /// reporting no retry; one stopped before `am start` stops nothing.
    /// Without the Ctrl-C a failure is reported for a retry and stops
    /// nothing (the negative control). One that launched hands its app to
    /// [`StartInFlight::end`], which stops it (logcat killed, force-stop).
    #[test]
    fn a_cancelled_device_launch_force_stops_what_it_launched() {
        let recording = || {
            RecordingRunner::new(FakeProcessRunner::new().with_hanging_stream("logcat", ["log"]))
        };
        let force_stop = "adb -s FAKE-SERIAL shell am force-stop it.example.fake";
        let am_start_failed = || Some(anyhow::anyhow!("`adb shell am start` failed: killed"));

        for (ctrl_c, error, launched, stops) in [
            (true, None, Some("it.example.fake"), true),
            (true, am_start_failed(), Some("it.example.fake"), true),
            (true, None, None, false),
            (false, am_start_failed(), Some("it.example.fake"), false),
        ] {
            let failed = error.is_some();
            let runner = recording();
            let current = AppSlot::default();
            let start = current.begin_start();
            if ctrl_c {
                assert!(!on_ctrlc(&current), "the start in flight is cancelled");
            }
            let mut lines = Vec::new();
            let flow = settle_device_launch(
                runner.clone(),
                "FAKE-SERIAL",
                Err(StoppedShort {
                    error,
                    launched: launched.map(str::to_string),
                }),
                start,
                &mut |line| lines.push(line.to_string()),
            );
            let case = format!("ctrl_c {ctrl_c}, failed {failed}, launched {launched:?}");
            let expected: Vec<&str> = if stops { vec![force_stop] } else { vec![] };
            assert_eq!(*runner.runs.lock().unwrap(), expected, "{case}");
            assert_eq!(flow.is_break(), ctrl_c, "{case}");
            assert_eq!(
                lines
                    .iter()
                    .any(|l| l.contains("watching for a source change")),
                failed && !ctrl_c,
                "{case}: {lines:?}"
            );
            assert!(!current.starting.load(Ordering::SeqCst), "{case}: ended");
        }

        let runner = recording();
        let current = AppSlot::default();
        let start = current.begin_start();
        assert!(!on_ctrlc(&current));
        let launch = AndroidLaunch {
            stream: runner.spawn_streaming("logcat", &[], None, &[]).unwrap(),
            package: "it.example.fake".to_string(),
        };
        let flow = settle_device_launch(
            runner.clone(),
            "FAKE-SERIAL",
            Ok(launch),
            start,
            &mut |_| {},
        );
        assert_eq!(flow, ControlFlow::Break(()));
        assert!(lock_slot(&current).is_none(), "nothing parked");
        assert_eq!(*runner.runs.lock().unwrap(), vec![force_stop]);
    }

    /// Criterion 3: a start that ignores the cancel flag (the desktop one)
    /// takes no start mark, so a raised flag cannot end it: its app is
    /// parked and its session is live.
    #[test]
    fn a_start_that_ignores_cancel_parks_its_app_whatever_the_flag_says() {
        let current = AppSlot::default();
        current.cancel.store(true, Ordering::SeqCst);
        let mut backend = FakeBackend::new(vec![Ok((Vec::new(), None))]);
        let launched = launch_hot(&mut backend, &current, &mut |_| {});
        assert!(matches!(launched, Launched::Live(_)));
        assert!(lock_slot(&current).is_some(), "the app is parked");
        assert!(!current.starting.load(Ordering::SeqCst));
        if let Some(app) = lock_slot(&current).take() {
            app.stop();
        }
    }

    /// The handler outside a start stops the live app and exits at once;
    /// during a cancellable start the first Ctrl-C raises the cancel flag
    /// and leaves the exit to the loop, and a second exits at once.
    #[test]
    fn ctrl_c_exits_at_once_except_during_a_cancellable_start() {
        let runner = FakeProcessRunner::new().with_hanging_stream("app", ["up"]);
        let stopped = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&stopped);
        let current = AppSlot::default();
        lock_slot(&current).replace(RunningApp {
            handle: runner.spawn_streaming("app", &[], None, &[]).unwrap(),
            teardown: Some(Box::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
            })),
        });
        assert!(on_ctrlc(&current), "an attached app: exit at once");
        assert_eq!(stopped.load(Ordering::SeqCst), 1, "after stopping it");
        assert!(lock_slot(&current).is_none());

        let current = AppSlot::default();
        let start = current.begin_start();
        assert!(!on_ctrlc(&current), "first Ctrl-C during a start");
        assert!(current.cancel.load(Ordering::SeqCst));
        assert!(on_ctrlc(&current), "a second Ctrl-C exits at once");
        drop(start);
        assert!(
            !current.starting.load(Ordering::SeqCst),
            "a dropped mark clears"
        );
    }

    /// Criterion 1: a Ctrl-C during the Android hot start, wired through
    /// the `install_ctrlc` hook, cancels the start instead of exiting; the
    /// start tears down what it launched (`start_android`'s unwind) and the
    /// watch ends with 0 — no patch, no relaunch, no retry.
    #[test]
    fn a_ctrl_c_during_the_android_hot_start_tears_it_down_and_ends_the_watch() {
        let captured: Arc<Mutex<Option<AppSlot>>> = Arc::new(Mutex::new(None));
        let mut backend = FakeBackend::on_device(vec![Ok((Vec::new(), None))]);
        backend.interrupt = Some(Interrupt {
            at_start: 1,
            slot: Arc::clone(&captured),
            unwound_by_start: true,
        });
        let calls = Arc::clone(&backend.calls);
        let started = Arc::clone(&backend.started);
        let teardowns = Arc::clone(backend.teardowns.as_ref().unwrap());
        let relauncher = FakeDeviceRelauncher::new();
        let relaunches = Arc::clone(&relauncher.relaunches);
        // The watcher outlives the loop: only the Ctrl-C can end it.
        let (keep_tx, keep_rx) = mpsc::channel::<mpsc::Sender<PathBuf>>();
        let hooks = RunHooks {
            watch: WatchHooks {
                install_ctrlc: Box::new(move |slot| {
                    *captured.lock().unwrap() = Some(slot);
                    Ok(())
                }),
                spawn_watcher: Box::new(|_, _| unreachable!("the tick watcher is for --no-hot")),
                hot: Some(HotHooks {
                    backend: Box::new(|_, _| panic!("a device run builds no desktop backend")),
                    android: Box::new(move |_, _, _| Ok(Box::new(backend) as Box<dyn HotBackend>)),
                    ios_sim: Box::new(|_, _, _| {
                        panic!("an Android run builds no simulator backend")
                    }),
                    spawn_watcher: Box::new(move |_, tx| {
                        keep_tx.send(tx).unwrap();
                        Ok(Box::new(()) as Box<dyn std::any::Any>)
                    }),
                }),
                device_relauncher: Box::new(move |_, _, _, _| {
                    Box::new(relauncher) as Box<dyn Relauncher>
                }),
            },
            web: WebRunHooks::fake(),
        };

        let out = run_in_with_hooks(
            &adb_lists_fake_android(),
            BuildFlags::default(),
            Some("FAKE-SERIAL".to_string()),
            true,
            false,
            false,
            false,
            hooks,
        )
        .unwrap();
        drop(keep_rx);

        assert_eq!(out, 0);
        assert_eq!(started.load(Ordering::SeqCst), 1, "no retry");
        assert_eq!(teardowns.load(Ordering::SeqCst), 1, "the launched app");
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(relaunches.load(Ordering::SeqCst), 0);
    }

    /// Criterion 1: a Ctrl-C during a hot restart's fresh start that lands
    /// after the start's last cancel check — the start hands its app back —
    /// stops that app before the loop ends with 0: the restarted app and
    /// the fresh one are both torn down, each after its session is
    /// dropped. Deterministic: the one change is queued before the loop
    /// runs and its sender outlives the loop, so only the Ctrl-C ends it.
    #[test]
    fn a_ctrl_c_during_an_android_hot_restart_stops_the_fresh_app_and_ends_the_loop() {
        let current = AppSlot::default();
        let mut backend = FakeBackend::on_device(vec![
            Ok((vec![restart(RestartReason::NoSeamHit)], None)),
            Ok((Vec::new(), None)),
        ]);
        backend.interrupt = Some(Interrupt {
            at_start: 2,
            slot: Arc::new(Mutex::new(Some(Arc::clone(&current)))),
            unwound_by_start: false,
        });
        let events = Arc::clone(&backend.events);
        let mut relauncher = FakeDeviceRelauncher::new();
        let (tx, rx) = mpsc::channel();
        tx.send(PathBuf::from("/w/app/src/a.rs")).unwrap();
        let mut lines = Vec::new();
        let out = hot_loop_with(
            &mut relauncher,
            &mut backend,
            rx,
            Duration::from_millis(10),
            &current,
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();
        drop(tx);

        assert_eq!(out, 0);
        assert_eq!(backend.started.load(Ordering::SeqCst), 2, "{lines:?}");
        assert_eq!(
            backend.teardowns(),
            2,
            "the restarted app and the fresh one"
        );
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                "session 1 dropped",
                "app 1 stopped",
                "session 2 dropped",
                "app 2 stopped"
            ],
            "criterion 6: each session goes before its app"
        );
        assert!(lock_slot(&current).is_none());
        assert_eq!(relauncher.relaunches.load(Ordering::SeqCst), 0);
    }

    /// Criterion 2: a Ctrl-C during a `--no-hot` device relaunch cancels it
    /// instead of exiting, and the relaunch loop ends with 0 once the
    /// relaunch has stopped what it launched. Deterministic as above: the
    /// change is queued up front and its sender outlives the loop.
    #[test]
    fn a_ctrl_c_during_a_no_hot_device_relaunch_ends_the_relaunch_loop() {
        let current = AppSlot::default();
        let mut relauncher = FakeDeviceRelauncher::new();
        relauncher.ctrl_c_at = Some(2);
        let (tx, rx) = mpsc::channel();
        tx.send(()).unwrap();
        let out = relaunch_loop_with(
            &mut relauncher,
            &rx,
            Duration::from_millis(10),
            &current,
            &mut |_| {},
        )
        .unwrap();
        drop(tx);

        assert_eq!(out, 0);
        assert_eq!(relauncher.relaunches.load(Ordering::SeqCst), 2);
        assert_eq!(
            relauncher.teardowns.load(Ordering::SeqCst),
            2,
            "the replaced app and the cancelled relaunch's app"
        );
        assert!(lock_slot(&current).is_none());
    }

    // ---- iOS simulator `--watch -d` -------------------------------------

    /// A booted iOS simulator as discovery reports it. The udid is a fake.
    fn fake_simulator() -> Device {
        Device {
            id: "FAKE-UDID".to_string(),
            name: "iPhone 15".to_string(),
            platform: Platform::Ios,
            kind: Kind::Simulator,
            os_version: None,
            connection_state: None,
        }
    }

    /// `xcrun simctl list devices --json` listing [`fake_simulator`] booted.
    const SIMCTL_BOOTED: &str = r#"{"devices": {"com.apple.CoreSimulator.SimRuntime.iOS-17-5": [
        {"udid": "FAKE-UDID", "name": "iPhone 15", "state": "Booted", "isAvailable": true}
    ]}}"#;

    fn ok(stdout: &str) -> Output {
        Output {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    /// A runner whose `xcrun simctl list devices --json` lists
    /// [`fake_simulator`] (`adb` and `devicectl` are absent).
    fn simctl_lists_fake_simulator() -> FakeProcessRunner {
        FakeProcessRunner::new().with("xcrun simctl list devices --json", ok(SIMCTL_BOOTED))
    }

    /// The bundle id [`simulator_project`] declares.
    const FAKE_BUNDLE: &str = "it.example.fake";

    const SIM_LAUNCH: &str = "xcrun simctl launch --console-pty FAKE-UDID it.example.fake";
    const SIM_TERMINATE: &str = "xcrun simctl terminate FAKE-UDID it.example.fake";

    /// A generated project the simulator pipeline accepts: `frust.toml`
    /// (bundle id [`FAKE_BUNDLE`]), `ios/Runner.xcodeproj`, and the Debug
    /// `Runner.app` the build would leave behind.
    fn simulator_project(tag: &str) -> PathBuf {
        use std::sync::atomic::AtomicU32;
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-cli-run-simulator-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ios/Runner.xcodeproj")).unwrap();
        std::fs::create_dir_all(
            dir.join("build/ios/Build/Products/Debug-iphonesimulator/Runner.app"),
        )
        .unwrap();
        std::fs::write(
            dir.join("frust.toml"),
            format!(
                "[app]\nname = \"fake\"\norg = \"it.example\"\n\n[ios]\nidentifier = \"{FAKE_BUNDLE}\"\n"
            ),
        )
        .unwrap();
        dir
    }

    /// A fake answering every step of the simulator pipeline on
    /// [`simulator_project`]: preflight, `xcodebuild`, `simctl install`,
    /// `simctl terminate`, and a `simctl launch --console-pty` stream that
    /// runs until killed.
    fn simulator_pipeline() -> FakeProcessRunner {
        simctl_lists_fake_simulator()
            .with(
                "xcode-select -p",
                ok("/Applications/Xcode.app/Contents/Developer\n"),
            )
            .with(
                "rustup target list --installed",
                ok("aarch64-apple-ios-sim\nx86_64-apple-ios\n"),
            )
            .with("xcrun xcodebuild", ok("Build succeeded"))
            .with("xcrun simctl install FAKE-UDID", ok(""))
            .with(SIM_TERMINATE, ok(""))
            .with_hanging_stream(SIM_LAUNCH, ["app up"])
    }

    /// The pipeline steps of a recorded run, preflight left out:
    /// `xcodebuild`, `install`, `launch`, `terminate`.
    fn pipeline_steps(all: &[String]) -> Vec<&'static str> {
        all.iter()
            .filter_map(|call| {
                if call.starts_with("stream xcrun xcodebuild ") {
                    Some("xcodebuild")
                } else if call.starts_with("xcrun simctl install FAKE-UDID ") {
                    Some("install")
                } else if call == &format!("spawn {SIM_LAUNCH}") {
                    Some("launch")
                } else if call == SIM_TERMINATE {
                    Some("terminate")
                } else {
                    None
                }
            })
            .collect()
    }

    /// Criterion 1: `frust run -d <udid> --watch` (debug) resolves the
    /// booted simulator through discovery and starts a hot session on it —
    /// the simulator backend is built for exactly that device, neither the
    /// desktop nor the Android one is, and the changed path reaches the
    /// session. The relaunch loop is never run, and the app is torn down
    /// when the loop ends.
    // Simulator discovery answers nothing off macOS (`IosSimulatorDiscovery`
    // consults `xcrun simctl` only there), so the fake listing is read on
    // macOS hosts only — as `watch_d_resolves_a_simulator_devicectl_also_lists_to_the_simulator`.
    #[cfg(target_os = "macos")]
    #[test]
    fn run_in_with_watch_and_an_ios_simulator_starts_a_hot_session() {
        let mut backend = FakeBackend::on_device(vec![Ok((
            vec![Outcome::Patched {
                ms: 5,
                components: 1,
            }],
            None,
        ))]);
        backend.set.roots = vec![PathBuf::from("/w/app")];
        let calls = Arc::clone(&backend.calls);
        let teardowns = Arc::clone(backend.teardowns.as_ref().unwrap());
        let seen_device: Arc<Mutex<Option<Device>>> = Arc::new(Mutex::new(None));
        let seen = Arc::clone(&seen_device);
        let relauncher = FakeDeviceRelauncher::new();
        let relaunches = Arc::clone(&relauncher.relaunches);
        let hooks = RunHooks {
            watch: WatchHooks {
                install_ctrlc: Box::new(|_| Ok(())),
                spawn_watcher: Box::new(|_, _| unreachable!("the tick watcher is for --no-hot")),
                hot: Some(HotHooks {
                    backend: Box::new(|_, _| panic!("a device run builds no desktop backend")),
                    android: Box::new(|_, _, _| {
                        panic!("a simulator run builds no Android backend")
                    }),
                    ios_sim: Box::new(move |_, info, device| {
                        assert_eq!(info.mode, BuildMode::Debug);
                        *seen.lock().unwrap() = Some(device.clone());
                        Ok(Box::new(backend) as Box<dyn HotBackend>)
                    }),
                    spawn_watcher: Box::new(|_, tx| {
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(150));
                            tx.send(PathBuf::from("/w/app/src/lib.rs")).unwrap();
                            std::thread::sleep(Duration::from_millis(400));
                        });
                        Ok(Box::new(()) as Box<dyn std::any::Any>)
                    }),
                }),
                device_relauncher: Box::new(move |_, _, device, watched| {
                    assert_eq!(device, &fake_simulator());
                    assert_eq!(watched, WatchedDevice::Simulator);
                    Box::new(relauncher) as Box<dyn Relauncher>
                }),
            },
            web: WebRunHooks::fake(),
        };

        let out = run_in_with_hooks(
            &simctl_lists_fake_simulator(),
            BuildFlags::default(),
            Some("FAKE-UDID".to_string()),
            true,
            false,
            false,
            false,
            hooks,
        )
        .unwrap();

        assert_eq!(out, 0);
        assert_eq!(seen_device.lock().unwrap().clone(), Some(fake_simulator()));
        assert_eq!(
            *calls.lock().unwrap(),
            vec![vec![PathBuf::from("/w/app/src/lib.rs")]]
        );
        assert_eq!(relaunches.load(Ordering::SeqCst), 0);
        assert_eq!(teardowns.load(Ordering::SeqCst), 1, "the app is torn down");
    }

    /// Criterion 1: `--no-hot` on a simulator never builds a hot backend
    /// and runs the relaunch loop over the simulator's relaunch target
    /// instead: the pipeline at start, then again on a change, the previous
    /// app torn down each time.
    #[cfg(target_os = "macos")]
    #[test]
    fn no_hot_on_a_simulator_runs_the_relaunch_loop_and_never_builds_a_backend() {
        let relauncher = FakeDeviceRelauncher::new();
        let relaunches = Arc::clone(&relauncher.relaunches);
        let teardowns = Arc::clone(&relauncher.teardowns);
        let hooks = RunHooks {
            watch: WatchHooks {
                install_ctrlc: Box::new(|_| Ok(())),
                spawn_watcher: Box::new(|_, tx| {
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(150));
                        tx.send(()).unwrap();
                        std::thread::sleep(Duration::from_millis(300));
                    });
                    Ok(Box::new(()) as Box<dyn std::any::Any>)
                }),
                hot: Some(HotHooks {
                    backend: Box::new(|_, _| panic!("--no-hot must not build a hot backend")),
                    android: Box::new(|_, _, _| panic!("--no-hot must not build a hot backend")),
                    ios_sim: Box::new(|_, _, _| panic!("--no-hot must not build a hot backend")),
                    spawn_watcher: Box::new(|_, _| panic!("--no-hot must not watch paths")),
                }),
                device_relauncher: Box::new(move |_, _, device, watched| {
                    assert_eq!(device, &fake_simulator());
                    assert_eq!(watched, WatchedDevice::Simulator);
                    Box::new(relauncher) as Box<dyn Relauncher>
                }),
            },
            web: WebRunHooks::fake(),
        };

        let out = run_in_with_hooks(
            &simctl_lists_fake_simulator(),
            BuildFlags::default(),
            Some("FAKE".to_string()),
            true,
            true,
            false,
            false,
            hooks,
        )
        .unwrap();

        assert_eq!(out, 0);
        assert_eq!(relaunches.load(Ordering::SeqCst), 2, "start + one change");
        assert_eq!(
            teardowns.load(Ordering::SeqCst),
            2,
            "the replaced app and the last one are both torn down"
        );
    }

    /// Criterion 1: a simulator app without `Capability::HotPatch` (a
    /// restart-only session) is started once and never asked to patch: its
    /// precondition is printed once ([`launch_hot`]'s `restart required:
    /// hot-patch builder unsupported: ...` line) and the simulator relaunch
    /// loop takes over — the first change tears the adopted hot app down
    /// and reruns the simulator pipeline.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_simulator_app_without_hot_patch_prints_builder_unsupported_once_and_relaunch_loops() {
        let backend = FakeBackend::on_device(vec![Ok((
            Vec::new(),
            Some("the app does not advertise the HotPatch capability".to_string()),
        ))]);
        let calls = Arc::clone(&backend.calls);
        let started = Arc::clone(&backend.started);
        let teardowns = Arc::clone(backend.teardowns.as_ref().unwrap());
        let relauncher = FakeDeviceRelauncher::new();
        let relaunches = Arc::clone(&relauncher.relaunches);
        let hooks = RunHooks {
            watch: WatchHooks {
                install_ctrlc: Box::new(|_| Ok(())),
                spawn_watcher: Box::new(|_, _| unreachable!("the tick watcher is for --no-hot")),
                hot: Some(HotHooks {
                    backend: Box::new(|_, _| panic!("a device run builds no desktop backend")),
                    android: Box::new(|_, _, _| {
                        panic!("a simulator run builds no Android backend")
                    }),
                    ios_sim: Box::new(move |_, _, _| Ok(Box::new(backend) as Box<dyn HotBackend>)),
                    spawn_watcher: Box::new(|_, tx| {
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(150));
                            tx.send(PathBuf::from("/w/app/src/lib.rs")).unwrap();
                            std::thread::sleep(Duration::from_millis(400));
                        });
                        Ok(Box::new(()) as Box<dyn std::any::Any>)
                    }),
                }),
                device_relauncher: Box::new(move |_, _, _, _| {
                    Box::new(relauncher) as Box<dyn Relauncher>
                }),
            },
            web: WebRunHooks::fake(),
        };

        let out = run_in_with_hooks(
            &simctl_lists_fake_simulator(),
            BuildFlags::default(),
            Some("FAKE-UDID".to_string()),
            true,
            false,
            false,
            false,
            hooks,
        )
        .unwrap();

        assert_eq!(out, 0);
        assert_eq!(started.load(Ordering::SeqCst), 1, "one start, one print");
        assert!(calls.lock().unwrap().is_empty(), "never asked to patch");
        assert_eq!(relaunches.load(Ordering::SeqCst), 1);
        assert_eq!(teardowns.load(Ordering::SeqCst), 1, "the adopted hot app");

        // The line that start printed, through the same `launch_hot`.
        let mut backend = FakeBackend::on_device(vec![Ok((
            Vec::new(),
            Some("the app does not advertise the HotPatch capability".to_string()),
        ))]);
        let current = AppSlot::default();
        let mut lines = Vec::new();
        let launched = launch_hot(&mut backend, &current, &mut |l| lines.push(l.to_string()));
        assert!(matches!(launched, Launched::Cold));
        assert_eq!(
            lines,
            vec![
                "restart required: hot-patch builder unsupported: the app does not advertise \
                 the HotPatch capability; hot reload unavailable, relaunching on change instead"
            ]
        );
        if let Some(app) = lock_slot(&current).take() {
            app.stop();
        }
    }

    /// A hot backend shaped like [`IosSimHotBackend`] over a recording
    /// runner: each start launches the app (`simctl launch --console-pty`)
    /// and hands it back as a [`simulator_app`], its session answering the
    /// scripted outcomes.
    struct ScriptedSimBackend {
        runner: Arc<RecordingRunner>,
        starts: VecDeque<Vec<Outcome>>,
        started: usize,
        events: Arc<Mutex<Vec<String>>>,
    }

    impl HotBackend for ScriptedSimBackend {
        fn watch_set(&mut self) -> Result<WatchSet> {
            Ok(WatchSet::default())
        }

        fn start(
            &mut self,
            _on_line: &mut dyn FnMut(&str),
            _cancel: &AtomicBool,
        ) -> Result<HotLaunch, HotStartError> {
            self.started += 1;
            let outcomes = self.starts.pop_front().expect("scripted start");
            let launch = IosLaunch {
                stream: ios_run::simctl::spawn_launch(&*self.runner, "FAKE-UDID", FAKE_BUNDLE)
                    .unwrap(),
                bundle_id: FAKE_BUNDLE.to_string(),
            };
            let session = FakeSession {
                outcomes: outcomes.into(),
                calls: Arc::new(Mutex::new(Vec::new())),
                restart_only: None,
                start: self.started,
                events: Arc::clone(&self.events),
            };
            let runner: Arc<dyn ProcessRunner + Send + Sync> = self.runner.clone();
            Ok((
                Box::new(session),
                simulator_app(runner, "FAKE-UDID", launch),
            ))
        }

        fn cancellable(&self) -> bool {
            true
        }
    }

    /// Criterion 2: on a simulator a `RestartRequired` prints its reason
    /// verbatim, drops the session, terminates the app (`simctl
    /// terminate`) and starts a fresh hot session (the simulator pipeline
    /// again, which relaunches the app); the fresh app is terminated when
    /// the loop ends. Never the cold relaunch.
    #[test]
    fn a_simulator_restart_required_terminates_the_app_and_restarts_hot() {
        let runner = RecordingRunner::new(simulator_pipeline());
        let mut backend = ScriptedSimBackend {
            runner: Arc::clone(&runner),
            starts: VecDeque::from([vec![restart(RestartReason::NoSeamHit)], Vec::new()]),
            started: 0,
            events: Arc::new(Mutex::new(Vec::new())),
        };
        let mut relauncher = FakeDeviceRelauncher::new();
        let (tx, rx) = mpsc::channel();
        let sender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            tx.send(PathBuf::from("/w/app/src/home.rs")).unwrap();
            std::thread::sleep(Duration::from_millis(250));
        });
        let current = AppSlot::default();
        let mut lines = Vec::new();
        let out = hot_loop_with(
            &mut relauncher,
            &mut backend,
            rx,
            Duration::from_millis(10),
            &current,
            &mut |line| lines.push(line.to_string()),
        )
        .unwrap();
        sender.join().unwrap();

        assert_eq!(out, 0);
        assert!(
            lines.contains(&format!("restart required: {}", RestartReason::NoSeamHit)),
            "{lines:?}"
        );
        assert_eq!(backend.started, 2, "{lines:?}");
        assert_eq!(
            pipeline_steps(&runner.all.lock().unwrap()),
            vec!["launch", "terminate", "launch", "terminate"]
        );
        assert_eq!(
            *backend.events.lock().unwrap(),
            vec!["session 1 dropped", "session 2 dropped"]
        );
        assert_eq!(relauncher.relaunches.load(Ordering::SeqCst), 0);
    }

    /// Criterion 2 (`--no-hot`, and a restart-only session's fallback): the
    /// real simulator relauncher runs the simulator pipeline — `xcodebuild`
    /// for the booted simulator in the Debug configuration, `simctl
    /// install`, `simctl launch --console-pty` — and parks the app; the next
    /// relaunch `simctl terminate`s it first, then runs the pipeline again;
    /// stopping the last app terminates it too.
    #[test]
    fn the_simulator_relauncher_reruns_the_simulator_pipeline_and_terminates_the_previous_app() {
        let root = simulator_project("relaunch");
        let runner = RecordingRunner::new(simulator_pipeline());
        let mut relauncher = SimulatorRelauncher {
            runner: runner.clone(),
            root: root.clone(),
            device: fake_simulator(),
            info: debug_info(),
        };
        let current = AppSlot::default();
        let mut lines = Vec::new();
        for _ in 0..2 {
            let flow = relauncher
                .relaunch(&current, &mut |line| lines.push(line.to_string()))
                .unwrap();
            assert_eq!(flow, ControlFlow::Continue(()), "{lines:?}");
            assert!(lock_slot(&current).is_some(), "the app is parked");
            assert!(!current.starting.load(Ordering::SeqCst), "the start ended");
        }
        lock_slot(&current).take().unwrap().stop();

        let all = runner.all.lock().unwrap().clone();
        assert_eq!(
            pipeline_steps(&all),
            vec![
                "xcodebuild",
                "install",
                "launch",
                "terminate",
                "xcodebuild",
                "install",
                "launch",
                "terminate",
            ],
            "{all:?}"
        );
        let build = all
            .iter()
            .find(|call| call.starts_with("stream xcrun xcodebuild "))
            .unwrap();
        assert!(build.contains("-configuration Debug"), "{build}");
        assert!(build.contains("-destination id=FAKE-UDID"), "{build}");
        let install = all
            .iter()
            .find(|call| call.starts_with("xcrun simctl install "))
            .unwrap();
        assert!(
            install.ends_with("build/ios/Build/Products/Debug-iphonesimulator/Runner.app"),
            "{install}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("pipeline failed")),
            "{lines:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Criterion 1/2 (`--no-hot`): a Ctrl-C landing after the simulator
    /// pipeline's `simctl launch` (past its last cancel check, so the
    /// pipeline still hands the app back) ends the relaunch with the app
    /// terminated and nothing parked. One landing before the pipeline
    /// launched anything ends it with nothing to terminate, whether the
    /// pipeline answered the cancel or a failure; without a Ctrl-C a
    /// failure is reported for a retry and terminates nothing (the negative
    /// control).
    #[test]
    fn a_cancelled_simulator_relaunch_terminates_what_it_launched() {
        let root = simulator_project("cancel");
        let runner = RecordingRunner::new(simulator_pipeline());
        let current = AppSlot::default();
        *runner.ctrl_c_on_spawn.lock().unwrap() = Some(Arc::clone(&current));
        let mut relauncher = SimulatorRelauncher {
            runner: runner.clone(),
            root: root.clone(),
            device: fake_simulator(),
            info: debug_info(),
        };
        let mut lines = Vec::new();
        let flow = relauncher
            .relaunch(&current, &mut |line| lines.push(line.to_string()))
            .unwrap();
        assert_eq!(flow, ControlFlow::Break(()));
        assert!(lock_slot(&current).is_none(), "nothing parked");
        assert!(!current.starting.load(Ordering::SeqCst));
        assert_eq!(
            pipeline_steps(&runner.all.lock().unwrap()),
            vec!["xcodebuild", "install", "launch", "terminate"]
        );
        let _ = std::fs::remove_dir_all(&root);

        for (ctrl_c, launched) in [(true, Ok(None)), (true, Err(())), (false, Err(()))] {
            let failed = launched.is_err();
            let runner = RecordingRunner::new(simulator_pipeline());
            let current = AppSlot::default();
            let start = current.begin_start();
            if ctrl_c {
                assert!(!on_ctrlc(&current), "the start in flight is cancelled");
            }
            let launched =
                launched.map_err(|()| anyhow::anyhow!("`xcrun simctl install` failed: killed"));
            let mut lines = Vec::new();
            let flow = settle_simulator_launch(
                runner.clone(),
                "FAKE-UDID",
                launched,
                start,
                &mut |line| lines.push(line.to_string()),
            );
            let case = format!("ctrl_c {ctrl_c}, failed {failed}");
            assert!(runner.all.lock().unwrap().is_empty(), "{case}");
            assert_eq!(flow.is_break(), ctrl_c, "{case}");
            assert_eq!(
                lines
                    .iter()
                    .any(|l| l.contains("watching for a source change")),
                failed && !ctrl_c,
                "{case}: {lines:?}"
            );
            assert!(!current.starting.load(Ordering::SeqCst), "{case}: ended");
        }
    }

    /// `--watch -d <simulator>` accepts only a debug build without
    /// `--features`, refusing before anything runs and naming the simulator.
    #[test]
    fn simulator_watch_refuses_a_non_debug_build_and_a_features_passthrough() {
        let runner = FakeProcessRunner::new();
        for info in [profile_info(), release_info()] {
            let err = run_watch_on_device(
                &runner,
                &fake_simulator(),
                &info,
                NO_EXTRA,
                false,
                WatchHooks::fake(),
            )
            .unwrap_err()
            .to_string();
            assert!(
                err.starts_with("--watch on an iOS simulator needs a debug build"),
                "{err}"
            );
        }
        let err = run_watch_on_device(
            &runner,
            &fake_simulator(),
            &debug_info(),
            &["extra".to_string()],
            false,
            WatchHooks::fake(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("--features"), "{err}");
        assert!(err.contains("`ios_run::spawn_session`"), "{err}");
    }

    /// Criterion 1: a Ctrl-C during the simulator hot start, wired through
    /// the `install_ctrlc` hook, cancels the start instead of exiting; the
    /// start tears down what it launched (`start_ios_sim` kills the console
    /// stream and terminates the app) and the watch ends with 0 — no patch,
    /// no relaunch, no retry.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_ctrl_c_during_the_simulator_hot_start_tears_it_down_and_ends_the_watch() {
        let captured: Arc<Mutex<Option<AppSlot>>> = Arc::new(Mutex::new(None));
        let mut backend = FakeBackend::on_device(vec![Ok((Vec::new(), None))]);
        backend.interrupt = Some(Interrupt {
            at_start: 1,
            slot: Arc::clone(&captured),
            unwound_by_start: true,
        });
        let calls = Arc::clone(&backend.calls);
        let started = Arc::clone(&backend.started);
        let teardowns = Arc::clone(backend.teardowns.as_ref().unwrap());
        let relauncher = FakeDeviceRelauncher::new();
        let relaunches = Arc::clone(&relauncher.relaunches);
        let (keep_tx, keep_rx) = mpsc::channel::<mpsc::Sender<PathBuf>>();
        let hooks = RunHooks {
            watch: WatchHooks {
                install_ctrlc: Box::new(move |slot| {
                    *captured.lock().unwrap() = Some(slot);
                    Ok(())
                }),
                spawn_watcher: Box::new(|_, _| unreachable!("the tick watcher is for --no-hot")),
                hot: Some(HotHooks {
                    backend: Box::new(|_, _| panic!("a device run builds no desktop backend")),
                    android: Box::new(|_, _, _| {
                        panic!("a simulator run builds no Android backend")
                    }),
                    ios_sim: Box::new(move |_, _, _| Ok(Box::new(backend) as Box<dyn HotBackend>)),
                    spawn_watcher: Box::new(move |_, tx| {
                        keep_tx.send(tx).unwrap();
                        Ok(Box::new(()) as Box<dyn std::any::Any>)
                    }),
                }),
                device_relauncher: Box::new(move |_, _, _, _| {
                    Box::new(relauncher) as Box<dyn Relauncher>
                }),
            },
            web: WebRunHooks::fake(),
        };

        let out = run_in_with_hooks(
            &simctl_lists_fake_simulator(),
            BuildFlags::default(),
            Some("FAKE-UDID".to_string()),
            true,
            false,
            false,
            false,
            hooks,
        )
        .unwrap();
        drop(keep_rx);

        assert_eq!(out, 0);
        assert_eq!(started.load(Ordering::SeqCst), 1, "no retry");
        assert_eq!(teardowns.load(Ordering::SeqCst), 1, "the launched app");
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(relaunches.load(Ordering::SeqCst), 0);
    }

    /// A booted simulator that `devicectl` also lists as a connected iOS
    /// device under the same udid is one simulator to `--watch -d`:
    /// discovery keeps the simulator entry (`devices::discover_all`), so
    /// the udid resolves to it rather than being ambiguous.
    #[cfg(target_os = "macos")]
    #[test]
    fn watch_d_resolves_a_simulator_devicectl_also_lists_to_the_simulator() {
        const DEVICECTL_TWIN: &str = r#"{"result": {"devices": [{
            "identifier": "FAKE-UDID",
            "deviceProperties": {"name": "iPhone 15", "osVersionNumber": "17.5"},
            "connectionProperties": {"pairingState": "paired", "tunnelState": "connected"}
        }]}}"#;
        let runner = simctl_lists_fake_simulator().with_file(
            "xcrun devicectl list devices --json-output",
            ok(""),
            DEVICECTL_TWIN,
        );
        let device = resolve_watch_device(&runner, "FAKE-UDID", false).unwrap();
        assert_eq!(device, fake_simulator());
    }

    /// The relaunch loops' console sink sees a devtools discovery line
    /// with its token redacted; every other line passes through unchanged.
    #[test]
    fn drained_app_lines_reach_the_sink_with_the_discovery_token_redacted() {
        let runner = FakeProcessRunner::new().with_stream(
            "app",
            [
                "frust-devtools listening on 54321 token s3cret",
                "app: ready",
            ],
            true,
        );
        let mut current = Some(RunningApp::from(
            runner.spawn_streaming("app", &[], None, &[]).unwrap(),
        ));
        let mut lines = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !drain_available_lines(&mut current, &mut |line| lines.push(line.to_string())) {
            assert!(std::time::Instant::now() < deadline, "the stream ended");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            lines,
            vec![
                "frust-devtools listening on 54321 token <redacted>",
                "app: ready",
            ]
        );
    }

    /// A physical iOS device is refused before any watch hook runs: no
    /// relauncher, hot backend, watcher or Ctrl-C handler is built for it,
    /// with or without `--no-hot`.
    #[test]
    fn a_physical_ios_device_never_reaches_a_relauncher() {
        for no_hot in [false, true] {
            let hooks = WatchHooks {
                install_ctrlc: Box::new(|_| panic!("a refused device installs no handler")),
                spawn_watcher: Box::new(|_, _| panic!("a refused device is not watched")),
                hot: Some(HotHooks {
                    backend: Box::new(|_, _| panic!("a refused device builds no backend")),
                    android: Box::new(|_, _, _| panic!("a refused device builds no backend")),
                    ios_sim: Box::new(|_, _, _| panic!("a refused device builds no backend")),
                    spawn_watcher: Box::new(|_, _| panic!("a refused device is not watched")),
                }),
                device_relauncher: Box::new(|_, _, _, _| {
                    panic!("a physical iOS device never reaches a relauncher")
                }),
            };
            let err = run_watch_on_device(
                &FakeProcessRunner::new(),
                &ios_physical_device(),
                &debug_info(),
                NO_EXTRA,
                no_hot,
                hooks,
            )
            .unwrap_err();
            assert_eq!(err.to_string(), WATCH_DEVICE_REJECTION, "no_hot {no_hot}");
        }
    }

    /// A simulator app's teardown terminates the launched bundle on the
    /// resolved simulator, once, after its console stream is killed.
    #[test]
    fn a_simulator_app_teardown_terminates_the_bundle() {
        let runner = RecordingRunner::new(simulator_pipeline());
        let launch = IosLaunch {
            stream: ios_run::simctl::spawn_launch(&*runner, "FAKE-UDID", FAKE_BUNDLE).unwrap(),
            bundle_id: FAKE_BUNDLE.to_string(),
        };
        simulator_app(runner.clone(), "FAKE-UDID", launch).stop();
        assert_eq!(*runner.runs.lock().unwrap(), vec![SIM_TERMINATE]);
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
