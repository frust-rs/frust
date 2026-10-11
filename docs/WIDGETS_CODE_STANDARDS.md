# Frust - WIDGETS Code Standards

Widget child-list, theming and animation conventions for the WIDGETS unit (`frust-widgets`,
`frust-theme`) — the rules a baseline-widget, catalog, or third-party design-system author
writes against. Everything shared with the rest of the workspace — language idioms, error
handling, naming, anti-patterns, interaction semantics, semantics, testing, comment
conventions, and the widget-authoring rules (erasure at the API boundary, the
`frust::authoring` seam) — lives in [CODE_STANDARDS.md](CODE_STANDARDS.md) and binds here too.

## Child Lists

- **Keyed lists are all-or-nothing, and keys must be unique.** `keyed(key, view)` marks a
  `Flex` child list for identity-based reconciliation; a mixed or duplicate key set
  `debug_assert!`s and falls back to positional matching in release (never panics live). A
  matched reorder relocates the existing widget rather than rebuilding it.

- **Children are a sequence; write tuples for mixed types.** `Column`/`Row`/`Stack`, the
  builders' `.children(..)` and every catalog list parameter take `impl ViewSeq<State, M>`
  (declare the marker generic `M` on the API; callers never name it). Pass a tuple of up to 12
  views when the children differ in type (`Column((app_bar(), body, footer))`), nest a tuple
  beyond 12, and keep `Vec<V>` for homogeneous or dynamic lists. A tuple reconciles by position;
  `keyed(..)` identity needs a `Vec`. An optional child is `.when(cond, || v)` /
  `.when_some(opt, |x| v)` — or an `Option` element in a tuple, which drops its slot the same way.
  An iterator goes through `views(iter)`; a bare `.collect()` passed to a sequence API needs
  `.collect::<Vec<_>>()`, and a turbofish caller of such an API writes `::<State, _>`.
- **A catalog list helper erases once, inside.** `fn items<M>(seq: impl ViewSeq<S, M>)` calls
  `seq.extend_views(&mut self.children)`; never ask callers for `Vec<AnyView>` or wrap elements
  in `any()` at the call site. A list that must stay erased (a `Vec<AnyView>` field, a
  `ChildPod`-fed helper) is marked `// erasure: keep <why>` for the tripwire.

## Erase at the API Boundary

Extends the shared rule in [CODE_STANDARDS.md](CODE_STANDARDS.md): erasure happens once, inside
the API. Reach for these idioms before `any()`: fluent builders (`column().child(..)`), tuples
for mixed child types, `views(iter)` for iterators, `either(cond, || a, || b)` for two arms, and
`impl View` / `impl ViewSeq` helper parameters. Use `Either` (or `either(..)`) only when the two
arms have different concrete view types. When both arms produce the same concrete type, compute the
differing props or children in an `if` and construct that one type once: `Either` reconciles a
Left/Right swap as a teardown and remount, so the subtree loses in-flight gestures and can orphan
focus, where the same type reconciles in place (an app migration hit this in button, gesture
detector and safe-area wrappers). The four APIs once taking pre-erased views now
erase internally: `frust-material`'s `button_group` icon family, `split_button`'s `leading_icon`,
`frust-beui`'s `extra_rows`, and `clean-signals-frust`'s `async_view`.

A site that must stay erased carries `// erasure: keep <why>` in one of six places: on the call's
own line, alone on the line above it, on the first line of an enclosing `vec![..]`, `let` statement
or `match`, on the enclosing fn signature (the `fn` line, its `->` line, or alone above the `fn`),
on an accumulator `let`, or inside the call. The accumulator form is a marker on, or alone above, a
`let [mut] v` whose type or initializer names `AnyView`: it covers every erasure call that is an
argument of `v.push(..)`, `v.insert(..)` or `v.extend(..)` in the same fn (not other vecs, not a
shadowing re-binding). The in-call form is an own-line marker strictly inside an erasure call's
parentheses, or directly before the argument holding it in the statement-level call; this is what
rustfmt produces for a multi-line `any(..)`. It covers that call for rule U and the T1-T7 keep
exemption; a marker inside a nested closure or block does not escape, and a bare marker does not
count. rustfmt moves a `{ // erasure: keep ..` trailer onto the first body line; that form is
accepted as fn-scoped. The rustfmt-stable placements are the end of the `->` line with `{` on the
next line, the end of the `any(..)` line, the accumulator `let` line (or alone above it), and the
in-call own-line form. The marker must carry a reason: a bare
`// erasure: keep` does not satisfy rule U (the checker enforces this). A fn-signature marker
covers every call in that fn, so its reason must cover them all. Do not add a marker merely to
silence the tripwire; [TESTING.md](TESTING.md) describes what `scripts/ci/erasure-check.sh`
enforces.

Codemod rule T7 skips some helpers (listed per bucket in its output). A helper whose same-file
call is taken by `&` (e.g. `build_child(&self.x_view())`), or is bound by an untyped
`let x = name(..);` and later borrowed as a bare `&x` or `&mut x` in the same block, is excluded as
"taken by reference". A helper with a lifetime parameter or an elided reference parameter is
rewritten to `impl View<S> + use<..>` listing type parameters only, in any edition, because `View`
is `'static` and a captured borrow would make the opaque type non-`'static`; an argument-position
`impl Trait` skips with a note. A local fn shadows a builder only in its own module and descendants.
Non-private helpers (`T7-visible`) are not rewritten without `--t7-visible`, because callers in
other modules may rely on `AnyView`; a caller picking such a helper two-arm in another module
needs `either` there, so the migration crosses module boundaries.

## App Code Style

