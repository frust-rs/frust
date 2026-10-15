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
| `frust-widgets` (baseline) | Baseline layout containers and interactive leaf widgets (text, forms, gestures, scrolling, a virtualized `ListView`, a four-slot `Scaffold`, `IndexedStack`) |
| `frust-widgets::canvas` | `CanvasView`/`CanvasWidget` — declarative custom painting over `PaintScene`, local-space, no semantics node |
| `frust-widgets::repaint_boundary` | `repaint_boundary(child)` (`.alpha(f32)`) and the shared `RetainedBoundary`: per frame `DrawRetained` when the subtree is clean, its transform's linear part unchanged, nothing inside published a paint and the ledger holds (key, extent, scale from `PaintScene::current_transform()`); else `PushRetained`+body+`PopRetained` when recording is allowed and the body is visible, else direct paint (kill switch `FRUST_NO_RETAINED_LAYERS` → plain bodies). `motion::AnimatedOpacity::cached(bool)` opts an opacity animation in (`AnimatedScale` unchanged) |
| `frust-widgets::pinch` | `PinchRecognizer` — a pure two-contact pinch state machine — and `pinch_detector`, the wrapper view feeding it from a gesture's contacts |
| `frust-widgets::pan_zoom` | `PanZoomView`/`PanZoomWidget` — one child under a drag-pan/pinch-or-wheel-zoom transform, with inertia and a `PanZoomController` handle |
| `frust-widgets::motion` | Implicit-animation and transition-pattern vocabulary |
| `frust-widgets::nav` | Imperative page-stack navigator, declarative router, and shared-element hero transitions — internally split across six `nav/*.rs` files (see below); single public path via `navigator.rs`'s re-exports |
| `frust-widgets::physics` | Pluggable scroll-motion strategy (`ScrollPhysics`, `OverscrollEffect`, `Simulation` ports, platform-adaptive defaults) the scroll surfaces consult — see *Scroll Physics* |
| `frust-widgets::viewport` / `slivers` | `CustomScrollView`/`ViewportWidget` (the sliver viewport) and the sliver family (`slivers::adapter` holds `sliver_box`); `scroll_core` holds the shared `ScrollSurfaceCore` — see *Slivers and the viewport* |
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
path). `frust-theme` is `frust-widgets`' primary consumer. The one reverse edge, `frust-theme` →
`frust-core`, is context-accessor sugar only and creates no cycle into `frust-widgets`.

Every container and interactive widget in `frust-widgets` is built through the crate's own public
authoring toolkit rather than touching `frust-core` primitives directly. The crate graph enforces
this structurally: the three built-in design systems (`frust-glyph`/`frust-material`/
`frust-cupertino`, PLUGINS unit) depend only on the `frust` facade (`default-features = false`)
plus `kurbo`/`peniko`, so they cannot reach a `frust-core` primitive `frust` doesn't re-export —
the same seam any third-party design system builds against (see *External Design-System
Contract*). `frust::authoring` is the toolkit both the baseline set and every plugin build from.

## Data Flow

- Theme delivery (detail; the cross-unit summary lives in [ARCHITECTURE.md](ARCHITECTURE.md)): a
  shell owns the active `Theme`; widgets recover it type-erased from paint/layout context so
  `frust-core` stays theme-agnostic, while app code reads a cloned `Theme` via reactive context.
- Widget resolution precedence: explicit builder value > theme token > unthemed-fallback constant,
  generally re-resolved every paint; `Text`/`TextInput` instead bake the resolved color at layout
  time, and `Text` also bakes the font family of an opted-in type-scale role (`.themed_family`).
- Introspection flow: a container implements `Widget::visit_children` (CORE unit) via the
  authoring toolkit's `VisitPods` trait and `visit_children!` macro (one line naming its
  `ChildPod` fields) so `RenderRoot::inspect()` can enumerate its children; a hand-rolled child
  list implements `VisitPods` by hand on the same seam. The baseline set and all five plugins use
  it; see WIDGETS_CODE_STANDARDS.md for the convention this obliges.
- Design-system layering: the five design-system plugins (PLUGINS unit) assemble their own
  `Theme` via `ThemeBuilder`'s editors over `frust-theme`'s neutral floor; `frust-cupertino`
  owns its "Liquid Glass" `GlassScale` recipe, degrading to the neutral
  `GlassScale::opaque_material()` where unsupported.
