# Frust - WIDGETS Code Standards

Theming and animation conventions for the WIDGETS unit (`frust-widgets`, `frust-theme`) — the
rules a baseline-widget, catalog, or third-party design-system author writes against.
Everything shared with the rest of the workspace — language idioms, error handling, naming,
anti-patterns, interaction semantics, semantics, testing, comment conventions — lives in
[CODE_STANDARDS.md](CODE_STANDARDS.md) and binds here too.

## Theming & Animation Conventions

- **Paint-time resolution is always safe; layout-time-baked resolution is only safe under
  the `set_theme` → `ChangeFlags` contract.** Most themed widgets resolve tokens from
  `PaintCtx` every paint pass and self-refresh on a live theme swap for free.
  `Text`/`TextInput` instead bake resolved glyph color into the shaped layout at LAYOUT
  time; a widget adding layout-time-baked resolution depends on relayout actually happening,
  so treat a theme change as forcing `ChangeFlags::LAYOUT`, not just `PAINT`.
- **Resolve theme tokens with an unthemed-fallback constant per resolved value.** A themed
  widget looks up `Theme::from_paint_ctx(ctx)`/`from_layout_ctx(ctx)`, falling back to a
  local constant (e.g. `Button`'s `FILL`/`RADIUS`) when no theme is threaded. Precedence is
  **explicit builder value > theme > fallback** (see `Text`'s `color_explicit` flag).
- **Token-not-hardcode: a widget authors against a `Theme` field first; a bare local
  constant is the documented fallback, not the default.** A hardcoded metric/color is a
  defect once a matching `ColorScheme`/`ShapeScale`/`Elevation`/`GlassScale`/`MotionScheme`
  field exists. Only a genuine token-scale gap earns a hand-tuned constant, and it stays
  named, doc-commented, and states *why* no token applies.
- **A contested or unsourced design fact is resolved against a primary source and cited with
  a retrieval date, not left as a guess** — record `<source>, retrieved <date>` in the
  module doc, alongside the **Community-approximate** marker
  ([CODE_STANDARDS.md](CODE_STANDARDS.md)'s Language Idioms) for values that stay
  genuinely unsourced.
- **Event-pass code never reads a theme — `EventCtx` carries none.** Only
  `LayoutCtx`/`PaintCtx` thread a theme; a metric an event handler also needs stays a plain
  constant read from both passes (`TextInput`'s `PAD_X`/`PAD_Y`/`CARET_W` precedent).
- **No `Instant::now()` in `frust-core`/`frust-widgets`.** Time enters the framework only
  from a shell, as the `FrameTime` passed into `RenderRoot::paint`/`PaintCtx::frame_time` —
  desktop reads its own `Instant` epoch; Android/iOS pass through the platform's frame
  clock. Widget code only *differences* two `FrameTime`s, never reads a wall clock directly.
- **An animation controller advances during paint, not on a timer.** A widget holds an
  `anim::AnimationController`, calls `advance(ctx.frame_time())` once per paint and, while
  it returns `true`, calls `PaintCtx::request_frame()`. **A layout-affecting animation calls
  `request_layout()` instead** (implies `request_frame`) — reserve bare `request_frame` for
  a paint-only animation, or the mobile intra-frame layout skip leaves it unresized
  ([SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)'s frame-pipeline data flow).
- **State-layer opacity has one source: `frust_material::state_layer`'s constants**
  (`HOVER_OPACITY`/`FOCUS_OPACITY`/`PRESSED_OPACITY`/ `DRAGGED_OPACITY`, M3 `StateTokens`,
  `plugins/material`) — a design-system widget imports them rather than hardcoding overlay
  opacity, taking the **maximum** of concurrently-active states, never their sum.
  `frust_material::list_item` is the reference consumer for the hover state specifically, and
  shows all three parts of the contract: it claims hover from its own uncaptured `Move` arm,
  latches the same hit test into `StateLayer::set_hovered` with a redraw gated on its
  changed-return, and re-syncs that flag from `PaintCtx::is_hovered` every paint
  (see `docs/CODE_STANDARDS.md`'s Interaction Semantics for the full convention).
- **Glyph's token set adds three resolution precedents.** A per-status color with no
  `ColorScheme` field (Success/Warning/Info) resolves `Theme::extension::<StatusPalette>()`
  first, before a role that already has one (Error) resolves it directly. `GlyphInk` is
  never brightness-swapped like a scheme role. Accent role split: `primary`/`on_primary` is
  accent text/icon ink, `primary_container`/`on_primary_container` is the bright fill —
  conflating the two is the catalog's most common accent bug.
- **A transition pattern's default timing resolves from `Theme.motion`, never a hand-rolled
  duration, and collapses under `reduce_motion`.**
  `PatternSwitcher`/`AnimatedOpacity`/`AnimatedScale` resolve `MotionScheme`'s
  duration/easing/spring tokens by default (an explicit `.timing(...)` call always wins);
  every pattern substitutes a short linear crossfade under `reduce_motion` instead of a
  bespoke variant.
- **A perpetual decorative loop calls `PaintCtx::request_frame_paced`
  (`TickClass::CosmeticLoop`), never bare `request_frame`.** `request_frame` stays
  `TickClass::Transition` (unpaced) — correct for a spring or any transition with a
  user-visible endpoint. A shimmer/spinner/pulse with no endpoint requests the paced class
  instead, letting the mobile frame gate throttle it to `MotionScheme::cosmetic_loop_rate`,
  and must still honor `reduce_motion` (freeze in place, stop requesting frames).
  **Input-driven frames are never paced.** A loop far slower than the cap (a ~500ms caret
  blink against a 30Hz shimmer) names its own cadence with `request_frame_paced_at(interval)`
  instead of the bare call — still `CosmeticLoop`-classified and still `reduce_motion`-honoring,
  just at an explicit interval rather than the theme's default rate. `TextInput`'s caret is the
  shipped example, with one deliberate exception to the freeze-in-place rule above: it freezes
  **visible** rather than hidden (a position cue must stay legible) while still dropping all
  frame requests when frozen.
- **Design-system code targets `frust_widgets::authoring` (reached as `frust::authoring`), never a
  sibling design-system crate.** A baseline widget never imports `frust-glyph`/`frust-material`/
  `frust-cupertino`; the container/callback plumbing, event routing, and callback erasure every
  widget needs live in the public `authoring` module instead — the same surface the three
  built-in design-system plugins themselves consume, enforced structurally now (each is a
  separate crate whose only production dependency is `frust`, so it cannot reach anything
  `authoring` doesn't re-export); `PRESSED_OPACITY`'s `frust_material::state_layer` re-export is
  compatibility-only. `PageTransition::Custom` needs an
  explicit `Timing::Duration`/`Timing::Spring` (`Timing::ThemeDefault` falls back to the M3
  default, 300ms + `Curve::Emphasized`); `reduce_motion` collapses it only programmatically,
  and an interactive edge-swipe pop calls it like every preset.
- **A new container implements `Widget::visit_children` via `authoring::visit_children!`,
  never by hand-writing an empty override.** The default (`Widget::visit_children`'s no-op) makes
  a container that skips this invisible to `WidgetTree::inspect`/devtools — its children exist in
  the retained tree but never show up in an inspector. Name every `ChildPod`-holding field to the
  macro (`visit_children!(leading, children)`); a field shape behind its own row/slot struct
  implements the toolkit's `VisitPods` trait for that struct instead and stays on the same seam
  (see WIDGETS_ARCHITECTURE.md's Data Flow).
- **A design system installs itself via `set_default_theme` + (if it bundles fonts)
  `register_app_fonts` from an `app!` `setup` block — never `Component::init` (no kept ordering
  contract) or `set_app_theme` (pins brightness, breaking platform dark/light following).**
  `frust_glyph::install()`/`frust_material::install()`/`frust_cupertino::install()` are the
  built-in callers; only Glyph's also registers fonts.

## See Also

- [CODE_STANDARDS.md](CODE_STANDARDS.md) — the shared conventions every widget also follows,
  including Interaction Semantics (`Widget::event`) and Semantics Conventions
- [WIDGETS_ARCHITECTURE.md](WIDGETS_ARCHITECTURE.md) — the unit's design
