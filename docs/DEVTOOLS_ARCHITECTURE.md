# Frust - DEVTOOLS Architecture

## Overview

DEVTOOLS is the framework's in-app debug service and its wire protocol — the Frust analogue of
the Flutter/Dart VM service: a running app hosts a small local server that external tooling
(`frust-drive`/`frust-tui`) connects to for widget-tree inspection, frame-stats streaming, and
input injection. Two crates split the two sides of the wire: `frust-devtools-protocol` is the
sanctioned, dependency-free leaf both sides share; `frust-devtools` is the framework-side
service a shell hosts. Tooling and framework never depend on each other — they meet only at the
protocol leaf.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how DEVTOOLS relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `frust-devtools-protocol` | NDJSON JSON-RPC 2.0 wire types: `Request`/`Response`/`Notification`/`Incoming`, the typed v1 `Method` set with per-method param/result structs, `encode_line`/`decode_line` framing, and `DISCOVERY_PREFIX`/`parse_discovery_line` — the single source of truth for the discovery-line contract both sides use |
| `frust-devtools::backend` | `DevtoolsBackend` trait a shell implements (`widget_tree`, `widget_props`, `metrics_snapshot`, `inject_tap`/`inject_scroll`/`inject_text`, `screenshot` defaulting to `NotSupported`), plus `AppInfo` and `BackendError` |
| `frust-devtools::service` | `Service::start`/`ServiceHandle`: binds `127.0.0.1:0`, owns a small internal current-thread tokio runtime, logs the discovery line, and exposes `publish_frame_stats` (bounded, drop-oldest, never blocks the caller) |
| `frust-devtools::{server,dispatch,frame_stats,hop}` | The accept loop, request→backend-call dispatch, the frame-stats broadcast bus, and the backend-thread hop that carries every backend call through one 1s-timeout channel round trip |
| `frust-shell-common::devtools` (feature `devtools`) | The shell-side `DevtoolsBackend` implementation: maps `RenderRoot::inspect()` output into protocol types, hops backend calls to each shell's UI thread, and drives service start/pump/frame-stats publish |
| `frust-drive::devtools_client` | The tool-side client: blocking `std::net::TcpStream` request/response plus a frame-stats subscription, `adb_forward_ephemeral`/`adb_forward_remove` for Android, and discovery-line parsing reused from the protocol leaf |

## Layer Dependencies

**The protocol leaf charter.** `frust-devtools-protocol` depends on `serde` (derive) and
`serde_json` only — no `tokio`, no framework crate, no other `frust-*` crate. It is the *one*
sanctioned crossing of the tooling-isolation charter (`docs/ARCHITECTURE.md`'s Cross-Unit Layer
Dependencies): pulling it into either side never drags the other side's dependency graph along.

**Framework-side isolation.** `frust-devtools` depends on the protocol leaf, `tokio` (workspace,
minimal features), and `log` — nothing else. It does not depend on `frust-core` or any shell; a
`DevtoolsBackend` implementation is how a shell hands it app state, so the service itself stays
fully decoupled from what it is inspecting. No tooling crate may depend on `frust-devtools`, and
`frust-devtools` may not depend on any tooling crate — the two sides meet only at the protocol
leaf, never directly.

**Tooling side.** `frust-drive` gains exactly one new dependency for this unit — the protocol
leaf — preserving its no-framework-crate, no-tokio charter (`docs/CLI_ARCHITECTURE.md`). The
client is `std::net::TcpStream` plus a background reader thread, mirroring `frust-drive`'s
existing process-output threading idioms rather than pulling in an async runtime.

**Trust model.** The service binds `127.0.0.1` only, never configurably wider, and ships with no
authentication in v1 — the trust boundary is the loopback interface itself (plus, on a device, an
`adb forward` a developer sets up deliberately), the same assumption a debugger attaching to the
process makes. Because the protocol's `input_*` methods drive real UI, a shell gates starting the
service on a debug/profile build via the `devtools` cargo feature (never a `debug_assertions`
runtime check alone) — a release build compiles the listener out entirely, and the
`FRUST_DEVTOOLS=0` env var is a runtime kill switch for the compiled-in case. See
[DEVELOPMENT.md](DEVELOPMENT.md) for the feature/build-mode funnel and
[CODE_STANDARDS.md](CODE_STANDARDS.md) for the frame-stats publish-ordering convention.

