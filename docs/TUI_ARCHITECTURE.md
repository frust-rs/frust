# Frust - TUI Architecture

## Overview

TUI is `frust-tui`, a mouse-first `ratatui` TEA (The Elm Architecture) terminal workbench for
driving Frust app projects — scaffold/build/run/doctor/clean as supervised sessions across
desktop/Android/iOS. It is a library with no binary of its own, consumed by `frust-cli` (bare
`frust` in an interactive terminal, or the explicit `tui` subcommand); it depends on
`frust-drive` + `frust-devtools-protocol` + `frust-mcp`, with zero dependency on any framework
rendering crate, mirroring `frust-cli`'s tooling-isolation charter (see
[ARCHITECTURE.md](ARCHITECTURE.md)'s Cross-Unit Layer Dependencies — tooling may depend on the
`frust-mcp` tooling crate). The workbench can embed an MCP server over its own sessions, letting an
AI agent drive them the same way a human does through the terminal.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how TUI relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `engine` | Pure TEA core: `AppState` model, `Message` enum, `update()` pure transition returning `Outcome`/`Effect`; terminal-free and unit-testable without a TTY. `engine::logstyle` classifies each log line once at push into per-line metadata (level, source, panic-fold role) consumed only by rendering. `SessionView` (`visible_indices`/`bottom_pos`) owns the visible-sequence/scroll-anchor math, computed once and consumed as-is by both scroll input and rendering. `engine::devtools` holds each session's DevTools view-model (`DevtoolsState`: five-phase connection, active tab, per-tab sub-state) as plain data + pure transitions mirroring what the bridges below report |
| `supervise` | Session-supervision layer over `frust-drive`: drives per-session build/run lifecycles and bridges process output into engine messages. `supervise::progress` is a pure, side-effect-free build-phase-label extractor over streamed output lines, called from the drain path. `supervise::devtools_bridge`/`supervise::metrics_bridge` are DevTools' impure half, one thread per session each: the former owns a blocking devtools-protocol connection (token handshake, coalesced frame-stats, on-demand widget-tree/props pulls, `adb forward` on Android — see [DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md)); the latter is a local `frust-drive::metrics` sampler with no build-feature gate, started once a session's Android identity resolves rather than once a discovery line lands. A device session's `stop` additionally dispatches a best-effort OS-level app termination (`adb shell am force-stop` / `simctl terminate`) on a tracked detached thread the supervisor bounded-joins from `stop_all`/`Drop` — shared by the user's stop keypress and MCP's `stop_app`/`restart_app`. `supervise::mcp_backend` implements `frust-mcp`'s `SessionBackend` over this same supervise layer — see Embedded MCP Surface below |
| `ui` | Render-only layer: paints `AppState` into `ratatui` frames and registers this frame's clickable/hoverable regions; never mutates engine state. `ui::views::sessions` renders the log pane from the engine's `SessionView` visible-sequence rather than re-deriving line membership. `ui::anim` holds pure animation primitives (braille spinner, shimmer sweep) themed via `Theme`. `ui::views::devtools` renders the DevTools chrome: the four-tab strip and its five connection-state screens (workbook §B12). `ui::views::mcp` renders the MCP panel: server status, the connected-client list, and a start/stop action (workbook §B13) |
| `runner` | Terminal lifecycle owner: `run()` first refuses a non-interactive terminal (stdin and stdout must both be TTYs) with a clean error, then owns raw-mode/panic-hook setup and the async event loop translating raw input into `Message`s and enacting `Effect`s |
| `lib.rs` | Public entry point (`run()`), reached only through `frust-cli` — no standalone binary |

## Layer Dependencies

