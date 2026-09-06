# Frust - TUI Code Standards

Conventions specific to `frust-tui`. Everything shared with the rest of the workspace —
language idioms, error handling, naming, anti-patterns, testing, comment conventions — lives in
[CODE_STANDARDS.md](CODE_STANDARDS.md) and binds here too; this doc records only what is
additional for the workbench.

## TUI Conventions

- **`ui` renders `&AppState`, never mutates the engine** — all state changes happen in
  `engine::update`; a render fn taking `&mut` state is a layering violation.
- **Every mouse action has keyboard parity** (e.g. the welcome Create button: `Enter`/`c`) —
  mouse support is additive, never the sole path.
- **Commands/keybindings have one registry**, `engine::palette::commands` — the palette and
  help overlay both render from it; never a parallel list.
- **fdemon and fdemon-pro are F0X IT LLC's own products** — code may be copied and adapted
  directly (not merely studied as a pattern). Adapt to frust-tui conventions and re-theme through
  the TUI `Theme` — never import an fdemon palette constant or hardcoded color.
- **New TUI surfaces get a Part-B section of the design workbook**
  (`workflow/plans/features/frust-tui/design/tui-design-workbook.html`) before or alongside
  implementation; shipped keybindings are reflected back into the workbook so it stays the
  binding source of truth.

## See Also

- [CODE_STANDARDS.md](CODE_STANDARDS.md) — the shared conventions the workbench also follows
- [TUI_ARCHITECTURE.md](TUI_ARCHITECTURE.md) — the unit's design
- [TUI_DEVELOPMENT.md](TUI_DEVELOPMENT.md) — the unit's version pins
