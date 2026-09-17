# Frust - TUI Architecture

## Overview

TUI is `frust-tui`, a mouse-first `ratatui` TEA (The Elm Architecture) terminal workbench for
driving Frust app projects — scaffold/build/run/doctor/clean as supervised sessions across
desktop/Android/iOS. It is a library with no binary of its own, consumed by `frust-cli` (bare
`frust` in an interactive terminal, or the explicit `tui` subcommand); it depends on
`frust-drive` + `frust-devtools-protocol` + `frust-mcp` + `frust-dap`, with zero dependency on any
framework rendering crate, mirroring `frust-cli`'s tooling-isolation charter (see
[ARCHITECTURE.md](ARCHITECTURE.md)'s Cross-Unit Layer Dependencies — tooling may depend on the
`frust-mcp` tooling crate). The workbench can embed an MCP server and/or a DAP server over its own
sessions — an AI agent and an IDE's debugger each drive the exact sessions a human sees in the
terminal, and both embeds are optional, off by default, and independently toggled.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how TUI relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `engine` | Pure TEA core: `AppState` model, `Message` enum, `update()` pure transition returning `Outcome`/`Effect`; terminal-free and unit-testable without a TTY. `engine::logstyle` classifies each log line once at push into per-line metadata (level, source, panic-fold role) consumed only by rendering. `SessionView` (`visible_indices`/`bottom_pos`) owns the visible-sequence/scroll-anchor math, computed once and consumed as-is by both scroll input and rendering. `engine::devtools` holds each session's DevTools view-model (`DevtoolsState`: five-phase connection, active tab, per-tab sub-state) as plain data + pure transitions mirroring what the bridges below report. `engine::dap_settings` holds the DAP settings dialog's model — focus walk, port validation, IDE selector, auto-start/auto-configure decision — as plain data loaded once at startup (not created/dropped with the dialog, unlike the run-config modal), so the preferences it holds back both the startup auto-start check and every `DapListening` report's auto-configure check whether or not the dialog is ever opened. `engine::build_launcher` holds the build-target picker's model (`ArtifactKind`): Android/iOS artifacts plus, on desktop, exactly the host's own bundle target (`DesktopBundleTarget::host()` — never a foreign-OS choice) |
| `supervise` | Session-supervision layer over `frust-drive`: drives per-session build/run lifecycles and bridges process output into engine messages. `supervise::progress` is a pure, side-effect-free build-phase-label extractor over streamed output lines, called from the drain path. `supervise::devtools_bridge`/`supervise::metrics_bridge` are DevTools' impure half, one thread per session each: the former owns a blocking devtools-protocol connection (token handshake, coalesced frame-stats, on-demand widget-tree/props pulls, `adb forward` on Android — see [DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md)); the latter is a local `frust-drive::metrics` sampler with no build-feature gate, started once a session's Android identity resolves rather than once a discovery line lands. A device session's `stop` additionally dispatches a best-effort OS-level app termination (`adb shell am force-stop` / `simctl terminate`) on a tracked detached thread the supervisor bounded-joins from `stop_all`/`Drop` — shared by the user's stop keypress and MCP's/DAP's `stop_app`/`restart_app`. An Android session also runs a liveness prober on its own tracked thread: `adb shell pidof` every 2s while streaming, and two consecutive not-alive answers close the logcat stream and land the session `Exited(false)` with an explanatory note line — the only way `adb logcat --pid` (which does not exit when its pid dies) reports an app's death. iOS needs no prober (a simulator session's console pty exits with the app); a physical iOS device's app death goes undetected (see [LIMITATIONS.md](LIMITATIONS.md)'s `tui-physical-ios-app-death-undetected`). `supervise::mcp_backend` implements `frust-mcp`'s `SessionBackend` (fourteen methods, including the three `frust-dap`-only ones — `subscribe_session_events`, `fetch_widget_tree`, `project_root`) over this same supervise layer — see Embedded MCP Surface and Embedded DAP Surface below. `supervise::session_feeds` holds the two deferred-answer registries the first two of those need (`project_root` answers synchronously, from a field read, so it needs none): `SessionSubscribers` (live `SessionEventFeed` senders, fed by `crate::runner` replaying whatever a `Message::Session` transition appended) and `PendingWidgetTrees` (in-flight widget-tree pulls awaiting the devtools bridge's own async reply). `supervise::dap_server` is the DAP counterpart of `mcp_backend`'s server-handle half: `DapServerHandle`/`DapStatus`, deliberately shaped identically to `McpServerHandle`/`McpStatus` |
| `ui` | Render-only layer: paints `AppState` into `ratatui` frames and registers this frame's clickable/hoverable regions; never mutates engine state. `ui::views::sessions` renders the log pane from the engine's `SessionView` visible-sequence rather than re-deriving line membership. `ui::anim` holds pure animation primitives (braille spinner, shimmer sweep) themed via `Theme`. `ui::views::devtools` renders the DevTools chrome: the four-tab strip and its five connection-state screens (workbook §B12). `ui::views::mcp` renders the MCP panel: server status, the connected-client list, and a start/stop action (workbook §B13). `ui::views::dap_settings` renders the DAP settings dialog: server status/toggle, port field, auto-start/auto-configure checkboxes, IDE selector, and the last generate-config result |
| `runner` | Terminal lifecycle owner: `run()` first refuses a non-interactive terminal (stdin and stdout must both be TTYs) with a clean error, then owns raw-mode/panic-hook setup and the async event loop translating raw input into `Message`s and enacting `Effect`s |
| `lib.rs` | Public entry point (`run()`), reached only through `frust-cli` — no standalone binary |

