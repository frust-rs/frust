//! Terminal lifecycle and the tokio event loop.
//!
//! Owns everything the pure engine and render layers deliberately don't: raw
//! mode + mouse capture, a panic hook that restores the terminal before the
//! default hook prints, the tokio runtime's `select!` over the crossterm
//! `EventStream` / the engine channel / a tick interval, and the dirty-frame
//! skip (only `terminal.draw` when the state changed or something is
//! animating).

use std::io::{self, IsTerminal, Stdout, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::EventStream;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton as CtMouseButton, MouseEventKind,
};
use frust_drive::android_build::{self, AndroidArtifact};
use frust_drive::devices::{Platform, default_discoverers, discover_all};
use frust_drive::doctor::{self, DoctorCtx, RealEnv};
use frust_drive::ios_build::{self, IosArtifact};
use frust_drive::process::{ProcessRunner, RealProcessRunner};
use frust_drive::scaffold::{self, TemplateContext};
use futures_util::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc::UnboundedSender;

use crate::engine::{
    ActiveModal, AddPluginDialog, AddPluginStep, AppState, BootstrapNode, BootstrapWizard,
    BuildFocus, BuildSpec, BuildTargetSpec, DevtoolsLaunch, DevtoolsState, DoctorCheck, Effect,
    Engine, Message, RegionId, RunFocus, Screen, WizardStep,
};
use crate::supervise::{
    DeviceTarget, DevtoolsBridge, SessionEvent, SessionEventKind, SessionId, SessionSpec,
    SessionState, Supervisor,
};
use crate::ui::mouse::MouseRegions;
use crate::ui::theme::Theme;

/// Frame tick cadence. Cheap because of the dirty-frame skip — a tick only
/// forces a draw while something is animating (the toast stack's own
/// auto-dismiss aging is the only thing that does today).
const TICK: Duration = Duration::from_millis(50);

/// Visible lines a `PageUp`/`PageDown` scrolls the log view (drawn rows, not
/// raw log indices — see `SessionView::visible_indices`). A fixed step (the
/// event translator has no viewport height); a comfortable page on typical
/// panes.
const PAGE_LINES: u64 = 10;

/// Pure check: the TUI requires interactive stdin AND stdout. Testable without
/// a TTY.
///
/// Today a no-controlling-terminal launch (e.g. CI, a background job) panics
/// via `ratatui::init()`'s `.expect`; a piped-stdout launch with stdin still a
/// TTY *succeeds* into raw mode and garbles the pipe with raw ANSI. Stdout
/// must be a TTY because that is where the TUI's frames are written;
/// requiring stdin as well is a deliberate conservative narrowing on top of
/// that, not a technical necessity — `crossterm` 0.29's `tty_fd()` falls back
/// to opening `/dev/tty` when stdin isn't a TTY, so `frust tui </dev/null`
/// used to work. This predicate has a twin, `frust-cli`'s `default_command`
/// (`crates/frust-cli/src/main.rs`) — the two must move in lockstep — and
/// means that previously-working stdin-redirected/stdout-TTY configuration
/// is now refused by design.
fn ensure_interactive_terminal(stdin_tty: bool, stdout_tty: bool) -> Result<()> {
    if stdin_tty && stdout_tty {
        Ok(())
    } else {
        anyhow::bail!(
            "the frust TUI needs an interactive terminal (stdin and stdout must be TTYs); \
             run `frust <command>` for non-interactive use, or `frust --help`"
        )
    }
}

/// Run the TUI: set up the terminal, run the loop, and restore on the way out
/// (including on panic, via the installed hook).
pub async fn run() -> Result<()> {
    ensure_interactive_terminal(io::stdin().is_terminal(), io::stdout().is_terminal())?;

    let mut terminal = ratatui::init();
    install_panic_hook();
    if let Err(e) = enable_mouse_capture() {
        // Non-fatal: the whole UI has keyboard parity, so a terminal that
        // rejects mouse capture still works.
        eprintln!("frust-tui: mouse capture unavailable: {e}");
    }

    let result = run_loop(&mut terminal).await;

    let _ = disable_mouse_capture();
    ratatui::restore();
    result
}

/// The main event loop.
async fn run_loop(terminal: &mut DefaultTerminal) -> Result<()> {
    let theme = Theme::frust_dark();
    let mut engine = Engine::new(AppState::new());
    let mut rx = engine.take_receiver();
    // The session supervisor and the channel every supervised session feeds.
    // Sessions are started elsewhere (`launch_sessions`, driven by the
    // run-config modal / device panel / palette); this loop
    // wires the channel and the kill/copy effect path every start call rides.
    // On return the supervisor's `Drop` stops+joins every session.
    let (mut supervisor, mut session_rx) = Supervisor::new(Arc::new(RealProcessRunner));
    // The DevTools bridges (workbook §B12): one connection thread per session
    // that has opened DevTools, reporting into the same engine channel. On
    // return its `Drop` stops and joins every thread (and removes any `adb`
    // forward they allocated).
    let mut devtools = DevtoolsBridge::new(Arc::new(RealProcessRunner));
    // A cloneable handle background tasks (device discovery, session
    // registration) post `Message`s back through.
    let msg_tx = engine.sender();
    let mut regions = MouseRegions::new();
    let mut reader = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    let mut needs_redraw = true;
    // Ids for ad-hoc (build/clean) sessions that never go through
    // `Supervisor::start`, minted downward from `u64::MAX` so they can never
    // collide with `Supervisor`'s own upward-counting ids for the life of one
    // run — see `apply_effect`'s `LaunchBuild`/`RunClean` enactment.
    let mut next_adhoc_id: u64 = u64::MAX;

    // Kick an initial device discovery + doctor preflight so the panel/chip
    // populate on open (the doctor run is the titlebar chip's cached
    // startup source, refreshed on demand via `d`/the chip/the panel re-run).
    let _ = msg_tx.send(Message::RefreshDevices);
    let _ = msg_tx.send(Message::RunDoctor);
    // The component-level report: seeds the titlebar toolchain-chip
    // rollup and the bootstrap wizard, and drives the fresh-machine auto-open
    // when the core toolchain is missing.
    let _ = msg_tx.send(Message::RunBootstrapReport);
    // Record whichever project came up active at startup (cwd-detected, or
    // the persisted most-recently-opened one — see `AppState::new`) as the
    // most-recently-opened, so opening the TUI itself counts as a "use" for
    // the recency ordering, not just an explicit switch.
    if let Some(root) = engine.state.project_root.clone() {
        crate::engine::record_recent_project(&root);
    }

    while !engine.state.should_quit {
        if needs_redraw {
            regions.begin_frame();
            terminal
                .draw(|frame| {
                    let mut ctx = crate::ui::mouse::MouseCtx::new(&mut regions);
                    crate::ui::render(frame, &engine.state, &theme, &mut ctx);
                })
                .context("drawing a frame")?;
            needs_redraw = false;
        }

        tokio::select! {
            maybe_event = reader.next() => {
                match maybe_event {
                    Some(Ok(event)) => {
                        for msg in translate_event(event, &engine.state, &regions) {
                            let out = engine.handle(msg);
                            needs_redraw |= out.redraw;
                            apply_effect(
                                out.effect,
                                &mut supervisor,
                                &mut devtools,
                                &msg_tx,
                                &mut next_adhoc_id,
                            );
                        }
                    }
                    // A read error (rare) is logged and ignored — the loop
                    // keeps running rather than tearing the terminal down.
                    Some(Err(e)) => eprintln!("frust-tui: terminal read error: {e}"),
                    None => break,
                }
            }
            Some(msg) = rx.recv() => {
                let out = engine.handle(msg);
                needs_redraw |= out.redraw;
                apply_effect(
                    out.effect,
                    &mut supervisor,
                    &mut devtools,
                    &msg_tx,
                    &mut next_adhoc_id,
                );
            }
            Some(ev) = session_rx.recv() => {
                let out = engine.handle(Message::Session(ev));
                needs_redraw |= out.redraw;
                apply_effect(
                    out.effect,
                    &mut supervisor,
                    &mut devtools,
                    &msg_tx,
                    &mut next_adhoc_id,
                );
            }
            _ = tick.tick() => {
                // Only touch the model while something is animating (a live
                // toast) — the dirty-frame skip keeps an idle workbench from
                // aging/redrawing anything. The Tick transition drops expired
                // toasts and reports whether the visible set changed.
                if engine.state.animating() {
                    let out = engine.handle(Message::Tick);
                    needs_redraw |= out.redraw;
                    apply_effect(
                    out.effect,
                    &mut supervisor,
                    &mut devtools,
                    &msg_tx,
                    &mut next_adhoc_id,
                );
                }
            }
        }
    }

    Ok(())
}

