# Frust - CLI Development

Template/scaffold development and version pins owned by the CLI unit (`frust-cli`,
`frust-drive`, `frust-mcp`, `frust-dap`). Shared prerequisites, build/run commands, the standard
verify gate, and the version-pin *policy* live in [DEVELOPMENT.md](DEVELOPMENT.md); the unit's
design lives in [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md).

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
| `tokio-util 0.7` (`frust-mcp`/`frust-dap`; also `frust-tui`, see [TUI_DEVELOPMENT.md](TUI_DEVELOPMENT.md)) | The `CancellationToken` graceful-shutdown seam, wired to Ctrl-C in `frust-mcp`'s standalone `run` and to the workbench's start/stop toggle for both embedded servers in `frust-tui` | `cargo test -p frust-mcp` && `cargo test -p frust-dap` && `cargo test -p frust-tui` |
| `icns 0.4` (`frust-drive`-only, `default-features = false` + `pngio`) | `icons::generate_icns`'s macOS `.icns` authoring — the same crate `cargo-bundle` itself uses; `pngio` is the only feature the PNG-only pipeline needs (drops the default JPEG-2000 codec) | `cargo test -p frust-drive` |

`.ico` authoring adds no new crate: `icons::generate_ico` rides the `ico` feature of the `image`
pin RENDER already owns exactly (see [RENDER_DEVELOPMENT.md](RENDER_DEVELOPMENT.md)) —
`image::codecs::ico`.

`frust-drive` also shares `frust-tui`'s `toml_edit` pin — see
[TUI_DEVELOPMENT.md](TUI_DEVELOPMENT.md).

### External-tool / template-side pins (not Cargo dependencies)

| Pin | Why | Tripwire |
|---|---|---|
| `cargo-packager 0.11.8` exact (external tool, shelled out to by `desktop_build::installer`) | Builds `.dmg`/NSIS/WiX/`.deb`/`.AppImage` installers over an assembled bundle; `frust doctor`'s `CargoPackagerValidator` gates on an exact version match (non-fatal — only `frust build --installer` needs it) and reports the install hint `cargo install cargo-packager --version 0.11.8 --locked` on a mismatch or absence | `cargo install cargo-packager --version 0.11.8 --locked` smoke + `cargo test -p frust-drive` |
| `winresource 0.1` (generated app's own build-dependency, `templates/app/Cargo.toml.tmpl` — not a workspace pin) | Embeds `windows/icon.ico` plus file/product version into the compiled `.exe`; the actively-maintained fork of `winres`, unmaintained since 2021 | scaffold e2e (`create_e2e`) |

## `frust-mcp`

`frust-mcp` has no `frust-cli` subcommand and no standalone binary — `frust_mcp::run` (the
Ctrl-C-driven, fixed-config entry point) and `serve_embedded` (the runtime-toggled one
`frust-tui` uses) are both library entry points only. Reaching an MCP server means starting
`frust-tui` and toggling its embedded server on (see [LIMITATIONS.md](LIMITATIONS.md) for what
that means for headless/CI use).

`frust-mcp`'s integration tests (`http_smoke`, `engine_lifecycle`, `tool_families`,
`mcp_embed_lifecycle`) each bind an ephemeral loopback port (`--port 0` / a hand-rolled NDJSON
fixture server), relevant in a network-restricted sandbox that blocks even loopback binds.

## `frust-dap`

`frust-dap` has no `frust-cli` subcommand and no standalone binary — like `frust-mcp`,
`frust_dap::serve_embedded` is a library entry point only, and its sole host is `frust-tui`'s DAP
settings dialog (see [TUI_ARCHITECTURE.md](TUI_ARCHITECTURE.md)'s Embedded DAP Surface). There is
no headless/CI way to reach a DAP server independent of the TUI.

Verify with the standard `-p frust-dap` forms: `cargo test -p frust-dap`,
`cargo clippy -p frust-dap --all-targets -- -D warnings`, `cargo fmt --check`. Its dev-dependency on
`frust-drive`'s `test-util` feature (the scripted `FakeProcessRunner`) is a feature toggle on a
dependency the crate already has — no new package enters `Cargo.lock`. Its embedded-server tests
bind ephemeral loopback ports, the same sandbox caveat as `frust-mcp`'s above.

`editors/vscode-frust` is a plain-JavaScript, unpublished VS Code extension (no TypeScript, no
bundler, no npm dependency) registering debug type `frust`. It never spawns a process: its
`DebugAdapterDescriptorFactory` returns a `vscode.DebugAdapterServer`, connecting to whatever port
a launch config's `debugServer` names, falling back to the `frust.dapPort` workspace setting and
then the `frust-dap` default (4849) — attach-by-port against the TUI's own embedded server, always.
`frust-dap`'s launch-config generation (the DAP settings dialog's `g`, or the auto-configure flow on
server start) writes a matching `.vscode/launch.json` entry through
`frust_dap::ide_config::vscode`, so a project the TUI has configured needs no manual `launch.json`
edit — only the extension itself installed (`npx @vscode/vsce package` → a local `.vsix`, run by
hand, not by any workspace gate — Node.js is not required to build or test the Rust workspace).

## See Also

- [DEVELOPMENT.md](DEVELOPMENT.md) — prerequisites, build/run/test gates, version-pin policy
- [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md) — the unit's design
- [TUI_DEVELOPMENT.md](TUI_DEVELOPMENT.md) — the workbench front-end's pins
