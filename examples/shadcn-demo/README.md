# shadcn-demo

The `frust-shadcn` design-system plugin's desktop gallery: every shipped
component, at least once, with a non-default variant beside the default. This
is the vehicle for the plugin's first live-window (desktop) runtime gate —
hover, cursor, overlays, and both bundled fonts (Inter, JetBrains Mono) all
get their first visual verification here.

Root-workspace member (covered by the `examples/*` glob in the root
`Cargo.toml`, not standalone) — desktop-only, no Android/iOS output.

## Run

```
cargo run -p shadcn-demo
```

Opens a real winit window titled "Shadcn Demo". A left-hand nav (built from
shadcn `button`/`separator`, not a bespoke sidebar widget) selects six
gallery pages:

1. **Primitives** — badges, avatars, kbd, labels, progress/spinner/skeleton,
   marker, breadcrumb, alert, card, item, empty state, buttons/button-group
   (including a disabled example).
2. **Controls** — checkbox, switch, radio group, toggle, toggle group,
   slider, accordion, collapsible, tabs, pagination.
3. **Inputs & Table** — input, textarea, input-group, native-select, a
   labelled field (including an invalid/error example), a table, an
   attachment card, chat bubbles, and a scroll area.
4. **Overlays** — dialog, alert-dialog, sheet, drawer, and the command
   dialog, each pushed as a transparent navigator page via
   `frust_shadcn::overlay::show_*`.
5. **Anchored** — popover (with placement knobs and a bottom-edge flip
   demo), tooltip, hover-card, dropdown menu, context menu (see its on-page
   caption for the desktop-shell left-click-only caveat), select, and
   combobox.
6. **Theming** — the seven `ShadcnBase` presets plus a light/dark toggle,
   driven live through `frust::set_app_theme`, plus the font-verification
   row (Inter body text, a JetBrains Mono `kbd` group).

See the project owner's task report (or `docs/DEVELOPMENT.md` once updated)
for the full per-page desktop-gate checklist.