/// Enact an engine-requested [`Effect`] — the runner owns the side effects the
/// pure engine can't perform: killing a session through the supervisor, writing
/// the system clipboard, discovering devices off-thread, and launching
/// sessions.
fn apply_effect(
    effect: Option<Effect>,
    supervisor: &mut Supervisor,
    devtools: &mut DevtoolsBridge,
    tx: &UnboundedSender<Message>,
    next_adhoc_id: &mut u64,
) {
    match effect {
        Some(Effect::StopSession(id)) => supervisor.stop(id),
        Some(Effect::Copy(text)) => copy_to_clipboard(&text),
        Some(Effect::RefreshDevices) => spawn_device_discovery(tx.clone()),
        Some(Effect::LaunchSessions(specs)) => launch_sessions(specs, supervisor, tx),
        Some(Effect::RecordRecentProject(path)) => crate::engine::record_recent_project(&path),
        Some(Effect::ProbeCleanSignals) => {
            // Neither the create wizard's arch cards nor the Add Plugin
            // dialog's registry cards are sibling-gated any longer —
            // clean-signals moved to a git+rev pin (`docs/DEVELOPMENT.md`'s
            // Version-Pin Policy), so there is no `../clean-signals-rs`
            // checkout left to probe for. Reply immediately rather than
            // touching disk for a check nothing acts on.
            let _ = tx.send(Message::CleanSignalsProbed(true));
        }
        Some(Effect::ScaffoldProject {
            directory,
            project_name,
            arch,
        }) => scaffold_project(directory, project_name, arch, tx.clone()),
        Some(Effect::RunDoctor) => spawn_doctor_run(tx.clone()),
        Some(Effect::LaunchBuild(spec)) => {
            let id = next_adhoc_session_id(next_adhoc_id);
            launch_build_session(spec, id, tx.clone());
        }
        Some(Effect::RunClean(root)) => {
            let id = next_adhoc_session_id(next_adhoc_id);
            launch_clean_session(root, id, tx.clone());
        }
        Some(Effect::RunBootstrapReport) => spawn_bootstrap_report(tx.clone()),
        Some(Effect::RunBootstrapCommand {
            program,
            args,
            label,
        }) => {
            let id = next_adhoc_session_id(next_adhoc_id);
            launch_bootstrap_fix_session(program, args, label, id, tx.clone());
        }
        Some(Effect::AddPlugin {
            project_root,
            id,
            features,
        }) => spawn_add_plugin(project_root, id, features, tx.clone()),
        // The bridge only *spawns* here: the `adb forward`, the TCP connect,
        // the handshake and the frame-stats pump all run on its own thread,
        // so a slow or unreachable service never stalls this loop.
        Some(Effect::DevtoolsConnect(target)) => devtools.connect(target, tx.clone()),
        Some(Effect::DevtoolsDisconnect(session)) => devtools.disconnect(session),
        Some(Effect::SetMouseCapture(on)) => set_mouse_capture(on),
        Some(Effect::SaveSidebarWidth(width)) => crate::engine::save_sidebar_width(width),
        None => {}
    }
}

/// Enact a mouse-capture toggle: a failure (a terminal that rejects
/// the sequence) is logged and ignored — the whole UI has keyboard parity, so
/// capture is never load-bearing.
fn set_mouse_capture(on: bool) {
    let result = if on {
        enable_mouse_capture()
    } else {
        disable_mouse_capture()
    };
    if let Err(e) = result {
        eprintln!("frust-tui: toggling mouse capture failed: {e}");
    }
    // The terminal-level toggle and the persisted
    // preference always travel together (this effect only ever fires from
    // `Message::ToggleMouseCapture`) — best-effort, like every other
    // settings write.
    crate::engine::save_mouse_capture(on);
}

/// Mint an id for an ad-hoc (build/clean) session — see `run_loop`'s
/// `next_adhoc_id` doc comment for why this never collides with a
/// `Supervisor`-issued id.
fn next_adhoc_session_id(next_adhoc_id: &mut u64) -> SessionId {
    let id = SessionId(*next_adhoc_id);
    *next_adhoc_id = next_adhoc_id.saturating_sub(1);
    id
}

/// Run `frust-drive`'s validator set off the UI thread (blocking `rustc`/
/// `cargo-ndk`/`xcrun` invocations), posting the flattened results back as
/// [`Message::DoctorResults`] — the titlebar chip's live source.
fn spawn_doctor_run(tx: UnboundedSender<Message>) {
    tokio::task::spawn_blocking(move || {
        let env = RealEnv;
        let ctx = DoctorCtx {
            runner: &RealProcessRunner,
            env: &env,
            is_macos: cfg!(target_os = "macos"),
        };
        let validators = doctor::default_validators();
        let results = doctor::run_all(&ctx, &validators)
            .into_iter()
            .map(|(name, validation)| DoctorCheck {
                name,
                status: validation.status,
                messages: validation.messages,
            })
            .collect();
        let _ = tx.send(Message::DoctorResults(results));
    });
}

/// Run `frust-drive`'s component-level report off the UI thread (the same
/// blocking probes `spawn_doctor_run` runs, reshaped by `build_report` into the
/// grouped Prerequisites/Android/iOS/Desktop components + fix commands the
/// bootstrap wizard consumes), posting it back as [`Message::BootstrapReport`]
/// — the titlebar chip's rollup source.
fn spawn_bootstrap_report(tx: UnboundedSender<Message>) {
    tokio::task::spawn_blocking(move || {
        let env = RealEnv;
        let ctx = DoctorCtx {
            runner: &RealProcessRunner,
            env: &env,
            is_macos: cfg!(target_os = "macos"),
        };
        let report = doctor::build_report(&ctx);
        let _ = tx.send(Message::BootstrapReport(report));
    });
}

/// Run a bootstrap wizard's guided fix command off the UI thread as a
/// supervised session — one command, never chained (the wizard only ever emits
/// an `auto_runnable` fix). It streams through the same ad-hoc-session machinery
/// [`launch_clean_session`] uses (`RegisterSession` + a `run_streaming` line
/// sink into the session's log tab, never the raw-mode tty), and on exit
/// re-runs the preflight report ([`Message::RunBootstrapReport`]) so the chip
/// and wizard reflect the now-fixed component — the fresh-machine flow.
fn launch_bootstrap_fix_session(
    program: String,
    args: Vec<String>,
    label: String,
    id: SessionId,
    tx: UnboundedSender<Message>,
) {
    // Bootstrap fixes (`rustup target add …`, `cargo install cargo-ndk`) are
    // machine-global — no project root, so the session groups under a synthetic
    // "toolchain" root that keeps its tab distinct from any project's sessions.
    let root = PathBuf::from("toolchain");
    let _ = tx.send(Message::RegisterSession {
        id,
        project_root: root,
        target_label: format!("fix: {label}"),
        // A toolchain fix runs `rustup`/`cargo`, not the app — there is no
        // devtools service to reach.
        devtools: DevtoolsLaunch::unavailable(),
    });
    tokio::task::spawn_blocking(move || {
        let _ = tx.send(session_state(id, SessionState::Building));
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let result = {
            let mut on_line = |line: &str| {
                let _ = tx.send(session_line(id, line.to_string()));
            };
            RealProcessRunner
                .run_streaming(&program, &arg_refs, None, &[], &mut on_line)
                .with_context(|| format!("failed to run `{program}`"))
        };
        let terminal = match result {
            Ok(out) if out.success => SessionState::Exited(true),
            Ok(out) => {
                if !out.stderr.trim().is_empty() {
                    let _ = tx.send(session_line(id, out.stderr.trim().to_string()));
                }
                SessionState::Exited(false)
            }
            Err(err) => {
                let _ = tx.send(session_line(id, format!("error: {err:#}")));
                SessionState::Exited(false)
            }
        };
        let _ = tx.send(session_state(id, terminal));
        // Re-preflight so the chip/wizard pick up the fixed component.
        let _ = tx.send(Message::RunBootstrapReport);
    });
}

/// Drive a resolved build off the UI thread directly through `frust-drive`'s
/// `android_build`/`ios_build` pipelines (never shelling out itself),
/// reporting progress as a session reusing the tab/log-view machinery: a
/// `RegisterSession` gives it a home, each built artifact path arrives as a
/// `"Built: {path}"` line (`SessionView::built_artifact_paths` parses it back
/// out), and the pipeline's `Result` becomes the terminal `Exited` state.
fn launch_build_session(spec: BuildSpec, id: SessionId, tx: UnboundedSender<Message>) {
    let _ = tx.send(Message::RegisterSession {
        id,
        project_root: spec.project_root.clone(),
        target_label: format!("build {}", build_target_label(&spec.target)),
        // A build session produces an artifact; nothing is running to inspect.
        devtools: DevtoolsLaunch::unavailable(),
    });
    tokio::task::spawn_blocking(move || {
        let _ = tx.send(session_state(id, SessionState::Building));
        // The drive build cores are print-free: their streamed gradle/
        // xcodebuild output arrives through this `on_line` sink (routed into
        // the build session's log tab) instead of `println!`ing to the raw-mode
        // TUI terminal, which would garble the whole screen (the tty-garbling
        // fix). Scoped so the `&tx` borrow ends before `tx` is reused below.
        let result = {
            let mut on_line = |line: &str| {
                let _ = tx.send(session_line(id, line.to_string()));
            };
            run_build(&RealProcessRunner, &spec, &mut on_line)
        };
        let terminal = match result {
            Ok(paths) => {
                for path in paths {
                    let _ = tx.send(session_line(id, format!("Built: {}", path.display())));
                }
                SessionState::Exited(true)
            }
            Err(err) => {
                let _ = tx.send(session_line(id, format!("error: {err:#}")));
                SessionState::Exited(false)
            }
        };
        let _ = tx.send(session_state(id, terminal));
    });
}