## Layer Dependencies

`frust-drive` supplies process running/streaming, device discovery, the doctor report, the
Android/iOS build/run pipelines, and the plugin registry (see
[CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md)). `frust-mcp` and `frust-dap` are TUI's second and third
tooling-crate dependencies: `frust-mcp`'s `SessionBackend` trait and `serve_embedded` entry point
are what the embedded MCP server (below) is built from, and `frust-dap`'s `serve_embedded` is the
DAP server's direct counterpart, driven over that same `SessionBackend` implementation — two
tooling-to-tooling crossings, not framework ones, sanctioned by
[ARCHITECTURE.md](ARCHITECTURE.md)'s Cross-Unit Layer Dependencies. `ratatui` renders the terminal
UI, `crossterm` reads raw input and mouse events (terminal mouse modes are unchanged by anything
below), and `ansi-to-tui` renders ANSI-coded log output. `arboard` (workspace pin `=3.6.1`, owned by
the PLUGINS unit — see [PLUGINS_DEVELOPMENT.md](PLUGINS_DEVELOPMENT.md)) is `crate::clipboard`'s
system-clipboard backend, held as a per-write instance rather than a long-lived handle.
`tokio` and `futures-util` drive the async event loop, channels, and stream combinators (`tokio`'s
`net` feature is enabled for both embedded servers' own loopback listeners); `tokio-util` supplies
the `CancellationToken` each server shuts down on; `toml_edit` gives format-preserving persistence
for the recent-projects store and the `[dap]` preferences table; `anyhow`/`thiserror` cover error
handling.

Internally, TUI enforces a strict TEA layering as a hard contract rather than convention: `engine`
is pure and terminal-free; `ui` only renders and registers mouse regions and never mutates state;
`runner` is the sole place raw events become `Message`s and `Effect`s get enacted. `update()` pushes
all impure work — I/O, spawning, clipboard access — out to the runner as `Effect`s rather than
performing it inline. The one deliberate exception: a `Message::Mcp` (an embedded MCP server's
question, carrying its own reply channel) is intercepted by `runner` **before** it reaches
`update()` and served directly against the live `Supervisor` — `update()`'s own `Mcp` arm is a
documented no-op, since answering needs supervisor/launch-record state the pure core cannot reach.
The TUI's `Theme` is its own brand palette, entirely independent of the
framework's `frust-theme` (see [WIDGETS_ARCHITECTURE.md](WIDGETS_ARCHITECTURE.md)) — TUI paints
itself, not a Frust app.

## Embedded MCP Surface

`supervise::mcp_backend::TuiSessionBackend` implements `frust-mcp`'s `SessionBackend` over the
workbench's own `Supervisor` — one session world, two front ends: an MCP agent drives the exact
sessions the user sees, not a second headless engine. Each trait method posts an `McpCommand` onto
the engine's channel and blocks on a reply the runner sends back (a bounded 5s deadline degrades to
a typed `WorkbenchUnreachable` error rather than hanging an agent). Shapes with no honest workbench
equivalent are reported as explicit absences, never invented values: a session's `devtools_client`
is always `None` (the connection lives inside `DevtoolsBridge`'s own thread, so a tool needing the
client handle directly gets a typed refusal — the workbench owns that socket). `fetch_widget_tree`
is the one exception: it *is* answerable in embedded mode, by routing the pull through the
workbench's own bridge instead (`PendingWidgetTrees`, above) rather than exposing the client itself
— so both an MCP agent's `widget_tree` tool and a DAP client's `frustWidgetTree` request work
despite `devtools_client` reporting none. Ad-hoc and physical-iOS sessions are
omitted from the MCP session list (no `RunTarget` variant fits them); net metrics stay `None`.
MCP-launched session records are bounded at `MCP_RECORD_CAP` (mirroring `frust-mcp`'s own
`TERMINAL_SESSION_CAP`, 32), oldest-terminal evicted first and a live session never evicted.
`run_app` checks the launch guard (below) first: a live session already on this project root and
target — started by hand or by a previous agent call — refuses with a typed
`EmbeddedError::AlreadyRunning`, checked *before* the `MCP_RECORD_CAP` cap; only once that passes
does the cap itself refuse with `EmbeddedError::TooManySessions`. Neither refusal runs any
bookkeeping, so a refusal can never itself grow `AppState::sessions` (see
[LIMITATIONS.md](LIMITATIONS.md)); `restart_app` exempts only the one session it is replacing from
both checks, so a 1-for-1 relaunch is never refused by its own predecessor while any other live
session on that target still blocks it.

