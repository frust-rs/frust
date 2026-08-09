# Doc Policy

**Set by:** agent
**Scale:** large (225,723 source LOC → 8 doc units, 3 stacks: Rust + Kotlin/Android + Swift/iOS)
**Structure:** hub-and-spoke (all spokes central in `docs/`, prefixed `<UNIT>_<DOCTYPE>.md`)

## Budgets

| Doc | Target | Cap |
|-----|--------|-----|
| spoke (`docs/<UNIT>_<DOCTYPE>.md`) | 400 | 600 |
| `docs/ARCHITECTURE.md` — pure link index | 150 | 200 |
| `docs/DEVELOPMENT.md`, `docs/CODE_STANDARDS.md` — shared-content indexes (repo-wide gates/rules + links) | 420 | 600 |
| `docs/REVIEW_FOCUS.md` — never split, consumed whole by the review agents | 200 | 280 |
| `CLAUDE.md` | 80 | 100 |

## Doc Units

`A`/`D`/`C` = which of `<UNIT>_ARCHITECTURE.md` / `_DEVELOPMENT.md` / `_CODE_STANDARDS.md` exists.
Partial coverage is expected — a unit with nothing to say beyond the shared index gets no spoke.

| Unit | Packages | Spokes |
|------|----------|--------|
| `CORE` | crates/frust-core, frust-scene, frust-reactive, frust, frust-paths | A D |
| `RENDER` | crates/frust-render, frust-text | A D |
| `WIDGETS` | crates/frust-widgets, frust-theme | A C |
| `SHELLS` | crates/frust-shell-{common,desktop,android,ios} | A D |
| `PLUGINS` | crates/frust-plugin, plugins/{shared-preferences,secure-storage,camera,clipboard,haptics,iap,clean-signals-frust,database} | A D C |
| `NATIVE_WIDGETS` | plugins/native-widgets | A |
| `CLI` | crates/frust-cli, frust-drive | A D |
| `TUI` | crates/frust-tui | A D C |

**Excluded** (consumers, not units — index line + own README only): examples/huddle, examples/shadertoy, examples/glyph-catalog, examples/layer-bench, examples/no-catalogs.

NATIVE_WIDGETS' pins and conventions ride the PLUGINS spokes (shared Apple FFI pins, one plugin
charter). Conventions binding more than one unit — the sanctioned-unsafe register, interaction
and semantics conventions, the platform-view contract — stay in the shared indexes, not a spoke.
`docs/TESTING.md` and `docs/LIMITATIONS.md` are auxiliary curated docs outside this schema set.