/// The blocking build call, dispatching on the resolved [`BuildTargetSpec`]
/// into `android_build::build`/`ios_build::build`. `on_line` is the print-free
/// drive cores' line sink — the caller routes it into the build session's log
/// tab (never the raw-mode tty; the tty-garbling fix).
fn run_build(
    runner: &dyn ProcessRunner,
    spec: &BuildSpec,
    on_line: &mut dyn FnMut(&str),
) -> Result<Vec<PathBuf>> {
    match &spec.target {
        BuildTargetSpec::Apk {
            split_per_abi,
            abis,
        } => {
            let target = AndroidArtifact::Apk {
                split_per_abi: *split_per_abi,
                abis: abis.clone(),
            };
            android_build::build(runner, &spec.project_root, &spec.info, &target, on_line)
                .map(|artifacts| artifacts.paths)
        }
        BuildTargetSpec::Appbundle => android_build::build(
            runner,
            &spec.project_root,
            &spec.info,
            &AndroidArtifact::Appbundle,
            on_line,
        )
        .map(|artifacts| artifacts.paths),
        BuildTargetSpec::IosApp {
            simulator,
            codesign,
        } => {
            let target = IosArtifact::App {
                simulator: *simulator,
                codesign: *codesign,
            };
            ios_build::build(runner, &spec.project_root, &spec.info, &target, on_line)
                .map(|artifacts| artifacts.paths)
        }
        BuildTargetSpec::Ipa { export_method } => {
            let target = IosArtifact::Ipa {
                export_method: export_method.clone(),
            };
            ios_build::build(runner, &spec.project_root, &spec.info, &target, on_line)
                .map(|artifacts| artifacts.paths)
        }
    }
}

/// The build-session tab label per artifact kind (`build apk`/`build ios`/…).
fn build_target_label(target: &BuildTargetSpec) -> &'static str {
    match target {
        BuildTargetSpec::Apk { .. } => "apk",
        BuildTargetSpec::Appbundle => "appbundle",
        BuildTargetSpec::IosApp { .. } => "ios",
        BuildTargetSpec::Ipa { .. } => "ipa",
    }
}

/// Build-output directories a clean session removes beyond `cargo clean`'s
/// own `target/` — the same set `frust-cli`'s `commands/clean.rs::REMOVED_DIRS`
/// removes; duplicated by value here since `clean` has no `frust-drive`
/// surface to call into (see `docs/ARCHITECTURE.md`'s Module Structure —
/// `clean` lives entirely in `frust-cli`, unlike `doctor`/`build`).
const CLEAN_REMOVED_DIRS: &[&str] = &["android/app/build", "android/.gradle", "build"];

/// Run `cargo clean` + remove the generated Android/iOS build directories for
/// `project_root` off the UI thread, reporting progress the same way
/// [`launch_build_session`] does.
fn launch_clean_session(project_root: PathBuf, id: SessionId, tx: UnboundedSender<Message>) {
    let _ = tx.send(Message::RegisterSession {
        id,
        project_root: project_root.clone(),
        target_label: "clean".to_string(),
        // A clean session removes build output; nothing is running to inspect.
        devtools: DevtoolsLaunch::unavailable(),
    });
    tokio::task::spawn_blocking(move || {
        let _ = tx.send(session_state(id, SessionState::Building));
        let terminal = match run_clean(&RealProcessRunner, &project_root, &tx, id) {
            Ok(()) => SessionState::Exited(true),
            Err(err) => {
                let _ = tx.send(session_line(id, format!("error: {err:#}")));
                SessionState::Exited(false)
            }
        };
        let _ = tx.send(session_state(id, terminal));
    });
}

/// The blocking clean call: `cargo clean` via the injected [`ProcessRunner`],
/// then remove [`CLEAN_REMOVED_DIRS`], reporting each step as a session line.
///
/// Runs `cargo clean` through [`ProcessRunner::run_streaming`] with `cwd` set
/// to `project_dir` — `run` has no `cwd` argument and would run against the
/// TUI process's own working directory instead of the (possibly different)
/// project the session was opened for. The streaming path's `on_line` sink is
/// forwarded straight into the session's log tab, a side benefit of the fix:
/// `cargo clean`'s own stdout now streams live instead of being discarded.
fn run_clean(
    runner: &dyn ProcessRunner,
    project_dir: &Path,
    tx: &UnboundedSender<Message>,
    id: SessionId,
) -> Result<()> {
    if !project_dir.join("frust.toml").exists() {
        let _ = tx.send(session_line(
            id,
            format!(
                "no `frust.toml` found in `{}` — nothing to clean.",
                project_dir.display()
            ),
        ));
        return Ok(());
    }

    let out = {
        let mut on_line = |line: &str| {
            let _ = tx.send(session_line(id, line.to_string()));
        };
        runner
            .run_streaming("cargo", &["clean"], Some(project_dir), &[], &mut on_line)
            .context("failed to run `cargo clean`")?
    };
    if out.success {
        let _ = tx.send(session_line(
            id,
            "Removed cargo build artifacts (`cargo clean`).".to_string(),
        ));
    } else {
        let _ = tx.send(session_line(
            id,
            format!("`cargo clean` failed:\n{}", out.stderr.trim()),
        ));
    }

    for rel in CLEAN_REMOVED_DIRS {
        let path = project_dir.join(rel);
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                let _ = tx.send(session_line(id, format!("Removed `{}`.", path.display())));
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("removing `{}`", path.display())),
        }
    }

    Ok(())
}

/// One single-line [`SessionEventKind::Lines`] batch message for `id`.
fn session_line(id: SessionId, text: String) -> Message {
    Message::Session(SessionEvent {
        id,
        kind: SessionEventKind::Lines(vec![text]),
    })
}

/// One [`SessionEventKind::State`] message for `id`.
fn session_state(id: SessionId, state: SessionState) -> Message {
    Message::Session(SessionEvent {
        id,
        kind: SessionEventKind::State(state),
    })
}

/// Apply a registry plugin's contributions to `project_root` off the UI thread
/// via [`frust_drive::plugin::add_plugin`] (format-preserving file edits, so
/// cheap), posting [`Message::AddPluginSucceeded`] with the per-edit report, or
/// [`Message::AddPluginFailed`] with the typed error rendered — the Add Plugin
/// dialog's apply step.
fn spawn_add_plugin(
    project_root: PathBuf,
    id: String,
    features: Vec<String>,
    tx: UnboundedSender<Message>,
) {
    tokio::task::spawn_blocking(move || {
        let feature_refs: Vec<&str> = features.iter().map(String::as_str).collect();
        let msg = match frust_drive::plugin::add_plugin(&project_root, &id, &feature_refs) {
            Ok(report) => Message::AddPluginSucceeded(report),
            Err(e) => Message::AddPluginFailed(e.to_string()),
        };
        let _ = tx.send(msg);
    });
}

/// Scaffold a new project off the UI thread via
/// `frust_drive::scaffold::generate` (the template is embedded, so this is
/// cheap), posting [`Message::ScaffoldSucceeded`] with the new project's
/// absolute root, or [`Message::ScaffoldFailed`] with a rendered error chain.
fn scaffold_project(
    directory: String,
    project_name: String,
    arch: Option<String>,
    tx: UnboundedSender<Message>,
) {
    tokio::task::spawn_blocking(move || {
        let msg = match do_scaffold(&directory, &project_name, arch.as_deref()) {
            Ok(project_root) => Message::ScaffoldSucceeded { project_root },
            Err(e) => Message::ScaffoldFailed(format!("{e:#}")),
        };
        let _ = tx.send(msg);
    });
}

/// The blocking scaffold: resolve `directory` against the process cwd, render
/// the embedded template with a default org/description and the dev-time
/// `frust` path/version, and return the new project's absolute root.
fn do_scaffold(directory: &str, project_name: &str, arch: Option<&str>) -> Result<PathBuf> {
    let dest = resolve_dest(directory)?;
    let ctx = TemplateContext {
        title_case_name: scaffold::title_case(project_name),
        project_name: project_name.to_string(),
        // The wizard doesn't collect org/description yet — use the same
        // defaults the CLI's `frust create` applies.
        org: "com.example".to_string(),
        description: "A new Frust application.".to_string(),
        frust_version: env!("CARGO_PKG_VERSION").to_string(),
        frust_path: resolve_frust_path(),
        deeplink_scheme: None,
        deeplink_host: None,
    };
    scaffold::generate(&dest, &ctx, None, false, arch)
        .with_context(|| format!("scaffolding into `{}`", dest.display()))?;
    Ok(dest.canonicalize().unwrap_or(dest))
}

/// Resolve the wizard's directory string against the process cwd (an absolute
/// path is used as-is).
fn resolve_dest(directory: &str) -> Result<PathBuf> {
    let dir = Path::new(directory);
    if dir.is_absolute() {
        Ok(dir.to_path_buf())
    } else {
        let cwd = std::env::current_dir().context("reading current directory")?;
        Ok(cwd.join(dir))
    }
}

/// The dev-time path to the `frust` facade crate (`<repo>/crates/frust`),
/// mirroring `frust create`'s default (a temporary `frust_path`
/// mechanism until the crates are published).
fn resolve_frust_path() -> String {
    let raw = Path::new(env!("CARGO_MANIFEST_DIR")).join("../frust");
    raw.canonicalize()
        .unwrap_or(raw)
        .to_string_lossy()
        .into_owned()
}

/// Discover devices off the UI thread (the `frust-drive` discoverer set is
/// blocking — `adb`/`xcrun` invocations), posting the result back as
/// [`Message::DevicesLoaded`]. A dropped receiver (shutdown) just drops the
/// send.
fn spawn_device_discovery(tx: UnboundedSender<Message>) {
    tokio::task::spawn_blocking(move || {
        let discoverers = default_discoverers();
        let (devices, _notes) = discover_all(&RealProcessRunner, &discoverers);
        let _ = tx.send(Message::DevicesLoaded(devices));
    });
}

