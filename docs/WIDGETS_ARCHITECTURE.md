# Frust - WIDGETS Architecture

## Overview

WIDGETS covers `frust-widgets`, the baseline widget set (layout, controls, text, gestures,
navigation, platform-view slots), built over `frust-core`/`frust-scene`/`frust-text`/`frust-theme`
through a shared authoring toolkit; and its sibling `frust-theme`, a design-token crate bundling
color/type/shape/elevation/motion/glass values into a `Theme` that widgets recover from context and
app code reads reactively. The built-in design systems are not part of this unit: they are sibling
plugin crates in PLUGINS (`frust-glyph`/`frust-material`/`frust-cupertino` and the two
external-origin ports, see [PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md)), built on the same
public authoring toolkit this doc describes.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how WIDGETS relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `frust-widgets::authoring` | Public container/callback toolkit every widget in the crate builds from, instead of touching `frust-core` primitives directly — reachable by app code as `frust::authoring` (the facade's re-export, CORE unit); also carries the `VisitPods` trait / `visit_children!` macro, the crate's introspection seam (see *Data Flow*) |
| `frust-widgets` (baseline) | Baseline layout containers and interactive leaf widgets (text, forms, gestures, scrolling, a virtualized `ListView`, a four-slot `Scaffold`) |
| `frust-widgets::canvas` | `CanvasView`/`CanvasWidget` — declarative custom painting over `PaintScene`, local-space, no semantics node |
| `frust-widgets::pinch` | `PinchRecognizer` — a pure two-contact pinch state machine — and `pinch_detector`, the wrapper view feeding it from a gesture's contacts |
| `frust-widgets::pan_zoom` | `PanZoomView`/`PanZoomWidget` — one child under a drag-pan/pinch-or-wheel-zoom transform, with inertia and a `PanZoomController` handle |
| `frust-widgets::motion` | Implicit-animation and transition-pattern vocabulary |
| `frust-widgets::nav` | Imperative page-stack navigator, declarative router, and shared-element hero transitions — internally split across six `nav/*.rs` files (see below); single public path via `navigator.rs`'s re-exports |
| `frust-widgets::physics` | Pluggable scroll-motion strategy (`ScrollPhysics` trait, `OverscrollEffect`, `Simulation` ports, platform-adaptive defaults) that `ScrollView`/`ListView` consult instead of hard-coding a feel — see *Scroll Physics* below |
| `frust-widgets::scroll_controller` | `ScrollController` — a cloneable, reactive-free handle onto a `ScrollView` or `ListView`'s offset/item position — see *Scroll Physics* and *Virtualized ListView* below |
| `frust-widgets::overlay` | The widget-author face of CORE's overlay portal: anchored placement geometry (`place`/`OverlayPlacement`), the `OverlaySlot` a widget hosting its own floated pod keeps, and the declarative `overlay_portal` wrapper |
| `frust-widgets::drag` | Drag-and-drop session model: `DragCoordinator`, `draggable()` source, `drag_target()` target, `auto_scroll_zone()`, `reorderable_list()` — single public path `drag`, re-exported flat from the crate root and the `frust` facade |
| `frust-widgets::platform_view` | Native-sibling compositing slot and input-shield wrapper for translucent surfaces |
| `frust-theme` | `Theme` aggregate and its token tables (color, type, shape, elevation, motion, glass); carries no design-language token module of its own — only the neutral/language-free floor (`Theme::neutral()` and friends) |

`nav/`'s files are all private and re-exported through `navigator.rs`, so the crate's public path is
unchanged: the widget/stack core, the ambient-context idioms, the option/callback vocabulary, the
controller, the view constructors, and the edge-swipe driver, with tests alongside in
`navigator_tests/` themed one file per concern.

## Layer Dependencies

`frust-widgets` depends on `frust-core` (`View`/`Widget` traits, layout, paint/event context,
semantics), `frust-scene` (renderer-agnostic draw commands), `frust-text` (text layout/editing
engine backing `frust-theme`'s typography tokens), `frust-theme` (design tokens bundled into
`Theme`), `peniko`/`kurbo` (shared color/geometry primitives), and the `image` crate (image decode
path). `frust-theme` is `frust-widgets`' primary consumer.

The one reverse edge is `frust-theme` depending on `frust-core`, but only for context-accessor
sugar (reading the active `Theme` from a context handle) — this does not create a cycle back into
`frust-widgets`.

Every container and interactive widget in `frust-widgets` is built through the crate's own public
authoring toolkit rather than touching `frust-core` primitives directly. The boundary is now
enforced structurally by the crate graph rather than a source-scan test: the three built-in design
systems are separate crates (`frust-glyph`/`frust-material`/`frust-cupertino`, PLUGINS unit) whose
only production dependency is the `frust` facade (`default-features = false`) plus `kurbo`/
`peniko` — they physically cannot reach a `frust-core` primitive `frust` doesn't re-export, the
same seam any third-party design system builds against (see *External Design-System Contract*
below). `frust::authoring` is the toolkit both the baseline set and every design-system plugin
build from.

## Data Flow

- Theme delivery (detail; the cross-unit summary lives in [ARCHITECTURE.md](ARCHITECTURE.md)): a
  shell owns the active `Theme`; widgets recover it type-erased from paint/layout context so
  `frust-core` stays theme-agnostic, while app code reads a cloned `Theme` via reactive context.
- Widget resolution precedence: explicit builder value > theme token > unthemed-fallback constant,
  generally re-resolved every paint; `Text`/`TextInput` instead bake the resolved color at layout
  time, and `Text` also bakes the font family of an opted-in type-scale role (`.themed_family`).
- Container plumbing: every container and interactive widget is built through the shared public
  authoring toolkit rather than touching `frust-core` primitives directly.
- Introspection flow: a container implements `Widget::visit_children` (CORE unit) via the
  authoring toolkit's `VisitPods` trait and `visit_children!` macro — one line naming its
  `ChildPod`-holding fields — so `WidgetTree::inspect`/`RenderRoot::inspect()` can enumerate its
  children; a hand-rolled child list (a row/slot struct behind an enum) implements `VisitPods`
  by hand instead and stays on the same seam. Both the baseline set and all five design-system
  plugins use it; see WIDGETS_CODE_STANDARDS.md for the authoring convention this obliges.
- Design-system layering: the five design-system plugins (PLUGINS unit) sit above the baseline
  set and the authoring seam, each an ordinary sibling crate assembling its own `Theme` via
  `ThemeBuilder`'s editors over `frust-theme`'s neutral floor rather than consuming a token module
  `frust-theme` ships for it.
- Glass/Cupertino chrome flow: `frust-cupertino` owns its own "Liquid Glass" `GlassScale` recipe
  (moved out of `frust-theme` with the rest of the catalog) and paints from it when supported,
  degrading to `frust-theme`'s neutral `GlassScale::opaque_material()` otherwise.
- Motion flow: implicit-animation and transition widgets resolve default timing from `Theme.motion`,
  collapsing to a short crossfade under `reduce_motion`; decorative loops request paced frames so
  the mobile frame gate can throttle them. `reduce_motion` is a floor, not an assignment: every
  baseline defaults it `false`, and a mobile shell OR's the OS accessibility preference over whatever
  the active theme authored (`effective = authored || os`, see SHELLS_ARCHITECTURE.md for the
  sensors) — an authored `true` is never un-reduced by an OS report of `false`.
- Navigation flow: `Navigator`/`Router` manage a page stack and declarative routes over the same
  container plumbing; `hero()` morphs a tagged child between pages during transitions.
  - **Alpha-zero paint redirect:** the split-crossfade presets (`M3SharedAxisX`, `M3FadeThrough`,
    `Glyph`) are hard, non-overlapping splits — before the split only the leaving page ramps 1→0,
    after it only the entering page ramps 0→1, so at most one page is visible at any instant and
    both resolve to alpha 0 at the split itself. Never assume a dual-visible crossfade window:
    `IosPush` keeps both visible (parallax + dim, never reaching 0) and `ReducedCrossfade` is a
    genuine crossfade, but the presets above are not. A page resolving to alpha 0 still runs its
    paint pass — hero-rect capture and other paint-time state — redirected into
    `frust_core::DiscardScene` so no scene command is emitted, because the engine rasterizes a
    layer in full before applying its alpha and an invisible page is otherwise wasted GPU work.
  - **Snapshot bracket eligibility:** a page surviving that redirect brackets its paint with
    `PaintScene::push_snapshot`/`pop_snapshot` when the transition is programmatic (not an
    interactive edge-swipe) and the frame's hero directives for it are empty — unconditionally then,
    even at alpha/scale identity, since the bracket itself is what tells a caching renderer the body
    is worth caching (`push_snapshot`'s default still emulates the transform+layer pair for a plain
    recorder). Each page and each `motion::switcher` child draws a process-unique snapshot key once
    for its pod's lifetime, and the bracket rect's origin follows the pod's absolute paint origin,
    keeping RENDER_ARCHITECTURE.md's frame-relative fingerprint slide-invariant.
  - **Navigator observation seams:** `NavigatorController::transition()` publishes a `Copy` `TransitionState`
    (active, progress, direction, depth pair, generation) on an `Rc<Cell<_>>` so chrome outside the
    subtree can observe transitions; reads during paint from a widget painted *after* the navigator
    are frame-exact, reads during `Component::build` are one frame stale. `PushOptions::on_visibility`
    and `PageVisibility` (Current/Visible/Covered) provide page-level visibility observation; the opt-in
    `cull_covered_builds` (default false) skips rebuilds for covered pages, but `on_cleanup` does **not**
    fire under a covering push (retained-state guarantee).
  - **Pop-result delivery:** `push_for_result`'s `on_result` callback needs `&mut State`, which the
    state-free view diff doesn't carry, so a pop still queues it — but the queuing rebuild also
    raises CORE's pending-result-flush flag, which the same rebuild drains via a `Housekeeping`
    broadcast (see CORE_ARCHITECTURE.md). An eager pop delivers its result within the rebuild that
    applied it; an interactive edge-swipe pop delivers on its settle frame. No further input event
    is required either way.
  - **RouteNavigator:** A thread-safe (`Arc<Mutex<Vec<NavRequest>>>`) navigation handle carrying no closures
    or reactive types, making it `Send + Sync` and usable via `provide_context` from any thread; off-thread
    requests wake the shell and apply in the next rebuild before the navigator reconciles.
  - **Route params:** Query parameters merge **under** path segment captures; path captures take precedence
    over query params of the same name, and named-route `path_for_name` round-trips unused params as query.
  - **Route-state observable (R-B1):** `NavigatorController::route_stack()`/`route_generation()` publish a
    `RouteStack` (route identities bottom→top, depth, generation) and a diffed `NavChange`
    (Initial/Push/Pop/Replace/Reset) from every committed-mutation site inside `publish_state()`. Staleness
    contract: an in-flight interactive edge-swipe pop is uncommitted, so the observable still reports the
    pre-swipe stack until the settle-frame publish; a cancelled swipe publishes nothing (the pre-swipe stack
    was already correct). Frame-accurate drag chrome wants `NavigatorController::transition()` instead.
  - **shell_route (nested navigators):** `shell_route(inner, builder, children)` is a pathless route binding
    a subtree to a second `NavigatorController` the app owns — the shell page stays retained on the enclosing
    stack while its children resolve onto `inner` (go_router's `ShellRoute`). A chain crossing the boundary
    splits there, one structural op per controller; the **keep rule** makes in-shell navigation issue zero
    ops on the enclosing controller once the shell page is already placed, so it and its retained inner
    navigator are never dropped. A deep link resolved before the shell page exists pre-queues its inner
    segment onto `inner`'s op queue and drains on `inner`'s first `build` — no flash, no second navigation.
    Chrome inside the shell page reads the **inner** navigator's `route_stack()` (or the facade's
    `RouteObserver` over it) for live in-shell state, never the enclosing stack, since a keep-ruled
    navigation never restamps the shell page's own route entry.
  - **Gesture policy (interactive edge-swipe):** `NavigatorWidget::swipe_armable` gates arming on
    `BackPolicy` — a `DismissAnimated` or `Veto` top page refuses to arm outright (arm-refusal, not a
    dismiss-signal bump) — and resolves the navigator-wide default by rank: page override
    (`PushOptions::pop_swipe`) > navigator explicit (`NavigatorView::pop_swipe`) > platform slot
    (`platform_pop_swipe`) > preset default (on for `PageTransition::IosPush`, off otherwise); the facade
    sets the platform slot from `cfg!(target_os = "ios")`. **R-B3-inner:** a left-edge `Down` does not
    capture, so a nested navigator underneath can arm too; the outer navigator only defers at the `Move`
    steal, after reading back the inner's claim — both navigators legitimately arm on `Down`.
  - **overlay_host() constructor:** A `NavigatorView` with pop_swipe disabled and no-op transition
    (not a new widget). Back-button interest is arbitrated by **rank taken from the wiring entry
    point**, never from when or how often a controller wires: an overlay host always outranks a plain
    navigator, and among navigator peers the order is **innermost-first** (most-recently-wired wins,
    LIFO) — mirroring Android's `OnBackPressedDispatcher` dispatch order and UIKit/SwiftUI's
    pop-from-top behavior. Host-first is the same topmost-layer rule, not an exception to it —
    go_router's outermost-first order on Android is the counter-example this deliberately avoids. A
    host with no overlays reports no interest and defers to the inner navigator. Registrants are
    released by mount liveness (`NavigatorController::is_mounted()`), not by a wire-count heuristic.
  - **Back reach follows input routing (R23):** ranking (above) decides who wins *among* claimants;
    a separate reach check decides who may claim at all. A navigator whose hosting page is not in
    its host navigator's `input_routed_pages()` reports `back_interest() == false` outright,
    whatever its own stack looks like — so a press can never pop an off-screen nested stack while
    the visible page stays put. Reach is input routing, not painting: a page under a *transparent*
    overlay is still `PageVisibility::Visible` but claims no back. A top-level navigator has no
    hosting page and is unconditionally reachable, so single-navigator apps are unaffected.
  - **Controller binding is structural:** a `NavigatorView`'s published cells (depth, back interest,
    transition, mount count) bind to the `NavigatorController` it was last built or rebuilt against;
    a controller swap is detected by identity (not just view type) and re-bound in the same rebuild
    pass, so ops and published state can never target different controllers and a swapped-away-from
    controller reliably reports unmounted.
  - **NavigatorWidget semantics (R23):** Now implements `Widget::semantics`, forwarding via
    `ChildPod::semantics_child` to exactly the pages input routing can reach (covered pages and pages
    under modals omitted outright), honoring the input-parity invariant documented in
    CODE_STANDARDS.md. Back arbitration's reach check (above) derives from this same
    `input_routed_pages()` set, so it is the single reach definition shared by input, semantics, and
    back.
