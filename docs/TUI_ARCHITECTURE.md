# Frust - TUI Architecture

## Overview

TUI is `frust-tui`, a standalone, mouse-first `ratatui` TEA (The Elm Architecture) terminal
workbench for driving Frust app projects — scaffold/build/run/doctor/clean as supervised sessions
across desktop/Android/iOS. It is built only on `frust-drive`, with zero dependency on any framework
rendering crate, mirroring `frust-cli`'s tooling-isolation charter.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how TUI relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `engine` | Pure TEA core: `AppState` model, `Message` enum, `update()` pure transition returning `Outcome`/`Effect`; terminal-free and unit-testable without a TTY |
| `supervise` | Session-supervision layer over `frust-drive`: drives per-session build/run lifecycles and bridges process output into engine messages |
| `ui` | Render-only layer: paints `AppState` into `ratatui` frames and registers this frame's clickable/hoverable regions; never mutates engine state |
| `runner` | Terminal lifecycle owner: raw-mode/panic-hook setup and the async event loop translating raw input into `Message`s and enacting `Effect`s |
| `lib.rs` / `main.rs` | Public entry point (`run()`) shared by the standalone `frust-tui` binary and `frust-cli`'s `tui` subcommand |

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
- Bootstrap/doctor: `frust-drive`'s doctor report drives a wizard in engine state; auto-runnable
  fixes launch as supervised sessions and re-preflight on exit.
- Perf: session log lines are scanned for `frust-perf` trace output into a per-session sparkline
  panel.
- Add-plugin: a guided dialog drives `frust-drive`'s plugin registry synchronously or via an ad-hoc
  session (see [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md) and
  [PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md)).

## Key Types

| Type | Purpose |
|------|---------|
| `AppState` / `Message` / `Outcome` / `Effect` | The TEA model, message vocabulary, and `update()`'s return type separating pure state mutation from side effects |
| `Engine` | Owns `AppState` plus the channel every async producer sends `Message`s into; the sole mutation entry point |
| `SessionSpec` / `SessionState` / `Supervisor` | Session vocabulary and the supervisor owning one thread per session, bridging process output into engine messages |
| `ActiveModal` | Exhaustively-matched enum selecting which modal/overlay is on top, shared by render dispatch and key routing so an unhandled new variant fails to compile |
| `MouseCtx` / `RegionId` | Per-frame clickable/hoverable region registry translated into `Message`s on the next input event |
| `Theme` / `Palette` | The TUI's own brand palette (independent of `frust-theme`), and the fuzzy command palette whose registry also drives the help overlay |