/// Launch one supervised session per spec (the run-config modal's checked
/// targets), registering each successfully-started session back into the model
/// so its events have a home. A spec that fails to start (e.g. a desktop
/// `cargo run` that can't spawn) is skipped — a device pipeline that fails
/// mid-build instead surfaces the error as a line in its own session tab.
fn launch_sessions(
    specs: Vec<SessionSpec>,
    supervisor: &mut Supervisor,
    tx: &UnboundedSender<Message>,
) {
    for spec in specs {
        match supervisor.start(&spec) {
            Ok(id) => {
                let _ = tx.send(Message::RegisterSession {
                    id,
                    project_root: spec.project_root.clone(),
                    target_label: target_label(&spec.target),
                    devtools: devtools_launch(&spec),
                });
            }
            Err(err) => eprintln!("frust-tui: failed to start session: {err:#}"),
        }
    }
}

/// What a launched app session's own config says about reaching its devtools
/// service (workbook §B12): the build mode decides whether the listener is
/// even compiled in, and an Android target additionally needs its `adb`
/// serial so the bridge can forward the device-loopback port to the host.
fn devtools_launch(spec: &SessionSpec) -> DevtoolsLaunch {
    let android_serial = match &spec.target {
        DeviceTarget::Device(device) if device.platform == Platform::Android => {
            Some(device.id.clone())
        }
        DeviceTarget::Device(_) | DeviceTarget::Desktop => None,
    };
    DevtoolsLaunch::from_launch(spec.build.mode, android_serial)
}

/// The short tab label for a launch target (`desktop`, or the device name).
fn target_label(target: &DeviceTarget) -> String {
    match target {
        DeviceTarget::Desktop => "desktop".to_string(),
        DeviceTarget::Device(device) => device.name.clone(),
    }
}

/// Copy `text` to the terminal's clipboard via an OSC 52 escape (broadly
/// supported, no clipboard-crate dependency). Best-effort: a terminal that
/// ignores OSC 52 simply drops it.
fn copy_to_clipboard(text: &str) {
    let payload = base64_encode(text.as_bytes());
    let seq = format!("\u{1b}]52;c;{payload}\u{07}");
    let mut out = stdout();
    let _ = out.write_all(seq.as_bytes());
    let _ = out.flush();
}

