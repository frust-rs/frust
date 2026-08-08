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

## See Also

- [DEVELOPMENT.md](DEVELOPMENT.md) — prerequisites, build/run/test gates, version-pin policy
- [TUI_ARCHITECTURE.md](TUI_ARCHITECTURE.md) — the unit's design
- [TUI_CODE_STANDARDS.md](TUI_CODE_STANDARDS.md) — the unit's conventions
