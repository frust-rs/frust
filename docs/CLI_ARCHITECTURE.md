# Frust - CLI Architecture

## Overview

CLI is the standalone `frust` command-line tool, the drive library behind it, and the MCP tooling
crate that exposes the same driving/diagnosis surface to AI agents. `frust-cli` ships the single
`frust` binary, which is also the TUI's front door: bare `frust` in an interactive terminal opens
the TUI workbench, and the explicit `tui` subcommand does the same. `frust-cli` is a thin `clap`
front-end with exactly one subcommand handler per command; `frust-drive` is the framework-free
library doing the actual work — scaffolding new Frust projects, validating the local toolchain,
discovering devices, and driving the Android/iOS run/build/clean pipelines. `frust-drive` has no
`clap` dependency so it can be shared as-is with `frust-tui` and `frust-mcp`; none of the three
depends on any framework crate — this unit drives Frust apps, it does not consume the framework.

`frust-mcp` is an MCP (Model Context Protocol) tooling crate: a session-supervision seam
(`SessionBackend`) plus 16 tools (session lifecycle, driving, diagnosis) exposed over the
Streamable HTTP transport, so an AI agent can launch, inspect, and drive a Frust app. `frust-cli`
carries no *direct* dependency on it and has no `mcp` subcommand — `frust-mcp` has no headless/CI
entry point in this unit. Its sole consumer for the MCP protocol itself is `frust-tui`, which
embeds an MCP server over its own sessions (see [TUI_ARCHITECTURE.md](TUI_ARCHITECTURE.md));
`SessionEngine`, this crate's own `SessionBackend` implementation, is retained as the reference/test
backend the crate's own test suite drives.