/// Minimal standard base64 (no dependency) for the OSC 52 clipboard payload.
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(b2 & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Translate one crossterm event into zero or more engine messages, using the
/// current frame's mouse regions for hit-testing.
fn translate_event(event: Event, state: &AppState, regions: &MouseRegions) -> Vec<Message> {
    match event {
        Event::Key(key) => {
            if key.kind == KeyEventKind::Release {
                return vec![];
            }
            translate_key(key.code, key.modifiers, state)
        }
        Event::Mouse(m) => {
            let (x, y) = (m.column, m.row);
            match m.kind {
                MouseEventKind::Moved => vec![Message::HoverChanged(regions.hover_at(x, y))],
                // Right-click opens a context menu for the row/pane under the
                // cursor; over empty space it closes an open menu.
                MouseEventKind::Down(CtMouseButton::Right) => match regions.context_at(x, y) {
                    Some(target) => vec![Message::OpenContextMenu { x, y, target }],
                    None if state.context_menu.is_some() => vec![Message::CloseContextMenu],
                    None => vec![],
                },
                MouseEventKind::Down(CtMouseButton::Left) => {
                    // A press on a drag region (splitter / scrollbar thumb)
                    // begins a drag and immediately applies the pressed
                    // position (a click on the scrollbar track jumps to it).
                    if let Some(kind) = regions.drag_at(x, y) {
                        vec![Message::DragStart(kind), Message::DragMove(x, y)]
                    } else if regions.hover_at(x, y) == Some(RegionId::CreateButton) {
                        vec![Message::CreatePressed]
                    } else {
                        vec![]
                    }
                }
                // A held-button move drives an in-progress drag.
                MouseEventKind::Drag(CtMouseButton::Left) => {
                    if state.active_drag.is_some() {
                        vec![Message::DragMove(x, y)]
                    } else {
                        vec![]
                    }
                }
                MouseEventKind::Up(CtMouseButton::Left) => {
                    if state.active_drag.is_some() {
                        vec![Message::DragEnd]
                    } else if state.create_pressed {
                        if regions.hover_at(x, y) == Some(RegionId::CreateButton) {
                            vec![Message::CreateActivate]
                        } else {
                            vec![Message::CreateCancel]
                        }
                    } else if let Some(msg) = regions.click_at(x, y) {
                        vec![msg]
                    } else if state.context_menu.is_some() {
                        // A left click outside the open menu dismisses it.
                        vec![Message::CloseContextMenu]
                    } else {
                        vec![]
                    }
                }
                MouseEventKind::ScrollDown => regions.scroll_at(x, y, true).into_iter().collect(),
                MouseEventKind::ScrollUp => regions.scroll_at(x, y, false).into_iter().collect(),
                _ => vec![],
            }
        }
        Event::Resize(w, h) => vec![Message::Resize(w, h)],
        _ => vec![],
    }
}

/// Translate one key press into engine messages, honoring the current mode
/// (search overlay open vs. normal) and screen (welcome vs. workbench).
///
/// Every log-view / tab action here has a mouse counterpart (tab click, wheel
/// scroll) — keyboard is the primary path, mouse additive (CODE_STANDARDS' TUI
/// keyboard-parity policy).
fn translate_key(code: KeyCode, mods: KeyModifiers, state: &AppState) -> Vec<Message> {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let shift = mods.contains(KeyModifiers::SHIFT);
    let alt = mods.contains(KeyModifiers::ALT);

    // Ctrl+Q always quits, even while typing a search or in a modal.
    if ctrl && matches!(code, KeyCode::Char('q')) {
        return vec![Message::Quit];
    }

    // Alt+m toggles mouse capture from anywhere — off hands the
    // terminal its native text selection back; keyboard operation stays whole.
    if alt && matches!(code, KeyCode::Char('m')) {
        return vec![Message::ToggleMouseCapture];
    }

    // An open context menu captures navigation keys — it sits on the top
    // z-layer above everything, so route to it before any modal/screen keys.
    if state.context_menu.is_some() {
        return match code {
            KeyCode::Esc => vec![Message::CloseContextMenu],
            KeyCode::Up => vec![Message::ContextMenuCursorUp],
            KeyCode::Down => vec![Message::ContextMenuCursorDown],
            KeyCode::Enter => vec![Message::ContextMenuActivate],
            _ => vec![],
        };
    }

    // While a modal is open it captures every other key. `active_modal` is
    // the single priority source — an exhaustive match here means a new
    // modal variant that isn't handled fails to compile rather than silently
    // falling through to the keys below.
    if let Some(modal) = state.active_modal() {
        return match modal {
            ActiveModal::Palette(palette) => translate_palette_key(code, mods, palette),
            ActiveModal::CreateWizard(wizard) => translate_wizard_key(code, mods, wizard.step),
            ActiveModal::Bootstrap(wizard) => translate_bootstrap_key(code, wizard),
            ActiveModal::AddPlugin(dialog) => translate_add_plugin_key(code, dialog),
            ActiveModal::RunConfig(modal) => translate_modal_key(code, mods, modal),
            ActiveModal::ProjectSwitcher => translate_switcher_key(code, state),
            // Modal exclusivity — see `crate::ui::render`'s
            // workbench-modal dispatch, which shares this same priority order.
            ActiveModal::DoctorPanel => translate_doctor_key(code),
            ActiveModal::BuildLauncher(launcher) => translate_build_key(code, mods, launcher),
            ActiveModal::CleanConfirm(_) => translate_clean_confirm_key(code),
            ActiveModal::HelpOverlay => translate_help_key(code),
        };
    }

    // While the search overlay is open, keys edit the query.
    if state.search.open {
        return match code {
            KeyCode::Esc => vec![Message::SearchCancel],
            KeyCode::Enter => vec![Message::SearchCommit],
            KeyCode::Backspace => vec![Message::SearchBackspace],
            KeyCode::Char(c) if !ctrl => vec![Message::SearchInput(c)],
            _ => vec![],
        };
    }

    // `?` opens the keyboard/help overlay from either top-level screen —
    // checked here, after the modal/search-capture blocks above (so it
    // never fires while typing `?` into a text field) and before every other
    // key below.
    if !ctrl && !alt && matches!(code, KeyCode::Char('?')) {
        return vec![Message::OpenHelpOverlay];
    }

    let has_active_session = state.active_session().is_some();
    let active_running = state
        .active_session()
        .is_some_and(|s| !s.state.is_terminal());

    // Ctrl+C stops the active running session, else falls through to quit.
    if ctrl && matches!(code, KeyCode::Char('c')) {
        return if active_running {
            vec![Message::StopSession]
        } else {
            vec![Message::Quit]
        };
    }

    // `Ctrl+P` opens the fuzzy command palette from either screen (mouse
    // parity: the titlebar ⌘ affordance / status-bar hint); `:` is the vim-ish
    // shorthand, handled in the match below.
    if ctrl && matches!(code, KeyCode::Char('p')) {
        return vec![Message::OpenPalette];
    }

    let workbench = matches!(state.screen, Screen::Workbench);
    // `Ctrl+O` opens the titlebar project switcher (mouse parity: the ▾
    // chevron). Only meaningful once a workbench is open — the welcome
    // screen has no project to switch away from.
    if ctrl && matches!(code, KeyCode::Char('o')) && workbench {
        return vec![Message::ToggleProjectSwitcher];
    }
    // DevTools mode owns the whole normal-mode key namespace for the active
    // session tab while it is open (workbook §B12's full-screen namespace
    // swap, the same shape the doctor panel's own key map takes over with):
    // the session-level keys below (`r`/`b`/`x`/`/`/`f`/`z`/`l`) are out of
    // scope inside it, so it defines its own small map from a clean slate.
    // The global chords above (`Ctrl+Q`, `⌥m`, `Ctrl+C`, `Ctrl+P`, `?`) and
    // session-tab switching (`Tab`/`Shift+Tab`) deliberately still apply.
    if let Some(devtools) = state.active_session().map(|s| &s.devtools)
        && devtools.open
    {
        return translate_devtools_key(code, state, devtools);
    }

    // Keyboard focus heuristic (this crate has no true focus system yet): with no
    // session open the devices panel owns the arrows/Space/Enter; once a
    // session is running the log view owns them (devices stay mouse- and
    // `r`-driven).
    let devices_focused = workbench && !has_active_session;

    match code {
        // Global quit.
        KeyCode::Char('q') => vec![Message::Quit],

        // `i` opens the toolchain bootstrap wizard from either screen (mouse
        // parity: the titlebar toolchain chip) — the fresh-machine flow.
        KeyCode::Char('i') => vec![Message::OpenBootstrapWizard],

        // `a` opens the Add Plugin dialog from either screen (mouse parity: the
        // sidebar "Add plugin" action / the palette). Gated on an open project
        // in `update` (a warn toast surfaces the reason on the welcome screen).
        KeyCode::Char('a') => vec![Message::OpenAddPlugin],

        // `:` opens the command palette from either screen (the `Ctrl+P`
        // shorthand above).
        KeyCode::Char(':') => vec![Message::OpenPalette],

        // Welcome keyboard parity: Enter / c activate the Create button.
        KeyCode::Char('c') if matches!(state.screen, Screen::Welcome) => activate_create(state),
        KeyCode::Enter if matches!(state.screen, Screen::Welcome) => activate_create(state),

        // ── Project switcher + devices panel + run-config (workbench) ──
        // `p` toggles the titlebar project switcher (mouse parity: the ▾
        // chevron / a sidebar project-row click carries `SwitchProject`
        // directly — see `translate_switcher_key` for the open-dropdown keys).
        KeyCode::Char('p') if workbench => vec![Message::ToggleProjectSwitcher],
        // `n` opens the create wizard from the workbench (mouse parity: the
        // sidebar ACTIONS "New project" row).
        KeyCode::Char('n') if workbench => vec![Message::OpenCreateWizard],
        // `r`/`Enter` open the run-config modal primed with the panel
        // selection; `R` re-runs discovery (mouse parity: the ⟳ affordance).
        KeyCode::Char('r') if workbench => vec![Message::OpenRunConfig],
        KeyCode::Char('R') if workbench => vec![Message::RefreshDevices],
        KeyCode::Enter if devices_focused => vec![Message::OpenRunConfig],
        KeyCode::Char(' ') if devices_focused => vec![Message::ToggleDeviceSelect],
        KeyCode::Up if devices_focused => vec![Message::DeviceCursorUp],
        KeyCode::Down if devices_focused => vec![Message::DeviceCursorDown],
        // `d` opens DevTools for the active session tab (workbook §B12) and,
        // with no session open, the doctor panel — the two contexts never
        // collide, and the doctor panel additionally stays on the sidebar
        // ACTIONS row and in the palette. `b` opens the build launcher
        // (mouse parity: the sidebar "Build" row).
        KeyCode::Char('d') if has_active_session => vec![Message::DevtoolsToggle],
        KeyCode::Char('d') if workbench => vec![Message::OpenDoctorPanel],
        KeyCode::Char('b') if workbench => vec![Message::OpenBuildLauncher],
        // `c` copies a build session's artifact path(s) when one is active
        // (mirroring the welcome screen's own `c` for Create, a different
        // screen/context); otherwise it opens the clean-confirm dialog (mouse
        // parity: the sidebar ACTIONS "Clean" row).
        KeyCode::Char('c') if has_active_session => vec![Message::CopyBuiltArtifacts],
        KeyCode::Char('c') if workbench => vec![Message::OpenCleanConfirm],

        // `s` toggles the narrow-terminal sidebar overlay (the
        // responsive breakpoint) — harmless above the narrow width, where
        // the sidebar already renders inline (see `views::workbench::render`).
        KeyCode::Char('s') if workbench => vec![Message::ToggleSidebarOverlay],
        // The overlay (when open) closes on `Esc` before the log-view's own
        // Esc arm below gets a chance (a closed overlay never intercepts it).
        KeyCode::Esc if state.sidebar_overlay_open => vec![Message::ToggleSidebarOverlay],

        // ── Log-view / tab controls (only meaningful with a session open) ──
        KeyCode::Char('x') if has_active_session => vec![Message::StopSession],
        // `t` toggles the active session's perf sparkline panel.
        KeyCode::Char('t') if has_active_session => vec![Message::TogglePerfPanel],
        KeyCode::Tab if has_active_session => vec![Message::NextTab],
        KeyCode::BackTab if has_active_session => vec![Message::PrevTab],
        KeyCode::Char(c @ '1'..='9') if has_active_session => {
            vec![Message::SelectTab(c as usize - '1' as usize)]
        }
        KeyCode::Char('/') if has_active_session => vec![Message::SearchOpen],
        KeyCode::Char('f') if has_active_session => vec![Message::ToggleFollow],
        KeyCode::Char('w') if has_active_session => vec![Message::ToggleWrap],
        // `l`/`L` cycle the log status bar's level-filter chip forward/back
        // (workbook §B11 proposed `f`/`Shift+f`, but `f` is already
        // `ToggleFollow` — `l`/`L` is the free key chosen instead); `z`
        // toggles the nearest panic/backtrace block's fold state (mouse
        // parity: a click on its `▶ n frames…` row).
        KeyCode::Char('l') if has_active_session => vec![Message::CycleLevelFilter(1)],
        KeyCode::Char('L') if has_active_session => vec![Message::CycleLevelFilter(-1)],
        KeyCode::Char('z') if has_active_session => vec![Message::ToggleNearestFold],
        KeyCode::Char('v') if has_active_session => vec![Message::SelectionBegin],
        KeyCode::Char('y') if has_active_session => vec![Message::CopySelection],
        KeyCode::Up if has_active_session && shift => vec![Message::SelectionExtendUp(1)],
        KeyCode::Down if has_active_session && shift => vec![Message::SelectionExtendDown(1)],
        KeyCode::Up if has_active_session => vec![Message::LogScrollUp(1)],
        KeyCode::Down if has_active_session => vec![Message::LogScrollDown(1)],
        KeyCode::PageUp if has_active_session => vec![Message::LogScrollUp(PAGE_LINES)],
        KeyCode::PageDown if has_active_session => vec![Message::LogScrollDown(PAGE_LINES)],
        KeyCode::Home if has_active_session => vec![Message::LogScrollToTop],
        KeyCode::End if has_active_session => vec![Message::LogScrollToBottom],
        KeyCode::Esc
            if state
                .active_session()
                .is_some_and(|s| s.selection.is_some()) =>
        {
            vec![Message::SelectionClear]
        }
        _ => vec![],
    }
}

/// Translate one key press while the active session tab is showing DevTools
/// (workbook §B12's own key table, binding): `d`/`Esc` return to the log,
/// `1`–`4` jump to a tab and `[`/`]` cycle them (connected only — there is no
/// strip to move through otherwise), `r` retries a failed connection, and
/// `Tab`/`Shift+Tab` still switch session tabs (a tab keeps its own
/// log/DevTools state, so leaving and coming back lands right where you were).
/// Every row has a mouse equivalent: a tab pill, the Retry button, and the
/// status row's back affordance.
///
/// The [`DevtoolsPhase`] match is exhaustive: a new screen has to decide what
/// its keys do rather than silently inheriting another screen's.
fn translate_devtools_key(
    code: KeyCode,
    state: &AppState,
    devtools: &DevtoolsState,
) -> Vec<Message> {
    use crate::engine::DevtoolsPhase;

    match code {
        KeyCode::Char('q') => return vec![Message::Quit],
        KeyCode::Esc | KeyCode::Char('d') => return vec![Message::DevtoolsClose],
        KeyCode::Tab => return vec![Message::NextTab],
        KeyCode::BackTab => return vec![Message::PrevTab],
        _ => {}
    }
    match devtools.phase() {
        DevtoolsPhase::Connected => match code {
            KeyCode::Char(c @ '1'..='4') => {
                vec![Message::DevtoolsTab(c as usize - '1' as usize)]
            }
            KeyCode::Char(']') => vec![Message::DevtoolsTabCycle(1)],
            KeyCode::Char('[') => vec![Message::DevtoolsTabCycle(-1)],
            _ => vec![],
        },
        // `r` retries only where a retry is offered: a live session whose
        // connection failed. A session that has already ended has no service
        // left to reach, and the failed screen drops its Retry button to match.
        DevtoolsPhase::Failed => match code {
            KeyCode::Char('r')
                if state
                    .active_session()
                    .is_some_and(|s| !s.state.is_terminal()) =>
            {
                vec![Message::DevtoolsRetry]
            }
            _ => vec![],
        },
        // Passive screens: they resolve on their own (or, for a release
        // build, never) — nothing to drive from here.
        DevtoolsPhase::Discovering | DevtoolsPhase::Connecting | DevtoolsPhase::Unavailable => {
            vec![]
        }
    }
}

/// Translate one key press while the run-config modal is open. `Esc` cancels,
/// `Enter` launches, `Tab`/arrows move focus, `Space` toggles the focused
/// target, `←`/`→` cycle the build mode (only on the mode row), and printable
/// characters edit the focused text field (flavor/defines). Mouse parity: the
/// modal registers a click region per control.
fn translate_modal_key(
    code: KeyCode,
    mods: KeyModifiers,
    modal: &crate::engine::RunConfig,
) -> Vec<Message> {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let text_field = matches!(modal.focus, RunFocus::Flavor | RunFocus::Defines);
    match code {
        KeyCode::Esc => vec![Message::CloseRunConfig],
        KeyCode::Enter => vec![Message::RunConfigLaunch],
        KeyCode::Tab => vec![Message::RunConfigFocusNext],
        KeyCode::BackTab => vec![Message::RunConfigFocusPrev],
        KeyCode::Up => vec![Message::RunConfigFocusPrev],
        KeyCode::Down => vec![Message::RunConfigFocusNext],
        KeyCode::Left if modal.focus == RunFocus::Mode => vec![Message::RunConfigCycleMode(-1)],
        KeyCode::Right if modal.focus == RunFocus::Mode => vec![Message::RunConfigCycleMode(1)],
        KeyCode::Backspace => vec![Message::RunConfigBackspace],
        // A space toggles the focused target, unless a text field is focused
        // (where it's a literal character — `defines` is space-separated).
        KeyCode::Char(' ') if !text_field => vec![Message::RunConfigToggleTarget],
        KeyCode::Char(c) if text_field && !ctrl => vec![Message::RunConfigInput(c)],
        _ => vec![],
    }
}

/// Translate one key press while the create wizard is open, honoring the
/// current step: the text steps (name/directory) edit their field, the arch
/// step moves the card highlight, and `Enter`/`Esc` advance/step-back
/// everywhere. Mouse parity: the wizard registers Next/Back/Cancel buttons and
/// a click region per arch card.
fn translate_wizard_key(code: KeyCode, mods: KeyModifiers, step: WizardStep) -> Vec<Message> {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    match step {
        WizardStep::Name | WizardStep::Directory => match code {
            KeyCode::Esc => vec![Message::CreateWizardBack],
            KeyCode::Enter | KeyCode::Tab => vec![Message::CreateWizardAdvance],
            KeyCode::Backspace => vec![Message::CreateWizardBackspace],
            KeyCode::Char(c) if !ctrl => vec![Message::CreateWizardInput(c)],
            _ => vec![],
        },
        WizardStep::Arch => match code {
            KeyCode::Esc => vec![Message::CreateWizardBack],
            KeyCode::Enter => vec![Message::CreateWizardAdvance],
            KeyCode::Left | KeyCode::Up => vec![Message::CreateWizardArchMove(-1)],
            KeyCode::Right | KeyCode::Down => vec![Message::CreateWizardArchMove(1)],
            _ => vec![],
        },
        WizardStep::Error => match code {
            KeyCode::Esc => vec![Message::CreateWizardBack],
            KeyCode::Enter => vec![Message::CreateWizardAdvance],
            _ => vec![],
        },
        // The off-thread scaffold is running — only Esc (a no-op back) is live.
        WizardStep::Scaffolding => match code {
            KeyCode::Esc => vec![Message::CreateWizardBack],
            _ => vec![],
        },
    }
}

/// Translate one key press while the titlebar project switcher is open.
/// `Esc` closes it, `↑`/`↓` move the highlighted row, `Enter` switches to the
/// highlighted row, and digits `1`-`9` jump straight to a project by index
/// (mouse parity: a dropdown item / sidebar project-row click).
fn translate_switcher_key(code: KeyCode, state: &AppState) -> Vec<Message> {
    match code {
        KeyCode::Esc => vec![Message::CloseProjectSwitcher],
        KeyCode::Up => vec![Message::ProjectSwitcherCursorUp],
        KeyCode::Down => vec![Message::ProjectSwitcherCursorDown],
        KeyCode::Enter => vec![Message::SwitchProject(state.project_switcher_cursor)],
        KeyCode::Char(c @ '1'..='9') => vec![Message::SwitchProject(c as usize - '1' as usize)],
        _ => vec![],
    }
}

/// Translate one key press while the doctor panel is open. `Esc` closes it,
/// `r` re-runs the validator set (mouse parity: the panel's Re-run button /
/// the titlebar chip).
fn translate_doctor_key(code: KeyCode) -> Vec<Message> {
    match code {
        KeyCode::Esc => vec![Message::CloseDoctorPanel],
        KeyCode::Char('r') => vec![Message::RunDoctor],
        _ => vec![],
    }
}

/// Translate one key press while the bootstrap wizard is open, honoring the
/// selected step. `Esc` closes it, `↑`/`↓` move the step-tree cursor,
/// `Tab`/`Shift+Tab` move the detail-pane fix cursor, `Space`/`Enter` toggle a
/// selected `Platforms` header (else `Enter` runs the selected fix), and `r`/`c`
/// run/copy the selected fix. Mouse parity: the wizard registers a click region
/// per step row, per fix row, and for the Run/copy/close affordances.
fn translate_bootstrap_key(code: KeyCode, wizard: &BootstrapWizard) -> Vec<Message> {
    let on_header = matches!(wizard.current_node(), BootstrapNode::PlatformsHeader);
    match code {
        KeyCode::Esc => vec![Message::CloseBootstrapWizard],
        KeyCode::Up => vec![Message::BootstrapNavUp],
        KeyCode::Down => vec![Message::BootstrapNavDown],
        KeyCode::Tab => vec![Message::BootstrapFixDown],
        KeyCode::BackTab => vec![Message::BootstrapFixUp],
        KeyCode::Char(' ') => vec![Message::BootstrapToggleExpand],
        // Enter toggles a header, otherwise runs the selected fix.
        KeyCode::Enter if on_header => vec![Message::BootstrapToggleExpand],
        KeyCode::Enter => vec![Message::BootstrapRunFix],
        KeyCode::Char('r') => vec![Message::BootstrapRunFix],
        KeyCode::Char('c') => vec![Message::BootstrapCopyFix],
        _ => vec![],
    }
}

/// Translate one key press while the Add Plugin dialog is open, honoring the
/// current step. `Esc` steps back / closes
/// everywhere; the select step moves the card highlight and `Enter` chooses;
/// the options step moves the feature cursor, `Space` toggles the focused
/// feature, and `Enter` applies; the report/error steps advance on `Enter`.
/// During the off-thread apply only `Esc` (a no-op back) is live — the create
/// wizard's `Scaffolding` precedent. Mouse parity: the view registers a click
/// region per card, per feature row, and for the Back/Apply/Cancel/close
/// affordances.
fn translate_add_plugin_key(code: KeyCode, dialog: &AddPluginDialog) -> Vec<Message> {
    match dialog.step {
        AddPluginStep::Select => match code {
            KeyCode::Esc => vec![Message::AddPluginBack],
            KeyCode::Enter => vec![Message::AddPluginAdvance],
            KeyCode::Up | KeyCode::Left => vec![Message::AddPluginSelectMove(-1)],
            KeyCode::Down | KeyCode::Right => vec![Message::AddPluginSelectMove(1)],
            _ => vec![],
        },
        AddPluginStep::Options => match code {
            KeyCode::Esc => vec![Message::AddPluginBack],
            KeyCode::Enter => vec![Message::AddPluginAdvance],
            KeyCode::Char(' ') => vec![Message::AddPluginToggleFeature],
            KeyCode::Up => vec![Message::AddPluginFeatureMove(-1)],
            KeyCode::Down => vec![Message::AddPluginFeatureMove(1)],
            _ => vec![],
        },
        AddPluginStep::Report | AddPluginStep::Error => match code {
            KeyCode::Esc => vec![Message::AddPluginBack],
            KeyCode::Enter => vec![Message::AddPluginAdvance],
            _ => vec![],
        },
        // The off-thread apply is running — only Esc (a no-op back) is live.
        AddPluginStep::Applying => match code {
            KeyCode::Esc => vec![Message::AddPluginBack],
            _ => vec![],
        },
    }
}

/// Translate one key press while the build-launcher modal is open — the same
/// shape as [`translate_modal_key`] (run-config), widened with the artifact
/// kind row (`←`/`→` cycles it) and the kind-conditional toggle rows
/// (`Space`).
fn translate_build_key(
    code: KeyCode,
    mods: KeyModifiers,
    launcher: &crate::engine::BuildLauncher,
) -> Vec<Message> {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let text_field = matches!(
        launcher.focus,
        BuildFocus::Flavor | BuildFocus::Defines | BuildFocus::ExportMethod
    );
    let toggle_row = matches!(
        launcher.focus,
        BuildFocus::SplitPerAbi | BuildFocus::Simulator | BuildFocus::NoCodesign
    );
    match code {
        KeyCode::Esc => vec![Message::CloseBuildLauncher],
        KeyCode::Enter => vec![Message::BuildLaunch],
        KeyCode::Tab => vec![Message::BuildFocusNext],
        KeyCode::BackTab => vec![Message::BuildFocusPrev],
        KeyCode::Up => vec![Message::BuildFocusPrev],
        KeyCode::Down => vec![Message::BuildFocusNext],
        KeyCode::Left if launcher.focus == BuildFocus::Kind => vec![Message::BuildCycleKind(-1)],
        KeyCode::Right if launcher.focus == BuildFocus::Kind => vec![Message::BuildCycleKind(1)],
        KeyCode::Left if launcher.focus == BuildFocus::Mode => vec![Message::BuildCycleMode(-1)],
        KeyCode::Right if launcher.focus == BuildFocus::Mode => vec![Message::BuildCycleMode(1)],
        KeyCode::Backspace => vec![Message::BuildBackspace],
        KeyCode::Char(' ') if toggle_row => vec![match launcher.focus {
            BuildFocus::SplitPerAbi => Message::BuildToggleSplitPerAbi,
            BuildFocus::Simulator => Message::BuildToggleSimulator,
            BuildFocus::NoCodesign => Message::BuildToggleNoCodesign,
            _ => unreachable!("guarded by `toggle_row`"),
        }],
        KeyCode::Char(c) if text_field && !ctrl => vec![Message::BuildInput(c)],
        _ => vec![],
    }
}

/// Translate one key press while the clean-confirm dialog is open. `Esc`
/// cancels, `Enter`/`y` confirms (mouse parity: the dialog's Clean/Cancel
/// buttons).
fn translate_clean_confirm_key(code: KeyCode) -> Vec<Message> {
    match code {
        KeyCode::Esc => vec![Message::CloseCleanConfirm],
        KeyCode::Enter | KeyCode::Char('y') => vec![Message::ConfirmClean],
        _ => vec![],
    }
}

/// Translate one key press while the keyboard/help overlay is open —
/// read-only reference content, so `Esc` or `?` again are its only
/// bindings.
fn translate_help_key(code: KeyCode) -> Vec<Message> {
    match code {
        KeyCode::Esc | KeyCode::Char('?') => vec![Message::CloseHelpOverlay],
        _ => vec![],
    }
}

/// Translate one key press while the command palette is open. `Esc` (or
/// `Ctrl+P`) closes it, `Enter` executes the selection, `↑`/`↓` move it,
/// `Backspace` edits the query, and printable characters extend the fuzzy
/// query. Mouse parity: each ranked row is a click target executing it.
fn translate_palette_key(
    code: KeyCode,
    mods: KeyModifiers,
    _palette: &crate::engine::Palette,
) -> Vec<Message> {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    if ctrl && matches!(code, KeyCode::Char('p')) {
        return vec![Message::ClosePalette];
    }
    match code {
        KeyCode::Esc => vec![Message::ClosePalette],
        KeyCode::Enter => vec![Message::PaletteExecute],
        KeyCode::Up => vec![Message::PaletteCursorUp],
        KeyCode::Down => vec![Message::PaletteCursorDown],
        KeyCode::Backspace => vec![Message::PaletteBackspace],
        KeyCode::Char(c) if !ctrl => vec![Message::PaletteInput(c)],
        _ => vec![],
    }
}

/// The Create action, emitted only from the welcome screen (keyboard parity
/// with the button click).
fn activate_create(state: &AppState) -> Vec<Message> {
    if matches!(state.screen, crate::engine::Screen::Welcome) {
        vec![Message::CreateActivate]
    } else {
        vec![]
    }
}

/// Install a panic hook that restores the terminal before the previous hook
/// runs. Hooks fire in install order here (we call the saved one last), so the
/// terminal is always usable by the time a backtrace prints.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_mouse_capture();
        ratatui::restore();
        previous(info);
    }));
}

