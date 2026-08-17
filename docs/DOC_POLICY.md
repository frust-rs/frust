# Doc Policy

**Set by:** agent
**Scale:** large (225,723 source LOC → 9 doc units, 3 stacks: Rust + Kotlin/Android + Swift/iOS)
**Structure:** hub-and-spoke (all spokes central in `docs/`, prefixed `<UNIT>_<DOCTYPE>.md`).
Re-derived at the 8→9 unit trigger (adding `DEVTOOLS`) and confirmed: `DEVTOOLS` shares the
tooling-isolation charter/verify-gate/pin policy with every unit rather than reading in isolation.

## Budgets

| Doc | Target | Cap |
|-----|--------|-----|
| spoke (`docs/<UNIT>_<DOCTYPE>.md`) | 400 | 600 |
| `docs/ARCHITECTURE.md` — pure link index | 150 | 200 |
| `docs/DEVELOPMENT.md`, `docs/CODE_STANDARDS.md` — shared-content indexes (repo-wide gates/rules + links) | 420 | 600 |
| `docs/REVIEW_FOCUS.md` — never split, consumed whole by the review agents | 200 | 280 |
| `CLAUDE.md` | 80 | 100 |

## Doc Units

`A`/`D`/`C` = which of `_ARCHITECTURE.md`/`_DEVELOPMENT.md`/`_CODE_STANDARDS.md` exists; a unit with nothing beyond the shared index gets no spoke.

| Unit | Packages | Spokes |
|------|----------|--------|
| `CORE` | crates/frust-core, frust-scene, frust-reactive, frust, frust-paths | A D |
| `RENDER` | crates/frust-render, frust-text | A D |
| `WIDGETS` | crates/frust-widgets, frust-theme | A C |
| `SHELLS` | crates/frust-shell-{common,desktop,macos,windows,linux,android,ios} | A D |
| `PLUGINS` | crates/frust-plugin, plugins/{shared-preferences,secure-storage,camera,clipboard,haptics,iap,clean-signals-frust,database,i18n,glyph,material,cupertino,shadcn} | A D C |
| `NATIVE_WIDGETS` | plugins/native-widgets | A |
| `CLI` | crates/frust-cli, frust-drive, frust-mcp, frust-dap | A D |
| `TUI` | crates/frust-tui | A D C |
| `DEVTOOLS` | crates/frust-devtools, frust-devtools-protocol | A |

**Excluded** (consumers, not units — index line + own README only): examples/huddle, examples/shadertoy, examples/glyph-catalog, examples/playground, examples/design-system-sample, editors/vscode-frust.

NATIVE_WIDGETS' pins/conventions ride PLUGINS' spokes (shared Apple FFI pins, one plugin charter).
Conventions binding more than one unit (sanctioned-unsafe register, interaction/semantics
conventions, platform-view contract) stay in the shared indexes, not a spoke. `docs/TESTING.md`
and `docs/LIMITATIONS.md` are auxiliary curated docs outside this schema set.
