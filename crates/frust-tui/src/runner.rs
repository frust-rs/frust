//! Terminal lifecycle and the tokio event loop.
//!
//! Owns everything the pure engine and render layers deliberately don't: raw
//! mode + mouse capture, a panic hook that restores the terminal before the
//! default hook prints, the tokio runtime's `select!` over the crossterm
//! `EventStream` / the engine channel / a tick interval, and the dirty-frame
//! skip (only `terminal.draw` when the state changed or something is
//! animating).

use std::io::{self, IsTerminal, Stdout};
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
use frust_drive::desktop_build;
use frust_drive::devices::{Platform, default_discoverers, discover_all};
use frust_drive::doctor::{self, DoctorCtx, RealEnv};
use frust_drive::ios_build::{self, IosArtifact};
use frust_drive::process::{ProcessRunner, RealProcessRunner};
use frust_drive::scaffold::{self, TemplateContext};
use frust_mcp::SharedBackend;
use futures_util::StreamExt;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc::UnboundedSender;

use crate::clipboard::{self, Backend as ClipboardBackend, ClipboardMode};
use crate::engine::{
    ActiveModal, AddPluginDialog, AddPluginStep, AppState, BootstrapNode, BootstrapWizard,
    BuildFocus, BuildSpec, BuildTargetSpec, DevtoolsLaunch, DevtoolsState, DoctorCheck, Effect,
    Engine, Message, Outcome, RegionId, RunFocus, Screen, ToastKind, WizardStep,
};
use crate::supervise::mcp_backend::MAX_ADHOC_SESSION_ID;
use crate::supervise::{
    DeviceTarget, DevtoolsBridge, McpServeCtx, McpSessionRecords, MetricsBridge,
    PendingWidgetTrees, SessionEvent, SessionEventKind, SessionId, SessionSpec, SessionState,
    SessionSubscribers, Supervisor, Teardown, TuiSessionBackend, mcp_session_state, serve_command,
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

/// The workbench's cargo-feature passthrough: none. `frust build`'s
/// `--features` is a CLI flag surface, and the build modal offers no
/// counterpart, so a workbench build compiles exactly the features its mode
/// selects (`BuildMode::cargo_features`) — named rather than written inline so
/// the empty slice at each pipeline call reads as a decision, not an omission.
const NO_EXTRA_FEATURES: &[String] = &[];

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
    // return its `Drop` stops every thread and waits them out against one
    // bounded deadline (see `spawn_teardown`), so a torn-down bridge removes
    // the `adb` forwards it allocated without ever hanging the exit.
    let mut devtools = DevtoolsBridge::new(Arc::new(RealProcessRunner));
    // The metrics-sampling bridges (workbook §B12's System/Network tabs):
    // one sampler thread per session whose Android identity has resolved
    // and whose DevTools has been opened, reporting into the same engine
    // channel. On return its `Drop` stops every thread the same bounded way.
    let mut metrics = MetricsBridge::new(Arc::new(RealProcessRunner));
    // A cloneable handle background tasks (device discovery, session
    // registration) post `Message`s back through.
    let msg_tx = engine.sender();
    let mut regions = MouseRegions::new();
    let mut reader = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    let mut needs_redraw = true;
    // Ids for ad-hoc (build/clean) sessions that never go through
    // `Supervisor::start`, minted downward from `MAX_ADHOC_SESSION_ID` so they
    // can never collide with `Supervisor`'s own upward-counting ids for the
    // life of one run — see `apply_effect`'s `LaunchBuild`/`RunClean`
    // enactment. `u64::MAX` itself is reserved (`UNRESOLVED_SESSION`), so an
    // MCP `run_app` that finds no workbench left to answer it can name an id
    // no session will ever hold.
    let mut next_adhoc_id: u64 = MAX_ADHOC_SESSION_ID;
    // What each MCP-describable session was launched from — the backing store
    // for the embedded server's snapshots and its `restart_app`. Empty (and
    // untouched) while no MCP server is running.
    let mut mcp_records = McpSessionRecords::new();
    // The embedded servers' two deferred-answer registries (see
    // `crate::supervise::session_feeds`): the open session-event feeds a DAP
    // client's output/exit pumps read, and the widget-tree pulls waiting on a
    // devtools-bridge report. Both are dropped when this loop returns, which
    // is what closes a DAP client's feeds instead of leaving them waiting on a
    // workbench that is gone.
    let mut subscribers = SessionSubscribers::new();
    let mut pending_trees = PendingWidgetTrees::new();
    // ONE backend, shared by both embedded servers. That sharing is the point:
    // a session an agent launched over MCP is the same session an editor sees
    // over DAP, and both are the tabs the user is looking at.
    let backend: SharedBackend = Arc::new(TuiSessionBackend::new(
        msg_tx.clone(),
        Arc::new(RealProcessRunner),
    ));
    // The clipboard backend (`crate::clipboard`): picked once here, since
    // stdout's TTY-ness is fixed for the process's life, and kept for the
    // whole run through `EffectCtx` rather than re-detected on every copy.
    // `FRUST_TUI_CLIPBOARD` (system|osc52|off; anything else/unset is auto)
    // overrides the environment-detected choice.
    let clipboard_mode = ClipboardMode::parse(std::env::var("FRUST_TUI_CLIPBOARD").ok().as_deref());
    let clipboard_backend = clipboard::detect(
        |name| std::env::var(name).ok(),
        io::stdout().is_terminal(),
        clipboard_mode,
    );
    if let ClipboardBackend::Disabled { reason } = clipboard_backend {
        let _ = msg_tx.send(Message::Notify {
            level: ToastKind::Warn,
            text: format!("Clipboard unavailable: {reason}"),
        });
    }

    // Kick an initial device discovery + doctor preflight so the panel/chip
    // populate on open (the doctor run is the titlebar chip's cached
    // startup source, refreshed on demand via `d`/the chip/the panel re-run).
    let _ = msg_tx.send(Message::RefreshDevices);
    let _ = msg_tx.send(Message::RunDoctor);
    // The component-level report: seeds the titlebar toolchain-chip
    // rollup and the bootstrap wizard, and drives the fresh-machine auto-open
    // when the core toolchain is missing.
    let _ = msg_tx.send(Message::RunBootstrapReport);
    // Offer the DAP server the chance to start itself. The *decision* (the
    // persisted `enabled`/`auto_start_in_ide` pair against the IDE detected
    // when the model was built, and whether this install has yet been told
    // once that auto-start opens a listener) stays in the pure core — this
    // only asks.
    let _ = msg_tx.send(Message::DapAutoStart);
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
                            let out = dispatch(&mut engine, msg, &mut FeedCtx {
                                subscribers: &mut subscribers,
                                pending_trees: &mut pending_trees,
                                records: &mcp_records,
                            });
                            needs_redraw |= out.redraw;
                            apply_effect(
                                out.effect,
                                &mut EffectCtx {
                                    engine: &mut engine,
                                    supervisor: &mut supervisor,
                                    devtools: &mut devtools,
                                    metrics: &mut metrics,
                                    tx: &msg_tx,
                                    next_adhoc_id: &mut next_adhoc_id,
                                    records: &mut mcp_records,
                                    backend: &backend,
                                    clipboard: clipboard_backend,
                                },
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
                // An embedded MCP server's command is served here rather than
                // in `update`: answering it needs the `Supervisor` and the
                // launch records the pure core deliberately cannot reach.
                // Nothing it changes bypasses the model — a launch registers
                // itself with `RegisterSession` like every other one.
                if let Message::Mcp(command) = msg {
                    serve_command(command, &mut McpServeCtx {
                        state: &engine.state,
                        supervisor: &mut supervisor,
                        records: &mut mcp_records,
                        tx: &msg_tx,
                        next_adhoc_id: &mut next_adhoc_id,
                        subscribers: &mut subscribers,
                        devtools: &mut devtools,
                        pending_trees: &mut pending_trees,
                    });
                } else {
                    let out = dispatch(&mut engine, msg, &mut FeedCtx {
                        subscribers: &mut subscribers,
                        pending_trees: &mut pending_trees,
                        records: &mcp_records,
                    });
                    needs_redraw |= out.redraw;
                    apply_effect(
                        out.effect,
                        &mut EffectCtx {
                            engine: &mut engine,
                            supervisor: &mut supervisor,
                            devtools: &mut devtools,
                            metrics: &mut metrics,
                            tx: &msg_tx,
                            next_adhoc_id: &mut next_adhoc_id,
                            records: &mut mcp_records,
                            backend: &backend,
                            clipboard: clipboard_backend,
                        },
                    );
                }
            }
            Some(ev) = session_rx.recv() => {
                let out = dispatch(&mut engine, Message::Session(ev), &mut FeedCtx {
                    subscribers: &mut subscribers,
                    pending_trees: &mut pending_trees,
                    records: &mcp_records,
                });
                needs_redraw |= out.redraw;
                apply_effect(
                    out.effect,
                    &mut EffectCtx {
                        engine: &mut engine,
                        supervisor: &mut supervisor,
                        devtools: &mut devtools,
                        metrics: &mut metrics,
                        tx: &msg_tx,
                        next_adhoc_id: &mut next_adhoc_id,
                        records: &mut mcp_records,
                        backend: &backend,
                        clipboard: clipboard_backend,
                    },
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
                        &mut EffectCtx {
                            engine: &mut engine,
                            supervisor: &mut supervisor,
                            devtools: &mut devtools,
                            metrics: &mut metrics,
                            tx: &msg_tx,
                            next_adhoc_id: &mut next_adhoc_id,
                            records: &mut mcp_records,
                            backend: &backend,
                            clipboard: clipboard_backend,
                        },
                    );
                }
            }
        }
    }

    // Quit: cancel whichever embedded servers are running, so their listeners
    // close here rather than only when the runtime is torn down. Both are
    // no-ops while nothing has started one.
    engine.stop_mcp();
    engine.stop_dap();

    Ok(())
}