fn stdout() -> Stdout {
    io::stdout()
}

fn enable_mouse_capture() -> io::Result<()> {
    crossterm::execute!(stdout(), EnableMouseCapture)
}

fn disable_mouse_capture() -> io::Result<()> {
    crossterm::execute!(stdout(), DisableMouseCapture)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, MouseEvent};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn ensure_interactive_terminal_requires_both_streams() {
        assert!(ensure_interactive_terminal(true, true).is_ok());
        assert!(ensure_interactive_terminal(true, false).is_err());
        assert!(ensure_interactive_terminal(false, true).is_err());
        assert!(ensure_interactive_terminal(false, false).is_err());
    }

    #[test]
    fn ensure_interactive_terminal_error_names_requirement_and_escape_hatch() {
        let err = ensure_interactive_terminal(false, false).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("interactive terminal"));
        assert!(message.contains("frust <command>"));
    }

    #[test]
    fn q_quits() {
        let state = AppState::default();
        let regions = MouseRegions::new();
        assert_eq!(
            translate_event(key(KeyCode::Char('q')), &state, &regions),
            vec![Message::Quit]
        );
    }

    #[test]
    fn ctrl_q_quits() {
        let state = AppState::default();
        let regions = MouseRegions::new();
        let ev = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert_eq!(translate_event(ev, &state, &regions), vec![Message::Quit]);
    }

    #[test]
    fn enter_and_c_activate_create_on_welcome() {
        let state = AppState::default(); // welcome
        let regions = MouseRegions::new();
        assert_eq!(
            translate_event(key(KeyCode::Enter), &state, &regions),
            vec![Message::CreateActivate]
        );
        assert_eq!(
            translate_event(key(KeyCode::Char('c')), &state, &regions),
            vec![Message::CreateActivate]
        );
    }

    #[test]
    fn hover_move_reports_region_under_cursor() {
        let state = AppState::default();
        let mut regions = MouseRegions::new();
        {
            let mut ctx = crate::ui::mouse::MouseCtx::new(&mut regions);
            ctx.button(
                ratatui::layout::Rect::new(0, 0, 10, 3),
                RegionId::CreateButton,
            );
        }
        let ev = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 2,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            translate_event(ev, &state, &regions),
            vec![Message::HoverChanged(Some(RegionId::CreateButton))]
        );
    }

    #[test]
    fn press_then_release_inside_activates() {
        let mut state = AppState::default();
        let mut regions = MouseRegions::new();
        {
            let mut ctx = crate::ui::mouse::MouseCtx::new(&mut regions);
            ctx.button(
                ratatui::layout::Rect::new(0, 0, 10, 3),
                RegionId::CreateButton,
            );
        }
        let down = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(CtMouseButton::Left),
            column: 2,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            translate_event(down, &state, &regions),
            vec![Message::CreatePressed]
        );
        state.create_pressed = true;
        let up = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Up(CtMouseButton::Left),
            column: 2,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            translate_event(up, &state, &regions),
            vec![Message::CreateActivate]
        );
    }

    #[test]
    fn alt_m_toggles_mouse_capture_anywhere() {
        let state = AppState::default();
        let regions = MouseRegions::new();
        let ev = Event::Key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::ALT));
        assert_eq!(
            translate_event(ev, &state, &regions),
            vec![Message::ToggleMouseCapture]
        );
    }

    #[test]
    fn right_click_over_a_context_region_opens_the_menu() {
        let state = AppState::default();
        let mut regions = MouseRegions::new();
        {
            let mut ctx = crate::ui::mouse::MouseCtx::new(&mut regions);
            ctx.context(
                ratatui::layout::Rect::new(0, 0, 10, 1),
                crate::engine::ContextTarget::SessionTab(3),
            );
        }
        let ev = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(CtMouseButton::Right),
            column: 2,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            translate_event(ev, &state, &regions),
            vec![Message::OpenContextMenu {
                x: 2,
                y: 0,
                target: crate::engine::ContextTarget::SessionTab(3),
            }]
        );
    }

    #[test]
    fn left_press_on_a_drag_region_starts_and_seeds_the_drag() {
        let state = AppState::default();
        let mut regions = MouseRegions::new();
        {
            let mut ctx = crate::ui::mouse::MouseCtx::new(&mut regions);
            ctx.drag(
                ratatui::layout::Rect::new(25, 3, 1, 20),
                crate::engine::DragKind::SidebarSplitter { body_left: 0 },
            );
        }
        let ev = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(CtMouseButton::Left),
            column: 25,
            row: 10,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            translate_event(ev, &state, &regions),
            vec![
                Message::DragStart(crate::engine::DragKind::SidebarSplitter { body_left: 0 }),
                Message::DragMove(25, 10),
            ]
        );
    }

    #[test]
    fn held_drag_move_routes_while_active() {
        let state = AppState {
            active_drag: Some(crate::engine::DragKind::SidebarSplitter { body_left: 0 }),
            ..Default::default()
        };
        let regions = MouseRegions::new();
        let ev = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(CtMouseButton::Left),
            column: 30,
            row: 10,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            translate_event(ev, &state, &regions),
            vec![Message::DragMove(30, 10)]
        );
        // A release ends the drag.
        let up = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Up(CtMouseButton::Left),
            column: 30,
            row: 10,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            translate_event(up, &state, &regions),
            vec![Message::DragEnd]
        );
    }

    #[test]
    fn context_menu_open_routes_nav_keys_and_left_click_outside_closes() {
        use crate::engine::{ContextMenu, ContextTarget, MenuEntry};
        let state = AppState {
            context_menu: Some(ContextMenu {
                x: 0,
                y: 0,
                target: ContextTarget::LogView,
                entries: vec![MenuEntry {
                    label: "Search logs…",
                    hint: "/",
                    message: Message::SearchOpen,
                    enabled: true,
                }],
                cursor: 0,
            }),
            ..Default::default()
        };
        let regions = MouseRegions::new(); // nothing registered (menu suppressed base)
        // Esc closes.
        assert_eq!(
            translate_event(key(KeyCode::Esc), &state, &regions),
            vec![Message::CloseContextMenu]
        );
        // Down navigates.
        assert_eq!(
            translate_event(key(KeyCode::Down), &state, &regions),
            vec![Message::ContextMenuCursorDown]
        );
        // A left click landing on no region closes the menu.
        let up = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Up(CtMouseButton::Left),
            column: 70,
            row: 20,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            translate_event(up, &state, &regions),
            vec![Message::CloseContextMenu]
        );
    }

    #[test]
    fn release_outside_pressed_cancels() {
        let state = AppState {
            create_pressed: true,
            ..Default::default()
        };
        let regions = MouseRegions::new(); // nothing under cursor
        let up = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Up(CtMouseButton::Left),
            column: 50,
            row: 50,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            translate_event(up, &state, &regions),
            vec![Message::CreateCancel]
        );
    }

    /// Regression for the clean-runs-in-the-wrong-directory bug: `run_clean`
    /// must route `cargo clean` through `project_dir`, not the TUI process's
    /// own `cwd` — asserted by scripting a [`FakeProcessRunner`] and checking
    /// its recorded `cwd` matches `project_dir` even though it differs from
    /// `std::env::current_dir()`.
    #[test]
    fn run_clean_runs_cargo_clean_in_the_project_dir_not_the_process_cwd() {
        use frust_drive::process::{FakeProcessRunner, Output};

        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let project_dir = std::env::temp_dir().join(format!(
            "frust-tui-run-clean-test-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&project_dir);
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(
            project_dir.join("frust.toml"),
            "[app]\nname = \"x\"\norg = \"y\"\n",
        )
        .unwrap();
        // Sanity: the fixture directory is not the process's own cwd — proves
        // a bare (cwd-agnostic) `run` couldn't have hit this directory.
        assert_ne!(project_dir, std::env::current_dir().unwrap());

        let runner = FakeProcessRunner::new().with(
            "cargo clean",
            Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            },
        );
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        run_clean(&runner, &project_dir, &tx, SessionId(1)).unwrap();

        assert_eq!(runner.recorded_cwd(), Some(project_dir.clone()));
        let _ = std::fs::remove_dir_all(&project_dir);
    }

    // ── Help overlay ──────────────────────────────────────────────────────────

    #[test]
    fn question_mark_opens_the_help_overlay_from_either_screen() {
        let regions = MouseRegions::new();
        assert_eq!(
            translate_event(key(KeyCode::Char('?')), &AppState::default(), &regions),
            vec![Message::OpenHelpOverlay],
            "welcome screen"
        );
        let workbench = AppState {
            screen: Screen::Workbench,
            ..Default::default()
        };
        assert_eq!(
            translate_event(key(KeyCode::Char('?')), &workbench, &regions),
            vec![Message::OpenHelpOverlay],
            "workbench"
        );
    }

    #[test]
    fn help_overlay_open_routes_esc_and_question_mark_to_close() {
        let state = AppState {
            help_open: true,
            ..Default::default()
        };
        let regions = MouseRegions::new();
        assert_eq!(
            translate_event(key(KeyCode::Esc), &state, &regions),
            vec![Message::CloseHelpOverlay]
        );
        assert_eq!(
            translate_event(key(KeyCode::Char('?')), &state, &regions),
            vec![Message::CloseHelpOverlay]
        );
        // Any other key is swallowed (read-only reference content).
        assert_eq!(
            translate_event(key(KeyCode::Char('z')), &state, &regions),
            Vec::<Message>::new()
        );
    }

    // ── Responsive breakpoints ────────────────────────────────────────────────

    #[test]
    fn s_toggles_the_sidebar_overlay_from_the_workbench_only() {
        let workbench = AppState {
            screen: Screen::Workbench,
            ..Default::default()
        };
        let regions = MouseRegions::new();
        assert_eq!(
            translate_event(key(KeyCode::Char('s')), &workbench, &regions),
            vec![Message::ToggleSidebarOverlay]
        );
        // No project/workbench open (welcome screen) — no-op.
        assert_eq!(
            translate_event(key(KeyCode::Char('s')), &AppState::default(), &regions),
            Vec::<Message>::new()
        );
    }

    #[test]
    fn esc_closes_an_open_sidebar_overlay_ahead_of_the_selection_clear_arm() {
        let state = AppState {
            screen: Screen::Workbench,
            sidebar_overlay_open: true,
            ..Default::default()
        };
        let regions = MouseRegions::new();
        assert_eq!(
            translate_event(key(KeyCode::Esc), &state, &regions),
            vec![Message::ToggleSidebarOverlay]
        );
    }

    // ── Perf sparkline panel ──────────────────────────────────────────────────

    #[test]
    fn t_toggles_the_perf_panel_only_with_an_active_session() {
        use crate::engine::SessionView;
        let regions = MouseRegions::new();
        let with_session = AppState {
            screen: Screen::Workbench,
            sessions: vec![SessionView::new(
                SessionId(0),
                PathBuf::from("/tmp/a"),
                "desktop",
            )],
            active_session: Some(0),
            ..Default::default()
        };
        assert_eq!(
            translate_event(key(KeyCode::Char('t')), &with_session, &regions),
            vec![Message::TogglePerfPanel]
        );
        let workbench = AppState {
            screen: Screen::Workbench,
            ..Default::default()
        };
        assert_eq!(
            translate_event(key(KeyCode::Char('t')), &workbench, &regions),
            Vec::<Message>::new(),
            "no active session — no-op"
        );
    }

    // ── DevTools mode key routing (workbook §B12) ─────────────────────────────

    /// A workbench with one running, devtools-capable desktop session that
    /// has already announced its service.
    fn devtools_state() -> AppState {
        use crate::engine::SessionView;
        let mut session = SessionView::with_devtools(
            SessionId(0),
            PathBuf::from("/tmp/huddle"),
            "desktop",
            DevtoolsLaunch::from_launch(frust_drive::build_info::BuildMode::Debug, None),
        );
        session.state = SessionState::Running;
        session.push_line_at(
            "frust-devtools listening on 53214 token cafe".to_string(),
            "12:00:00",
        );
        AppState {
            screen: Screen::Workbench,
            project_root: Some(PathBuf::from("/tmp/huddle")),
            projects: vec![PathBuf::from("/tmp/huddle")],
            sessions: vec![session],
            active_session: Some(0),
            ..Default::default()
        }
    }

    /// The §B12 entry/exit round trip driven purely by keys through the real
    /// translate → `update` path: `d` opens DevTools (connecting, because a
    /// discovery line already landed), `2` switches to the System tab once
    /// connected, and `Esc` returns to the log view.
    #[test]
    fn d_opens_devtools_digits_switch_tabs_and_esc_returns_to_the_log() {
        use crate::engine::{ConnEvent, DevtoolsPhase, DevtoolsTab, update};
        let regions = MouseRegions::new();
        let mut state = devtools_state();

        let msgs = translate_event(key(KeyCode::Char('d')), &state, &regions);
        assert_eq!(msgs, vec![Message::DevtoolsToggle]);
        let out = update(&mut state, msgs[0].clone());
        assert!(state.active_session().unwrap().devtools.open);
        assert!(
            matches!(out.effect, Some(Effect::DevtoolsConnect(_))),
            "opening connects against the already-announced service"
        );

        // While connecting, the tab keys have no strip to move through.
        assert_eq!(
            translate_event(key(KeyCode::Char('2')), &state, &regions),
            Vec::<Message>::new()
        );

        update(
            &mut state,
            Message::DevtoolsConn(
                SessionId(0),
                ConnEvent::Connected {
                    app_name: "huddle".to_string(),
                    caps: Vec::new(),
                },
            ),
        );
        assert_eq!(
            state.active_session().unwrap().devtools.phase(),
            DevtoolsPhase::Connected
        );

        let msgs = translate_event(key(KeyCode::Char('2')), &state, &regions);
        assert_eq!(msgs, vec![Message::DevtoolsTab(1)]);
        update(&mut state, msgs[0].clone());
        assert_eq!(
            state.active_session().unwrap().devtools.active_tab,
            DevtoolsTab::System
        );
        let msgs = translate_event(key(KeyCode::Char(']')), &state, &regions);
        assert_eq!(msgs, vec![Message::DevtoolsTabCycle(1)]);

        // The session-view key namespace is swapped while DevTools is open:
        // the log-view keys below it are out of scope, not silently reused.
        for swallowed in ['f', 'w', 'z', 'l', '/'] {
            assert_eq!(
                translate_event(key(KeyCode::Char(swallowed)), &state, &regions),
                Vec::<Message>::new(),
                "`{swallowed}` belongs to the log view, not DevTools"
            );
        }
        // Session-tab switching and the global chords still work.
        assert_eq!(
            translate_event(key(KeyCode::Tab), &state, &regions),
            vec![Message::NextTab]
        );
        assert_eq!(
            translate_event(key(KeyCode::Char('q')), &state, &regions),
            vec![Message::Quit]
        );

        let msgs = translate_event(key(KeyCode::Esc), &state, &regions);
        assert_eq!(msgs, vec![Message::DevtoolsClose]);
        update(&mut state, msgs[0].clone());
        assert!(!state.active_session().unwrap().devtools.open);
        // Back in the log view, `d` is the entry key again and `f` is the
        // log view's own follow toggle once more.
        assert_eq!(
            translate_event(key(KeyCode::Char('f')), &state, &regions),
            vec![Message::ToggleFollow]
        );
    }

    #[test]
    fn r_retries_only_on_the_failed_screen_of_a_live_session() {
        use crate::engine::{ConnEvent, update};
        let regions = MouseRegions::new();
        let mut state = devtools_state();
        update(&mut state, Message::DevtoolsToggle);
        update(
            &mut state,
            Message::DevtoolsConn(
                SessionId(0),
                ConnEvent::Failed("connection refused".to_string()),
            ),
        );
        assert_eq!(
            translate_event(key(KeyCode::Char('r')), &state, &regions),
            vec![Message::DevtoolsRetry]
        );

        // A session that has already ended offers no retry — there is no
        // service left to reach (the button is dropped from the screen too).
        state.sessions[0].state = SessionState::Exited(true);
        assert_eq!(
            translate_event(key(KeyCode::Char('r')), &state, &regions),
            Vec::<Message>::new()
        );
    }

    #[test]
    fn d_still_opens_the_doctor_panel_with_no_session_open() {
        let regions = MouseRegions::new();
        let workbench = AppState {
            screen: Screen::Workbench,
            ..Default::default()
        };
        assert_eq!(
            translate_event(key(KeyCode::Char('d')), &workbench, &regions),
            vec![Message::OpenDoctorPanel],
            "the doctor panel keeps `d` in the context DevTools cannot claim"
        );
    }
}
