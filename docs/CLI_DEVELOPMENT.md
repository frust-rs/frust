# Frust - CLI Development

Template/scaffold development and version pins owned by the CLI unit (`frust-cli`,
`frust-drive`). Shared prerequisites, build/run commands, the standard verify gate, and the
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

`frust-drive` also shares `frust-tui`'s `toml_edit` pin — see
[TUI_DEVELOPMENT.md](TUI_DEVELOPMENT.md).

## See Also

- [DEVELOPMENT.md](DEVELOPMENT.md) — prerequisites, build/run/test gates, version-pin policy
- [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md) — the unit's design
- [TUI_DEVELOPMENT.md](TUI_DEVELOPMENT.md) — the workbench front-end's pins