- Motion flow: implicit-animation and transition widgets resolve default timing from `Theme.motion`,
  collapsing to a short crossfade under `reduce_motion`; decorative loops request paced frames. `reduce_motion` is a floor, not an assignment: every
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
  - **Retained pages:** each page carries a `RetainedBoundary` (`PageEntry.boundary`; the switcher's
    `child_boundary`/`exiting_boundary`). A page **at rest paints plain**; only a programmatic
    transition's progress-0 seed frame records the leaving page and the transition frames draw it
    (`DrawRetained`), the entering page of a slide paints plain. A body that used a paint side channel
    (IME, platform view, shield, overlay, selection toolbar, hero) paints every frame, and hero pages
    never draw a page. Fractional slides sit up to half a pixel off; a boundary cannot see a clip from a
    clipping container above it, so it may record a body the engine then refuses to retain.
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
  - **Fallback rank:** a `BackFallback` (an app rule, e.g. back at a tab root returns home) ranks after every
    navigator by role and claims only when none does; it is live while a `frust::back_fallback(&handle, child)`
    wrapper is mounted (plus a first-wire grace) — one in a hidden branch still counts, so gate its interest.
  - **Back reach follows input routing (R23):** ranking (above) decides who wins *among* claimants;
    a separate reach check decides who may claim at all. A navigator whose hosting page is not in
    its host navigator's `input_routed_pages()` reports `back_interest() == false` outright,
    whatever its own stack looks like — so a press can never pop an off-screen nested stack while
    the visible page stays put. Reach is input routing, not painting: a page under a *transparent*
    overlay is still `PageVisibility::Visible` but claims no back. `IndexedStack` is the second reach
    installer: a navigator in a non-selected child claims no back, so per-tab stacks route it to the
    selected tab only. A top-level navigator (no hosting page, no `IndexedStack` above it) is
    unconditionally reachable, so single-navigator apps are unaffected.
  - **Controller binding is structural:** a `NavigatorView`'s published cells (depth, back interest,
    transition, mount count) bind to the `NavigatorController` it was last built or rebuilt against;
    a controller swap is detected by identity (not just view type) and re-bound in the same rebuild
    pass, so ops and published state can never target different controllers and a swapped-away-from
    controller reliably reports unmounted.
  - **NavigatorWidget semantics (R23):** Now implements `Widget::semantics`, forwarding via
    `ChildPod::semantics_child` to exactly the pages input routing can reach (covered pages and pages
    under modals omitted outright), honoring the input-parity invariant documented in CODE_STANDARDS.md.
    Back arbitration's reach check (above) derives from this same `input_routed_pages()` set, so it is
    the single reach definition shared by input, semantics, and back.
