# Frust - TUI Architecture

## Overview

TUI is `frust-tui`, a mouse-first `ratatui` TEA (The Elm Architecture) terminal workbench for
driving Frust app projects — scaffold/build/run/doctor/clean as supervised sessions across
desktop/Android/iOS. It is a library with no binary of its own, consumed by `frust-cli` (bare
`frust` in an interactive terminal, or the explicit `tui` subcommand); it is built only on
`frust-drive`, with zero dependency on any framework rendering crate, mirroring `frust-cli`'s
tooling-isolation charter.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how TUI relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `engine` | Pure TEA core: `AppState` model, `Message` enum, `update()` pure transition returning `Outcome`/`Effect`; terminal-free and unit-testable without a TTY. `engine::logstyle` classifies each log line once at push into per-line metadata (level, source, panic-fold role) consumed only by rendering. `SessionView` (`visible_indices`/`bottom_pos`) owns the visible-sequence/scroll-anchor math, computed once and consumed as-is by both scroll input and rendering. `engine::devtools` holds each session's DevTools view-model (`DevtoolsState`: five-phase connection, active tab, per-tab sub-state) as plain data + pure transitions mirroring what the bridges below report |
| `supervise` | Session-supervision layer over `frust-drive`: drives per-session build/run lifecycles and bridges process output into engine messages. `supervise::progress` is a pure, side-effect-free build-phase-label extractor over streamed output lines, called from the drain path. `supervise::devtools_bridge`/`supervise::metrics_bridge` are DevTools' impure half, one thread per session each: the former owns a blocking devtools-protocol connection (token handshake, coalesced frame-stats, on-demand widget-tree/props pulls, `adb forward` on Android — see [DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md)); the latter is a local `frust-drive::metrics` sampler with no build-feature gate, started once a session's Android identity resolves rather than once a discovery line lands |
| `ui` | Render-only layer: paints `AppState` into `ratatui` frames and registers this frame's clickable/hoverable regions; never mutates engine state. `ui::views::sessions` renders the log pane from the engine's `SessionView` visible-sequence rather than re-deriving line membership. `ui::anim` holds pure animation primitives (braille spinner, shimmer sweep) themed via `Theme`. `ui::views::devtools` renders the DevTools chrome: the four-tab strip and its five connection-state screens (workbook §B12) |
| `runner` | Terminal lifecycle owner: `run()` first refuses a non-interactive terminal (stdin and stdout must both be TTYs) with a clean error, then owns raw-mode/panic-hook setup and the async event loop translating raw input into `Message`s and enacting `Effect`s |
| `lib.rs` | Public entry point (`run()`), reached only through `frust-cli` — no standalone binary |

## Layer Dependencies

`frust-drive` is TUI's sole framework-adjacent dependency, supplying process running/streaming,
device discovery, the doctor report, the Android/iOS build/run pipelines, and the plugin registry
(see [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md)). `ratatui` renders the terminal UI, `crossterm`
reads raw input and mouse events, and `ansi-to-tui` renders ANSI-coded log output. `tokio` and
`futures-util` drive the async event loop, channels, and stream combinators; `toml_edit` gives
format-preserving persistence for the recent-projects store; `anyhow`/`thiserror` cover error
handling.

Internally, TUI enforces a strict TEA layering as a hard contract rather than convention: `engine`
is pure and terminal-free; `ui` only renders and registers mouse regions and never mutates state;
`runner` is the sole place raw events become `Message`s and `Effect`s get enacted. `update()` pushes
all impure work — I/O, spawning, clipboard access — out to the runner as `Effect`s rather than
performing it inline. The TUI's `Theme` is its own brand palette, entirely independent of the
framework's `frust-theme` (see [WIDGETS_ARCHITECTURE.md](WIDGETS_ARCHITECTURE.md)) — TUI paints
itself, not a Frust app.

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
  five-phase connection through a connect `Effect`; `supervise::DevtoolsBridge` performs the token
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