`frust-drive` supplies process running/streaming, device discovery, the doctor report, the
Android/iOS build/run pipelines, and the plugin registry (see
[CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md)). `frust-mcp` is TUI's second tooling-crate dependency:
its `SessionBackend` trait and `serve_embedded` entry point are what the embedded MCP server (below)
is built from — a tooling-to-tooling crossing, not a framework one, sanctioned by
[ARCHITECTURE.md](ARCHITECTURE.md)'s Cross-Unit Layer Dependencies. `ratatui` renders the terminal
UI, `crossterm` reads raw input and mouse events, and `ansi-to-tui` renders ANSI-coded log output.
`tokio` and `futures-util` drive the async event loop, channels, and stream combinators (`tokio`'s
`net` feature is enabled for the embedded server's own loopback listener); `tokio-util` supplies the
`CancellationToken` the server shuts down on; `toml_edit` gives format-preserving persistence for
the recent-projects store; `anyhow`/`thiserror` cover error handling.

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
is always `None` (the connection lives inside `DevtoolsBridge`'s own thread, so driving/inspection
tools get a typed refusal — the workbench owns that socket); ad-hoc and physical-iOS sessions are
omitted from the MCP session list (no `RunTarget` variant fits them); net metrics stay `None`.
MCP-launched session records are bounded at `MCP_RECORD_CAP` (mirroring `frust-mcp`'s own
`TERMINAL_SESSION_CAP`, 32), oldest-terminal evicted first and a live session never evicted;
`run_app`/`restart_app` refuse once the cap of live MCP sessions is reached with a typed
`EmbeddedError::TooManySessions`, before any bookkeeping runs, so a refusal can never itself grow
`AppState::sessions` (see [LIMITATIONS.md](LIMITATIONS.md)).

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

## Data Flow

- Terminal input (`crossterm`) becomes a `Message`; `engine::update` runs a pure state transition
  returning `Outcome`/`Effect`; the runner enacts `Effect`s (launch/kill session, clipboard, ad-hoc
  build/clean) and `ui::render` repaints, re-registering mouse regions for the next input event.
- Session lifecycle: `Supervisor` spawns a session via `frust-drive` (desktop `cargo run`, or
  Android/iOS build → install → launch → logcat), streaming output back as `SessionState`
  transitions and log messages the engine consumes.
- Follow-tail is per-session, never a persisted default: every session starts following its log
  tail; scrolling up disengages follow for that session only; scrolling to bottom or `f`
  re-engages it. No global setting is written to disk.
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
  handshake token redacted (`SessionView::push_line_at`), so neither the log view nor MCP's
  `app_logs` tool can read it back out (see [DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md)).
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
| `ActiveModal` | Exhaustively-matched enum selecting which modal/overlay is on top, shared by render dispatch and key routing so an unhandled new variant fails to compile |
| `MouseCtx` / `RegionId` | Per-frame clickable/hoverable region registry translated into `Message`s on the next input event |
| `Theme` / `Palette` | The TUI's own brand palette (independent of `frust-theme`), and the fuzzy command palette whose registry also drives the help overlay |
| `PhaseLabel` | A parsed, displayable build/install/launch phase (`supervise::progress`); `None` when the streamed output doesn't match a recognized shape |
| `LogLevel` / `LineMeta` / `LevelFilter` | Per-line log classification (`engine::logstyle`) — level/source/fold-role metadata computed once at push, and the filter narrowing the visible line sequence |
| `DevtoolsState` | Per-session DevTools view-model (`engine::devtools`): five-phase connection (`ConnState`), active tab, and each tab's own sub-state — `PerformanceTab` (frame focus/scrub), `InspectorTab` (tree/selection), `MetricsState`/`SamplingState`/`MetricsIdentity` (System/Network's sampler status and rings) |
| `TuiSessionBackend` | `supervise::mcp_backend`'s `frust_mcp::SessionBackend` implementation over the workbench's own `Supervisor` — the embedded MCP server's one seam into the workbench |
| `McpServerHandle` / `McpStatus` | The running embedded server's cancel/registry handle (`AppState::mcp`), carrying the generation its `McpListening`/`McpStopped` reports are gated against, and the status (`Stopped`/`Starting`/`Listening{port, clients}`) it and the panel read; a `Stopped` status paired with `AppState::mcp_error` is the panel's failed state |
