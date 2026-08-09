# Frust

Frust is a Rust-native, mobile-first declarative UI framework: a `View`/`Widget` retained tree with
signal-based reactivity, rendered via vello/wgpu, with Android/iOS/desktop shells, a plugin tier,
and CLI/TUI tooling. It is a multi-crate Cargo workspace documented hub-and-spoke — this file is
the index; follow a link below rather than reading source cold.

## Documentation

| Topic | Doc |
|-------|-----|
| Architecture (root index, cross-unit shape) | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) |
| CORE — view/widget lifecycle, layout, reactive, facade | [docs/CORE_ARCHITECTURE.md](docs/CORE_ARCHITECTURE.md) |
| RENDER — GPU render pipeline, text shaping | [docs/RENDER_ARCHITECTURE.md](docs/RENDER_ARCHITECTURE.md) |
| WIDGETS — widget set, design-system catalogs, theme | [docs/WIDGETS_ARCHITECTURE.md](docs/WIDGETS_ARCHITECTURE.md) |
| SHELLS — desktop/Android/iOS host integration | [docs/SHELLS_ARCHITECTURE.md](docs/SHELLS_ARCHITECTURE.md) |
| PLUGINS — OS-capability plugins | [docs/PLUGINS_ARCHITECTURE.md](docs/PLUGINS_ARCHITECTURE.md) |
| NATIVE_WIDGETS — native-control plugin | [docs/NATIVE_WIDGETS_ARCHITECTURE.md](docs/NATIVE_WIDGETS_ARCHITECTURE.md) |
| CLI — `frust` command + drive library | [docs/CLI_ARCHITECTURE.md](docs/CLI_ARCHITECTURE.md) |
| TUI — terminal workbench | [docs/TUI_ARCHITECTURE.md](docs/TUI_ARCHITECTURE.md) |
| Coding conventions (shared across units) | [docs/CODE_STANDARDS.md](docs/CODE_STANDARDS.md) |
| Build, run, test, environment | [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) |
| Test tiers and gates | [docs/TESTING.md](docs/TESTING.md) |
| Accepted limitations register | [docs/LIMITATIONS.md](docs/LIMITATIONS.md) |
| Review priorities and hot spots | [docs/REVIEW_FOCUS.md](docs/REVIEW_FOCUS.md) |
| Doc structure/budget record | [docs/DOC_POLICY.md](docs/DOC_POLICY.md) |

Per-unit `*_DEVELOPMENT.md` / `*_CODE_STANDARDS.md` spokes hang off those last two indexes.

## Must-Know Commands

```
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check && cargo build -p no-catalogs
```

This is the standard verify gate. [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) § Test is canonical —
it also covers the standalone-workspace gates (e.g. `huddle`/`clean-signals-frust`).

## Agent Guardrails

- Version pins are LAW (see [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) § Version-Pin Policy) — never
  bump vello/wgpu/ratatui/etc. independently.
- `examples/huddle`, `examples/shadertoy`, `examples/glyph-catalog`, `examples/playground`, and
  `plugins/clean-signals-frust` are standalone workspaces excluded from the root graph — run their
  gates from their own directories.
- `workflow/` is a separate nested repo — never commit it.
- Doc edits must respect the budgets recorded in [docs/DOC_POLICY.md](docs/DOC_POLICY.md).
- `clean-signals` is git+rev-pinned (`910f626` on `master`) to its public repo —
  `examples/huddle`, `plugins/clean-signals-frust`, and `templates/app`'s clean-signals
  scaffold variant must all resolve the identical git+rev spec (two resolution routes
  would give Cargo two crate identities) — do not change one without the others.
