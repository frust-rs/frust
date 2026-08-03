# Doc Policy

**Set by:** agent
**Scale:** large (225,723 source LOC, 22 non-example packages → 8 doc units, 3 stacks: Rust + Kotlin/Android + Swift/iOS)
**Structure:** hub-and-spoke (all spokes central in `docs/`)

## Budgets

| Doc | Target | Cap |
|-----|--------|-----|
| spoke (`docs/<UNIT>_ARCHITECTURE.md`) | 400 | 600 |
| index (`docs/ARCHITECTURE.md`) | 150 | 200 |
| `docs/REVIEW_FOCUS.md` | 200 | 280 |
| `CLAUDE.md` | 80 | 100 |

## Doc Units

| Unit | Packages | Doc path |
|------|----------|----------|
| `CORE` | crates/frust-core, crates/frust-scene, crates/frust-reactive, crates/frust, crates/frust-paths | docs/CORE_ARCHITECTURE.md |
| `RENDER` | crates/frust-render, crates/frust-text | docs/RENDER_ARCHITECTURE.md |
| `WIDGETS` | crates/frust-widgets, crates/frust-theme | docs/WIDGETS_ARCHITECTURE.md |
| `SHELLS` | crates/frust-shell-common, crates/frust-shell-desktop, crates/frust-shell-android, crates/frust-shell-ios | docs/SHELLS_ARCHITECTURE.md |
| `PLUGINS` | crates/frust-plugin, plugins/shared-preferences, plugins/secure-storage, plugins/camera, plugins/clipboard, plugins/haptics, plugins/clean-signals-frust | docs/PLUGINS_ARCHITECTURE.md |
| `NATIVE_WIDGETS` | plugins/native-widgets | docs/NATIVE_WIDGETS_ARCHITECTURE.md |
| `CLI` | crates/frust-cli, crates/frust-drive | docs/CLI_ARCHITECTURE.md |
| `TUI` | crates/frust-tui | docs/TUI_ARCHITECTURE.md |

**Excluded** (consumers, not units — index line + own README only): examples/huddle, examples/shadertoy, examples/glyph-catalog, examples/layer-bench, examples/no-catalogs.

`docs/DEVELOPMENT.md` and `docs/CODE_STANDARDS.md` remain central/shared (no per-unit spokes of those types yet); `docs/TESTING.md` and `docs/LIMITATIONS.md` are auxiliary curated docs outside this schema set.