`AppState::mcp` holds the running server's handle (`None` = stopped); `mcp_panel_open`/`mcp_error`
back the MCP panel (§B13) and its retained failure reason. `Engine::start_mcp`/`stop_mcp` bind or
cancel a `frust_mcp::serve_embedded` task against a `ClientRegistry`; `Effect::StartMcpServer`/
`StopMcpServer` are how `update()` requests that (the handle is a live resource the pure core
cannot construct). Each `start_mcp` mints a monotonic **generation**, stamped on the handle and on
both of the server's reports (`McpListening`/`McpStopped`); `update()` applies a report only when
its generation names the currently-installed server, since a stopping server's task outlives the
handle drop (dropping its `CancellationToken` does not itself cancel an already-scheduled task) and
an ungated late report could otherwise clobber a successor. The MCP panel and the sidebar ACTIONS
row (`M` toggles the server; `m` opens the panel; `s` inside the panel toggles it) read the
connected-client list live off the registry at render time — nothing messages the engine when a
client connects or disconnects — so `AppState::animating()` keeps the redraw tick alive while the
panel is open.

## Embedded DAP Surface

`frust_dap::serve_embedded` runs over the **same** `TuiSessionBackend` and the **same**
`Supervisor` the embedded MCP server drives — one shared backend, handed to both — so a debug
session an IDE launches and a session the user launches by hand are indistinguishable to the
workbench once running, and stopping one from either side stops the app, never the other front
end. Two of `frust-dap`'s three `SessionBackend`-only methods are what make this possible without a
second supervision layer (the third, `project_root`, is a live field read needing none):
`subscribe_session_events` is served from `supervise::session_feeds`'
`SessionSubscribers` (seeded from a session's retained log, then kept live as `crate::runner`
replays each `Message::Session` transition's new lines and, on the session's end, its terminal
state as a `SessionEvent::Exited`); `fetch_widget_tree` is served **live** through the workbench's
own `DevtoolsBridge` connection via `PendingWidgetTrees`, with the bridge's own 3s per-request
timeout comfortably inside `mcp_backend`'s 5s `REPLY_DEADLINE` so a live pull always lands before
the caller's own deadline would fire.

