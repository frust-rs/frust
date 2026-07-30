# Frust - CLI Architecture

## Overview

CLI is the standalone `frust` command-line tool and the drive library behind it. `frust-cli` is a
thin `clap` front-end with exactly one subcommand handler per command; `frust-drive` is the
framework-free library doing the actual work — scaffolding new Frust projects, validating the local
toolchain, discovering devices, and driving the Android/iOS run/build/clean pipelines. `frust-drive`
has no `clap` dependency so it can be shared as-is with `frust-tui`; neither crate depends on any
framework crate — this unit drives Frust apps, it does not consume the framework.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how CLI relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `frust-cli::commands` | One thin handler per subcommand; the sole construction site for the injected `RealProcessRunner` |
| `frust-drive::process` | The `ProcessRunner` trait plus `RealProcessRunner`/`FakeProcessRunner` — the sole seam for shelling out |
| `frust-drive::scaffold` | Manifest-driven template rendering that produces a new Frust project tree |
| `frust-drive::doctor` | Pluggable environment validators plus a structured, non-blocking toolchain report |
| `frust-drive::devices` | Pluggable per-platform device discovery, aggregated non-fatally |
| `frust-drive::build_info` | The debug/profile/release + flavor funnel shared by run and build |
| `frust-drive` Android/iOS pipelines | The four platform pipelines: compile → install → launch/stream |
| `frust-drive::plugin` | Static plugin registry plus the idempotent project-mutation engine that applies it |

## Layer Dependencies

`frust-cli` depends on `clap` for argument parsing and on `frust-drive` for every operation it
performs; it holds no toolchain-invocation logic of its own. `frust-drive` has zero dependency on
`clap` or any framework crate — this is a charter boundary, not incidental, since `frust-tui` links
the same library without pulling in a CLI parser or the render/widget stack. `frust-cli`'s `tui`
subcommand hands off entirely to `frust-tui`, whose own dependency is on `frust-drive` alone (see
[TUI_ARCHITECTURE.md](TUI_ARCHITECTURE.md)).

Within `frust-drive`, `anyhow` sits at the CLI/pipeline-core boundary while library-contract errors
use `thiserror` enums; `serde`/`serde_json`/`toml`/`toml_edit` handle manifest and build-report
serialization plus format-preserving `Cargo.toml` edits; `minijinja` and `include_dir` embed and
render the `templates/app/` tree at compile time. `notify` and `ctrlc` are `frust-cli`-only —
`run --watch`'s filesystem watcher and the Ctrl-C group-kill handler have no reason to live in the
shared library.

CLI carries several boundary facts as hard contracts rather than style: `frust-drive`'s
build/run/scaffold/plugin logic is print-free, threading an `on_line` sink or returning values, so
only the CLI and TUI front-ends ever print; every external tool invocation goes through the injected
`&dyn ProcessRunner`, with `commands::dispatch` as the single `Real*` construction site; scaffold
mutation is idempotent by contract (byte-identical or `AlreadyPresent`, never a silent partial
write); and streaming spawns always pipe stdout/stderr rather than inheriting the terminal, so raw
child output can't garble a caller's raw-mode terminal (relevant to `frust-tui`).

## Data Flow

- `Cli` (`clap`) parses `argv` into a `Command`; `commands::dispatch` builds the one
  `RealProcessRunner` and injects it into every handler — no handler ever shells out directly.
- `create`: CLI args convert into `frust-drive::scaffold::generate`, which renders the embedded
  `templates/app/` tree against a `TemplateContext` — a pure file-write, no `ProcessRunner`
  involved.
- `doctor`/`devices`: `dispatch` runs `frust-drive`'s independent, non-fatal `Validator`/
  `DeviceDiscovery` sets through the injected runner; the CLI renders the resulting report.
- `run`/`build`: CLI args become a `BuildInfo`, which drives `frust-drive`'s Android/iOS pipelines
  (compile → install → launch/stream) through the same `ProcessRunner`; desktop falls back to a
  `cargo run` passthrough with an optional `--watch` loop.
- `tui`: `Command::Tui` hands off entirely to `frust-tui`'s own async runtime (see
  [TUI_ARCHITECTURE.md](TUI_ARCHITECTURE.md)).
- `plugin add`: `frust-drive::plugin::add_plugin` looks up a `PluginSpec` and applies its
  `Contribution`s as idempotent, format-preserving edits to a generated project (see
  [PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md) for the plugins this distributes).

## Key Types

| Type | Purpose |
|------|---------|
| `ProcessRunner` / `RealProcessRunner` / `FakeProcessRunner` | The seam every external tool invocation goes through, real or faked |
| `BuildInfo` / `BuildMode` | The debug/profile/release + flavor funnel shared by run and build |
| `Validator` / `DoctorReport` | The doctor subsystem's pluggable checks and its structured report |
| `DeviceDiscovery` / `Device` | Device discovery abstraction and its result shape |
| `TemplateContext` | Render/path substitution variables for `frust create`'s scaffold |
| `PluginSpec` / `Contribution` | A plugin registry entry and the idempotent project edits it applies |
| `Cli` / `Command` | The `clap`-derived argument surface for the `frust` binary |