/// Everything the deferred-answer registries need around one `update` call —
/// see [`crate::supervise::session_feeds`] for why the fan-out lives here
/// rather than inside the pure transition.
struct FeedCtx<'a> {
    /// The open session-event feeds.
    subscribers: &'a mut SessionSubscribers,
    /// The widget-tree pulls waiting on a bridge report.
    pending_trees: &'a mut PendingWidgetTrees,
    /// The launch records a session's `frust-mcp` terminal state is derived
    /// from — read-only here.
    records: &'a McpSessionRecords,
}

/// Apply one message, feeding the deferred-answer registries around it.
///
/// Two messages are more than a state transition to an embedded server:
///
/// - `Message::Session` may append log lines or end the session, both of
///   which a [`SessionEventFeed`](frust_mcp::engine::SessionEventFeed)
///   subscriber is owed. The cursor is taken **before** `update` runs and the
///   delta replayed **after**, so what a subscriber receives is exactly what
///   the model retained — already token-redacted by
///   `SessionView::push_line_at`, with no second redaction path to keep in
///   step.
/// - `Message::DevtoolsInspector` may be the answer to a `widget_tree` pull a
///   backend caller is blocked on. It is read, never consumed: the Inspector
///   tab still gets it through `update` exactly as before.
///
/// Every other message goes straight through.
fn dispatch(engine: &mut Engine, msg: Message, feeds: &mut FeedCtx<'_>) -> Outcome {
    match &msg {
        Message::DevtoolsInspector(session, event) => {
            feeds.pending_trees.resolve(*session, event);
            engine.handle(msg)
        }
        Message::Session(event) => {
            let session = event.id;
            let cursor = SessionSubscribers::cursor(&engine.state, session);
            let out = engine.handle(msg);
            if let Some(cursor) = cursor {
                let records = feeds.records;
                feeds.subscribers.replay(&engine.state, cursor, |view| {
                    mcp_session_state(records, view)
                });
                if !cursor.was_terminal && session_is_terminal(engine, session) {
                    // The bridge is torn down with the session, so a pull
                    // queued on it may never be served; the waiter is told
                    // rather than left to time out.
                    feeds
                        .pending_trees
                        .refuse(session, "the session ended before its widget tree arrived");
                }
            }
            out
        }
        _ => engine.handle(msg),
    }
}

/// Whether the workbench currently holds `session` in a terminal state.
fn session_is_terminal(engine: &Engine, session: SessionId) -> bool {
    engine
        .state
        .sessions
        .iter()
        .any(|view| view.id == session && view.state.is_terminal())
}

