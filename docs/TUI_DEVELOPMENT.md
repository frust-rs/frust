# Frust - TUI Development

Version pins owned by the TUI unit (`frust-tui`). Shared prerequisites, the workbench's run
command, the standard verify gate, and the version-pin *policy* live in
[DEVELOPMENT.md](DEVELOPMENT.md); the unit's design lives in
[TUI_ARCHITECTURE.md](TUI_ARCHITECTURE.md).

## Version Pins

The pins this unit owns, under [DEVELOPMENT.md](DEVELOPMENT.md)'s Version-Pin Policy (pins are
LAW; re-run the row's tripwire after touching it, and never run a blind `cargo update`):

| Pin | Why | Tripwire |
|---|---|---|
| `ratatui 0.30` / `crossterm 0.29` / `ansi-to-tui 8.0.1` minor | `frust-tui`'s render/terminal/log stack, pre-1.0 churn expected | `cargo test -p frust-tui` |
| `toml_edit 0.25` minor (shared with the CLI unit's `frust-drive`, see [CLI_DEVELOPMENT.md](CLI_DEVELOPMENT.md)) | `frust-tui`'s config persistence and `frust-drive::plugin`'s format-preserving Cargo.toml/manifest edits | `cargo test -p frust-tui` && `cargo test -p frust-drive` |
| `tokio-util 0.7` (shared with the CLI unit's `frust-mcp`/`frust-dap`, see [CLI_DEVELOPMENT.md](CLI_DEVELOPMENT.md)) | The `CancellationToken` both embedded servers shut down on — `frust_mcp::serve_embedded`'s and `frust_dap::serve_embedded`'s | `cargo test -p frust-tui` && `cargo test -p frust-mcp` && `cargo test -p frust-dap` |

`frust-tui`'s own `tokio` feature set gained `net` (a feature add, not a version bump) so both
embedded servers' loopback `TcpListener`s bind on this crate's own runtime rather than depending on
another workspace member's feature unification.

`frust-tui`'s `crate::clipboard` system-clipboard backend consumes the `arboard =3.6.1` pin owned by
the PLUGINS unit (see [PLUGINS_DEVELOPMENT.md](PLUGINS_DEVELOPMENT.md)'s Version Pins row), which
also lists this crate among the pin's tripwire consumers — re-run `cargo check -p frust-tui`
alongside that row's other checks after touching the pin.

The DAP settings dialog's preferences persist in the same `~/.config/frust/tui.toml` the
recent-projects store uses, under a `[dap]` table (`enabled`, `auto_start_in_ide`,
`auto_configure_ide`, `port`, `ide_override`) loaded through the same format-preserving `toml_edit`
path — see [TUI_ARCHITECTURE.md](TUI_ARCHITECTURE.md)'s Embedded DAP Surface for what each key
does.

## Testing the embedded MCP server

`crates/frust-tui/tests/mcp_embedded.rs` drives the embedded server end to end against a real
workbench event loop — a real Streamable-HTTP client against a real `frust_mcp::serve_embedded`,
observed through the same `AppState` the workbench renders, over a scripted `FakeProcessRunner`
(no process is ever really spawned). It runs under the standard `cargo test -p frust-tui` gate,
with no `#[ignore]`. Every wait is on a produced signal (a ready port, an HTTP response, an engine
message, or a bounded yield-until-condition poll) rather than a sleep.

## Testing on Windows

[`scripts/testing/tui-windows-gate.sh`](../scripts/testing/tui-windows-gate.sh) is the repeatable
Windows gate for the TUI, driven from a Linux host over ssh against a Windows box (default
`dell_mini_pc`): it ships a commit, runs the tooling crates' `cargo test` suites there, then drives
the real `frust.exe` workbench over `ssh -tt` (Windows OpenSSH's ConPTY) inside a private local tmux
server, asserting on captured screens — startup/project detection, persistence with `HOME` cleared,
`frust create` via the CLI and the TUI wizard, duplicate-free project rows, desktop run/stop/quit
with no orphaned processes, key handling, IDE DAP-config generation, and MCP/DAP server start/stop.

Run it as `scripts/testing/tui-windows-gate.sh [--sha <rev>] [--host <ssh-host>] [--skip-tests]
[--skip-e2e]`. It needs a Windows host reachable by ssh with an OpenSSH server, the Rust `msvc`
toolchain, `git`/`tar`, Smart App Control off, and a local tmux; it only creates/deletes its own
scratch state under `C:\dev\wintui-gate-*` on the box and only kills its own processes.

Stays manual — an ssh-driven ConPTY session can't judge these: mouse input, glyph rendering in
Windows Terminal/conhost, and whether the app's GUI window is actually visible (the app runs outside
the interactive desktop session over ssh, so the gate can only confirm process liveness).

The gate script above runs `cargo test` only; it does not run clippy. `cargo clippy --workspace
--all-targets -- -D warnings` against the `msvc` toolchain is clean and is worth running as a
separate manual or CI step on a Windows host after touching Windows-only code paths (`#[cfg(windows)]`
blocks, `frust-drive::process`'s Windows process-tree kill, `host_path`'s Windows arms) — the standard
verify gate in [DEVELOPMENT.md](DEVELOPMENT.md) already covers the host it runs on, but that host is
not Windows.

## See Also

- [DEVELOPMENT.md](DEVELOPMENT.md) — prerequisites, build/run/test gates, version-pin policy
- [TUI_ARCHITECTURE.md](TUI_ARCHITECTURE.md) — the unit's design
- [TUI_CODE_STANDARDS.md](TUI_CODE_STANDARDS.md) — the unit's conventions