- IndexedStack flow: `IndexedStack(index, children)` builds and rebuilds every child (each keeps its state, e.g. a
  tab's navigator stack) but paints, routes input to and exposes semantics for the selected one only; broadcasts reach
  all. Hidden tabs are skipped by the per-pod layout rule and reuse their cached layout when revealed unchanged. Deselecting cancels the child's capture and focus and hides its IME.
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
  area in logical px; `safe_area` neither pads by nor removes them (`consuming` copies them) and `padding()` ignores them.
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
  `OverlaySlot` (whose substituted-state route forwards layout requests to the owner) and forwards four calls to it (rebuild; layout, loosely against the window;
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
  release with nothing hovered. A begun drag vetoes the takeover of every enclosing scroll surface,
  not only the nearest, for as long as the session runs. `drag_target()` resolves against bounds
  the coordinator's registry collects at paint — never a hit test — so later registration wins an
  overlap and a drop can land across containers (two kanban columns, a list and a trash bin).
  Keyboard lift/cycle/drop (`move_to_next_target`/`move_to_previous_target`, Enter/Space/arrows,
  Escape to cancel) is the assistive-technology path through the whole walk, driving the same
  coordinator from the source's own focus; semantics advertise a `Role::Button` source and a
  `Role::Group` target, each supporting `Action::Click`, but not identically: the target's `Click`
  drops a live, accepting KEYBOARD session onto it (a pointer session's `Up` belongs to its source
  and must pass through every ancestor target on the capture chain), while the source's `Click`
  drops (or cancels) its own live keyboard session and is a plain tap otherwise (see
  `drag-ghost-pod-no-semantics` in [LIMITATIONS.md](LIMITATIONS.md)). `auto_scroll_zone()` drives
  an attached `ScrollController` toward whichever edge the dragged pointer sits inside
  (`AutoScroll::edge_px`/`max_px_per_s`). `reorderable_list()` composes both primitives over keyed
  rows with `N + 1` gap targets, firing `on_reorder(state, from, to)` on an actual move. A desktop
  `InputEvent::FileDrop` opens an `ExternalFiles` session so a `drag_target::<Vec<PathBuf>>`
  accepts an OS file drop the same way a widget-originated session would (see
  [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)).
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
| authoring module (`build_child`/`rebuild_child`/`teardown_child` generic over the child view, `rebuild_children`, `route_event`, `VisitPods`/`visit_children!`) | The sanctioned seam for authoring any widget against `frust-core`, including child-introspection |
| `Navigator` / `Router` / `hero()` | Page-stack and declarative routing plus shared-element transitions |
| `ButtonStyle`, `ScrollInfo`, `IconData`/`IconSource`, `ImageSource`/`ImageFit` | Small per-widget config/state types shared across the baseline widget set |
| `ScrollController` / `ScrollSubscription` / `AnimateTo` / `ItemAlignment` | Programmatic-scroll handle for `ScrollView`/`ListView`, its `on_change` guard, the animate-to duration/curve, and the keyed-row landing alignment — see *Scroll Physics* and *Virtualized ListView* |
| `ListView` / `ListViewWidget` | Baseline virtualized list: windowed rebuild-time materialization, positional or keyed (`ChildKey`) row identity, optional variable extents, refresh/overscroll parity with `ScrollView` |
| `CustomScrollView` / `ViewportWidget` / `SliverBoxView` / `ScrollAxis` | Sliver viewport and its box adapter (`ViewportWidget`, `SliverBoxWidget` are frust-widgets-only); `ScrollAxis` is re-exported flat from frust-widgets and the facade |
| `NavigatorController::transition()` / `TransitionState` / `PageVisibility` | Navigation state observation seams |
| `RouteNavigator` / `NavRequest` | Off-thread-safe navigation handle (Arc-backed plain data, no reactive types) |
| `NavigatorController::route_stack()` / `RouteStack` / `NavChange` | Route-state observable: page stack as route identities, diffed change label — see Data Flow |
| `shell_route()` | Pathless route binding a subtree to a second, app-owned `NavigatorController` — nested-navigator ("shell") composition |
| `BackPolicy` | Per-page back-press disposition (`Pop`/`DismissAnimated`/`Veto`) gating both a back press and edge-swipe arming |
| `scaffold()` / `ScaffoldView` | Four-slot screen layout (`app_bar`/`body`/`bottom_bar`/`fab`), self-sizing inset consumption (R-B4-inset) |

## Architectural Facts & Constraints

### External Design-System Contract
The three built-in design systems (`plugins/{glyph,material,cupertino}`, PLUGINS unit) prove this
contract: each is an ordinary sibling crate on the public authoring/theme seam, with no
special-cased access. Each flat re-exports its catalog at the crate root (`frust_glyph::app_bar`,
`frust_material::AppBar`, `frust_cupertino::CupertinoButton`), exposes its token constructors as
free functions (`baseline()`, `color_scheme_light()`/`_dark()`, `type_scale()`, `shape_scale()`,
`elevation()`, `motion_scheme()`), and its `install()` seeds the theme with
`frust::set_default_theme(baseline())` from an `app!` `setup` block. Glyph's and Material's
`baseline()` also attach the `NativeTypefaces` extension — Cupertino's does not (see Key Types and
NATIVE_WIDGETS_ARCHITECTURE.md's theme ladder).

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
LIMITATIONS.md. `plugins/shadcn` (55 components) and `plugins/beui` (81 motion-first parts) are
production-scale external-origin ports on the same seam — see PLUGINS_ARCHITECTURE.md's
Design-System Plugins.

Three more seams are part of the same public authoring surface: opt-in hover claiming
(`EventCtx::claim_hover`/`PaintCtx::is_hovered`) for state-layer-style interaction chrome, cursor
requests (`EventCtx::set_cursor`/`CursorIcon`, also flat-re-exported as `frust::CursorIcon`) for a
design system's own hover/drag affordances, and the overlay portal — `place`/`OverlaySlot`/
`overlay_portal`, re-exported through `frust::authoring` — for a popover, menu, tooltip or context
menu (see *Overlay flow* above).

### Erasure at the API Boundary
Authoring code rarely names `AnyView`: `AnyView::new`/`any()` is idempotent (an argument already
an `AnyView<State>` is returned unchanged), so erasure happens once, inside the API that stores
the child; `build_child`/`rebuild_child`/`teardown_child` are generic over `V: View<State>`, and
only `rebuild_children`'s `view_of` (mixed/keyed lists) and `OverlaySlot` stay `&AnyView` seams.
- **Fluent builders:** `column()`/`row()` (`FlexView`: `child`/`flex`/`keyed`/`children`/`push`/
  `when`/`when_some`) and `stack()` (`StackView`: `child`/`children`/`when`/`when_some`), flat
  re-exported through `frust_widgets` and `frust`. The `Column`/`Row`/`Stack` sugar remains,
  taking `Vec<V>` with `V: View<State>` (a `Vec`, not an iterator, so `Column(iter.collect())` still infers).
- **Parameter contract** (widgets, facade, plugins): a single slot is `impl View<State>`
  (scaffold `app_bar`/`bottom_bar`/`fab`, plugin icon/leading/trailing/title/action); an
  optional slot is `Option<V>` (`app_bar_opt`, `bottom_bar_opt`, `overlay`); a plugin list is
  `impl IntoIterator<Item = impl View<State>>`; a builder closure is `impl Fn(..) -> V`
  (navigator/`overlay_host`, `NavigatorController` push/replace, `Route::new`/`shell_route`/
  `Router::error_builder`, `ListView` item builders, `DraggableView::ghost`; `reorderable_list`
  takes `impl IntoIterator<Item = (K, V)>`).
- **Caller consequences:** a bare `None` on an optional slot becomes `None::<AnyView<S>>`; an
  explicit turbofish gains a `_` for the view type; `.collect()` into an iterator parameter needs `::<Vec<_>>()`.

### Reactive-Free Design
`frust-widgets` contains no `reactive_graph` symbols — the crate is entirely signal-free. Reactive
bridging lives in the CORE facade (`router_glue.rs`, `back_glue.rs`), keeping the widget set
reusable; nav/navigator state is held in plain `Rc<Cell<_>>`/`Arc<Mutex<_>>`.

### Icon Generation
`crates/frust-widgets/src/icons/mod.rs` is **generated** by `scripts/gen_icons.py` from a hardcoded
`STARTER_SET`; hand-edits are destroyed on the next run — expand the script's source list instead.

### Text Widget Alignment
`TextInput`'s live-edited text is always start-aligned regardless of `TextStyle::align`: parley's
`PlainEditor` exposes no alignment hook (a parley limitation, no per-field workaround).
`TextView` honors `align`.

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
`ScrollView`, `ListView` and the sliver viewport (`scroll.rs`/`list_view.rs`/`viewport.rs`) are thin
owners (viewport geometry, child origins, list windowing) around one `ScrollSurfaceCore` plus the
`ScrollSurface` trait in `scroll_core.rs` (crate-internal, not facade API). The core is the single
copy of the drag accumulator and edge pull, the takeover with claim and veto, fling/settle/ballistic,
controller attach/drain/jump/animate/publish, and the `on_scroll` owed-notification rule. Each owner
supplies only its position arithmetic (`pixels`, `displacement`, `set_allowed_position`,
`decay_displacement`, `settle_to_edge`, base offset): `ScrollWidget` keeps an out-of-range offset,
`ListViewWidget` a clamped offset plus `overscroll`. A new surface implements `ScrollSurface` and
copies none of the machinery.

The feel is a pluggable strategy (`crate::physics`, mirroring Flutter): a `ScrollPhysics` trait (drag
mapping, boundary rejection, ballistic simulation, fling thresholds, spring; composed with
`.chain(parent)`, never inheritance) is orthogonal to `OverscrollEffect` (how a rejected displacement
paints: `Translate`/`Stretch`/`None`) and to `Simulation` (Friction/Spring/Clamping/Bouncing ports).
`default_physics()`/`default_overscroll_effect()` pick the pair per platform (Android `Clamping` +
`Stretch`; elsewhere `Bouncing` + `Translate`) unless the app names its own. `RubberBand` is an
explicit opt-in only, and the only physics still on the legacy hand-rolled fling/settle path
(`create_ballistic_simulation` returns `None` by design). Wheel input is a physics-independent hard
clamp. A release carries interrupted momentum forward only when it plainly continues it, as in Flutter's
`ScrollDragController.end`.

Each drag `Move` hands the physics that move's raw delta against the live position (a
path-dependent mapping). The core accumulates the boundary-rejected excess as its own edge-pull
state, so pull-to-refresh and `Stretch` fire under a clamping physics too. Stretch is a paint-only
affine scale about the held edge (`stretch_transform`); no layout pass reads the pull. A ballistic
simulation that ends fully pinned outward stops early, handing the residual pull to the settle path.

**Nested-scroll arbitration** runs the ambient-claim shape of the navigator's edge-swipe arming
(`nav::ambient`'s `SWIPE_CLAIM`, see *Data Flow*): a surface pushes a fresh claim cell around the
`Down` it forwards, the nearest scrollable underneath reports what it could do with the drag
(`InnerScrollState`, nearest-inner pairing), and the outer reads that at the touch-slop takeover,
deferring when the inner can consume the direction. A claim registers only with real scroll capacity
(a deliberate deviation from Flutter's `BouncingScrollPhysics`). The report is a `Down`-time
snapshot and the defer decision is sticky for the gesture. All surfaces also run a live **multi-contact veto**, a per-gesture cell replaced on
every primary `Down` and consulted on every `Move` takeover check, so a nested
`pinch_detector`/`pan_zoom` reporting a second contact still suppresses the takeover. Claims are
axis-aware: a viewport registers only into a same-axis host and drops a claim from a surface on the
other axis.

**Programmatic scroll (`scroll_controller.rs`).** `ScrollController` is a cloneable, reactive-free
handle that `ScrollView`/`ListView`/`CustomScrollView::controller` attaches (facade:
`ScrollController`, `ScrollSubscription`, `AnimateTo`, `ItemAlignment`). One surface holds a handle
at a time (the latest attach wins) and dropping the widget detaches it.
Writes (`jump_to`/`animate_to`, plus `scroll_to_item` on a keyed list) are recorded, not applied;
the surface drains them at its next layout or paint (a `ListView` also at rebuild), so the clamp
lands against fresh extents. The queue is bounded by superseding within a segment (the offset
commands since the last item command): a jump supersedes every earlier offset command in its segment,
an animate supersedes an earlier animate but keeps a preceding jump; superseding never crosses a
queued `scroll_to_item`. Item commands keep an ordered channel capped at 8 (the oldest drops, with a
debug log). Recording raises CORE's pending-result-flush flag
(`frust_core::mark_pending_result_flush`) so a frame-gated mobile shell runs the applying frame.
`jump_to` clamps to `[0, max_offset]`, stops any fling or settle, and (like `animate_to` and
`scroll_to_item`) ends an established live drag, whose remaining `Move`s and `Up` fall through to the
child; a hold still inside slop stays armed. `animate_to` runs a `TweenSimulation` on the release
fling's ballistic pump; a user `Down` or wheel interrupts it, a later command replaces it, and
`reduce_motion` collapses it to a jump. A fling, a settle and a tween are never live together. Notifications flush like a jump's: every frame a
tween moves, and the frame it lands on, owes the next frame's Housekeeping flush, so `on_scroll`
(and `ListView` near-start/near-end) arrive with no input event; release flings and settles stay one
event late. An item request that moves nothing (unknown key, or `Nearest` on a fully visible row)
still ends a running controller tween where it stands and clears an earlier request's settle loop,
but leaves a user fling, settle or drag alone. `is_animating` is true only while the attached
surface has a live tween; `on_change` fires only when the published `ScrollInfo` changes.
`scroll_to_item` is list-only (`CustomScrollView` and `ScrollView` ignore it with a debug log). A
desktop shell ignores the pending-result-flush flag, so a command from a handler that requests no
redraw waits for the next frame another input wakes (as `PanZoomController`).

### Virtualized ListView (baseline)
`ListView`/`ListViewWidget`/`list_view()` live in `frust-widgets` proper (`list_view.rs`); the
facade re-exports them unconditionally. `frust_material::list_view` is a `#[deprecated]` alias.
Each `rebuild` reads the retained widget's scroll offset and cached viewport to materialize only the
visible window plus a buffer; with `sliver_fixed_extent_list` it is among the few `View`s that
read retained element state during `rebuild`. Layout measures only existing pods and paint builds nothing,
so the row builder never runs outside `rebuild`. Refresh, overscroll, claim/veto and physics
come from the shared core; the clamped-windowing/paint-only-overscroll split (`list_view.rs`) survives them.

**Two row-identity models.** `ListView::builder` reconciles by raw index (right for append-only,
truncate-only or full-replace data; a mid-list mutation reattaches state to whatever now sits at the
index). `builder_keyed` adds a `ChildKey` per row (as `keyed()` does for `Flex`, see WIDGETS_CODE_STANDARDS.md) so state follows the row, and re-anchors its offset in `rebuild`, before
windowing, so a single-sided prepend/removal above the viewport never visibly jumps the content. A
same-frame mutation on both sides of the anchor can still miss that correction (registered). A
duplicate key `debug_assert!`s with no positional fallback in release: the first slot wins.

**Variable extents are keyed-only.** `.estimated_item_extent(px)` switches a keyed list from the
closed-form uniform path to a measured-by-key cache (estimate for unvisited rows, laid-out height
once visited); on a positional list it `debug_assert!`s and is inert. A measurement revising a row
above the viewport's top is committed on the following rebuild so the anchor row never moves; during
a fling it accumulates and commits at settle.

**Scroll-to-item.** `scroll_to_item(key, ItemAlignment, animated)` (`Start`/`Center`/`End`/`Nearest`)
resolves `key` from measured extents plus the estimate for unlaid rows; a key outside the window
costs one O(item count) `key_of` scan, not per frame. The jump lands on the estimate first when
rows ahead were never measured, then settle-and-correct re-resolves each following layout, bounded
to 4 layouts or within 0.5px, so an animated request over unmeasured rows can end with a small
correction snap. An unknown key, or any key on a positional list, is a no-op (debug log).

### Slivers and the viewport
`custom_scroll_view((slivers...))` (`viewport.rs`, `CustomScrollView`/`ViewportWidget`, the third
`ScrollSurface`; `.axis(ScrollAxis)`, `.cache_extent`, `.controller`) composes children that speak
the sliver protocol (`frust_core::sliver`, see CORE_ARCHITECTURE.md). The layout loop:

1. Sanitise `SliverConstraints` per child from the running scroll/overlap/cache state.
2. Lay each child out via `ChildPod::layout_sliver_child`; a plain box child is wrapped by
   `sliver_box`, any non-forwarding wrapper is laid out as a box at that geometry.
3. A `scroll_offset_correction` restarts the pass (10 cycles max; debug asserts, release keeps the last).
4. Clamp, drain the controller, relayout once if the position moved.
5. Place children in layout only: paint requests a layout, so paint-driven motion shows a frame late.

Paint runs last to first, hit-testing first to last, within `hit_test_extent`; the semantics node is
`Role::ScrollView` with `scroll_x`/`scroll_y` per axis. An unbounded main axis shrink-wraps; a
horizontal viewport builds its core on its own axis (only the stretch overscroll transform is still
transposed).

| Sliver | Behavior |
|--------|----------|
| `sliver_box(child)` | Adapts a box child; the geometry every box fallback uses |
| `sliver_padding(insets, child)` | Main-axis insets scroll before/after the child, cross insets narrow it; the cache window is recomputed from the incoming one |
| `sliver_fill_remaining(child)` | `has_scroll_body(true)`: tight to the remaining paint extent. `false`: fills viewport minus preceding scroll extent, or the child's extent if larger |
| `sliver_fixed_extent_list` / `_keyed` | Lazy list of equal-extent rows, closed-form geometry |
| `sliver_list(...)` | Lazy keyed rows that size themselves; measured-by-key extents shared with `ListView` |
| `sliver_grid` / `_keyed` | Lazy grid; `SliverGridDelegate::FixedCrossAxisCount` or `MaxCrossAxisExtent` |
| `sliver_persistent_header(delegate)` | `{min_extent, max_extent, build(shrink_offset, overlaps_content)}`, `.pinned`/`.floating` |
| `keep_alive(child)` | Row scope of `sliver_list` only (a request in a nested list's row pins the enclosing `sliver_list` row): parked rows sit uncapped in a `ChildKey` side map, re-adopted when the key returns, released when it leaves the data or the list tears down; parking cancels a gesture |

**Cache-window rule.** A lazy sliver materialises exactly the rows covering
`[scroll_offset + cache_origin, + remaining_cache_extent)`; the viewport's `cache_extent`
(`DEFAULT_CACHE_EXTENT`, frust-widgets-only) is the buffer, with no separate constant. Rows are built only at rebuild
from the last layout's constraints; paint requests one more frame until the rows cover the window
(`ListView`'s catch-up). `ListView` and the sliver lists share the windowed reconcilers and the
`probe_key`/`locate_key` lookup in `slivers/reconcile.rs`. Near-edge callbacks are checked at layout,
only once the list has moved, and delivered through the Housekeeping broadcast.

**Correction-based anchoring.** `sliver_list` places rows from an anchor row. Keyed prepends and
removals above the anchor (and a leading edge disagreeing with the list's start) are reported as
`scroll_offset_correction` and applied by the retry loop, not a private nudge. It applies only while
the list's start is at or above the viewport's leading edge; `locate_key` scans up to 64 items
around the anchor.

**Persistent header.** The child is laid out tight at `max(min, max - shrink)`,
`shrink = min(scroll_offset, max)`. Pinned: `paint_origin = overlap` (pinned headers stack), the
header paints at least `min_extent` while its layout extent shrinks to 0, so the next sliver scrolls
under it (`overlaps_content = overlap > 0`). Floating: it keeps its own effective offset and reveals
only under a Forward `user_scroll_direction`, which the viewport sets only during a drag or fling;
floating+pinned never paints below `min_extent`. Layout records shrink and `overlaps_content`; paint
requests a frame when stale; the next `View::rebuild` calls `build` (against the previous view's, as
`ListView` does), so `build` must be pure and the content lags geometry by one frame. A change under
a 0.5 px tolerance (`SHRINK_REBUILD_TOLERANCE`, crate-internal) skips the rebuild unless it lands on 0 or `max_extent`. An
outside sliver can hold `SliverPersistentHeaderWidget` (frust-widgets-only; not yet in the facade)
as a geometry core, as Material's `sliver_app_bar` does.

**Forwarding wrappers** forward `Widget::as_sliver`: `Either`/`EitherOf3..8`, `hero`, `shield`,
`auto_scroll_zone`, `GestureDetector`, plus `AnyView`/component boundaries (CORE). Others are boxes.

**Reference surface and limits.** The playground Slivers page
(`examples/playground/src/pages/slivers.rs`) composes a pinned Material `sliver_app_bar`, a floating
`sliver_persistent_header`, a variable-extent `sliver_list`, a 4-column `sliver_grid` and
`sliver_fill_remaining` under a `ScrollController`; its smoke test covers bar heights, wheel, jumps
and zero layout-audit mismatches. v1 scope limits are registered as
`sliver-v1-scope`; `ScrollView`/`ListView` are not yet viewport-based
(`scroll-view-list-view-not-viewport-based`).

### Recent Additions
`overlay` and `TextInput`'s hosted `selection_toolbar()` (*Overlay flow*, *Text Selection and the
Clipboard*); the `drag` module (*Drag-and-drop flow*); `TransitionState`/`PageVisibility`,
`overlay_host()`, `RouteNavigator`; `TextInput` read-only mode and `content_type` IME hints;
`TextView` alignment, `.max_lines`, `.overflow(TextOverflow)`; `container()`/`colored_box()`,
`divider()`, `icon_button()`; `column()`/`row()`/`stack()` builders (*Erasure at the API Boundary*).