/// Everything enacting one [`Effect`] needs from [`run_loop`]'s scope —
/// bundled the same way [`McpServeCtx`] bundles the MCP-command path's
/// handles, and constructed fresh at each call site so the borrows live no
/// longer than the enactment itself.
struct EffectCtx<'a> {
    /// The engine — needed only by the two MCP-server effects, which own a
    /// live server handle rather than a value (see
    /// [`crate::engine::Engine::start_mcp`]).
    engine: &'a mut Engine,
    /// The session supervisor (launch/stop).
    supervisor: &'a mut Supervisor,
    /// The per-session DevTools connection threads.
    devtools: &'a mut DevtoolsBridge,
    /// The per-session metrics sampler threads.
    metrics: &'a mut MetricsBridge,
    /// The engine channel every off-thread task reports back through.
    tx: &'a UnboundedSender<Message>,
    /// The ad-hoc session-id counter (see [`run_loop`]).
    next_adhoc_id: &'a mut u64,
    /// The MCP launch records a started session is recorded in.
    records: &'a mut McpSessionRecords,
    /// The one [`TuiSessionBackend`] both embedded servers are started over —
    /// built once per run, so an MCP agent and a DAP client drive the same
    /// session world rather than two backends over the same supervisor.
    backend: &'a SharedBackend,
    /// The clipboard backend `run_loop` picked once at startup (see
    /// `crate::clipboard`'s module doc) — kept for the whole run rather than
    /// re-detected on every copy.
    clipboard: ClipboardBackend,
}

/// Enact an engine-requested [`Effect`] — the runner owns the side effects the
/// pure engine can't perform: killing a session through the supervisor, writing
/// the system clipboard, discovering devices off-thread, and launching
/// sessions.
fn apply_effect(effect: Option<Effect>, ctx: &mut EffectCtx<'_>) {
    let EffectCtx {
        engine,
        supervisor,
        devtools,
        metrics,
        tx,
        next_adhoc_id,
        records,
        backend,
        clipboard: clipboard_backend,
    } = ctx;
    match effect {
        Some(Effect::StopSession(id)) => supervisor.stop(id),
        Some(Effect::Copy(text)) => {
            if let Err(reason) = clipboard::write(*clipboard_backend, &text) {
                let _ = tx.send(Message::Notify {
                    level: ToastKind::Warn,
                    text: format!("Copy failed: {reason}"),
                });
            }
        }
        Some(Effect::RefreshDevices) => spawn_device_discovery(tx.clone()),
        Some(Effect::LaunchSessions(specs)) => launch_sessions(specs, supervisor, tx, records),
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
        // The bridge only *spawns* and *signals* here: the `adb forward`, the
        // TCP connect, the handshake and the frame-stats pump all run on its
        // own thread, and a teardown's wait is handed to `spawn_teardown`
        // below — so a slow, unreachable, or wedged service never stalls this
        // loop.
        Some(Effect::DevtoolsConnect(target)) => {
            spawn_teardown(devtools.connect(target, tx.clone()));
        }
        Some(Effect::DevtoolsDisconnect(session)) => spawn_teardown(devtools.disconnect(session)),
        // Inspector pulls only *queue* here: the bridge thread serves them
        // between frame windows over its own blocking client.
        Some(Effect::DevtoolsFetchTree { session }) => devtools.fetch_tree(session, tx),
        Some(Effect::DevtoolsFetchProps { session, id }) => devtools.fetch_props(session, id, tx),
        // Metrics sampling has the same shape — and the sharper teardown
        // hazard, since its `adb` probes are unbounded (see `spawn_teardown`).
        Some(Effect::MetricsStart(target)) => spawn_teardown(metrics.start(target, tx.clone())),
        Some(Effect::MetricsStop(session)) => spawn_teardown(metrics.stop(session)),
        // The embedded MCP server (workbook §B13). Only the runner can start
        // one: the backend it serves is built over *this* loop's supervisor,
        // and the server itself is spawned on the runtime this loop runs on.
        // Everything the server then reports (its bound port, a bind failure)
        // travels back as an ordinary `Message`.
        Some(Effect::StartMcpServer) => {
            engine.start_mcp(Arc::clone(backend), crate::engine::DEFAULT_MCP_PORT);
        }
        Some(Effect::StopMcpServer) => {
            engine.stop_mcp();
        }
        // The embedded DAP server (the same shape, one crate over): the
        // backend is the *same* one the MCP server gets, and the project root
        // is the workbench's own — a DAP client never chooses one (see
        // `frust_dap::serve_embedded`).
        Some(Effect::StartDapServer { port }) => {
            engine.start_dap(Arc::clone(backend), port);
        }
        Some(Effect::StopDapServer) => {
            engine.stop_dap();
        }
        Some(Effect::SaveDapSetting(setting)) => crate::engine::save_dap_setting(setting),
        // IDE-config generation reads and writes real files under the project
        // root, so it goes to the blocking pool like every other filesystem
        // effect here and reports back as an ordinary `Message`.
        Some(Effect::GenerateIdeConfig(request)) => {
            spawn_ide_config_generation(request, tx.clone());
        }
        Some(Effect::Batch(effects)) => {
            for effect in effects {
                apply_effect(
                    Some(effect),
                    &mut EffectCtx {
                        engine,
                        supervisor,
                        devtools,
                        metrics,
                        tx,
                        next_adhoc_id,
                        records,
                        backend,
                        clipboard: *clipboard_backend,
                    },
                );
            }
        }
        Some(Effect::SetMouseCapture(on)) => set_mouse_capture(on),
        Some(Effect::SaveSidebarWidth(width)) => crate::engine::save_sidebar_width(width),
        None => {}
    }
}

/// Write (or refresh) an IDE's DAP client config off the UI thread, posting
/// the outcome back as [`Message::DapIdeConfig`].
///
/// `generate_ide_config` merges into whatever the editor already has on disk,
/// so it reads, parses, creates directories and writes — none of which belongs
/// on the event loop. Every ending is reported: a written/updated/skipped
/// file, the IDE that has no DAP config format at all (`Ok(None)`, which only
/// the JetBrains pair reaches here — the pure core refuses the others before
/// asking for this effect), and a failure, which is retained and shown rather
/// than dropped.
fn spawn_ide_config_generation(
    request: crate::engine::IdeConfigRequest,
    tx: UnboundedSender<Message>,
) {
    tokio::task::spawn_blocking(move || {
        let report = match frust_dap::ide_config::generate_ide_config(
            Some(request.ide),
            request.port,
            &request.project_root,
        ) {
            Ok(Some(result)) => crate::engine::DapIdeReport::Written {
                ide: request.ide,
                result,
            },
            Ok(None) => crate::engine::DapIdeReport::Unsupported(request.ide),
            Err(e) => crate::engine::DapIdeReport::Failed(e.to_string()),
        };
        let _ = tx.send(Message::DapIdeConfig(report));
    });
}

