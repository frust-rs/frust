# shadcn-demo

The `frust-shadcn` design-system plugin's desktop gallery: every shipped
component, at least once, with a non-default variant beside the default. This
is the vehicle for the plugin's live-window (desktop) runtime gate — hover,
cursor, overlays, motion, and both bundled fonts (Inter, JetBrains Mono) all
get their visual verification here.

Root-workspace member (covered by the `examples/*` glob in the root
`Cargo.toml`, not standalone) — desktop-only, no Android/iOS output.

## Run

```
cargo run -p shadcn-demo
```

Opens a real winit window titled "Shadcn Demo". The app shell is the catalog's
own `sidebar_provider`: a collapsible icon-mode nav panel (groups, labels, an
active state, a badge, per-item and per-group actions, one nested sub-menu, a
header and footer slot, and the edge rail) beside a `sidebar_inset` holding a
top bar with the `sidebar_trigger`. Ctrl/Cmd+B toggles the panel; so does the
rail, the trigger, and — on the Layout page — a second provider's own trigger.

Ten pages:

1. **Primitives** — badges, avatars, kbd, labels, progress/spinner/skeleton,
   marker, breadcrumb, alert, card, item, empty state, buttons/button-group
   (including a disabled example).
2. **Controls** — checkbox, switch, radio group, toggle, toggle group,
   slider, accordion, collapsible, tabs, pagination.
3. **Inputs & Table** — input, textarea, input-group, native-select, a
   labelled field (including an invalid/error example), `input_otp` (digit,
   alphanumeric and disabled groups), a table, an attachment card, chat
   bubbles, and a scroll area.
4. **Layout** — `carousel` (controlled full-width slides and an uncontrolled
   one-third basis), `resizable` (a controlled horizontal group with grips and
   an uncontrolled vertical one), and a second `sidebar` in its `floating`
   variant docked right.
5. **Overlays** — dialog, alert-dialog, four sheet sides plus a scrolling and a
   close-button-less variant, four drawer directions plus a snap-point drawer,
   and the command dialog — each pushed as a transparent navigator page via
   `frust_shadcn::overlay::show_*`, and each **animating out** on dismissal.
6. **Anchored** — popover (with placement knobs and a bottom-edge flip demo),
   tooltip, hover-card, dropdown menu, context menu (right-click, live),
   select, and combobox — all **kept mounted** with `.open(flag)` so their exit
   ramps play.
7. **Chat** — `message_scroller` over the `message` family: stick-to-bottom,
   detach-on-scroll-up with viewport preservation, the floating scroll-to-end
   button, and re-stick. A composer appends; "Simulate incoming" delivers the
   next canned arrival.
8. **Questionnaire** — a five-item flow (single-choice, multi-choice, required,
   free-text, skippable) with letter shortcuts, progress, validation, and an
   answer summary on submit.
9. **Data Table** — the full data-table recipe as app code: sort comparators,
   a name filter, page slicing, a selection set with a tri-state select-all
   checkbox, a column-visibility menu and per-row action menus.
10. **Theming** — the seven `ShadcnBase` presets plus a light/dark toggle,
    driven live through `frust::set_app_theme`, plus the font-verification row
    (Inter body text, a JetBrains Mono `kbd` group).

## Layout note: bounded constraints

`frust_shadcn::overlay`'s mounting contract requires an overlay host (a
navigator page or a full-area `Stack`) to sit under **bounded** constraints — a
`scroll_view` hands its child an infinite height, which a host coerces to zero.
`sidebar_inset` wraps its children in a plain `Column`, whose inflexible
children are laid out under an unbounded main axis, so this app re-tightens the
height once (a `SizedBox` at the window's own logical height, read from
`frust::WindowMetrics`) and every page then either scrolls inside that slot or
hosts overlays against it. `src/main.rs`'s module docs carry the full chain.

See the project owner's task report (or `docs/DEVELOPMENT.md`) for the
per-page desktop-gate checklist.