## Data Flow

- **Discovery.** The service logs one line built from `DISCOVERY_PREFIX` via `log::info!` on
  start; tooling recovers the port from a log/logcat stream via `parse_discovery_line` (a
  substring match tolerant of timestamp/tag prefixes — the same function formats and parses, so
  the two sides can never drift). Android/iOS device access additionally routes through
  `frust-drive`'s `adb_forward_ephemeral`/`adb_forward_remove` (Android) to map a device-loopback
  port to a host-loopback one; iOS physical-device forwarding is not implemented in v1 (simulator
  and desktop connect directly over localhost) — see [LIMITATIONS.md](LIMITATIONS.md).
- **Request/response.** A client connects over TCP, sends one NDJSON `Request` per line; the
  service decodes it, dispatches to the typed `Method`, and calls the corresponding
  `DevtoolsBackend` method on a dedicated backend thread (one call at a time, in arrival order),
  capped by a 1s timeout that returns an error response to the client rather than blocking it
  indefinitely. `handshake` bypasses the backend entirely, answered from state captured at
  startup, so a client can always identify even a wedged app.
- **The UI-thread hop.** Most backend calls need live app state, which only the UI thread owns.
  `frust-shell-common::devtools`'s `DevtoolsBackend` implementation pushes a request onto a
  process-wide queue and blocks on a reply channel; each shell drains that queue once per frame
  through its own hop mechanism — desktop wakes via its winit `EventLoopProxy`, Android and iOS
  have no wake and instead pump the queue at the top of every frame tick (their display loop
  already runs continuously while resumed). An injected tap/scroll/text event is dispatched
  through each shell's real input path (never called directly against widget code), so it passes
  the same hit-testing, capture, and focus routing real input does; a mobile shell also marks the
  event as having occurred so its frame gate cannot skip the frame that should show its effect.
- **Frame stats.** `FrameStats::record`'s existing per-frame hook publishes to the devtools
  broadcast bus *ahead of* the `perf::enabled()` gate — subscribing to devtools frame stats is
  its own opt-in, independent of `FRUST_TRACE` (see [CODE_STANDARDS.md](CODE_STANDARDS.md)). The
  bus is bounded and drop-oldest on every hop it crosses (service → client, and again in
  `frust-drive`'s client-side relay), so a slow subscriber never blocks the frame thread and only
  ever loses the *oldest* unread sample.
- **Build-mode funnel.** `BuildMode::cargo_features()` (`frust-drive::build_info`) selects
  `["frust/perf-trace", "frust/devtools"]` for Debug and Profile, `["lean"]` for Release — the
  same two-layer gate `perf-trace` already established, now covering the devtools feature too.
  See [DEVELOPMENT.md](DEVELOPMENT.md) and [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md).

## Key Types

| Type | Purpose |
|------|---------|
| `Method` / `Request` / `Response` / `Notification` / `Incoming` | The typed v1 NDJSON JSON-RPC message set and framing discriminator (`frust-devtools-protocol`) |
| `WidgetTreeDump` / `WidgetNode` / `WidgetProps` | The wire shape of an inspected widget tree and one widget's props |
| `DevtoolsBackend` | The trait a shell implements to answer every devtools request; the seam decoupling the service from `frust-core` |
| `Service` / `ServiceHandle` / `ServiceConfig` | The framework-side server: start/stop, `publish_frame_stats`, and its 1s backend-call timeout |
| `DevtoolsUi` | `frust-shell-common`'s per-shell view the hop's per-frame `pump` drains against |
| `DevtoolsClient` | `frust-drive`'s blocking tool-side client: typed requests plus a `subscribe_frame_stats` receiver |

## See Also

- [ARCHITECTURE.md](ARCHITECTURE.md) — root index, tooling-isolation charter
- [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md) — the shell-side hop and frame pipeline it plugs into
- [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md) — the tool-side client's crate boundary
