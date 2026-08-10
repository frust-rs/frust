# Frust - CLI Development

Template/scaffold development and version pins owned by the CLI unit (`frust-cli`,
`frust-drive`, `frust-mcp`). Shared prerequisites, build/run commands, the standard verify gate, and the
version-pin *policy* live in [DEVELOPMENT.md](DEVELOPMENT.md); the unit's design lives in
[CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md).

## Template development

`frust create` embeds `templates/app/` into the binary at compile time; the hidden,
development-only `--template-dir <path>` flag iterates on template files without
rebuilding the embedded copy. Every scaffold also gets a default launcher icon set and
the platform-specific edge-to-edge/safe-area/keyboard-inset and back-navigation glue
the generated app needs — see [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)'s cross-cutting
host-signal flow.

`--arch clean-signals` scaffolds a clean-architecture variant (controller + use-case +
`async_view` over `clean-signals-frust`) instead of the default notes-app template;
`clean-signals` is git+rev-pinned to its public GitHub repo (see
[CORE_DEVELOPMENT.md](CORE_DEVELOPMENT.md)'s version pins), so the scaffold builds on any
machine with no sibling checkout required.

**Platform embedding modules ship in-repo, not templated.**
`platform/android/frust-embedding` and `platform/ios/FrustEmbedding` are consumed by a
scaffolded project by path — edit the module in place and rebuild the consuming app
directly, no re-scaffold needed. A scaffolded project's embedding path is
machine-specific: moving it means editing `gradle.properties`'s `frust.embedding.dir`
line (Android) or the local package reference in `project.pbxproj` (iOS); `frust clean`
also removes the redirected Gradle build output.

The scaffold's own end-to-end tests (`create_e2e`, `create_ios`, `build_e2e`) are `#[ignore]`d
and listed with the other manual/gated tests in [DEVELOPMENT.md](DEVELOPMENT.md)'s Test section.

## Version Pins

The pins this unit owns, under [DEVELOPMENT.md](DEVELOPMENT.md)'s Version-Pin Policy (pins are
LAW; re-run the row's tripwire after touching it, and never run a blind `cargo update`):

| Pin | Why | Tripwire |
|---|---|---|
| `notify 8` minor (`frust-cli`-only) | `frust run --watch`'s filesystem watcher (`ctrlc` floats, shared by `frust-drive`/`frust-cli`, unpinned; `frust-drive`'s copy now enables the `termination` feature so `frust-drive::interrupt` also catches SIGTERM/SIGHUP — needed to scrub the plaintext release-signing file on a CI runner's kill, not just Ctrl-C. Visible consequence: interrupting a `--release` `frust run` before logcat streaming begins now exits 130 for SIGTERM/SIGHUP, where an unhandled signal previously exited 143/129; the logcat phase's own Ctrl-C-means-stop override still exits 0 for all three signals) | `cargo test -p frust-cli` |
| `rmcp 1.7` minor (`frust-mcp`-only; resolves 1.8.0) | `frust-mcp`'s MCP Streamable-HTTP server. `default-features = false` trims rmcp's client/reqwest/auth/elicitation surface this tool-only, loopback-bound server never uses; `server` + `transport-streamable-http-server` are the transport it runs, `macros` pulls in the `#[tool_router]`/`#[tool]`/`#[tool_handler]` attribute macros the tools are built from | `cargo test -p frust-mcp` |
| `axum 0.8` (`frust-mcp`-only) | The HTTP layer rmcp's `StreamableHttpService` nests into | `cargo test -p frust-mcp` |
| `base64 0.22` (`frust-mcp`-only) | Encodes/decodes screenshot payloads in MCP tool results | `cargo test -p frust-mcp` |
| `tokio-util 0.7` (`frust-mcp`-only) | The `CancellationToken` graceful-shutdown seam wired to Ctrl-C | `cargo test -p frust-mcp` |

`frust-drive` also shares `frust-tui`'s `toml_edit` pin — see
[TUI_DEVELOPMENT.md](TUI_DEVELOPMENT.md).

## `frust mcp`

`frust mcp` runs `frust-mcp`'s Streamable HTTP server (the `mcp` subcommand is a thin
`Runtime::new` + `block_on` shim over `frust_mcp::run`, mirroring `commands::tui`). `--port`
defaults to 4848 (`--port 0` picks an OS-assigned ephemeral port); `--project` defaults to the
current directory and must exist, but needs no `frust.toml`. Once listening it prints one line —
`frust-mcp listening on http://127.0.0.1:<PORT>/mcp` — and serves until Ctrl-C.

`frust-mcp`'s integration tests (`http_smoke`, `engine_lifecycle`, `tool_families`) each bind an
ephemeral loopback port (`--port 0` / a hand-rolled NDJSON fixture server), relevant in a
network-restricted sandbox that blocks even loopback binds.

## See Also

- [DEVELOPMENT.md](DEVELOPMENT.md) — prerequisites, build/run/test gates, version-pin policy
- [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md) — the unit's design
- [TUI_DEVELOPMENT.md](TUI_DEVELOPMENT.md) — the workbench front-end's pins