`frust-dap` is this unit's fourth tool surface, but — unlike `frust-mcp` — it has **no standalone
process of its own**: there is no `frust dap` subcommand. It is a DAP (Debug Adapter Protocol)
launch-orchestration library `frust-tui` embeds and serves over loopback TCP from inside the
workbench (see [TUI_ARCHITECTURE.md](TUI_ARCHITECTURE.md)'s Embedded DAP Surface), letting an IDE
(VS Code + `editors/vscode-frust`, or any DAP-speaking client) drive the same session an editor's
user is looking at in the terminal. Stepping — breakpoints, stack, variables — is deliberately
absent: ruling D6 delegates that to native lldb tooling instead of reimplementing a debugger.
Reusing the host's own `frust_mcp::SessionBackend` — never a `SessionEngine` of its own — is what
makes an editor and the workbench's UI drive the same app sessions (see Layer Dependencies below).

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
| `frust-drive::build_info` | The debug/profile/release + flavor funnel shared by run and build; `BuildMode::cargo_features()` also selects the `frust/perf-trace`+`frust/devtools` cargo-feature pair for Debug/Profile (see [DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md)) |
| `frust-drive::devtools_client` | Blocking NDJSON client for the devtools wire protocol (`widget_tree`/`widget_props`/`metrics_snapshot`/`screenshot`/`tap`/`scroll`/`text`/frame-stats subscription) plus `adb forward` helpers for Android — the tool-side half of [DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md) |
| `frust-drive::metrics` | Pure `/proc`/`/sys`/`adb` parsers plus a `MetricsSampler` background-thread stream, feeding `frust-tui`'s DevTools System/Network tabs and `frust-mcp`'s `metrics` tool (Linux desktop + Android only in v1, no build-feature gate). Honest about scope: network counters are namespace-wide (desktop) or device-wide (Android), never per-process — see [LIMITATIONS.md](LIMITATIONS.md) |
| `frust-drive` Android/iOS pipelines | The four platform pipelines: compile → install → launch/stream; `android_run::spawn_session` and the iOS pipelines' `spawn_session`/`spawn_physical_session` return the launched app's identity (`AndroidLaunch{stream, package}` / `IosLaunch`) so a caller can address that exact app later without re-deriving it — `frust-tui`'s `Supervisor` uses both to best-effort terminate the app on stop (see [TUI_ARCHITECTURE.md](TUI_ARCHITECTURE.md)), and `frust-mcp`'s iOS-Simulator teardown uses `IosLaunch` the same way |
| `frust-drive::desktop_run` | The desktop `cargo run` launch plan (mode → cargo-args mapping) as a standalone, reusable value builder; `frust-mcp` spawns desktop sessions through it. `frust-tui` has its own equivalent logic and has not converged onto this module yet |
| `frust-drive::plugin` | Static plugin registry plus the idempotent project-mutation engine that applies it |
| `frust-drive::interrupt` | The process-wide SIGINT/SIGTERM/SIGHUP + panic-hook owner; scrubs registered secret files before the process dies |
| `frust-mcp::config` | `McpConfig` — port (`DEFAULT_MCP_PORT` 4848; `0` lets the OS assign an ephemeral one); bind address is hard-coded to `127.0.0.1`, never configurable |
| `frust-mcp::backend` | `SessionBackend` — the sync trait (fourteen methods) the tool layer drives via `Arc<dyn SessionBackend>` (`SharedBackend`). Sync by charter: no `async-trait`, so the async/blocking bridge (`spawn_blocking`) lives in the tool layer, not the trait. `SessionEngine` is this crate's own implementation; an embedder (`frust-tui`) supplies its own instead. `stop_app`/`restart_app`'s "Blocking" describes how long the *call* takes to return, not what has finished when it does — an implementer may treat a stop as a request, with the best-effort OS-level app termination still in flight. The last three methods carry default implementations that refuse honestly rather than forcing every embedder to implement them: `subscribe_session_events` (`None`; a push feed of log lines plus the session's end, `frust-dap`'s log-pump consumer), `fetch_widget_tree` (a typed not-supported error; one widget-tree dump for a backend whose devtools connection it does not expose), and `project_root` (`None`; the directory the *next* `run_app` would build from, read live rather than cached or injected — `frust-dap`'s launch path calls it per launch and refuses in-band on `None`). `SessionEventFeed::channel()` and the paired `SessionEventSink` are the public sending half an embedder builds its own feed from |
| `frust-mcp::clients` | `ClientRegistry`/`ClientEntry`/`ClientGuard` — an RAII-tracked count of connected MCP clients (opaque id, connect time) for `serve_embedded`'s caller to read. A guard's `Drop` is the only disconnect signal available (rmcp exposes none), so a client that vanishes without a clean teardown lingers in the registry until the server stops |
| `frust-mcp::server` | Binds `rmcp`'s `StreamableHttpService` at `/mcp` behind an `axum` router, with a `LocalSessionManager` for MCP session state and rmcp's built-in `allowed_hosts` Host-header guard left at its loopback-only default. `serve` builds a fresh backend/registry and runs until cancelled; `serve_embedded(backend, registry, bind_port, ready, cancel)` is the runtime-toggled entry point an embedder starts and stops repeatedly against its own long-lived `ClientRegistry`. Logs the one endpoint line (`frust-mcp listening on http://127.0.0.1:<PORT>/mcp`) via `log::info!` once listening — `frust-mcp` is print-free, see Layer Dependencies below |
| `frust-mcp::handler` | `McpHandler` — the `ToolRouter`/`#[tool]` registrations for all 16 tools, holding an `Arc<dyn SessionBackend>` rather than a concrete engine. Dispatch is thin: each tool body forwards straight into `tools`, so tool logic is unit-tested without the rmcp stack |
| `frust-mcp::engine` | `SessionEngine` — launches and supervises app sessions (desktop/Android/iOS Simulator) headlessly; `run_app` returns a `SessionId` immediately, with a launch that can't even spawn landing as `SessionState::Failed` (a device launch's failure otherwise arrives minutes later). Retains a 10,000-line log ring (with the devtools handshake token redacted before it ever enters the ring — see [DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md)) and a 600-sample frame-stats ring per session, plus up to `TERMINAL_SESSION_CAP` (32) terminal sessions' worth of history — oldest-terminal eviction on insert, a live session is never evicted. Connects the in-app devtools service the same way `frust-drive::devtools_client`'s callers do: discovery-line parsing, `adb_forward_ephemeral`, handshake token. System-metrics sampling is Android-only (`StreamHandle` exposes no pid on desktop/iOS, so `SessionState::Exited` also carries a success flag rather than a real exit code). Teardown joins a session's threads with a bounded 5s wait, then detaches; `shutdown()` itself now joins every detached teardown before returning, so a stop is provably complete rather than possibly still killing in the background. iOS-Simulator teardown issues `simctl terminate <udid> <bundle_id>`, mirroring the Android `am force-stop` — the same lesson `frust-tui`'s own teardown carries. `subscribe_session_events` (the `SessionBackend` implementation) adds a bounded (4096-line), single-subscriber push feed over the same redacted ring — seeded then live under the ring's own lock, with in-band `[frust] <N> log line(s) dropped (…)` loss markers on overflow, and a terminal `Exited` event closing the feed when the session ends — `frust-dap`'s log pump is its consumer |
| `frust-mcp::tools` | The agent-facing surface, in three families over the backend: session (`list_devices`/`list_sessions`/`run_app`/`stop_app`/`restart_app`/`app_logs`), driving (`find_widgets`/`tap`/`scroll`/`enter_text`/`widget_props`), diagnosis (`widget_tree`/`performance`/`metrics`/`screenshot`) — plus `ping`. A tool taking `session_id` resolves it: explicit id, else the sole live session, else the sole session that ever ran, else an ambiguity error — reported differently for the two cases (several *live* sessions want a `session_id`; several *ended* ones mean nothing is running at all), since a dead session is never counted as running. `find_widgets` and a query-targeted `tap` fetch the widget tree exactly once and filter/resolve locally, never polling the app to "settle" a query. `screenshot` tries the app's own devtools capability first, falls back to `adb screencap` on Android, and refuses outright on desktop/iOS Simulator. `performance`/`metrics` report "unavailable" with a reason, never a zeroed reading, when there is nothing to report. Failures come back as in-band `ToolError`s (with candidate/session lists for disambiguation), not protocol errors |
| `frust-dap::protocol` | Content-Length-framed JSON codec (10MB message cap, 4KB header-line cap) plus a `DapMessage`/`LaunchArguments`/`Capabilities` surface trimmed to orchestration-v1 — no breakpoint/stack/variable types |
| `frust-dap::server` | `serve_embedded`/`serve_tcp` — the loopback accept loop (4-client semaphore cap, `127.0.0.1`-only, hard-coded) binding `DEFAULT_DAP_PORT` (4849, a fixed default rather than an OS-assigned ephemeral one, so an editor's launch config can name it ahead of time) unless the embedder overrides it, plus the per-connection session state machine: `initialize` enforced first, a writer task stamps monotonic `seq`, the session itself emits only the `initialized` event (the adapter owns `output`/`exited`/`terminated`), and `on_disconnect` fires exactly once per connection so adapter teardown can assume idempotency. 30s pre-handshake timeout; no idle timeout by design. `serve_embedded(backend, registry, bind_port, ready, cancel)` is the direct counterpart of `frust_mcp::serve_embedded`, down to argument shape — it carries no `project_root` of its own, since the backend answers `SessionBackend::project_root()` live, per launch; the embedder's (`frust-tui`'s) `CancellationToken` stops the accept loop *and* every live session, each spawned under a child token |
| `frust-dap::clients` | `DapClientRegistry`/`DapClientEntry`/`DapClientGuard` — an RAII-tracked, `frust_mcp::ClientRegistry`-shaped record of connected editors. A guard is minted at accept (before a client has said who it is) and named once `initialize` arrives; `Drop` is the only disconnect signal, matching `frust-mcp`'s own guard discipline |
| `frust-dap::sanitize` | `console_safe` — replaces every control character (`char::is_control`) with U+FFFD before a client-supplied string reaches a rendered console: the ignored-`projectRoot` note and the client name/id the registry records, both destined for a raw-mode terminal an ANSI escape could otherwise repaint |
| `frust-dap::adapter` | `OrchestrationAdapter`, implementing the server's `DapAdapter` seam over the **host's own** `frust_mcp::SharedBackend` — one app per connection, launched from whatever directory `SessionBackend::project_root()` answers **at launch time** (no injected/cached root; a `None` answer refuses the launch in-band); a client's `launchArguments.projectRoot` is never honored (an ignored value is echoed back, sanitized, as a Debug Console note — see [LIMITATIONS.md](LIMITATIONS.md) `dap-tcp-unauthenticated-v1`). `on_initialize` (sync) names the connection in the registry; a log-subscription pump forwards lines as `output` events, an exit watch turns a terminal session into `exited` then `terminated`; teardown stops only the app *this connection* launched, never the backend itself; also answers the `frustRestart`/`frustWidgetTree` custom requests |
| `frust-dap::ide_config` | Generates/merges the client-side DAP launch config an editor needs to attach: `detect_parent_ide` (nine `ParentIde` variants — VS Code family, Cursor, Zed, IntelliJ/Android Studio, Neovim, Emacs, Helix — sniffed from seven environment variables) plus one generator per IDE (`vscode`'s `launch.json`, shared by Neovim; `zed`'s `.zed/debug.json` naming a best-effort/**unverified** `"CodeLLDB"` adapter; `emacs`'s `.frust/dap-emacs.el`; `helix`, which always reports `Skipped` — Helix has no spawnable-adapter transport this server can satisfy). `run_generator` owns all file I/O so each generator stays a pure string-in/string-out function |

## Layer Dependencies

`frust-cli` depends on `clap` for argument parsing and on `frust-drive` for every operation it
performs; it holds no toolchain-invocation logic of its own. `frust-drive` has zero dependency on
`clap` or any framework crate — this is a charter boundary, not incidental, since `frust-tui` links
the same library without pulling in a CLI parser or the render/widget stack. The one sanctioned
exception is `frust-devtools-protocol` (the dependency-free wire-protocol leaf `devtools_client`
speaks) — never `frust-devtools` itself, which stays framework-side (see
[DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md)). `frust-cli`'s `tui` subcommand (and its
bare-`frust` default) hands off to `frust-tui` with an **in-process library call**
(`frust_tui::run()`, not a subprocess), whose own dependency set is `frust-drive` +
`frust-devtools-protocol` + `frust-mcp` + `frust-dap` (see
[TUI_ARCHITECTURE.md](TUI_ARCHITECTURE.md)). `frust-cli` itself carries **no direct dependency** on
`frust-mcp` or `frust-dap` — no `mcp` subcommand, no `dap` subcommand, no `frust dap` process — but
both enter its build graph **transitively** through `frust-tui`, the sole embedding boundary: an
MCP or DAP client reaches a session only through a running `frust-tui`, never headlessly through
the CLI (see [LIMITATIONS.md](LIMITATIONS.md) for the no-headless/CI consequence).

`frust-mcp` is this unit's third tool surface, not reachable through `frust-cli` on its own. It
is async, unlike `frust-drive`, and owns no runtime of its own — `serve`/`serve_embedded` run on
whatever runtime the caller (a standalone binary, or `frust-tui`'s own runtime) drives them from;
every call it makes into `frust-drive` goes through `tokio::task::spawn_blocking`, never a runtime
worker directly. Its own dependency surface is `frust-drive` + `frust-devtools-protocol` (the same
sanctioned leaf) plus `rmcp`/`axum`/`tokio`/`tokio-util`/`base64`/`serde`/`log` — never
`frust-devtools`, never a framework crate, not even as a dev-dependency (the tooling-isolation rule
this widens is [ARCHITECTURE.md](ARCHITECTURE.md)'s). Trust model: bound to `127.0.0.1` only,
guarded by rmcp's default Host-header check, with **no MCP-level authentication** — any local
process that can reach the port can connect and drive apps through it, a deliberate v1 stance
ported from fdemon-pro (the devtools handshake token still protects the app-side service itself;
see [DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md)).

`frust-dap` is this unit's fourth tool surface, and — like `frust-tui` — depends on the `frust-mcp`
tooling crate itself, but the other way round from `frust-mcp`'s own reference implementation: it
consumes **`SessionBackend`**, not `SessionEngine`, so it supervises no sessions of its own and an
embedder's own supervisor (`frust-tui`'s) is what a launch actually drives. Its dependency set is
`frust-drive` + `frust-mcp` plus `tokio`/`tokio-util`/`serde`/`serde_json`/`log`/`thiserror` — no
`frust-devtools-protocol` (it never speaks the devtools wire directly; every widget-tree pull goes
through the backend) and no new external crate beyond the workspace's existing pins (`tokio` gains
only `rt`/`io-util`/`net`/`sync`/`macros`/`time` — no `rt-multi-thread` or `signal`, since the
runtime and the process's signal handling both belong to the embedder). Print-free contract:
`frust-dap` carries the strictest of the unit's three tripwires — a **zero** allowlist, the same
raw-mode-host-terminal reason `frust-drive`/`frust-mcp` carry, only stricter because it has no
stdout of its own at all to fall back on (see [REVIEW_FOCUS.md](REVIEW_FOCUS.md)).

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
only `frust-cli`'s own command handlers print. `frust-mcp/src` is print-free too, guarded by its own
`print_free_cores` tripwire mirroring `frust-drive`'s (`frust-mcp` is linked into `frust-tui`'s
raw-mode terminal, so a stray `println!` would garble it — see [REVIEW_FOCUS.md](REVIEW_FOCUS.md)):
it logs its one endpoint line via `log::info!` instead of printing it, and everything else it emits
is structured tool output or `log`. `frust-dap/src` carries the same tripwire shape with a zero
allowlist (see Layer Dependencies above). Every external tool invocation goes through the injected
`&dyn ProcessRunner`, with `commands::dispatch` as the single `Real*` construction site; scaffold
mutation is idempotent by contract (byte-identical or `AlreadyPresent`, never a silent partial
write); and streaming spawns always pipe stdout/stderr rather than inheriting the terminal, so raw
child output can't garble a caller's raw-mode terminal (relevant to `frust-tui`).

## Data Flow

- `Cli` (`clap`) parses `argv` into an optional `Command`; with none given, `main` resolves a
  TTY-gated default (stdin and stdout must both be terminals) to `Command::Tui`, otherwise it
  prints help and exits 2 (exit code 2 is preserved; help content is unchanged — only the stream
  moved, from stderr to stdout). The two non-interactive refusals deliberately differ in **exit code**, not just
  wording — bare `frust` → help on stdout + exit 2; explicit `frust tui` → error on stderr + exit 1.
  `dispatch` then builds the one `RealProcessRunner` and injects it into every handler — no handler
  ever shells out directly.
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
- `tui` (explicit subcommand, or the bare-`frust` default above): `Command::Tui` hands off entirely
  to `frust-tui`'s own async runtime (see [TUI_ARCHITECTURE.md](TUI_ARCHITECTURE.md)), which may in
  turn start `frust_mcp::serve_embedded` and/or `frust_dap::serve_embedded` over its own
  sessions — the only path an MCP or DAP client reaches a Frust app through in this unit (see
  [LIMITATIONS.md](LIMITATIONS.md)).
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
| `Cli` / `Command` | The `clap`-derived argument surface for the `frust` binary; `Cli::command` is an `Option<Command>` so bare `frust` (no subcommand) resolves via the TTY-gated default rather than a clap parse error |
| `SessionBackend` (`frust-mcp`) | The fourteen-method sync trait both `frust-mcp`'s tools and `frust-dap`'s adapter drive via `Arc<dyn SessionBackend>` — an embedder's own supervisor implements it once, for both |
| `DapAdapter` seam (`AdapterResponse`, `EventSender`) | The trait `frust-dap::server` defines and `OrchestrationAdapter` implements: per-request response shape plus the cloneable handle an adapter pushes `output`/`exited`/`terminated` events through |
| `DapClientRegistry` / `DapClientEntry` / `DapClientGuard` | The connected-editor registry `serve_embedded`'s caller reads to show who is attached, RAII-removed on disconnect |
| `ParentIde` / `IdeConfigResult` / `ConfigAction` | `frust-dap::ide_config`'s detected-or-overridden IDE, and the outcome of generating that IDE's DAP launch config (`Created`/`Updated`/`Skipped(reason)`) |