/// Wait out a stopped DevTools bridge thread off the event loop.
///
/// Both bridges hand a signalled (and, for devtools, already muted) thread
/// back as a [`Teardown`] instead of joining it, because a bridge thread can
/// be parked in a call this side cannot bound — an `adb` probe against an
/// unresponsive device has no wall-clock timeout at all. Waiting for one here
/// would freeze the whole workbench: no repaint, no input, not even quit, on
/// a path every Android session's terminal transition takes. So the wait goes
/// to `spawn_blocking`, the same pool the other blocking effects in
/// `apply_effect` use (`LaunchSessions`, `RunDoctor`, the ad-hoc build/clean
/// sessions), and degrades to a lagging background task instead.
///
/// Only the *waiting* is deferred: each bridge's own bookkeeping already
/// happened synchronously above, in effect order, so two effects for the same
/// session can never be reordered by the blocking pool.
///
/// **Quit.** On the way out of [`run_loop`] each bridge's `Drop` signals every
/// remaining thread and then waits *inline* against one shared deadline —
/// there is no loop left to protect, and a torn-down bridge must not leave an
/// `adb forward` behind if it can help it. Any teardown still parked in the
/// blocking pool is waited out the same bounded way by the runtime's own
/// shutdown. So quitting with a wedged device costs a bounded pause (a
/// fraction of a second), never a hang.
fn spawn_teardown(teardown: Option<Teardown>) {
    if let Some(teardown) = teardown {
        tokio::task::spawn_blocking(move || teardown.wait());
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
///
/// [`NO_EXTRA_FEATURES`] is passed to both mobile pipelines: the workbench's
/// build modal exposes mode/flavor/defines/version, not cargo features, so a
/// TUI build compiles exactly what the mode selects — the `--features`
/// passthrough is a CLI flag surface only.
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
            android_build::build(
                runner,
                &spec.project_root,
                &spec.info,
                &target,
                NO_EXTRA_FEATURES,
                on_line,
            )
            .map(|artifacts| artifacts.paths)
        }
        BuildTargetSpec::Appbundle => android_build::build(
            runner,
            &spec.project_root,
            &spec.info,
            &AndroidArtifact::Appbundle,
            NO_EXTRA_FEATURES,
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
            ios_build::build(
                runner,
                &spec.project_root,
                &spec.info,
                &target,
                NO_EXTRA_FEATURES,
                on_line,
            )
            .map(|artifacts| artifacts.paths)
        }
        BuildTargetSpec::Ipa { export_method } => {
            let target = IosArtifact::Ipa {
                export_method: export_method.clone(),
            };
            ios_build::build(
                runner,
                &spec.project_root,
                &spec.info,
                &target,
                NO_EXTRA_FEATURES,
                on_line,
            )
            .map(|artifacts| artifacts.paths)
        }
        // The desktop pipeline reports non-fatal observations (a missing or
        // too-small icon, a generated Info.plist, an unsigned .app) as typed
        // notes on its report rather than printing them — this is the one
        // place they become log lines. Its failures are a typed
        // `DesktopBuildError` (host lock, manifest, compile, codesign), which
        // `?` carries to the caller's `error: {err:#}` line: never a raw
        // stderr dump into the workbench's raw-mode terminal.
        BuildTargetSpec::DesktopBundle { target } => {
            let report =
                desktop_build::build(runner, &spec.project_root, &spec.info, *target, on_line)?;
            for note in &report.notes {
                on_line(&format!("note: {note}"));
            }
            Ok(vec![report.root, report.executable])
        }
    }
}

/// The build-session tab label per artifact kind (`build apk`/`build ios`/
/// `build linux`/…).
fn build_target_label(target: &BuildTargetSpec) -> &'static str {
    match target {
        BuildTargetSpec::Apk { .. } => "apk",
        BuildTargetSpec::Appbundle => "appbundle",
        BuildTargetSpec::IosApp { .. } => "ios",
        BuildTargetSpec::Ipa { .. } => "ipa",
        BuildTargetSpec::DesktopBundle { target } => target.as_str(),
    }
}

