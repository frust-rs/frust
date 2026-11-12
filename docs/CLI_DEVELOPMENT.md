# Frust - CLI Development

Template/scaffold development and version pins owned by the CLI unit (`frust-cli`,
`frust-drive`, `frust-mcp`, `frust-dap`). Shared prerequisites, build/run commands, the standard
verify gate, and the version-pin *policy* live in [DEVELOPMENT.md](DEVELOPMENT.md); the unit's
design lives in [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md).

## Template development

`frust create` embeds `crates/frust-drive/templates/app/` into the binary at compile time; the hidden,
development-only `--template-dir <path>` flag iterates on template files without
rebuilding the embedded copy. Every scaffold also gets a default launcher icon set and
the platform-specific edge-to-edge/safe-area/keyboard-inset and back-navigation glue
the generated app needs — see [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)'s cross-cutting
host-signal flow.

`--arch clean-signals` scaffolds a clean-architecture variant (controller + use-case +
`async_view` over `clean-signals-frust`, whose loading/data/error closures return plain `impl View`
with no `any()`) instead of the default single-file counter template (`src/lib.rs`);
`clean-signals` is a crates.io dependency (see
[CORE_DEVELOPMENT.md](CORE_DEVELOPMENT.md)'s version pins), so the scaffold builds on any
machine with no sibling checkout required.

A generated app ships the `build/` root layout by construction — `.cargo/config.toml`'s
`[build] target-dir = "build/rust"`, the Gradle build-directory redirect, the jniLibs source set,
and the `.gitignore` entries all render pre-migrated; see [DEVELOPMENT.md](DEVELOPMENT.md)'s Build
Output Layout for the shape and *Migrating an already-scaffolded app to the build/ layout* for a
pre-existing app that predates it. `crates/frust-cli/tests/profile_sync.rs`'s
`android_cargo_config_identical_between_root_and_template` test compares the template's
`.cargo/config.toml` against the repo root's structurally, not byte-for-byte: their `[target.*]`
rustflags tables must match, and the template's `[build]` table must be exactly `target-dir =
"build/rust"` — the root's config deliberately carries no `[build]` table, since this checkout's
own `target/` is referenced directly by docs and CI and must not move.

**Platform embedding modules ship inside the shell crates, not templated.**
`crates/frust-shell-android/platform/android/frust-embedding` and
`crates/frust-shell-ios/platform/ios/FrustEmbedding` (each shell crate packages its embedding, so
a project built from the published crates can resolve it) are reached by a scaffolded project
through `packages::locate` and `platform_wiring::sync`, never through a tracked path. The Android
location (`frust.embedding.dir`, plus `frust.plugin.<module>.dir` per plugin module) lives in the
gitignored `android/local.properties`, read by `settings.gradle.kts`'s `frustLocalDir(key)`; the
iOS one is a gitignored `ios/FrustEmbedding` symlink (one `ios/<Package>` per plugin package)
that `project.pbxproj` references by relative path. In path mode (`--frust-path`) an edit to the
checkout's embedding rebuilds without re-scaffolding. `frust create` runs the sync after
rendering (`--no-sync` skips it); `frust run`/`frust build` with `-d android|ios` refresh it before
building; `frust doctor`'s "Platform packages" section prints the resolved directories. `frust
clean` also removes the redirected Gradle output.

The scaffold's own end-to-end tests are `#[ignore]`d (`crates/frust-cli/tests`) and listed with
the other manual/gated tests in [DEVELOPMENT.md](DEVELOPMENT.md)'s Test section:

```
cargo test -p frust-cli --test create_e2e -- --ignored    # path mode (also build_e2e; create_ios on macOS)
cargo test -p frust-cli --test create_e2e scaffolded_project_builds_from_the_registry -- --ignored
```

The first checks the local.properties and symlink wiring (`assert_path_mode_wiring`); the second
builds a registry-mode project and needs the release on crates.io. The facade package is
`frust-ui` ([lloydmeta/frunk#258](https://github.com/lloydmeta/frunk/issues/258) — crates.io
`frust` is another project's); `frust create` writes `frust = { package = "frust-ui", .. }`.
Debug/profile/hot builds enable `<key>/perf-trace`, `<key>/devtools` and `<key>/hotpatch`, where
`<key>` is the app's dependency key for `frust-ui`, resolved by `frust-drive::facade_key` from
cargo metadata (fallback: manifest read; default `frust`).

## App version and build number

Each build resolves its version name and build number independently, in this order:
`--build-name` / `--build-number`, then `[app] version` / `build-number` in `frust.toml`
(`frust-drive::build_info::resolve_version`), then the platform default below. `frust create`
writes `version = "1.0.0"` and `build-number = 1`.

`version` is one to three dot-separated non-negative integers (`2`, `2.3`, `2.3.4`); a suffix such
as `1.0.0-beta` is rejected when `frust.toml` is read, because the same string becomes the Android
`versionName` and the iOS `CFBundleShortVersionString`. `build-number` and `--build-number` must be
`>= 1`.

| Target | Name / number when neither is set |
|---|---|
| Android (`build apk`, `appbundle`; `run -d <android>`) | `1.0` / `1`, written to `android/local.properties` |
| iOS (`build ios`, `ipa`; `run` on a device) | Nothing is passed, so the Xcode project's own values apply (the scaffold ships `1.0` / `1`) |
| macOS bundle with a generated `Info.plist` | `0.1.0` / `1` |
| Installer (`--installer`) | Name `0.1.0`; the build number is not used |
| Windows `.exe` | Fixed in the project's `windows/build.rs`; flags and `[app]` do not reach it |
| Linux bundle, web | No version is embedded (an installer still takes the name) |

A release `frust build apk`, `appbundle` or `ipa` prints `warning: app version not set: building
<name> (<number>); ...` when either value fell to its default. Debug and profile builds, other
targets and `frust run` stay silent.

A project that ships its own `macos/Info.plist` has it copied verbatim and never rewritten. If its
`CFBundleShortVersionString` or `CFBundleVersion` differs from a value that came from a flag or
`frust.toml`, the build reports a mismatch note; edit the plist or the setting to agree.

`frust run` on an iOS simulator passes no version, so the Xcode project's values apply there.

## Adding android/ and ios/ to an existing crate

No command does this. The CLI commands are `create`, `doctor`, `devices`, `clean`, `plugin`,
`tui`, `run` and `build`, and `frust create` refuses a non-empty directory unless `--overwrite`
is passed. This procedure was run on 2026-11-12 on a Linux host (Manjaro) against a
checkout, with `--frust-path` and no network. Run from the crate directory; `<name>` is the
crate name and `<org>` its reverse-DNS prefix:

```
frust create /tmp/scaffold --project-name <name> --org <org> --frust-path <checkout>
cp -r /tmp/scaffold/android /tmp/scaffold/ios /tmp/scaffold/frust.toml /tmp/scaffold/assets .
cp -r /tmp/scaffold/.cargo .             # optional: Android link flags, build/rust target dir
rm -f android/local.properties ios/FrustEmbedding   # machine-local, rewritten below
frust build apk       # first step writes the wiring; stops at the signing gate until keys exist
frust doctor          # "Platform packages" lists the resolved embedding directories
```

1. Scaffold a throwaway app with the same `--project-name` and `--org` as the crate (the
   Kotlin package, `applicationId`, bundle id and `-l<name>` link flag are derived from them).
   Omit `--frust-path` for a registry release.
2. Copy `android/`, `ios/`, `frust.toml` and `assets/` (the icon named in `frust.toml`). Copy
   `.cargo/config.toml` and the `.gitignore` entries from the scaffold if you want the same
   Android link flags, `build/rust` target dir and ignored machine-local files.
3. In the crate's `Cargo.toml`, set `[lib] crate-type = ["cdylib", "staticlib", "rlib"]`: the
   `cdylib` is the Android library, the `staticlib` is what Xcode links. The library name must
   equal the name in `android/app/src/main/AndroidManifest.xml`
   (`dev.frust.nativeLibrary`) and the iOS `-l<name>` flag. Depend on `frust-ui` as the
   generated manifest does (`frust = { package = "frust-ui", .. }`).
4. End `src/lib.rs` with `frust::app!(RootComponent, setup = { .. })`. It generates the Android
   JNI exports and the iOS C-ABI exports. Desktop preview also needs the scaffold's `src/main.rs`
   (not tested here).
5. Wire the machine-local paths: `frust run -d <android|ios>` and `frust build` refresh
   `android/local.properties` and the `ios/FrustEmbedding` symlink from `cargo metadata`, as
   `frust create` does. `frust build apk` was used here: it printed the two wiring lines and then
   stopped at the release-signing gate, before Gradle.

If the crate is a member of a workspace with path dependencies, the generated app needs no
`[workspace]` table of its own and none is added. The same steps worked from a member crate
(`app`, depending on a sibling `core` crate by path). Two differences: cargo ignores
`[profile.*]` tables in a member's manifest (and warns), so keep them in the workspace root, and
`Cargo.lock` lives at the workspace root. The wiring reads `cargo metadata`, so it does not
depend on where the target directory is.

What ran: `cargo check` on the host and `cargo check --target aarch64-linux-android --lib`
passed in both layouts (offline, from a warm cache); the wiring wrote `android/local.properties`
and the symlink, and `frust doctor` resolved both embeddings. What did not run: the Gradle
build, `cargo ndk` packaging, any Xcode build, and any device run.

## Version Pins

The pins this unit owns, under [DEVELOPMENT.md](DEVELOPMENT.md)'s Version-Pin Policy (pins are
LAW; re-run the row's tripwire after touching it, and never run a blind `cargo update`):

| Pin | Why | Tripwire |
|---|---|---|
| `notify 8` minor (`frust-cli`-only) | `frust run --watch`'s filesystem watcher (`ctrlc` floats, shared by `frust-drive`/`frust-cli`, unpinned; `frust-drive`'s copy now enables the `termination` feature so `frust-drive::interrupt` also catches SIGTERM/SIGHUP — needed to scrub the plaintext release-signing file on a CI runner's kill, not just Ctrl-C. Visible consequence: interrupting a `--release` `frust run` before logcat streaming begins now exits 130 for SIGTERM/SIGHUP, where an unhandled signal previously exited 143/129; the logcat phase's own Ctrl-C-means-stop override still exits 0 for all three signals) | `cargo test -p frust-cli` |
| `rmcp 2.2` minor (`frust-mcp`-only; 2.0.0 fixes GHSA-9pj6-vhgr-3mwh, 3.x is the sessionless protocol rewrite) | `frust-mcp`'s MCP Streamable-HTTP server. `default-features = false` trims rmcp's client/reqwest/auth/elicitation surface this tool-only, loopback-bound server never uses; `server` + `transport-streamable-http-server` are the transport it runs, `macros` pulls in the `#[tool_router]`/`#[tool]`/`#[tool_handler]` attribute macros the tools are built from | `cargo test -p frust-mcp` |
| `axum 0.8` (`frust-mcp`-only) | The HTTP layer rmcp's `StreamableHttpService` nests into | `cargo test -p frust-mcp` |
| `base64 0.22` (`frust-mcp`; also `frust-shell-common`, optional under its `hotpatch` feature) | Encodes/decodes screenshot payloads in MCP tool results; in the app, decodes each devtools patch chunk | `cargo test -p frust-mcp` && `cargo check -p frust-shell-common --features hotpatch` |
| `tokio-util 0.7` (`frust-mcp`/`frust-dap`; also `frust-tui`, see [TUI_DEVELOPMENT.md](TUI_DEVELOPMENT.md)) | The `CancellationToken` graceful-shutdown seam, wired to Ctrl-C in `frust-mcp`'s standalone `run` and to the workbench's start/stop toggle for both embedded servers in `frust-tui` | `cargo test -p frust-mcp` && `cargo test -p frust-dap` && `cargo test -p frust-tui` |
| `icns 0.4` (`frust-drive`-only, `default-features = false` + `pngio`) | `icons::generate_icns`'s macOS `.icns` authoring — the same crate `cargo-bundle` itself uses; `pngio` is the only feature the PNG-only pipeline needs (drops the default JPEG-2000 codec) | `cargo test -p frust-drive` |
| `object 0.37.3` (`frust-drive`-only, `default-features = false` + `read`/`write`/`elf`/`macho`/`coff`) | The hot-patch builder's object IO: the base image's symbol table (`hotpatch::symbols`), the undefined-symbol stub object (`hotpatch::stub`), link interception and the fat/thin links. The 0.37 line pairs with `gimli` 0.32 as dx 0.7.10's `addr2line` does | `cargo test -p frust-drive --lib hotpatch::` && `bash scripts/ci/hotpatch-canary.sh` |
| `ar 0.9.0` (`frust-drive`-only) | Reads rlib members and writes the fat link's archive (`hotpatch::fat_link`, `hotpatch::android`) | as `object` |
| `gimli 0.32.3` (`frust-drive`-only, `default-features = false` + `read`/`std`) | The DWARF reader behind the L3 layout gate (`hotpatch::layout`) | `cargo test -p frust-drive --lib hotpatch::layout` && `bash scripts/ci/hotpatch-canary.sh` |
| `rustc-demangle 0.1.28` (`frust-drive`-only) | v0 demangling for the seam identity gate (`hotpatch::seams`): a changed component `State` type is a new seam symbol | `cargo test -p frust-drive --lib hotpatch::seams` |
| `pdb 0.8.0` (`frust-drive`-only, `cfg(windows)`; dx 0.7.10's version) | A Windows image's symbol table comes from its PDB (`hotpatch::pe`), and the Windows layout gate reads PDB type records (`hotpatch::pdb_layout`) | CI's `hotpatch-windows` job — commands in [TESTING.md](TESTING.md) |

`.ico` authoring adds no new crate: `icons::generate_ico` rides the `ico` feature of the `image`
pin RENDER already owns exactly (see [RENDER_DEVELOPMENT.md](RENDER_DEVELOPMENT.md)) —
`image::codecs::ico`.

`frust-drive` also shares `frust-tui`'s `toml_edit` pin — see
[TUI_DEVELOPMENT.md](TUI_DEVELOPMENT.md).

### External-tool / template-side pins (not Cargo dependencies)

| Pin | Why | Tripwire |
|---|---|---|
| `cargo-packager 0.11.8` exact (external tool, shelled out to by `desktop_build::installer`) | Builds `.dmg`/NSIS/WiX/`.deb`/`.AppImage` installers over an assembled bundle; `frust doctor`'s `CargoPackagerValidator` gates on an exact version match (non-fatal — only `frust build --installer` needs it) and reports the install hint `cargo install cargo-packager --version 0.11.8 --locked` on a mismatch or absence | `cargo install cargo-packager --version 0.11.8 --locked` smoke + `cargo test -p frust-drive` |
| `winresource 0.1` (generated app's own build-dependency, `crates/frust-drive/templates/app/Cargo.toml.tmpl` — not a workspace pin) | Embeds `windows/icon.ico` plus file/product version into the compiled `.exe`; the actively-maintained fork of `winres`, unmaintained since 2021 | scaffold e2e (`create_e2e`) |

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
`frust-dap`'s launch-config generation (the DAP settings dialog's `g`, which refreshes the entry; or
the automatic flow, which fires on an app launch and, on a fresh server bind, for the active
session's project — and never rewrites a frust entry already present) writes a matching
`.vscode/launch.json` entry through `frust_dap::ide_config::vscode`, so a project the TUI has
configured needs no manual `launch.json` edit — only the extension itself installed
(`npx @vscode/vsce package` → a local `.vsix`, run by hand, not by any workspace gate — Node.js is
not required to build or test the Rust workspace).

## See Also

- [DEVELOPMENT.md](DEVELOPMENT.md) — prerequisites, build/run/test gates, version-pin policy
- [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md) — the unit's design
- [TUI_DEVELOPMENT.md](TUI_DEVELOPMENT.md) — the workbench front-end's pins