- Safe-area flow: `safe_area(child)` pads by the resolved `WindowInsets` on its enabled edges, then
  **removes** what it consumed from its subtree via `WindowInsets::consuming` — installed for the
  child through `LayoutCtx::with_window_insets`/`PaintCtx::with_window_insets` — so a self-insetting
  descendant, or a nested `safe_area`, does not inset the same edge twice. `.minimum` is extra
  padding, never consumed, and `view_insets` (the IME) always flows through unchanged. A pod floated
  through the overlay portal carries its owner's consumed `WindowInsets` along in its
  `OverlayEntry`, so a paint-time read inside floated content still agrees with the layout-time one
  it saw under its owner's `LayoutCtx`; hit testing and event routing are unaffected.
  `WindowInsets` also carries `corner_insets: CornerInsets` — four physical corners (`top_left`,
  `top_right`, `bottom_left`, `bottom_right`), each the window control's protrusion beyond the safe
  area in logical px; `safe_area` neither pads by nor removes them (`consuming` copies them) and
  `padding()` ignores them.
- Scaffold flow: `scaffold(body)` assembles the four fixed chrome slots (`app_bar`/`body`/`bottom_bar`/`fab`)
  most screens compose around, theme-agnostic (a design system's own bar/nav-bar/FAB widgets plug into the
  slots from app code). **R-B4-inset:** the Scaffold itself consumes no window inset — `app_bar` and
  `bottom_bar` self-size for the top/bottom inset the same way (reading `ctx.window_insets()` in their own
  `layout` — Material's `app_bar`/`search_app_bar` self-inset the top and the left/right edges by default and
  `navigation_bar` the bottom only, see [PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md); the sliver and
  bottom app bars do not, wrap them in `safe_area`), and `body` is never pre-inset; `fab` is the one slot
  the Scaffold insets on the caller's behalf, floating above `bottom_bar` when present and off the raw
  window edge otherwise. **Bar contract:** the Glyph app bar and the Material top and sliver app bars
  (listed in [PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md)) shift their leading slot right by
  `top_left.width` and their trailing edge left by `top_right.width` whenever that corner's height > 0;
  the Scaffold is unchanged. The shift assumes the bar spans the window's top edge: layout cannot see a
  bar's window-space position and corners are never consumed, so there is no automatic detection — a bar
  hosted in a pane, sheet or dialog opts out with `corner_shift(false)`. Other catalogs' top bars
  (Cupertino's navigation bar, the shadcn/beUI headers) do not yet apply the rule; see
  `corner-insets-ios-26-only` in [LIMITATIONS.md](LIMITATIONS.md).
- Overlay flow: anchored placement is framework-owned — `place(anchor, content, area, placement)`
  and `OverlaySlot`, which resolve a side, a cross-axis alignment, an offset, a collision flip and a
  clamp-back-inside, pure and total — so a widget, a catalog and an app all place a floated surface
  the same way instead of each deriving the geometry again. A widget hosting its own pod keeps an
  `OverlaySlot` and forwards four calls to it (rebuild; layout, loosely against the window;
  paint-time placement and registration; event, before its own children); `overlay_portal(child)`
  is the declarative case, anchoring a surface to that child's bounds and diffing it against the
  same application state. Three coordinate spaces meet in a slot — window space (the registered rect
  and the broadcast payload), owner-local space (what the owner's `event` sees) and pod space — and
  the slot owns both the translation and the pod's capture lifetime. `frust-shadcn`'s tooltip and
  hover card ride the slot directly, the tooltip registering `Tooltip`/`Transparent` (so every press
  reaches the main tree as if it were not there) and the hover card `Floating`/`Interactive` (so its
  content can actually be pressed); the design-system catalogs' own `overlay::anchored` hosts still
  carry their plugin-local copies of the pattern. See CORE_ARCHITECTURE.md's Overlay Portal for the
  mechanism itself.
- Drag-and-drop flow: an ambient, Rc-shared `DragCoordinator` (one per scope, cloneable like
  `ScrollController`) is the single source of truth for a session — idle/armed/dragging/dropping/
  cancelled — and its typed `Box<dyn Any>` payload, so several targets type-gate against one
  handle. `draggable()` arms on a primary press and begins once a `DragPolicy` is crossed (a mouse
  travels past a distance threshold, a touch press holds through a long-press — `Auto` picks per
  press from the pointer source), floating a ghost through the overlay portal anchored at
  `OverlayAnchor::Window` (see CORE_ARCHITECTURE.md's Overlay Portal) and cancelling on Escape or a
  release with nothing hovered. `drag_target()` resolves against bounds the coordinator's registry
  collects at paint — never a hit test — so later registration wins an overlap and a drop can land
  across containers (two kanban columns, a list and a trash bin). Keyboard lift/cycle/drop
  (`move_to_next_target`/`move_to_previous_target`, Enter/Space/arrows) drives the same coordinator
  from the source's own focus; semantics advertise `Role::Button` on the source and target with
  `Action::Click` wired as a lift/drop toggle only (see `drag-ghost-pod-no-semantics` in
  [LIMITATIONS.md](LIMITATIONS.md)). `auto_scroll_zone()` drives an attached `ScrollController`
  toward whichever edge the dragged pointer sits inside (`AutoScroll::edge_px`/`max_px_per_s`).
  `reorderable_list()` composes both primitives over keyed rows with `N + 1` gap targets, firing
  `on_reorder(state, from, to)` on an actual move. A desktop `InputEvent::FileDrop` opens an
  `ExternalFiles` session so a `drag_target::<Vec<PathBuf>>` accepts an OS file drop the same way a
  widget-originated session would (see [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)).
- Platform-view flow: `platform_view()`/`shield()` publish native-compositing slots and input-shield
  rects each frame for the shell layer to reconcile against native views.
- Canvas flow: `canvas(paint)` takes `Fn(&mut dyn PaintScene, Size, &PaintCtx)`; the widget
  translates to its own origin and clips to its bounds before invoking the closure, so painting
  happens entirely in local space. `.size(Size)`/`.expand()` (default) pick the sizing policy;
  `.on_hit(Fn(Point, Size) -> bool)` narrows hit-testing and gates `.on_tap`/`.on_pointer`;
  `.repaint_key(impl Hash)` requests a repaint when an external fingerprint changes, since the paint
  closure itself is not comparable across a rebuild.
- Pinch flow: `PinchRecognizer::handle(PointerId, &PointerEvent, time_ms) -> Option<ScaleEvent>`
  reduces a gesture's tracked contacts (the first two, in `Down` order) to the same
  `ScaleEvent`/`ScalePhase` stream a desktop source produces. `pinch_detector(child)` captures the
  pointer and calls `EventCtx::capture_contacts()` on the first primary `Down`, forwards the first
  contact to the child unchanged, and steals an in-progress child gesture with one synthesized
  `Cancel` once a pinch begins (`on_scale` never fires from a `Cancel`); a desktop `InputEvent::Scale`
  reaches `on_scale` only when the child ignores it first. Nesting two detectors is unsupported.
  `pinch_detector` captures the ambient multi-contact veto cell (`scroll.rs`'s
  `ambient_scroll_veto`) on its claiming `Down` and holds it raised while it tracks a second
  contact, so a pinch beginning over a single-finger press inside a `ScrollView`/`ListView`
  survives the claimant's own travel past slop; it still never joins the Down-time nested-scroll
  claim itself (see below), unlike `pan_zoom`.
- Pan-zoom flow: `pan_zoom(child)` places its child in a `ChildPod` transformed by
  `Affine::translate(offset) * Affine::scale(scale)` (`PanZoomTransform`), so frust-core's
  transformed-pod seam (see CORE_ARCHITECTURE.md) inverse-maps the child's hit-testing and events. A
  primary `Down` is offered to the child first; an ignored press pans on the claimant's moves. An
  `InputEvent::Scale` (desktop ctrl/⌘+wheel, trackpad pinch) is likewise offered to the child first
  and applied here when ignored; a touch pinch is recognised in-widget by a `PinchRecognizer` fed
  from every primary `Down`'s contacts, so a second finger routes here even when the child owns the
  first. On a `Down` the child ignores, the view publishes the nested-scroll claim (the same ambient
  cell `ScrollView`/`ListView` use, both drag directions registered unconditionally) so an enclosing
  scrollable defers to it; when the child owns the press instead, the view captures the ambient
  multi-contact veto cell and holds it raised while a second contact is tracked, so a pinch
  beginning over that child-owned press is not taken over by the enclosing scroll surface once the
  claimant crosses slop — the veto clears, and the surface's ordinary takeover resumes, once back
  down to one contact. A `Scroll` arriving between `Scale`
  events clears the stale `last_focal` left by an open bracket (a macOS trackpad pinch whose
  modifier released mid-gesture finishes as wheel events) so a later unrelated wheel notch does not
  anchor on it. `.min_scale`/`.max_scale` (default `0.25`/`8.0`) clamp zoom; `.inertia(bool)` (off by
  default) glides a released pan on a per-axis `FrictionSimulation`; `.on_transform(Fn(&mut State,
  PanZoomTransform))` notifies after every change; `.controller(PanZoomController)` attaches a
  cloneable handle (`jump_to`/`fit_to_bounds`/`fit_rect`/`transform`/`viewport_size`/`content_size`)
  whose commands apply at the next layout or paint. Panning is unbounded; only panning glides.

## Key Types

| Type | Purpose |
|------|---------|
| `Theme` / `DesignLanguage` / `ThemeBuilder` | `frust-theme`'s aggregate design-token bundle, tagged by baseline (Material3/Cupertino/Glyph), plus a fluent editor |
| `ColorScheme`, `TypeScale`, `ShapeScale`, `Elevation`, `MotionScheme`, `GlassScale`, `StatusPalette` | Individual token-group types composed into `Theme`; `frust-theme` ships only each one's `neutral()` floor — a design system assembles its own values via `ThemeBuilder`'s per-group editors |
| `NativeTypefaces` / `FontFace` | A `ThemeExtensions` payload carrying a design system's own native-control font bytes (button/body face pair); no baseline `frust-theme` value attaches it — `frust-glyph::baseline()`, `frust-material::baseline()`, `frust-shadcn::theme()`, and `frust-beui::theme()` all do — see NATIVE_WIDGETS_ARCHITECTURE.md's theme ladder |
| authoring module (`build_child`/`rebuild_child`/`teardown_child`/`rebuild_children`, `route_event`, `VisitPods`/`visit_children!`) | The sanctioned seam for authoring any widget against `frust-core`, including child-introspection |
| `Navigator` / `Router` / `hero()` | Page-stack and declarative routing plus shared-element transitions |
| `ButtonStyle`, `ScrollInfo`, `IconData`/`IconSource`, `ImageSource`/`ImageFit` | Small per-widget config/state types shared across the baseline widget set |
| `ScrollController` / `ScrollSubscription` / `AnimateTo` / `ItemAlignment` | Programmatic-scroll handle for `ScrollView`/`ListView`, its `on_change` guard, the animate-to duration/curve, and the keyed-row landing alignment — see *Scroll Physics* and *Virtualized ListView* |
| `ListView` / `ListViewWidget` | Baseline virtualized list: windowed rebuild-time materialization, positional or keyed (`ChildKey`) row identity, optional variable extents, refresh/overscroll parity with `ScrollView` |
| `NavigatorController::transition()` / `TransitionState` / `PageVisibility` | Navigation state observation seams |
| `RouteNavigator` / `NavRequest` | Off-thread-safe navigation handle (Arc-backed plain data, no reactive types) |
| `NavigatorController::route_stack()` / `RouteStack` / `NavChange` | Route-state observable: page stack as route identities, diffed change label — see Data Flow |
| `shell_route()` | Pathless route binding a subtree to a second, app-owned `NavigatorController` — nested-navigator ("shell") composition |
| `BackPolicy` | Per-page back-press disposition (`Pop`/`DismissAnimated`/`Veto`) gating both a back press and edge-swipe arming |
| `scaffold()` / `ScaffoldView` | Four-slot screen layout (`app_bar`/`body`/`bottom_bar`/`fab`), self-sizing inset consumption (R-B4-inset) |

## Architectural Facts & Constraints

### External Design-System Contract
The three built-in design systems — `frust-glyph`/`frust-material`/`frust-cupertino`
(`plugins/{glyph,material,cupertino}`, PLUGINS unit) — are themselves proof of this contract: each
is an ordinary sibling crate depending on `frust` (`default-features = false`) plus `kurbo`/
`peniko` only, built entirely on the public authoring/theme seam a third party gets too, no
special-cased access. Each follows the same conventions: its catalog is flat re-exported at the
crate root (`frust_glyph::app_bar`, `frust_material::AppBar`, `frust_cupertino::CupertinoButton`),
its token constructors are free functions (`baseline()`, `color_scheme_light()`/`_dark()`,
`type_scale()`, `shape_scale()`, `elevation()`, `motion_scheme()`) rather than an in-crate module
`frust-theme` used to ship, and `install()` seeds the theme with `frust::set_default_theme(baseline())`
from an `app!` `setup` block. `frust-glyph::baseline()` and `frust-material::baseline()` additionally
attach the `NativeTypefaces` theme extension (their bundled monospace and Roboto faces,
respectively) — Cupertino's does not — see Key Types and NATIVE_WIDGETS_ARCHITECTURE.md's theme
ladder.

`DesignLanguage` is `#[non_exhaustive]` with a `Custom(&'static str)` variant (compared by string
content, not interning identity) so a third-party design system can tag its identity without a
breaking enum change; every built-in `==` branch site treats an unrecognized tag as the
neutral/System path by construction. The public seam an external design system builds against is
three-fold: the `frust_widgets::authoring` toolkit (container/callback plumbing, `visit_children!`,
re-exported as `frust::authoring`), `Theme`'s token bus (`ThemeBuilder`'s per-group editors,
`Theme::neutral()` as a language-free baseline), and `ThemeExtensions` for typed, no-lock-in
payloads a baseline doesn't carry (e.g. `NativeTypefaces`, see Key Types) — plus `IconData::resolve`/
`same`, promoted `pub` so an out-of-tree catalog can paint its own icon glyphs the same way a
built-in one does. `examples/design-system-sample` is the reference proof for a genuinely external
crate (outside this repo's own workspace, unlike the three built-ins above) — a themed catalog plus
installer built on `frust`'s public API alone (see ARCHITECTURE.md's Examples table); its one
finding, that `Widget::semantics` cannot be exercised from out of tree, is registered in
LIMITATIONS.md. `plugins/shadcn` (`frust-shadcn`, PLUGINS unit) is the production-scale companion
proof: a 55-component port of a real third-party design system (shadcn/ui) built on this same
seam, not a sample. `plugins/beui` (`frust-beui`) is a second external-origin port on the same
seam — beUI, an 81-part motion-first web catalog — proving the contract holds for a design system
whose whole premise is per-component animation, not just static layout. See
PLUGINS_ARCHITECTURE.md's Design-System Plugins for both.

Three more seams are part of the same public authoring surface: opt-in hover claiming
(`EventCtx::claim_hover`/`PaintCtx::is_hovered`) for state-layer-style interaction chrome, cursor
requests (`EventCtx::set_cursor`/`CursorIcon`, also flat-re-exported as `frust::CursorIcon`) for a
design system's own hover/drag affordances, and the overlay portal — `place`/`OverlaySlot`/
`overlay_portal`, re-exported through `frust::authoring` — for a popover, menu, tooltip or context
menu (see *Overlay flow* above).

### Reactive-Free Design
`frust-widgets` contains no `reactive_graph` symbols crate-wide — the crate is entirely signal-free.
Reactive bridging lives in the CORE unit's facade (`router_glue.rs`, `back_glue.rs`), where observer
callbacks and context providers can reach the reactive runtime. This boundary keeps the widget set
reusable and decouples it from the reactive layer; state machine state in nav/navigator is held in
plain `Rc<Cell<_>>`/`Arc<Mutex<_>>` instead.

### Icon Generation
`crates/frust-widgets/src/icons/mod.rs` is **generated** by `scripts/gen_icons.py` from a hardcoded
`STARTER_SET`. Hand-edits to this file are destroyed on the next `gen_icons.py` run; expand the icon
set by modifying the script's source list, not the generated output.

### Text Widget Alignment
`TextInput`'s live-edited text is always start-aligned, regardless of `TextStyle::align`, because
parley's `PlainEditor` exposes no text-alignment hook. This is a parley limitation, not a frust
design decision; users cannot work around it per-field. `TextView` does honor `align`.

### Text Selection and the Clipboard
Four gestures reach a `TextInput`'s selection and only two raise the toolbar: a **stationary
long-press** selects the word under the press and opens the bar (the only one a touch-only device
has); a **double-tap** selects the word and deliberately opens nothing, since a bar over a word the
user is about to type over is in the way; a **tap inside an existing selection** keeps it and
toggles the bar on the release, so a drag starting inside a selection is still an ordinary caret
drag; a **secondary press** claims focus, moves no caret and toggles the bar (desktop only — no
mobile shell delivers `Secondary`). Everything else puts the bar away: any text change, a blur or
focus release, a scroll, Escape, any primary `Down`, and any applied verb. `Cancel` is the one
exception, touching neither selection nor bar, per the never-mutate-on-cancel convention.

The verbs arrive two ways and converge on one handler. A shell dispatches an
`InputEvent::EditCommand` (a platform edit menu, a hardware clipboard key, a chord it chose to
decode itself); or the widget decodes a chord from a plain `Key` — `ctrl` **or** `meta` plus
`c`/`x`/`v`/`a`, case-insensitively, which covers every desktop platform uniformly, plus the legacy
spellings (`Copy`/`Cut`/`Paste` named keys, `Ctrl+Insert`, `Shift+Insert`, `Shift+Delete`). `alt` is
never a chord modifier, and any other chorded character is consumed rather than typed. Refusals are
the field's own call and still count as *handled*: an obscured field copies and cuts nothing
(neither the buffer nor its bullet mirror), a collapsed selection makes copy and cut no-ops, a paste
sanitised down to nothing inserts nothing, and a disabled field never sees a verb at all because it
never holds focus — a read-only field does (see *Read-only Text Fields* below). Nothing here touches
a host clipboard — see CORE_ARCHITECTURE.md for the write/request slots the shell drains.

The bar itself is somebody else's widget: the field hosts an `OverlaySlot` and fills it from the
process-wide selection-toolbar builder, so `frust-widgets` never names the view that floats and no
builder installed means no bar at all. The pod is mounted and dropped in `rebuild` (the only pass
carrying a `BuildCtx`) and only when the enabled verb set changes, then placed in `paint` against
the selection's bounding box, or the caret rect when the selection is collapsed. Under the `Native`
policy the field floats nothing and only publishes its request. `selection_toolbar()` is the
baseline that builder yields by default — a pill of text buttons for the enabled verbs, following
every other baseline widget's fire-on-up-inside contract, with cut/copy/select-all riding
`EventCtx::dispatch_edit_command` and paste riding `EventCtx::request_paste` because only a shell
may read the host clipboard. Its labels are English-only on purpose: an app or design system
replaces the whole view through `set_selection_toolbar_builder` (or the cooperative set-if-unset
install), which is where localisation belongs.

The same verbs are published on the field's **own** semantics node as accesskit custom actions,
under the predicates the bar is built from rather than on whether the bar is up — the floated pod
contributes no semantics, so a screen-reader user can reach nothing on the bar itself. **Those
actions are advertised but not invocable today:** the shell-to-core accessibility seam carries
`(node_id, action)` and drops the `ActionRequest::data` a custom-action id rides in, so nothing
delivers them; the field's own half is complete the moment one arrives as an `EditCommand`. The
selection range is not published either — accesskit models one as a pair of positions into
`Role::TextRun` nodes, and this field contributes a single leaf carrying its text as a plain value.

### Read-only Text Fields
`TextInput::read_only(true)` makes a field uneditable without dimming it — dimming stays keyed to
`enabled` alone, never to read-only, so a live-styled static mock does not pop to full alpha when it
goes live. Focus and mutation answer separate questions: a read-only field is still **focusable and
copyable** — Material 3's and Apple's HIG's convention, and the plain reading that visible text is
text a user can select. It takes focus on a press, drag-selects, long-presses to a toolbar offering
copy and select-all, and answers those verbs in full, while it refuses typed characters, IME
composition and commits, and the editing/caret-motion keys; cut and paste are answered with no effect
and no `on_change`. A paste chord never asks the shell to read the host clipboard at all, so an
answer that could only be discarded never reaches the host (and never raises iOS's system paste
prompt). Escape still ends the session — a field that can hold focus needs a keyboard way out — and
its caret is drawn but does not blink, since a blink advertises an insertion point this field does
not have. A focused read-only field publishes `ImeState { active: true, suppress_soft_keyboard:
true, .. }` (see CORE_ARCHITECTURE.md's Key Types for the shell obligation this carries).
`enabled(false)` remains the stronger claim: it refuses focus outright regardless of `read_only`.

### Scroll Physics
`ScrollView`/`ListView` (`scroll.rs`/`list_view.rs`) share a pluggable scroll-motion strategy
(`crate::physics`, mirroring Flutter's `ScrollPhysics`) rather than a hard-coded feel: a
`ScrollPhysics` trait (drag mapping, boundary rejection, ballistic simulation, fling thresholds,
spring — composed by parenting via `.chain(parent)`, never inheritance) is orthogonal to
`OverscrollEffect` (how a rejected/held displacement paints — `Translate`/`Stretch`/`None`) and to
`Simulation` (the ballistic curve a physics hands back — Friction/Spring/Clamping/Bouncing ports of
Flutter's own). `default_physics()`/`default_overscroll_effect()` (`crate::physics`) select the
installed pair platform-adaptively — Android: `Clamping` + `Stretch`; every other platform:
`Bouncing` + `Translate` — and both surfaces install that pair unless an app names its own.
`RubberBand`, the flat-resistance feel both surfaces used to hard-code, is no longer any platform's
default; it remains reachable as an explicit `.physics(RubberBand::new())` opt-in, and is the only
physics still driving both widgets' legacy hand-rolled fling/settle path
(`create_ballistic_simulation` returns `None` by design). Wheel input stays a physics-independent
hard clamp on both surfaces. A release's fling velocity carries the interrupted motion's momentum
forward only when it plainly continues it — same direction, and faster than half the physics' own
mapped share of that carried velocity — mirroring Flutter's `ScrollDragController.end` guards
rather than gating on the raw interrupted speed.

Every drag `Move` hands the physics *that move's* raw finger delta against the live position
(Flutter's own per-move convention), so a depth-aware curve reads a real overscroll depth instead of
a fixed zero — at the cost of a path-dependent mapping, where the same total pull split across a
different number of moves need not land on the same pixel. Both widgets accumulate the physics'
boundary-rejected excess as their own edge-pull state, outside the trait, so pull-to-refresh and
`OverscrollEffect::Stretch`'s paint-side intensity both fire under a clamping physics at zero
displacement and not only a bouncing one. Stretch is a paint-only affine scale about the held edge;
no layout pass reads the pull. A ballistic simulation that ends fully pinned outward stops early
rather than pumping the rest of its curve, handing the residual pull to the release-settle path.

**Nested-scroll arbitration** (`scroll.rs`) runs the same ambient-claim shape as the navigator's
edge-swipe arming (**R-B3-inner**, `nav::ambient`'s `SWIPE_CLAIM`, see *Data Flow* above): a scroll
surface pushes a fresh claim cell around the `Down` it forwards (`with_scroll_claim`), the nearest
scrollable reached underneath reports what it could do with the gesture into it (`InnerScrollState`,
nearest-inner pairing — a claim always lands in its immediate enclosing surface's cell, never a
grandparent's), and the outer reads that back at the touch-slop takeover, deferring instead of
taking over when the inner can consume the drag's direction. A claim registers only with real
scroll capacity (`max_scroll_extent > min_scroll_extent`) — without it, a content-fits inner under
an always-accepting physics (the `Bouncing` family) would defer every drag to itself despite having
nothing to scroll, a deliberate UIKit-default deviation from Flutter's own `BouncingScrollPhysics`,
which still claims a fits-viewport surface. The report is a `Down`-time snapshot
and the outer's defer decision is sticky for the rest of the gesture — content that becomes (or
stops being) scrollable mid-drag never registers, and a deferred gesture never hands back — the same
class of accepted tradeoff the navigator's own Down-time claim already lives with. Both `ScrollView`
and `ListView` also run a live **multi-contact veto** alongside that Down-time claim: a per-gesture
cell either surface replaces on every primary `Down` and consults on every `Move` takeover check, so
a nested `pinch_detector`/`pan_zoom` that reports a second contact after the claim snapshot was taken
still suppresses the takeover (see *Pinch flow*/*Pan-zoom flow* above).

`ScrollInfo`'s shape, wheel handling, and its consumers — `frust-shadcn`'s `scroll_area`,
`frust-glyph`'s `app_bar` scroll-collapse — are unaffected: the seam changes only what computes
drag/post-release motion, never `ScrollInfo`'s contract.

**Programmatic scroll (`scroll_controller.rs`).** `ScrollController` is a cloneable, reactive-free
handle `ScrollView::controller`/`ListView::controller` attaches, reachable by app code as
`frust::{ScrollController, ScrollSubscription, AnimateTo, ItemAlignment}` (the facade's
re-export): one surface holds a handle at a time — the most recent attach wins, a rebuild never
steals a handle another surface already holds — and dropping the attached widget detaches it. A write (`jump_to`/`animate_to`, plus
`scroll_to_item` on a keyed list) is recorded, not applied immediately; the attached surface drains
its queue at the start of its next layout or paint (a `ListView` also drains at rebuild, before it
plans its window), so the clamp lands against the freshly measured extent rather than a stale one.
The queue is bounded by superseding within a segment — the offset commands recorded since the last
item command: a jump supersedes every earlier offset command in its segment (cancelling any tween
and setting the position outright), and an animate supersedes an earlier animate but keeps a
preceding jump, its start position — so a segment holds at most one jump and one animate in
whatever order they were recorded; superseding never crosses a queued `scroll_to_item`. Item
commands keep their own ordered channel under a cap of 8 — recording one past the cap drops the
oldest (debug-build log), re-applying the rule when the drop merges two segments — so the whole
queue never exceeds the cap plus two offset entries per segment. A superseded jump no longer raises
a transient near-start/near-end edge notification. A command queued before any surface attaches
still waits for that surface's first layout, so an unattached handle accumulates at most that same
bound rather than growing further. Recording raises CORE's pending-result-flush flag
(`frust_core::mark_pending_result_flush`, see CORE_ARCHITECTURE.md) so a frame-gated mobile shell
runs the frame that applies it even when nothing else is dirty. `jump_to` clamps to `[0,
max_offset]` and stops any fling or release-settle in flight; like `animate_to` and
`scroll_to_item`, it also ends an *established* live drag on the surface the same way a `Down`
does — the rest of the gesture's `Move`s and its `Up` fall through to the child, and the release
starts no fling or settle. A hold still inside touch slop stays armed instead: the slop takeover
and the child's `Cancel` still happen, and the finger can go on to start the drag afterward, the
way Flutter's `jumpTo` during a hold behaves. `animate_to` eases there through a `TweenSimulation`
driven by the same ballistic
pump a release fling uses, so paint cadence and boundary physics are shared, not duplicated — a
user `Down` or wheel interrupts it, a later command replaces it, and the theme's `reduce_motion`
flag collapses it to a `jump_to`. Every post-release start — a fling, a release-settle, a
controller tween — goes through that same single tear-down path first, so a fling, a settle and a
tween are never live together. `is_animating` is true only while the *currently attached* surface
has a live controller tween: a handle that is dropped, taken over by another surface, or swapped
for a different handle mid-tween reports `false`, and the new handle reports the widget's own live
state. Reads (`offset`/`max_offset`/`viewport_extent`) are the surface's last published snapshot;
`on_change` fires only when the published `ScrollInfo` actually changes, after a layout, paint or
event pass. `scroll_to_item(key, ItemAlignment, animated)` is list-only — a `ScrollView` holding
the handle ignores it (debug-build log) — and is covered in *Virtualized ListView* below.
`frust-shadcn`'s `scroll_area` takes `.controller()` too, and a primary-button drag on its thumb
maps pointer travel through the same `jump_to`. A desktop shell does not consult the
pending-result-flush flag the way the mobile frame gate does (see SHELLS_ARCHITECTURE.md's *Mobile
frame path*), so a `jump_to`/`animate_to` issued from a handler that itself requests no redraw
waits for the next desktop frame some other input wakes — the same gap `PanZoomController` already
has.

### Virtualized ListView (baseline)
`ListView`/`ListViewWidget`/`list_view()` live in `frust-widgets` proper (`list_view.rs`), not a
design-system catalog — the facade re-exports them unconditionally regardless of which (if any)
design-system plugin an app depends on. `frust_material::list_view` is a `#[deprecated]` alias set
kept only so an existing call site resolves; new code uses the baseline path.

Each frame's `rebuild` reads the retained widget's own scroll offset and cached viewport to
materialize only the visible window (plus a small buffer) — the only `View` in the framework that
reads retained element state during `rebuild`. Layout only measures pods that already exist and
paint builds nothing, so the row-builder closure never runs outside `rebuild` — the same invariant
every other widget upholds, exercised here against a windowed rather than fixed child set.

**Two row-identity models.** `ListView::builder` reconciles rows by raw index — correct for
append-only, truncate-only, or full-replace data, but a mid-list insert/remove/reorder silently
reattaches a row's retained state to whatever now sits at that index. `ListView::builder_keyed`
adds a `ChildKey` per row (the same identity `keyed()` uses for `Flex`, see CODE_STANDARDS.md) so
state follows the row through a mutation instead; a keyed list additionally re-anchors its scroll
offset in `rebuild`, before windowing, so a prepend/removal above the viewport never visibly jumps
the content the user is looking at **when the mutation is single-sided** — a same-frame mutation on
both sides of the anchor (e.g. a prepend above *and* an append below in one frame) can still miss
the anchor-shift correction itself, a documented, deferred gap (`list_view.rs`'s module doc, *Cache
hygiene*). Unlike `Flex`'s keyed reconciler, a duplicate key on `ListView`
`debug_assert!`s but has **no positional fallback** in release — the first slot claiming a key wins
deterministically, later duplicates rebuild fresh.

**Variable extents are keyed-only.** `.estimated_item_extent(px)` switches a keyed list from the
closed-form uniform path to a measured-by-key extent cache (an estimate for unvisited rows, the
row's own layout height once visited); calling it on a positional list `debug_assert!`s and is
inert in release. A measurement that revises a row's height above the viewport's top edge is
corrected through the same rebuild-time anchoring path — recorded at layout, committed on the
following rebuild so the anchor row never visibly moves. A correction observed mid-fling
accumulates and commits only once the fling settles, so it never fights the fling pump.

**Refresh/overscroll parity with ScrollView** (see *Scroll Physics*, above, for the shared seam
itself). `on_refresh_release` and drag overscroll share `scroll.rs`'s `pub(crate)`
resistance/trigger/settle constants *and* nested-scroll claim/veto cells with `ScrollView`, not just
the constants — `ScrollView` is no longer the framework's only pull-to-refresh-capable or nest-aware
widget, and the default feel is the same platform-adaptive physics rather than flat rubber-band.
`ListView` replaces and consults the same live multi-contact veto `ScrollView` does (see *Nested-scroll
arbitration*, above), so a pinch or pan-zoom beginning over a single-finger press inside a list
survives the claimant's travel past slop the same way it does inside a `ScrollView`. The
clamped-windowing/paint-only-overscroll split (`list_view.rs`'s module doc, *Windowing offset vs.
painted offset*) survives every installed physics unchanged: only what computes the past-edge
displacement differs, never how this widget stores or paints it.

Virtualization exists because eager materialization doesn't scale: an in-repo host bench shows
`ListView`'s rebuild+layout cost staying flat against item count while an eagerly-built
`ScrollView`+`Column` scales roughly linearly, and a structural assertion beside it pins the
windowed-materialization fact itself (not the timing) at `N = 10,000`.

**Scroll-to-item.** A keyed list's `ScrollController::scroll_to_item(key, ItemAlignment, animated)`
(`ItemAlignment`: `Start`/`Center`/`End`/`Nearest`) resolves `key` from the list's measured extents
plus the variable-extent estimate for rows not yet laid out; a key outside the materialized window
costs one `key_of` scan over the data in order (O(item count), not per frame). `ListView` publishes
its own placement offset so the controller's reads agree with what is actually painted. The jump or
animation lands on the estimate first when rows ahead of it were never measured, then settle-and-
correct re-resolves the aligned position each following layout and nudges to it, bounded to 4
layouts or until within 0.5px of the target — a pending request never coexists with an established
drag (the settle loop's drag guard is defensive only) — so an animated request over never-measured
rows can end with a small correction snap rather than retargeting mid-animation.
An unknown key, or any key
on a positional (non-keyed) list, is a no-op
(debug-build log); a `ScrollView` holding the handle ignores an item command the same way.

### Recent Additions
**Overlay, selection and drag-and-drop:** the `overlay` module and `TextInput`'s hosted
`selection_toolbar()` (see *Overlay flow*, *Text Selection and the Clipboard*); the `drag` module —
coordinator, draggable source, drop target, auto-scroll, reorderable list (see *Drag-and-drop
flow*). **Navigator observation:** `TransitionState`/`PageVisibility`; `overlay_host()`; route
params merge query under path captures; `RouteNavigator` off-thread navigation. **Text:**
`TextInput` read-only mode and `content_type` IME hints; `TextView` alignment, `.max_lines`,
`.overflow(TextOverflow)` (see RENDER_ARCHITECTURE.md). **Baseline primitives:**
`container()`/`colored_box()`, `divider()`, `icon_button()`.