/// Build-output directories a clean session removes beyond `cargo clean`'s
/// own `target/` — the same set `frust-cli`'s `commands/clean.rs::REMOVED_DIRS`
/// removes; duplicated by value here since `clean` has no `frust-drive`
/// surface to call into (see `docs/ARCHITECTURE.md`'s Module Structure —
/// `clean` lives entirely in `frust-cli`, unlike `doctor`/`build`).
const CLEAN_REMOVED_DIRS: &[&str] = &[
    "android/app/build",
    "android/build",
    "android/.gradle",
    "build",
    "dist",
];

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
    records: &mut McpSessionRecords,
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
                // Record the launch even with no MCP server running: an agent
                // that connects later must see the sessions the *user*
                // started, not only its own (one session world), and nothing
                // can reconstruct a spec after the fact.
                records.insert(id, spec);
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
            ActiveModal::McpPanel => translate_mcp_key(code),
            ActiveModal::DapSettings(settings) => translate_dap_settings_key(code, settings.focus),
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
    // session-tab switching (`Tab`/`Shift+Tab`) deliberately still apply —
    // except `Tab` on the connected Performance tab, which `Message`s its
    // own chart↔breakdown focus cycle instead (see
    // `translate_devtools_key`'s doc).
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

        // `m` opens the MCP panel and `M` starts/stops the embedded MCP
        // server, from either screen (workbook §B13) — the server hosts the
        // *workbench*, not one project, so neither is workbench-gated. Mouse
        // parity: the sidebar ACTIONS "MCP" row toggles it, the palette's
        // "MCP server…" row opens the panel. `Alt+m` (mouse capture) is
        // matched far above, so the two never collide.
        KeyCode::Char('m') => vec![Message::OpenMcpPanel],
        KeyCode::Char('M') => vec![Message::ToggleMcpServer],

        // `D` opens the DAP settings dialog from either screen — the embedded
        // debug adapter serves the *workbench*, like MCP, so it isn't
        // workbench-gated either. `d` is already taken twice over (DevTools
        // with a session open, the doctor panel without one), so the dialog
        // takes the shifted key and the server toggle lives inside it (`s`)
        // rather than claiming a second top-level binding. Mouse parity: the
        // sidebar ACTIONS "DAP" row; the palette carries both rows.
        KeyCode::Char('D') => vec![Message::OpenDapSettings],

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
/// (workbook §B12's own key table, binding): `d` always returns to the log,
/// `1`–`4` jump to a tab and `[`/`]` cycle them (connected only — there is no
/// strip to move through otherwise), `r` retries a failed connection, and
/// `Tab`/`Shift+Tab` still switch session tabs everywhere except the
/// Performance tab, where `Tab` instead cycles its own chart↔breakdown focus
/// (§B12's Performance-tab row) — a tab keeps its own log/DevTools state, so
/// leaving and coming back lands right where you were. Every row has a mouse
/// equivalent: a tab pill, the Retry button, a chart column, and the status
/// row's back affordance.
///
/// **`Esc` is two-stage inside the Performance tab, one beyond what §B12
/// draws**: with a frame scrubbed, the first `Esc` only drops back to the
/// live tail ([`Message::DevtoolsPerfClearSelection`]); a second `Esc` (or
/// the first, with nothing selected) leaves DevTools
/// ([`Message::DevtoolsClose`]) same as every other screen. Without this, a
/// scrub session's only way out would also blow away the pinned frame in the
/// same keystroke.
///
/// **`Tab` is per-tab-scoped**: it cycles the active tab's own panes where
/// that tab has any (Performance's chart↔breakdown, Inspector's tree↔props)
/// and otherwise keeps its workbench meaning of switching session tabs — one
/// `on_<tab>` gate each, so a third tab claiming `Tab` adds a gate rather
/// than rewriting the arm.
///
/// **`r` means refresh only on a connected Inspector**, and retry only on the
/// failed screen — §B12's "mutually exclusive contexts, no live collision".
/// The two live in different [`DevtoolsPhase`] arms below, so neither can
/// shadow the other.
///
/// The [`DevtoolsPhase`] match is exhaustive: a new screen has to decide what
/// its keys do rather than silently inheriting another screen's.
fn translate_devtools_key(
    code: KeyCode,
    state: &AppState,
    devtools: &DevtoolsState,
) -> Vec<Message> {
    use crate::engine::{DevtoolsPhase, DevtoolsTab};

    let on_performance = devtools.active_tab == DevtoolsTab::Performance;
    let on_inspector = devtools.active_tab == DevtoolsTab::Inspector;
    let connected = matches!(devtools.phase(), DevtoolsPhase::Connected);

    match code {
        KeyCode::Char('q') => return vec![Message::Quit],
        KeyCode::Char('d') => return vec![Message::DevtoolsClose],
        KeyCode::Esc => {
            if on_performance && devtools.performance.has_selection() {
                return vec![Message::DevtoolsPerfClearSelection];
            }
            return vec![Message::DevtoolsClose];
        }
        KeyCode::Tab if connected && on_performance => {
            return vec![Message::DevtoolsPerfFocusCycle];
        }
        KeyCode::Tab if connected && on_inspector => {
            return vec![Message::DevtoolsInspectorFocusCycle];
        }
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
            // `←`/`→` scrub the Performance chart's selection across its
            // 120-frame window (§B12's Performance-tab row); meaningless on
            // any other tab.
            KeyCode::Left if on_performance => vec![Message::DevtoolsPerfScrub(-1)],
            KeyCode::Right if on_performance => vec![Message::DevtoolsPerfScrub(1)],
            // The Inspector tree: move, expand/collapse, re-pull.
            KeyCode::Up | KeyCode::Char('k') if on_inspector => {
                vec![Message::DevtoolsInspectorSelect(-1)]
            }
            KeyCode::Down | KeyCode::Char('j') if on_inspector => {
                vec![Message::DevtoolsInspectorSelect(1)]
            }
            KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ') if on_inspector => {
                vec![Message::DevtoolsInspectorExpand]
            }
            KeyCode::Left if on_inspector => vec![Message::DevtoolsInspectorCollapse],
            KeyCode::Char('r') if on_inspector => vec![Message::DevtoolsInspectorRefresh],
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

/// Translate one key press while the MCP panel is open (workbook §B13):
/// `s` starts or stops the embedded server without leaving the panel, and
/// `Esc`/`m` close it — closing the panel is never a stop. Mouse parity: the
/// panel's own Start/Stop and Close buttons.
fn translate_mcp_key(code: KeyCode) -> Vec<Message> {
    match code {
        KeyCode::Esc | KeyCode::Char('m') => vec![Message::CloseMcpPanel],
        KeyCode::Char('s') => vec![Message::ToggleMcpServer],
        _ => vec![],
    }
}

/// Translate one key press while the DAP settings dialog is open.
///
/// `Tab`/`↓`/`↑` walk the controls, `Enter`/`Space` activate the focused one,
/// `←`/`→` cycle the IDE selector from anywhere (moving focus there, exactly
/// as the run-config modal's mode selector does), `s` starts/stops the server
/// (the MCP panel's own key), `g` generates the IDE config, and `Esc`/`D`
/// close — closing is never a stop. Mouse parity: every row and both action
/// buttons register a click region.
///
/// Typed characters reach the port field **only while it has focus**, so the
/// `s`/`g` shortcuts are never swallowed by a text field the user isn't in —
/// and a port field the user *is* in never turns `s` into a server toggle.
fn translate_dap_settings_key(code: KeyCode, focus: crate::engine::DapFocus) -> Vec<Message> {
    use crate::engine::DapFocus;
    match code {
        KeyCode::Esc => vec![Message::CloseDapSettings],
        KeyCode::Tab | KeyCode::Down => vec![Message::DapSettingsFocusNext],
        KeyCode::BackTab | KeyCode::Up => vec![Message::DapSettingsFocusPrev],
        KeyCode::Left => vec![Message::DapSettingsCycleIde(-1)],
        KeyCode::Right => vec![Message::DapSettingsCycleIde(1)],
        // `Space` activates rather than typing: a space is never part of a
        // port, so the checkbox meaning wins even inside the field.
        KeyCode::Enter | KeyCode::Char(' ') => vec![Message::DapSettingsActivate],
        KeyCode::Backspace => vec![Message::DapSettingsBackspace],
        KeyCode::Char(c) if focus == DapFocus::Port => vec![Message::DapSettingsInput(c)],
        KeyCode::Char('D') => vec![Message::CloseDapSettings],
        KeyCode::Char('s') => vec![Message::ToggleDapServer],
        KeyCode::Char('g') => vec![Message::DapSettingsGenerate],
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
    use frust_drive::build_info::{BuildArgs, BuildInfo, BuildMode};
    use frust_drive::desktop_build::DesktopBundleTarget;
    use frust_mcp::engine::{SessionEvent as McpSessionEvent, SessionState as McpSessionState};

    /// The runner's own half of the session-event feed: `dispatch` is what
    /// turns an applied `Message::Session` into the lines and the ending a
    /// subscriber is owed. Driving it here rather than only through
    /// `SessionSubscribers` is the point — this is the wiring that would
    /// silently stop feeding a DAP client if a future refactor routed a
    /// session message around it.
    #[tokio::test]
    async fn dispatch_feeds_a_subscriber_the_lines_and_the_exit_a_session_produces() {
        let mut engine = Engine::new(AppState::default());
        let mut subscribers = SessionSubscribers::new();
        let mut pending_trees = PendingWidgetTrees::new();
        let records = McpSessionRecords::new();

        engine.handle(Message::RegisterSession {
            id: SessionId(0),
            project_root: std::path::PathBuf::from("/tmp/frust-tui-dispatch"),
            target_label: "desktop".to_string(),
            devtools: DevtoolsLaunch::unavailable(),
        });
        let view = engine
            .state
            .sessions
            .first()
            .expect("the session registered");
        let feed = subscribers.subscribe(view, None);

        let mut feeds = FeedCtx {
            subscribers: &mut subscribers,
            pending_trees: &mut pending_trees,
            records: &records,
        };
        dispatch(
            &mut engine,
            Message::Session(SessionEvent {
                id: SessionId(0),
                kind: SessionEventKind::Lines(vec!["hello".to_string()]),
            }),
            &mut feeds,
        );
        assert_eq!(
            feed.try_recv(),
            Ok(McpSessionEvent::Log("hello".to_string())),
            "a line batch reaches the feed through the runner, not only through the model"
        );

        dispatch(
            &mut engine,
            Message::Session(SessionEvent {
                id: SessionId(0),
                kind: SessionEventKind::State(SessionState::Exited(true)),
            }),
            &mut feeds,
        );
        assert_eq!(
            feed.try_recv(),
            Ok(McpSessionEvent::Exited {
                state: McpSessionState::Exited { success: true }
            })
        );
        assert!(
            feed.try_recv().is_err(),
            "and the feed closes rather than going quiet"
        );
    }

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

    // ── Desktop bundle builds ─────────────────────────────────────────────────

    /// A project fixture for the desktop-bundle build arm: a temp directory
    /// with a `frust.toml` naming the app, and (optionally) the binary a
    /// successful `cargo build --release` would have produced.
    fn desktop_fixture(tag: &str, with_binary: Option<&str>) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "frust-tui-desktop-build-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("frust.toml"),
            "[app]\nname = \"my_app\"\norg = \"dev.f0x\"\n",
        )
        .unwrap();
        if let Some(binary) = with_binary {
            let out = dir.join("target").join("release");
            std::fs::create_dir_all(&out).unwrap();
            std::fs::write(out.join(binary), b"#!/bin/sh\ntrue\n").unwrap();
        }
        dir
    }

    fn desktop_spec(project_root: PathBuf, target: DesktopBundleTarget) -> BuildSpec {
        BuildSpec {
            project_root,
            info: BuildInfo::from_args(
                BuildArgs {
                    release: true,
                    ..BuildArgs::default()
                },
                BuildMode::Release,
            )
            .unwrap(),
            target: BuildTargetSpec::DesktopBundle { target },
        }
    }

    /// The one `cargo` invocation the desktop pipeline makes for a release
    /// build of a project whose `Cargo.toml` can't be read (the fixture's):
    /// the release-lean preflight fails open and keeps `lean`.
    const DESKTOP_RELEASE_BUILD: &str = "cargo build --release --features lean";

    /// The whole desktop arm end to end through `run_build`: compile output
    /// and the pipeline's own progress reach the session's line sink, each
    /// `BundleNote` is surfaced as a log line (nothing is printed), and the
    /// bundle plus its executable come back as the session's artifacts.
    ///
    /// Skipped when the environment names a `CARGO_TARGET_DIR`: the pipeline
    /// then looks for the compiled binary there rather than in the fixture,
    /// and planting a file inside a developer's shared target directory is not
    /// this test's business.
    #[test]
    fn a_desktop_bundle_build_streams_its_notes_into_the_session_log() {
        use frust_drive::process::{FakeProcessRunner, Output};

        let Some(host) = DesktopBundleTarget::host() else {
            return; // a host with no desktop bundle layout of its own
        };
        if std::env::var_os("CARGO_TARGET_DIR").is_some() {
            return;
        }
        let binary = if host == DesktopBundleTarget::Windows {
            "my_app.exe"
        } else {
            "my_app"
        };
        let dir = desktop_fixture("notes", Some(binary));
        let runner = FakeProcessRunner::new().with(
            DESKTOP_RELEASE_BUILD,
            Output {
                success: true,
                stdout: "    Finished `release` profile [optimized]".to_string(),
                stderr: String::new(),
            },
        );

        let mut lines = Vec::new();
        let paths = run_build(&runner, &desktop_spec(dir.clone(), host), &mut |line| {
            lines.push(line.to_string())
        })
        .unwrap();

        assert!(lines.iter().any(|l| l.starts_with("[cargo] ")), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("bundle: ")), "{lines:?}");
        // The fixture configures no `[desktop] icon`, so the pipeline reports
        // exactly that as a note rather than failing the build.
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("note: ") && l.contains("icon")),
            "{lines:?}"
        );
        assert_eq!(paths.len(), 2, "the bundle root and its executable");
        assert!(paths[1].is_file(), "{paths:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A failed compile comes back as the pipeline's typed error (carrying its
    /// own bounded output tail), which the caller renders as one `error:` log
    /// line — never a raw stderr dump into the raw-mode terminal.
    #[test]
    fn a_failed_desktop_bundle_build_surfaces_the_typed_error() {
        use frust_drive::process::{FakeProcessRunner, Output};

        let Some(host) = DesktopBundleTarget::host() else {
            return;
        };
        let dir = desktop_fixture("failed", None);
        let runner = FakeProcessRunner::new().with(
            DESKTOP_RELEASE_BUILD,
            Output {
                success: false,
                stdout: String::new(),
                stderr: "error[E0425]: cannot find value `nope` in this scope".to_string(),
            },
        );

        let err = run_build(&runner, &desktop_spec(dir.clone(), host), &mut |_| {}).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("cargo build --release"), "{message}");
        assert!(message.contains("E0425"), "{message}");
        let _ = std::fs::remove_dir_all(&dir);
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

    /// The Performance tab's own key table: `Tab` cycles chart↔breakdown
    /// instead of switching session tabs, `←`/`→` scrub, and `Esc` is
    /// two-stage — clears the selection first, only then leaves DevTools.
    #[test]
    fn performance_tab_owns_tab_and_arrows_and_esc_is_two_stage() {
        use crate::engine::{ConnEvent, DevtoolsTab, PerfFocus, update};
        let regions = MouseRegions::new();
        let mut state = devtools_state();
        update(&mut state, Message::DevtoolsToggle);
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
            state.active_session().unwrap().devtools.active_tab,
            DevtoolsTab::Performance,
            "Performance is the default tab"
        );
        // A non-empty ring, so there is something to scrub.
        let frames = (0..10u64)
            .map(|n| frust_devtools_protocol::FrameStats {
                n,
                total_us: 16_000,
                rebuild_us: 8_000,
                layout_us: 3_000,
                paint_us: 2_000,
                encode_us: 1_400,
                acquire_us: 600,
                submit_us: 1_000,
                skipped: false,
            })
            .collect();
        update(
            &mut state,
            Message::DevtoolsConn(SessionId(0), ConnEvent::Frames(frames)),
        );

        // `Tab` cycles focus rather than switching session tabs.
        let msgs = translate_event(key(KeyCode::Tab), &state, &regions);
        assert_eq!(msgs, vec![Message::DevtoolsPerfFocusCycle]);
        update(&mut state, msgs[0].clone());
        assert_eq!(
            state.active_session().unwrap().devtools.performance.focus,
            PerfFocus::Breakdown
        );

        // `←`/`→` scrub.
        let msgs = translate_event(key(KeyCode::Left), &state, &regions);
        assert_eq!(msgs, vec![Message::DevtoolsPerfScrub(-1)]);
        let msgs = translate_event(key(KeyCode::Right), &state, &regions);
        assert_eq!(msgs, vec![Message::DevtoolsPerfScrub(1)]);
        update(&mut state, Message::DevtoolsPerfScrub(0));
        assert!(
            state
                .active_session()
                .unwrap()
                .devtools
                .performance
                .has_selection()
        );

        // First `Esc` only clears the selection, staying in DevTools.
        let msgs = translate_event(key(KeyCode::Esc), &state, &regions);
        assert_eq!(msgs, vec![Message::DevtoolsPerfClearSelection]);
        update(&mut state, msgs[0].clone());
        assert!(state.active_session().unwrap().devtools.open);
        assert!(
            !state
                .active_session()
                .unwrap()
                .devtools
                .performance
                .has_selection()
        );

        // Second `Esc`, with nothing selected, leaves DevTools like every
        // other screen.
        let msgs = translate_event(key(KeyCode::Esc), &state, &regions);
        assert_eq!(msgs, vec![Message::DevtoolsClose]);
        update(&mut state, msgs[0].clone());
        assert!(!state.active_session().unwrap().devtools.open);
    }

    /// Off the Performance tab, `Tab` still switches session tabs — the
    /// override above is scoped to the one tab that owns the key.
    #[test]
    fn tab_still_switches_session_tabs_off_the_performance_tab() {
        use crate::engine::{ConnEvent, update};
        let regions = MouseRegions::new();
        let mut state = devtools_state();
        update(&mut state, Message::DevtoolsToggle);
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
        update(&mut state, Message::DevtoolsTab(1)); // System
        assert_eq!(
            translate_event(key(KeyCode::Tab), &state, &regions),
            vec![Message::NextTab]
        );
    }

    /// §B12's Inspector key row, driven end to end through the real
    /// translate → `update` path: `j`/`k` move the selection, `Enter`
    /// expands, `Tab` flips tree↔props, and `r` re-pulls the tree as an
    /// effect (rather than colliding with the failed screen's retry).
    #[test]
    fn inspector_keys_move_expand_flip_focus_and_refresh() {
        use crate::engine::{ConnEvent, InspectorEvent, InspectorFocus, update};
        use frust_devtools_protocol::{WidgetNode, WidgetTreeDump};

        let regions = MouseRegions::new();
        let mut state = devtools_state();
        update(&mut state, Message::DevtoolsToggle);
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
        // `3` selects the Inspector, which pulls its first snapshot.
        let msgs = translate_event(key(KeyCode::Char('3')), &state, &regions);
        assert_eq!(msgs, vec![Message::DevtoolsTab(2)]);
        let out = update(&mut state, msgs[0].clone());
        assert_eq!(
            out.effect,
            Some(Effect::DevtoolsFetchTree {
                session: SessionId(0)
            })
        );

        let leaf = |id: u64| WidgetNode {
            id,
            type_name: "frust_widgets::text::TextWidget".to_string(),
            debug_label: None,
            bounds: None,
            children: Vec::new(),
        };
        update(
            &mut state,
            Message::DevtoolsInspector(
                SessionId(0),
                InspectorEvent::TreeArrived(WidgetTreeDump {
                    roots: vec![WidgetNode {
                        id: 1,
                        type_name: "frust_widgets::flex::FlexWidget".to_string(),
                        debug_label: None,
                        bounds: None,
                        children: vec![
                            WidgetNode {
                                id: 2,
                                type_name: "frust_widgets::padding::PaddingWidget".to_string(),
                                debug_label: None,
                                bounds: None,
                                children: vec![leaf(3)],
                            },
                            leaf(4),
                        ],
                    }],
                }),
            ),
        );

        // `j`/`k` move within the flattened rows.
        let msgs = translate_event(key(KeyCode::Char('j')), &state, &regions);
        assert_eq!(msgs, vec![Message::DevtoolsInspectorSelect(1)]);
        update(&mut state, msgs[0].clone());
        assert_eq!(
            state
                .active_session()
                .unwrap()
                .devtools
                .inspector
                .selected_id(),
            Some(2)
        );
        let msgs = translate_event(key(KeyCode::Char('k')), &state, &regions);
        assert_eq!(msgs, vec![Message::DevtoolsInspectorSelect(-1)]);
        update(&mut state, msgs[0].clone());

        // `←` collapses the (selected) root, `Enter` — like `→`/`Space` —
        // re-expands it.
        let msgs = translate_event(key(KeyCode::Left), &state, &regions);
        assert_eq!(msgs, vec![Message::DevtoolsInspectorCollapse]);
        update(&mut state, msgs[0].clone());
        assert_eq!(
            state
                .active_session()
                .unwrap()
                .devtools
                .inspector
                .rows()
                .len(),
            1
        );
        let msgs = translate_event(key(KeyCode::Enter), &state, &regions);
        assert_eq!(msgs, vec![Message::DevtoolsInspectorExpand]);
        update(&mut state, msgs[0].clone());
        assert_eq!(
            state
                .active_session()
                .unwrap()
                .devtools
                .inspector
                .rows()
                .len(),
            4
        );

        // `Tab` is the Inspector's own tree↔props flip here, not a session
        // tab switch.
        let msgs = translate_event(key(KeyCode::Tab), &state, &regions);
        assert_eq!(msgs, vec![Message::DevtoolsInspectorFocusCycle]);
        update(&mut state, msgs[0].clone());
        assert_eq!(
            state.active_session().unwrap().devtools.inspector.focus,
            InspectorFocus::Props
        );

        // `r` on a connected Inspector re-pulls the tree.
        let msgs = translate_event(key(KeyCode::Char('r')), &state, &regions);
        assert_eq!(msgs, vec![Message::DevtoolsInspectorRefresh]);
        let out = update(&mut state, msgs[0].clone());
        assert_eq!(
            out.effect,
            Some(Effect::DevtoolsFetchTree {
                session: SessionId(0)
            })
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

    // ── Embedded MCP server (workbook §B13) ──────────────────────────────

    #[test]
    fn m_opens_the_mcp_panel_and_shift_m_toggles_the_server() {
        let regions = MouseRegions::new();
        for state in [
            AppState::default(), // welcome
            AppState {
                screen: Screen::Workbench,
                ..Default::default()
            },
        ] {
            assert_eq!(
                translate_event(key(KeyCode::Char('m')), &state, &regions),
                vec![Message::OpenMcpPanel]
            );
            assert_eq!(
                translate_event(key(KeyCode::Char('M')), &state, &regions),
                vec![Message::ToggleMcpServer]
            );
        }
    }

    /// `Alt+m` is matched before any plain letter, so the capture toggle and
    /// the MCP keys cannot collide (§B13's no-overload claim).
    #[test]
    fn alt_m_still_toggles_mouse_capture() {
        let regions = MouseRegions::new();
        let state = AppState::default();
        let ev = Event::Key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::ALT));
        assert_eq!(
            translate_event(ev, &state, &regions),
            vec![Message::ToggleMouseCapture]
        );
    }

    #[test]
    fn the_open_mcp_panel_owns_s_and_closes_on_esc_or_m() {
        let regions = MouseRegions::new();
        let state = AppState {
            screen: Screen::Workbench,
            mcp_panel_open: true,
            ..Default::default()
        };
        assert_eq!(
            translate_event(key(KeyCode::Char('s')), &state, &regions),
            vec![Message::ToggleMcpServer],
            "`s` starts/stops without leaving the panel"
        );
        for code in [KeyCode::Esc, KeyCode::Char('m')] {
            assert_eq!(
                translate_event(key(code), &state, &regions),
                vec![Message::CloseMcpPanel]
            );
        }
        // The workbench's own `s` (sidebar overlay) is out of scope while the
        // panel is up — the modal captures the namespace.
        assert_eq!(
            translate_event(key(KeyCode::Char('b')), &state, &regions),
            Vec::<Message>::new()
        );
    }

    /// `D` is the DAP settings dialog's key from either top-level screen —
    /// the shifted key, because `d` is already DevTools (with a session) and
    /// the doctor panel (without one).
    #[test]
    fn shift_d_opens_the_dap_settings_dialog_from_either_screen() {
        let regions = MouseRegions::new();
        for screen in [Screen::Welcome, Screen::Workbench] {
            let state = AppState {
                screen,
                ..Default::default()
            };
            assert_eq!(
                translate_event(key(KeyCode::Char('D')), &state, &regions),
                vec![Message::OpenDapSettings],
                "{screen:?}"
            );
        }
    }

    /// The dialog's own key map, including the one context-sensitive rule:
    /// a character reaches the port field only while that field has focus, so
    /// `s`/`g`/`D` stay shortcuts everywhere else.
    #[test]
    fn the_dap_dialog_key_map_routes_shortcuts_and_typing_by_focus() {
        use crate::engine::DapFocus;
        let state = AppState {
            dap_settings_open: true,
            ..Default::default()
        };
        let regions = MouseRegions::new();
        for (code, expected) in [
            (KeyCode::Esc, Message::CloseDapSettings),
            (KeyCode::Char('D'), Message::CloseDapSettings),
            (KeyCode::Tab, Message::DapSettingsFocusNext),
            (KeyCode::BackTab, Message::DapSettingsFocusPrev),
            (KeyCode::Right, Message::DapSettingsCycleIde(1)),
            (KeyCode::Left, Message::DapSettingsCycleIde(-1)),
            (KeyCode::Enter, Message::DapSettingsActivate),
            (KeyCode::Char(' '), Message::DapSettingsActivate),
            (KeyCode::Char('s'), Message::ToggleDapServer),
            (KeyCode::Char('g'), Message::DapSettingsGenerate),
        ] {
            assert_eq!(
                translate_event(key(code), &state, &regions),
                vec![expected],
                "{code:?} with the server action focused"
            );
        }

        let mut state = state;
        state.dap_settings.focus = DapFocus::Port;
        for c in ['s', 'g', 'D', '4'] {
            assert_eq!(
                translate_event(key(KeyCode::Char(c)), &state, &regions),
                vec![Message::DapSettingsInput(c)],
                "`{c}` must type into a focused port field, not fire a shortcut"
            );
        }
        // …and the navigation keys still navigate out of it.
        assert_eq!(
            translate_event(key(KeyCode::Esc), &state, &regions),
            vec![Message::CloseDapSettings]
        );
    }
}