Binds every user-facing snippet: templates, READMEs, rustdoc, the website and playground code
strings. The canonical example is `crates/frust-drive/templates/app/src/lib.rs.tmpl`.

- One `Component` per screen; `build` returns `impl View<Self::State>` and reads as one
  declarative tree (`scaffold(body).app_bar(..).fab(..)`, fluent `column().child(..)`).
- Tuples, `views(..)` and `either(..)` only where a builder cannot express the shape.
- Helpers that keep `build` shallow are associated fns in a separate inherent `impl X { .. }`,
  called `Self::name(..)` — never free view fns, never extra methods inside `impl Component`.
- Stateful pieces are child components via `component(..)`.
- Every name is imported; no inline `frust::icon(..)` or `frust_material::fab(..)` paths.
- No `+ use<>` unless required.
- The app root derives `Default` (`frust::app!` default-constructs it, so root config is a
  literal, not a field) and `frust::app!(Root, setup = { .. })` comes last.
- Event closures type their state (`|count: &mut u32| ..`); rustfmt 2024.

## Theming & Animation Conventions

- **Paint-time resolution is always safe; layout-time-baked resolution is only safe under
  the `set_theme` → `ChangeFlags` contract.** Most themed widgets resolve tokens from
  `PaintCtx` every paint pass and self-refresh on a live theme swap for free.
  `Text`/`TextInput` instead bake resolved glyph color into the shaped layout at LAYOUT
  time, and `Text` also bakes its font family there when opted in with `.themed_family(..)`;
  a widget adding layout-time-baked resolution depends on relayout actually happening,
  so treat a theme change as forcing `ChangeFlags::LAYOUT`, not just `PAINT`.
- **Resolve theme tokens with an unthemed-fallback constant per resolved value.** A themed
  widget looks up `Theme::from_paint_ctx(ctx)`/`from_layout_ctx(ctx)`, falling back to a
  local constant (e.g. `Button`'s `FILL`/`RADIUS`) when no theme is threaded. Precedence is
  **explicit builder value > theme > fallback**, decided per value: `Text`'s
  `color_explicit`/`family_explicit` flags are independent (`.color(..)` sets one,
  `.family(..)` the other, `.style(..)` both), so an explicit color never blocks a themed family.
- **A component that takes its text family from the theme resolves it at layout.** Text built
  with `text(..)` opts in via `.themed_family(ThemeTextType::..)` naming its role — only the
  family resolves; size, weight, line height and tracking stay the view's own. A widget shaping
  its own text runs reads `Theme::from_layout_ctx(ctx)` in `layout` and keys its shaped-run
  cache on the resolved style, so a theme swap reshapes it (`frust-material`'s `dialog` and
  `badge` show one of each; shadcn's `crate::text::themed_family(style, theme, role)` and beUI's
  `crate::text::{themed_style, role_family}`, consumed through
  `Label`/`LabelRun`/`WrappedRun::layout_themed`, are the same opt-in for a catalog's
  self-shaped runs, each resolving the role's slot with
  `ThemeTextType::style_in(&theme.type_scale)` — the same lookup `.themed_family(..)` makes —
  instead of keeping its own copy of the role-to-slot table). A build-time `.family(..)` or a
  hardcoded font stack is a legitimate choice too — `TypeScale` has no monospace slot, so mono
  text has no role to name — but neither a live theme swap nor
  `frust_testing::frame::pin_type_scale` can redirect it. A baseline `TextInput` has no
  themed-family seam at all — its `effective_style` resolves only color from the theme, never
  family — so a field keeps whatever explicit or system family it was built with
  (`textinput-no-themed-family` in [LIMITATIONS.md](LIMITATIONS.md)).
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
- **Glyph's token set adds five resolution precedents.** A per-status color with no
  `ColorScheme` field (Success/Warning/Info) resolves `Theme::extension::<StatusPalette>()`
  first, before a role that already has one (Error) resolves it directly. `GlyphInk` is
  never brightness-swapped like a scheme role. Accent role split: `primary`/`on_primary` is
  accent text/icon ink, `primary_container`/`on_primary_container` is the bright fill —
  conflating the two is the catalog's most common accent bug. `--fg-dim` has no `ColorScheme`
  role at all, so it resolves to `on_surface_variant` in **every** position — fill or text
  alike, not just as a fill — reading brighter than the source's literal hex (`toggle`'s
  off-knob test pins the shipped delta: themed dark paints `#a39c88` where the source authors
  `#6b6556`). A border authored as alpha-over-fg is a **per-widget call, not a backdrop
  rule**: `toggle`'s track border strokes a true alpha stroke over `on_surface` (its backdrop
  is caller-arbitrary), while the catalog's container-class widgets — accordion, appbar,
  card, badge, empty_state, navbar, dialog, list, menu — stroke the pre-flattened `outline`
  role directly.
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
  Each built-in design-system crate's `install()` is a caller; which of them also register fonts
  is recorded once, in [PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md)'s Design-System Plugins.

- The same **explicit builder value > theme > fallback constant** precedence binds a `ThemeExtensions`
  payload identically to a plain token: a consumer (a design-system plugin or a fully external
  catalog) checks its own explicit override first, then `Theme::extension::<T>()`, and only then a
  hardcoded fallback — see
  [NATIVE_WIDGETS_ARCHITECTURE.md](NATIVE_WIDGETS_ARCHITECTURE.md)'s typeface ladder for the
  shipped instance.

## See Also

- [CODE_STANDARDS.md](CODE_STANDARDS.md) — the shared conventions every widget also follows,
  including Interaction Semantics (`Widget::event`) and Semantics Conventions
- [WIDGETS_ARCHITECTURE.md](WIDGETS_ARCHITECTURE.md) — the unit's design