`Engine::start_dap`/`stop_dap` and `AppState::dap` (`DapServerHandle`/`DapStatus`) mirror
`start_mcp`/`stop_mcp`/`AppState::mcp` exactly, including the monotonic **generation** tag
(`next_dap_generation`, a separate counter from MCP's) that gates `DapListening`/`DapStopped`
reports against a stopping server's late arrival, and the client registry
(`frust_dap::DapClientRegistry`) the DAP settings dialog and sidebar read live at render time.
`ActiveModal::DapSettings` (key `D`; `s` inside the dialog toggles the server, `g` generates the
IDE config, `Esc` closes) is backed by `engine::dap_settings::DapSettings` — loaded once at
startup from the persisted `[dap]` table (`enabled`, `auto_start_in_ide`, `auto_configure_ide`,
`port`, `ide_override`, `intro_seen`) in `~/.config/frust/tui.toml`, not created and dropped with
the dialog, so two effects fire whether or not anyone ever opens it: at startup,
`Message::DapAutoStart` starts the server when
`enabled || (auto_start_in_ide && a parent IDE is detected)`; on every `DapListening` report whose
bound port actually changed, `auto_configure_ide` (default on) writes the detected/overridden IDE's
DAP launch config via `frust_dap::ide_config` — an implicit, config-file-writing side effect of a
successful server start, not only something the dialog's `g` triggers by hand. That auto-start is
**gated once per install**: the first `DapAutoStart` that would otherwise have bound silently
instead opens this dialog carrying a one-time notice (`DapSettings::intro_port`/`intro_notice`)
naming the port, starts nothing itself, and persists `intro_seen = true` immediately — so the
notice is spent even if the user quits without acting, and on that run only the dialog's own Start
action binds a listener. Every later launch is the silent auto-start the defaults ask for.
`intro_seen` is burned only when the gate actually fires, so an install whose early launches are
outside an IDE still gets the notice on its first launch inside one (see
[LIMITATIONS.md](LIMITATIONS.md)'s `dap-tcp-unauthenticated-v1` for why this asymmetry with MCP —
which has no auto-start path at all — is disclosed rather than removed). `start_dap` refuses (a
retained reason, no server started) when no project is open in the workbench, since a debug launch
needs a directory to build from. Once running, the server needs no restart across a project
switch: `TuiSessionBackend::project_root` answers live from the workbench's current state, so each
launch builds from whichever project is open *at that moment*, not whichever was open when the
server started.

## Data Flow

- Terminal input (`crossterm`) becomes a `Message`; `engine::update` runs a pure state transition
  returning `Outcome`/`Effect`; the runner enacts `Effect`s (launch/kill session, clipboard, ad-hoc
  build/clean) and `ui::render` repaints, re-registering mouse regions for the next input event.
- Session lifecycle: `Supervisor` spawns a session via `frust-drive` (desktop `cargo run` — its
  plan resolved by `frust_drive::desktop_run`, the same construction site `frust-cli run` and
  `frust-mcp` use — or Android/iOS build → install → launch → logcat), streaming output back as
  `SessionState` transitions and log messages the engine consumes.
- Launch guard: one live (non-terminal) session per `(project root, target)` — `AppState::live_session_for`
  (`live_session_for_excluding` for a restart, which exempts the session being replaced) is
  consulted by the run-config modal, run-on-all-devices, and the embedded MCP/DAP backend alike, so
  a user-started launch and an agent-started one refuse each other symmetrically. A refusal is a
  Warn toast ('<name>: already running here — stop it first') in the workbench and a typed
  `EmbeddedError::AlreadyRunning` over MCP/DAP; ad-hoc (targetless: build/clean/bootstrap-fix) and
  terminal sessions never occupy a target, and the guard is blind to build mode/flavor — only
  project root and target identity matter.
- Quit path: `Message::RequestQuit` (global `q`, the DevTools-pane `q`, Ctrl+C with no running
  session, or the palette's Quit) checks `AppState::live_session_count()`: zero live sessions quits
  immediately, otherwise `AppState.quit_confirm` opens a dialog warning that N running session(s)
  will be force-stopped; `Enter`/`y`/its Quit button confirm, `Esc`/`n`/Cancel close it.
  `Message::Quit` remains Ctrl+Q's deliberate bypass, routed before any modal so it still works with
  the dialog already open.
- Closing a tab: `Message::CloseTab(usize)`/`CloseActiveTab` (keyboard `X`; context menu 'Close
  tab' when terminal or 'Stop & close' when live; palette 'Close tab') removes a terminal session's
  view immediately; a live one is marked `close_on_exit` and stopped like `StopSession`, then
  removed once its terminal-state bookkeeping (toast, DevTools disconnect, metrics stop) has run.
  Index repair on removal: the active tab if it was the removed one moves to whatever now sits at
  that index, else the previous index, else none; an index before the active one shifts the active
  index down by one; an index after it leaves the active index unchanged. A context menu targeting
  the closed tab is cleared; the supervisor's session map is not pruned.
- Build sessions: a picked `ArtifactKind` becomes a `BuildTargetSpec`; a desktop bundle target runs
  `frust_drive::desktop_build::build` under `spawn_blocking`, with each returned `BundleNote`
  (missing/undersized icon, generated `Info.plist`, unsigned bundle, …) appended as a `note:` log
  line rather than printed — the same pattern Android/iOS build sessions use.
- Follow-tail is per-session, never a persisted default: every session starts following its log
  tail; scrolling up disengages follow for that session only; scrolling to bottom or `f`
  re-engages it. No global setting is written to disk. Entering line-selection mode (below) also
  pauses follow-tail, restoring it on exit only when it was on at entry and the cursor is still at
  the tail.
- Log line-selection mode: `v` (`Message::SelectEnter`) enters a per-session mode, anchored at the
  newest visible line, that pauses follow-tail and swaps the runner's whole key namespace to a
  dedicated table: `↑`/`k`/`Shift+↑` and `↓`/`j`/`Shift+↓` move the range cursor one line
  (`Message::SelectMove`), `PageUp`/`PageDown` move it a page (`Message::SelectPage`), `Home`/`End`
  jump to the ends (`Message::SelectHome`/`SelectEnd`), `y` copies the range from the redacted log
  store with an Info toast and exits the mode (`Message::CopySelection`), and `Esc`/`v` exit without
  copying (`Message::SelectExit`). Every other key is swallowed — a modal- or session-mutating key
  pressed mid-selection must not fire behind the highlight — except the global `Ctrl+Q`/`Alt+m`
  chords and an already-open menu/modal/search, which keep their usual precedence; the mouse wheel
  still scrolls the view. The engine has no viewport height of its own, so the cursor is kept on
  screen against a fixed row window rather than a real scroll extent. Every drawn log row also
  registers a per-row `RegionId::LogRow(abs)` region firing `Message::LogRowClicked(abs)`: the first
  click after entering the mode anchors the range, a later click moves its end — what a click means
  is decided in `update()`, not by the region itself. The status bar shows an accent SELECT
  indicator while the mode is active (degrading by segment below roughly 120 columns). Terminal
  mouse modes are unaffected; `Alt+m` still suspends mouse capture as usual.
- Copy affordances: besides selection-mode `y`, a row-carrying `ContextTarget::LogView { row }`
  backs a right-click 'Copy line' entry (`Message::CopyLine(abs)`) that copies one line from the
  redacted store with an Info toast (a preview truncated to 60 characters) or a Warn toast if the
  line has since been evicted from the ring.
- Clipboard backend: `crate::clipboard` picks one of System (OS clipboard via `arboard`), OSC 52
  (DCS-chunked under GNU screen), or Disabled, from the `FRUST_TUI_CLIPBOARD` environment variable
  (`system`/`osc52`/`off`; unset or unrecognised is Auto-detected from SSH/display/TTY signals) —
  read once at startup and held for the run. A `Disabled` backend shows a one-time startup Warn
  toast naming the reason; any write failure shows a 'Copy failed: <reason>' toast via the pure
  `Message::Notify { level, text }`, which the runner turns into a rendered toast without `update()`
  itself performing any I/O.
- Build-phase labels: while a session is `Building`/`Installing`, the supervisor's drain path runs
  `supervise::progress` over the streamed lines and emits a `SessionEventKind::Phase`; the engine
  stores the latest label as `current_phase` on the session view and clears it on any state
  transition out of that transient window (the sole clearing point, order-independent). The `ui`
  layer renders it as an animated spinner tab glyph plus a shimmered status label.
- Log styling: `engine::logstyle` classifies each pushed line once (level, source, and Rust
  panic/backtrace fold-block role) into per-line metadata alongside the line store. Rendering
  applies chrome from that metadata (level badges, timestamps, source tags), folds panic
  backtraces into a collapsible group keyed by absolute line index (so folds survive ring
  eviction), and a level filter narrows the visible line sequence — follow-tail tracks the tail of
  that visible sequence, and every scroll step (arrows, PageUp/PageDown, wheel, scrollbar drag)
  moves through that same sequence, with a collapsed panic block counting as one step. Lines
  already carrying their own ANSI color pass through unstyled.
- Bootstrap/doctor: `frust-drive`'s doctor report drives a wizard in engine state; auto-runnable
  fixes launch as supervised sessions and re-preflight on exit.
- Perf: session log lines are scanned for `frust-perf` trace output into a per-session sparkline
  panel.
- Add-plugin: a guided dialog drives `frust-drive`'s plugin registry synchronously or via an ad-hoc
  session (see [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md) and
  [PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md)).
- DevTools: a discovery line captured at `SessionView::push_line` drives `engine::devtools`'s
  five-phase connection through a connect `Effect`; the ring copy of that line is stored with its
  handshake token redacted (`SessionView::push_line_at`), so neither the log view, MCP's `app_logs`
  tool, nor a DAP client's `output` events can read it back out — all three read the same redacted
  store (see [DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md)).
  `supervise::DevtoolsBridge` performs the token
  handshake, coalesces frame stats, and serves on-demand widget-tree/props pulls, reporting back as
  `ConnEvent`s the engine mirrors into ring/tab state for `ui::views::devtools` to render.
  `supervise::MetricsBridge` runs in parallel (Android sessions only in v1), feeding the System/
  Network tabs' rings the same coalesced-batch way. Both bridges are retained for the session's
  full life regardless of whether DevTools is open (see [DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md)).

## Key Types

| Type | Purpose |
|------|---------|
| `AppState` / `Message` / `Outcome` / `Effect` | The TEA model, message vocabulary, and `update()`'s return type separating pure state mutation from side effects |
| `Engine` | Owns `AppState` plus the channel every async producer sends `Message`s into; the sole mutation entry point |
| `SessionSpec` / `SessionState` / `Supervisor` | Session vocabulary and the supervisor owning one thread per session, bridging process output into engine messages |
| `SessionTarget` | A session's launch identity (`Desktop` \| `Device { id, name, platform }`), distinct from its display label; devices compare by id alone. Backs the launch guard's `(project root, target)` key and `SessionView.target`/`Message::RegisterSession.target` |
| `ActiveModal` | Exhaustively-matched enum selecting which modal/overlay is on top, shared by render dispatch and key routing so an unhandled new variant fails to compile. `QuitConfirm` shares CleanConfirm's priority tier, rendering over either top-level screen |
| `MouseCtx` / `RegionId` | Per-frame clickable/hoverable region registry translated into `Message`s on the next input event; `RegionId::LogRow(abs)` is one such region per drawn log row, and the row-carrying `ContextTarget::LogView { row }` lets the context menu know which row (if any) was right-clicked |
| `Theme` / `Palette` | The TUI's own brand palette (independent of `frust-theme`), and the fuzzy command palette whose registry (`engine::palette::commands`) also drives the help overlay. A runner test tripwires registry drift: it translates every single-printable-char hint through the real key-routing path under a state satisfying that entry's gate and asserts the resulting message matches the registry's own (documented exceptions: multi-key hints, the context-dependent `d` pair, and `y`, each asserted separately). The help overlay renders key-less (mouse/palette-only) entries with a '—' hint and a legend row explaining it |
| `PhaseLabel` | A parsed, displayable build/install/launch phase (`supervise::progress`); `None` when the streamed output doesn't match a recognized shape |
| `LogLevel` / `LineMeta` / `LevelFilter` | Per-line log classification (`engine::logstyle`) — level/source/fold-role metadata computed once at push, and the filter narrowing the visible line sequence |
| `DevtoolsState` | Per-session DevTools view-model (`engine::devtools`): five-phase connection (`ConnState`), active tab, and each tab's own sub-state — `PerformanceTab` (frame focus/scrub), `InspectorTab` (tree/selection), `MetricsState`/`SamplingState`/`MetricsIdentity` (System/Network's sampler status and rings) |
| `TuiSessionBackend` | `supervise::mcp_backend`'s `frust_mcp::SessionBackend` implementation over the workbench's own `Supervisor` — the one seam both the embedded MCP server and the embedded DAP server drive the workbench through |
| `McpServerHandle` / `McpStatus` | The running embedded server's cancel/registry handle (`AppState::mcp`), carrying the generation its `McpListening`/`McpStopped` reports are gated against, and the status (`Stopped`/`Starting`/`Listening{port, clients}`) it and the panel read; a `Stopped` status paired with `AppState::mcp_error` is the panel's failed state |
| `DapServerHandle` / `DapStatus` | The DAP server's own cancel/registry handle (`AppState::dap`) and status, shaped identically to `McpServerHandle`/`McpStatus` — same generation-gating discipline, same `Stopped`/`Starting`/`Listening{port, clients}` shape |
| `SessionSubscribers` / `PendingWidgetTrees` | `supervise::session_feeds`'s two deferred-answer registries backing `TuiSessionBackend`'s DAP-only methods: live `SessionEventFeed` senders replayed from each session transition, and in-flight `frustWidgetTree` pulls awaiting the devtools bridge's own reply |
| `DapSettings` / `DapFocus` / `DapPrefs` | The DAP settings dialog's model (`engine::dap_settings`) and its persisted backing (`engine::persist`'s `[dap]` table: `enabled`/`auto_start_in_ide`/`auto_configure_ide`/`port`/`ide_override`/`intro_seen`) — loaded once at startup, consulted for auto-start and auto-configure whether or not the dialog is ever opened; `intro_seen` is the one-time first-run auto-start notice's spent flag |
