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
| `frust-drive::manifest` | The shared `frust.toml` reader (`[app]`/`[android]`/`[ios]`/`[signing]`), replacing the duplicate deserialisers the Android/iOS run pipelines used to each carry |
| `frust-drive::scaffold` | Manifest-driven template rendering that produces a new Frust project tree |
| `frust-drive::doctor` | Pluggable environment validators plus a structured, non-blocking toolchain report |
| `frust-drive::devices` | Pluggable per-platform device discovery, aggregated non-fatally |
| `frust-drive::build_info` | The debug/profile/release + flavor funnel shared by run and build |
| `frust-drive` Android/iOS pipelines | The four platform pipelines: compile → install → launch/stream |
| `frust-drive::plugin` | Static plugin registry plus the idempotent project-mutation engine that applies it |
| `frust-drive::interrupt` | The process-wide SIGINT/SIGTERM/SIGHUP + panic-hook owner; scrubs registered secret files before the process dies |

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
render the `templates/app/` tree at compile time. `notify` is `frust-cli`-only — `run --watch`'s
filesystem watcher has no reason to live in the shared library. `ctrlc` is a `frust-drive`
dependency (`frust-cli` also links it directly for its own `--watch` group-kill handler);
`frust-drive::interrupt` is the process's single SIGINT/SIGTERM/SIGHUP owner (see Data Flow below),
and a second `ctrlc::set_handler` anywhere in the same process is a hard error by design.

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
- `build --release` (Android): `android_build::signing` resolves the four release-signing values
  (`storeFile`/`storePassword`/`keyAlias`/`keyPassword`) **once**, from the properties file named by
  `frust.toml`'s `[signing]` section (default `android/key.properties`, optionally key-prefixed) with
  `[signing.env]`-named env-var fallbacks (blank counts as absent; the four `ANDROID_*` names apply
  when `[signing.env]` is absent), confirming the resolved `storeFile` lands on a real file. It does
  not parse `build.gradle.kts` — instead the pipeline hands Gradle exactly what it resolved:
  `write_resolved` serialises the same material to `android/.frust-signing.properties` (unprefixed
  keys, absolute `storeFile`, owner-only), and the generated Gradle template reads that file *first*.
  Gate and artifact see the identical values by construction, not by a predicate that can drift.
  The file exists only for the Gradle invocation — a `Drop` guard removes it on return, and
  `frust-drive::interrupt` (the process's single SIGINT/SIGTERM/SIGHUP + panic-hook owner) scrubs it
  on a signal or an abort-panic release build too, so the plaintext passwords never outlive the build
  that needed them.
  **Backstop:** the by-construction guarantee has one precondition nothing can check up front — that
  the project's `build.gradle.kts` actually contains the generated-file read. A project whose
  template predates it (there is no `frust upgrade`) falls through to Gradle's debug signing config
  and exits 0. Both release pipelines (`android_build::build_with_env` and
  `android_run::prepare_session`, so `frust run --release` refuses before install) grep the captured
  Gradle output of a *successful* build for the template's fallback marker
  (`FRUST-SIGNING-FALLBACK`, or the legacy prose `release build is debug-signed` that every
  template Frust has shipped carries, so the installed base is covered too) and hard-fail — no
  `build.gradle.kts` parsing, keyed only on what Gradle actually reported it did. Renaming the
  template's warning without moving the matcher breaks this contract.
  `[signing] external = true` waives both the gate and the backstop for signing Frust cannot inspect
  (CI, a Gradle signing plugin) and warns on every release build instead of promising a signature it
  can't verify. `key.properties` + the four `ANDROID_*` variables remain as a fallback for a hand-run
  `./gradlew` (e.g. from Android Studio) — that path carries no Frust promise.
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
| `Manifest` / `SigningSection` / `SigningEnv` | Parsed `frust.toml` shape (`[app]`/`[android]`/`[ios]`/`[signing]`/`[signing.env]`) shared by every pipeline that reads the manifest |
| `ResolvedSigning` / `GeneratedProperties` | The signing gate's one resolved-material value, and the owner-only generated-properties guard that writes/deletes it around a Gradle invocation |
| `Cli` / `Command` | The `clap`-derived argument surface for the `frust` binary |
