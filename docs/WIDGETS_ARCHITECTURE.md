# Frust - WIDGETS Architecture

## Overview

WIDGETS covers `frust-widgets`, the baseline widget set (layout, controls, text, gestures,
navigation, platform-view slots), built over `frust-core`/`frust-scene`/`frust-text`/`frust-theme`
through a shared authoring toolkit; and its sibling `frust-theme`, a design-token crate bundling
color/type/shape/elevation/motion/glass values into a `Theme` that widgets recover from context and
app code reads reactively. The built-in design systems are sibling plugin crates in PLUGINS
(`frust-glyph`/`frust-material`/`frust-cupertino` and the two external-origin ports, see
[PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md)) on the same public authoring toolkit.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how WIDGETS relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `frust-widgets::authoring` | Public container/callback toolkit every widget in the crate builds from, instead of touching `frust-core` primitives directly — reachable by app code as `frust::authoring` (the facade's re-export, CORE unit); also carries the `VisitPods` trait / `visit_children!` macro, the crate's introspection seam (see *Data Flow*) |
| `frust-widgets` (baseline) | Baseline layout containers and interactive leaf widgets (text, forms, gestures incl. vertical/horizontal/pan drag callbacks (18 px axis slop, 36 px pan, first to cross wins; release velocity on the raw input clock; a same-axis scroll view takes the drag first; `drag_start_behavior` picks the start point; `cancel` runs on the next state-bearing pass), `on_double_tap` (300 ms window; `on_tap` waits for it) and `on_tap_down`/`on_tap_up`/`on_tap_cancel` press state (exactly one up or cancel per down; a second contact cancels when a pinch/pan-zoom sits below), `on_long_press_move_update`/`on_long_press_end`/`on_long_press_cancel` for a fired long press (a scroll veto, see *Long-press Follow-ups*), scrolling, `wrap()` (children flow along a main axis and break into runs, Flutter's `Wrap`), a virtualized `ListView`, a four-slot `Scaffold`, `IndexedStack`, `container`/`colored_box` (each corner radius clamps to half the shorter side: infinity is a capsule, NaN or negative is square, and `TextureView` clamps alike), `divider`, `icon_button`), plus the `shortcuts` chord-binding wrapper and the baseline Tab stops (`Button`, `Checkbox`, `Radio` and `Slider` publish their focused semantics node) — see WIDGETS_CODE_STANDARDS.md's Keyboard Focus and Shortcuts |
| `frust-widgets::clip` | `clip(child)` (`.radius(..)`) clips the child's overflow to the view's own bounds; a rounded clip takes the largest resolved corner for all four; layout, events and semantics pass through, except that a press outside the bounds never reaches the child (a held capture keeps its events) |
| `frust-widgets::keep_focus` | `keep_focus(child)` (`KeepFocusView`): a press inside it keeps the live focus session, so a focused field keeps its caret and keyboard while the child still handles the press (`EventCtx::keep_focus`; see CORE_ARCHITECTURE.md's Focus/IME Lifecycle). It is not `overlay_portal(..).preserve_focus(..)`, which decides whether a floated surface's own presses move focus |
| `frust-widgets::canvas` | `CanvasView`/`CanvasWidget` — declarative custom painting over `PaintScene`, local-space, no semantics node; `semantics(child)` (`label`/`value`/`hint`/`role`/`heading`/`merged`/`tappable`/`image`/`button`/`hidden`) and `semantics_label(text)` are the semantics-only counterpart, no painting |
| `frust-widgets::repaint_boundary` | `repaint_boundary(child)` (`.alpha(f32)`) and the shared `RetainedBoundary`: per frame `DrawRetained` when the subtree is clean, its transform's linear part unchanged, nothing inside published a paint and the ledger holds (key, extent, scale from `PaintScene::current_transform()`) and covers the serial it recorded under (runs of consecutive recordings at one origin, size and linear part; the first draw confirms; it must have recorded on the immediately previous paint; serial 0 = bare ctx skips the check); else `PushRetained`+body+`PopRetained` when recording is allowed and the body is visible, else direct paint (kill switch `FRUST_NO_RETAINED_LAYERS` → plain bodies). `motion::AnimatedOpacity::cached(bool)` opts an opacity animation in (`AnimatedScale` unchanged) |
| `frust-widgets::glass_surface` | `glass_surface(child)` (`.tier(GlassTier)`, `.corner_radius`, `.material`) paints one `GlassScale` tier behind its child: shadow (clipped outside the surface), rounded clip, `backdrop_blur` + fills, child, hairline; an explicit material beats the theme tier, and with neither it paints an opaque fill. Each surface spends one of the per-frame backdrops (RENDER unit); blur intent is sigma in logical px; the corner radius clamps like `ShapeScale::resolve` (infinite is a capsule, NaN or negative square), and under a transform the sigma follows its mean scale while the lens samples the rotated rectangle's device bounding box |
| `frust-widgets::pinch` | `PinchRecognizer` (a pure two-contact state machine) and `pinch_detector`, the wrapper feeding it from a gesture's contacts |
| `frust-widgets::pan_zoom` | `PanZoomView`/`PanZoomWidget` — one child under a pan/pinch-or-wheel-zoom transform, with inertia and a `PanZoomController` handle |
| `frust-widgets::motion` | Implicit-animation and transition vocabulary, `card_switcher` (see *Card Switcher*), `detent` (pure `Detents` snap-point math + headless `DetentController`) and `presence(open, child)` (`.timing`, `.on_exited`): kept-mounted and owner-controlled — always built and laid out, painted only while visible, input only while open, semantics only while visible; `on_exited` rides the next Housekeeping flush and fires only once an exit is painted (a cleanup cue); core retains no absent view. `MotionExtras` (`extended` 800, `reduced_fade` 120 ms; 0 snaps) sets every reduce-motion collapse |
| `frust-widgets::nav` | Imperative page-stack navigator, declarative router, and shared-element hero transitions — internally split across six private `nav/*.rs` files (stack core, ambient-context idioms, options/callbacks, controller, view constructors, edge-swipe driver); single public path via `navigator.rs`'s re-exports. A page's transition resolves `Router::push_with` options, then `Route::transition`, then the navigator default (all collapse under reduce motion). `PushOptions::barrier`/`barrier_dismissible` give a page a route-owned scrim painted under it: never moved by the page's transition, alpha following the route's eased progress (instant under `TransitionSpec::NONE`); an unhandled press and release that lands on no content (layout wrappers are transparent) dismisses through the page's `BackPolicy`, and taps are ignored while a transition is in flight |
| `frust-widgets::physics` | Pluggable scroll-motion strategy (`ScrollPhysics`, `OverscrollEffect`, `Simulation` ports, platform-adaptive defaults) the scroll surfaces consult — see *Scroll Physics* |
| `frust-widgets::page_view` | `page_view` + `PageController` (Flutter `PageView` parity: `viewport_fraction`, `pad_ends`, `on_page_changed`, `animate_to_page`/`jump_to_page`, live fractional `page()`, both axes) over `custom_scroll_view` with a crate-private page sliver; `on_page_changed` fires once the page has settled, not mid-drag; eager children only (`page-view-eager-children`) |
| `frust-widgets::viewport` / `slivers` | `CustomScrollView`/`ViewportWidget` (the sliver viewport) and the sliver family (`slivers::adapter` holds `sliver_box`); `scroll_core` holds the shared `ScrollSurfaceCore` — see *Slivers and the viewport* |
| `frust-widgets::scroll_controller` | `ScrollController` — a cloneable, reactive-free handle onto a `ScrollView` or `ListView`'s offset/item position — see *Scroll Physics* and *Virtualized ListView* below |
| `frust-widgets::scrollbar` | The opt-in overlay scrollbar for the vertical `ScrollView`/`ListView` (`.scrollbar(ScrollbarStyle)`; no horizontal or `CustomScrollView` bar): thumb geometry, hover widening, a paint-clock fade (`reduce_motion` snaps it), thumb drag and track paging written through the bound `ScrollController`'s `jump_to`; its state lives on `ScrollSurfaceCore`, and without the call nothing paints, hit-tests or binds |
| `frust-widgets::overlay` | The widget-author face of CORE's overlay portal: anchored placement geometry (`place`/`OverlayPlacement`), the `OverlaySlot` a widget hosting its own floated pod keeps, and the declarative `overlay_portal` wrapper |
| `frust-widgets::drag` | Drag-and-drop session model: `DragCoordinator`, `draggable()` source, `drag_target()` target, `auto_scroll_zone()`, `reorderable_list()` — single public path `drag`, re-exported flat from the crate root and the `frust` facade |
| `frust-widgets::platform_view` | Native-sibling compositing slot and input-shield wrapper for translucent surfaces |
| `frust-widgets::window_drag_region` | `window_drag_region(child)` (`.resize(edge)`, `.double_click_toggles_maximize(bool)`) — offers a primary press to the child first and pushes `BeginMove`/`BeginResize` only for one the child ignores; never captures the pointer |
| `frust-theme` | `Theme` aggregate and its token tables (color, type, shape, elevation, motion, glass); carries no design-language token module of its own — only the neutral/language-free floor (`Theme::neutral()` and friends) |

## Layer Dependencies

`frust-widgets` depends on `frust-core` (`View`/`Widget` traits, layout, paint/event context,
semantics), `frust-scene` (renderer-agnostic draw commands), `frust-text` (text layout/editing
engine backing `frust-theme`'s typography tokens), `frust-theme` (design tokens bundled into
`Theme`), `peniko`/`kurbo` (shared color/geometry primitives), and the `image` crate (image decode
path). `frust-theme` is `frust-widgets`' primary consumer. The one reverse edge, `frust-theme` →
`frust-core`, is context-accessor sugar only and creates no cycle into `frust-widgets`.

Every container and interactive widget is built through the crate's own public authoring toolkit, not
`frust-core` primitives directly. The crate graph enforces this: the three built-in design systems
(`frust-glyph`/`frust-material`/`frust-cupertino`, PLUGINS unit) depend only on the `frust` facade
(`default-features = false`) plus `kurbo`/`peniko`, so they cannot reach a `frust-core` primitive `frust` doesn't
re-export: the seam any third-party design system builds against (see *External Design-System Contract*).

## Data Flow

- Theme delivery (detail; the cross-unit summary lives in [ARCHITECTURE.md](ARCHITECTURE.md)): a shell owns the active
  `Theme`; widgets recover it type-erased from paint/layout context so `frust-core` stays theme-agnostic, while app code reads a
  cloned `Theme` via reactive context.
- Widget resolution precedence: explicit builder value > theme token > unthemed-fallback constant, generally re-resolved every
  paint; `Text`/`TextInput` instead bake the resolved color and a themed family (`.themed_family`/`.themed_mono_family`) at
  layout: `Text` yields to an explicit `.family()`/`.style()`, `TextInput` does not.
- Text scale: a `TextScale` theme extension carries the OS factor a mobile shell applies through `Theme::with_text_scale`
  (`TypeScale::scaled`; clamps `TEXT_SCALE_MIN` 0.8, body/label/title max 2.0, display/headline max 1.5). Default-sized
  `Text`/`TextInput` scale at layout through `frust_theme::scale_style` (body-role clamp); explicit sizes never scale, and catalogs
  pass explicit sizes from the already-scaled `TypeScale`. A theme read from context may go back to `set_app_theme` without
  compounding (`Theme::authored_type_scale`).
- Introspection flow: a container implements `Widget::visit_children` (CORE unit) via the authoring toolkit's `VisitPods`
  trait and `visit_children!` macro (one line naming its `ChildPod` fields; a hand-rolled child list implements `VisitPods` by
  hand) so `RenderRoot::inspect()` can enumerate its children. The baseline set and all five plugins use it; see
  WIDGETS_CODE_STANDARDS.md.
- Design-system layering: the five design-system plugins (PLUGINS unit) assemble their own `Theme` via `ThemeBuilder`'s
  editors over `frust-theme`'s neutral floor; `frust-cupertino` owns its "Liquid Glass" `GlassScale` recipe, degrading to the
  neutral `GlassScale::opaque_material()` where unsupported.
- Motion flow: implicit-animation and transition widgets resolve default timing from `Theme.motion`, collapsing to a short
  crossfade under `reduce_motion`; decorative loops request paced frames. `reduce_motion` is a floor, not an assignment: every
  baseline defaults it `false`, and a mobile shell OR's the OS accessibility preference over the theme's (`effective = authored ||
  os`, sensors in SHELLS_ARCHITECTURE.md) — an authored `true` is never un-reduced by an OS report of `false`.
- Navigation flow: `Navigator`/`Router` manage a page stack and declarative routes; `hero()` morphs a tagged child between pages.
  - **Alpha-zero paint redirect:** the split-crossfade presets (`M3SharedAxisX`, `M3FadeThrough`, `Glyph`) are hard splits: before
    it only the leaving page ramps 1→0, after it only the entering page ramps 0→1, so at most one page is visible at any instant
    and both resolve to alpha 0 at the split. Never assume a dual-visible window: `IosPush` (parallax + dim) and `ReducedCrossfade`
    keep both visible. A page at alpha 0 still runs its paint pass (hero-rect capture, paint-time state) redirected into
    `frust_core::DiscardScene`, so no scene command is emitted.
  - **Retained pages:** each page carries a `RetainedBoundary` (`PageEntry.boundary`; the switcher's `child_boundary`/
    `exiting_boundary`). A page **at rest paints plain**; only a programmatic transition's progress-0 seed frame records the
    leaving page and the transition frames draw it (`DrawRetained`); a slide's entering page paints plain. A body using a paint
    side channel (IME, platform view, shield, overlay, selection toolbar, hero) paints every frame, and hero pages never draw a
    page. Fractional slides sit up to half a pixel off; a boundary cannot see a clip above it, so it may record a body the engine
    then refuses to retain.
  - **Navigator observation seams:** `NavigatorController::transition()` publishes a `Copy` `TransitionState` (active,
    progress, direction, depth pair, generation) on an `Rc<Cell<_>>` for chrome outside the subtree: reads during paint from a
    widget painted *after* the navigator are frame-exact, reads during `Component::build` one frame stale.
    `PushOptions::on_visibility` and `PageVisibility` (Current/Visible/Covered) observe page visibility; the opt-in
    `cull_covered_builds` (default false) skips rebuilds for covered pages, but `on_cleanup` does **not** fire under a covering
    push.
  - **Pop-result delivery:** `push_for_result`'s `on_result` needs `&mut State`, which the state-free view diff lacks, so a
    pop queues it and raises CORE's pending-result-flush flag, drained by the rebuild's `Housekeeping` broadcast (see
    CORE_ARCHITECTURE.md): an eager pop delivers in that rebuild, an edge-swipe pop on its settle frame. `pop_with_result` is
    single-shot per page: an immediate repeat for the same top page before the first applies is dropped, its result discarded.
  - **RouteNavigator:** A thread-safe (`Arc<Mutex<Vec<NavRequest>>>`) navigation handle with no closures or reactive types
    (`Send + Sync`, usable via `provide_context` from any thread); off-thread requests wake the shell and apply in the next rebuild.
  - **Route params:** Query parameters merge **under** path captures (a capture wins); `path_for_name` round-trips unused params as query.
  - **Route-state observable (R-B1):** `NavigatorController::route_stack()`/`route_generation()` publish a `RouteStack` (route
    identities bottom→top, depth, generation) and a diffed `NavChange` (Initial/Push/Pop/Replace/Reset) from every committed
    mutation in `publish_state()`. An in-flight edge-swipe pop reports the pre-swipe stack until its settle-frame publish;
    frame-accurate drag chrome wants `NavigatorController::transition()`.
  - **shell_route (nested navigators):** `shell_route(inner, builder, children)` is a pathless route binding a subtree to a second
    app-owned `NavigatorController`: the shell page stays retained on the enclosing stack while its children resolve onto `inner`
    (go_router's `ShellRoute`). A chain crossing the boundary splits there, one structural op per controller. `go` makes the stack
    exactly the matched chain, keeping pages by key, so a shell survives any `go` crossing it and only the top page animates;
    `push`/`replace` issue zero enclosing ops once it is placed. To keep a page below a shell, nest it:
    `Route::new("/", ..).child(shell_route(..))`. A deep link resolved before the shell page exists pre-queues its inner segment,
    drained on `inner`'s first `build`. Chrome inside the shell reads the **inner** navigator's `route_stack()`.
  - **Gesture policy (interactive edge-swipe):** `NavigatorWidget::swipe_armable` gates arming on `BackPolicy`: a `DismissAnimated`
    or `Veto` top page refuses to arm outright, and the navigator-wide default resolves by rank: page override
    (`PushOptions::pop_swipe`) > navigator explicit (`NavigatorView::pop_swipe`) > platform slot (`platform_pop_swipe`, set by the
    facade from `cfg!(target_os = "ios")`) > preset default (on for `PageTransition::IosPush`). Under `reduce_motion` an armed
    swipe collapses to `ReducedCrossfade` on its first paint. **R-B3-inner:** a left-edge `Down` does not capture, so a nested
    navigator underneath can arm too; the outer only defers at the `Move` steal, after reading back the inner's claim.
  - **overlay_host() constructor:** A `NavigatorView` with pop_swipe disabled and no-op transition (not a new widget). Back-button
    interest is arbitrated by **rank taken from the wiring entry point**, never from when or how often a controller wires: an
    overlay host outranks a plain navigator, and among navigator peers the order is **innermost-first** (most-recently-wired wins),
    mirroring Android's `OnBackPressedDispatcher`. A host with no overlays reports no interest. Registrants are released by mount
    liveness (`NavigatorController::is_mounted()`), not a wire-count heuristic.
  - **Fallback rank:** a `BackFallback` (an app rule, e.g. back at a tab root returns home) ranks after every navigator and claims
    only when none does; it is live while a `frust::back_fallback(&handle, child)` wrapper is mounted (plus a first-wire grace),
    even in a hidden branch, so gate its interest.
  - **Back reach follows input routing (R23):** ranking decides who wins *among* claimants; a reach check decides who may claim. A
    navigator whose hosting page is not in its host's `input_routed_pages()` reports `back_interest() == false`, so a press never
    pops an off-screen nested stack. Reach is input routing, not painting: a page under a *transparent* overlay is still
    `PageVisibility::Visible` but claims no back. `IndexedStack` is the second reach installer; a top-level navigator is always
    reachable.
  - **Controller binding is structural:** a `NavigatorView`'s published cells (depth, back interest, transition, mount count) bind
    to the `NavigatorController` it was last built against; a swap is detected by identity and re-bound in the same rebuild pass, so
    ops and published state never target different controllers and the old one reports unmounted.
  - **NavigatorWidget semantics (R23):** Now implements `Widget::semantics`, forwarding via `ChildPod::semantics_child` to
    exactly the pages input routing can reach (covered pages and pages under modals omitted outright), honoring the input-parity
    invariant documented in CODE_STANDARDS.md. Back arbitration's reach check (above) derives from this same
    `input_routed_pages()` set, so it is the single reach definition shared by input, semantics, and back.
- IndexedStack flow: `IndexedStack(index, children)` builds and rebuilds every child (each keeps its state, e.g. a tab's
  navigator stack) but paints, routes input to and exposes semantics for the selected one only; broadcasts reach all. Hidden tabs
  are skipped by the per-pod layout rule and reuse their cached layout when revealed unchanged. Deselecting cancels the child's
  capture and focus and hides its IME.
- Safe-area flow: `safe_area(child)` pads by the resolved `WindowInsets` on its enabled edges, then **removes** what it consumed
  from its subtree via `WindowInsets::consuming` (installed through `LayoutCtx::with_window_insets`/`PaintCtx::with_window_insets`),
  so a self-insetting descendant or nested `safe_area` does not inset the same edge twice. `.minimum` is extra padding, never
  consumed; `view_insets` (the IME) flows through unchanged unless `.keyboard(true)` (default off) lifts the bottom edge to
  `max(view_padding.bottom, view_insets.bottom)` and consumes both. A pod floated through the overlay portal carries its owner's
  consumed `WindowInsets` in its `OverlayEntry`, so paint-time reads agree with layout-time ones; hit testing is unaffected.
  `WindowInsets` also carries `corner_insets: CornerInsets` (four physical corners, each the window control's protrusion beyond
  the safe area in logical px); `safe_area` neither pads by nor removes them and `padding()` ignores them.
- Scaffold flow: `scaffold(body)` assembles the four fixed chrome slots (`app_bar`/`body`/`bottom_bar`/`fab`), theme-agnostic (a
  design system's bars plug into the slots from app code). **R-B4-inset:** the Scaffold itself consumes no window inset;
  `app_bar` and `bottom_bar` self-size for the top/bottom inset by reading `ctx.window_insets()` in their own `layout` (Material's
  `app_bar`/`search_app_bar` self-inset the top and the left/right edges by default and `navigation_bar` the bottom only, see
  [PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md); the sliver and bottom app bars do not, wrap them in `safe_area`), and `body`
  is never pre-inset; `fab` is the one slot the Scaffold insets itself, floating above `bottom_bar` when present and off the raw
  window edge otherwise. **Bar contract:** the Glyph app bar and the Material top and sliver app bars shift their leading slot
  right by `top_left.width` and their trailing edge left by `top_right.width` whenever that corner's height > 0. The shift
  assumes the bar spans the window's top edge (layout cannot see a bar's window-space position), so a bar hosted in a pane, sheet
  or dialog opts out with `corner_shift(false)`. Other catalogs' top bars do not yet apply the rule; see
  `corner-insets-ios-26-only` in [LIMITATIONS.md](LIMITATIONS.md).
- Overlay flow: anchored placement is framework-owned — `place(anchor, content, area, placement)` and `OverlaySlot` resolve a side,
  a cross-axis alignment, an offset, a collision flip and a clamp-back-inside, pure and total — so a widget, a catalog and an app
  place a floated surface the same way. A widget hosting its own pod keeps an `OverlaySlot` (whose substituted-state route forwards
  layout requests to the owner) and forwards four calls to it (rebuild; layout, loosely against the window; paint-time placement
  and registration; event, before its own children); `overlay_portal(child)` is the declarative case, anchoring a surface to that
  child's bounds. Window, owner-local and pod space meet in a slot, which owns both the translation and the pod's capture lifetime.
  `frust-shadcn`'s tooltip and hover card ride the slot directly; the catalogs' own `overlay::anchored` hosts still carry
  plugin-local copies. See CORE_ARCHITECTURE.md's Overlay Portal for the mechanism.
- Drag-and-drop flow: an ambient, Rc-shared `DragCoordinator` (one per scope, cloneable like `ScrollController`) is the single
  source of truth for a session (idle/armed/dragging/dropping/cancelled) and its typed `Box<dyn Any>` payload, so several targets
  type-gate against one handle. `draggable()` arms on a primary press and begins once a `DragPolicy` is crossed (a mouse travels
  a distance threshold, a touch press holds through a long-press; `Auto` picks per press from the pointer source), floating a
  ghost through the overlay portal at `OverlayAnchor::Window` (CORE_ARCHITECTURE.md's Overlay Portal) and cancelling on Escape or
  a release with nothing hovered. A begun drag vetoes the takeover of every enclosing scroll surface for the session.
  `drag_target()` resolves against bounds the coordinator's registry collects at paint, never a hit test, so later registration
  wins an overlap and a drop can land across containers. Keyboard lift/cycle/drop (`move_to_next_target`/`move_to_previous_target`,
  Enter/Space/arrows, Escape) is the assistive-technology path, driving the same coordinator from the source's focus; semantics
  advertise a `Role::Button` source and a `Role::Group` target, each supporting `Action::Click`: the target's drops a live,
  accepting KEYBOARD session onto it (a pointer session's `Up` belongs to its source), the source's drops (or cancels) its own
  live keyboard session and is a plain tap otherwise (see `drag-ghost-pod-no-semantics` in [LIMITATIONS.md](LIMITATIONS.md)).
  `auto_scroll_zone()` drives an attached `ScrollController` toward whichever edge the dragged pointer sits inside
  (`AutoScroll::edge_px`/`max_px_per_s`). `reorderable_list()` composes both over keyed rows with `N + 1` gap targets, firing
  `on_reorder(state, from, to)` on an actual move. A desktop `InputEvent::FileDrop` opens an `ExternalFiles` session so a
  `drag_target::<Vec<PathBuf>>` accepts an OS file drop like a widget-originated one (see [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)).
- Platform-view flow: `platform_view()`/`shield()` publish native-compositing slots and input-shield rects each frame
  for the shell layer to reconcile against native views.
- Canvas flow: `canvas(paint)` takes `Fn(&mut dyn PaintScene, Size, &PaintCtx)`; the widget translates to its own
  origin and clips to its bounds first, so painting happens entirely in local space. `.size(Size)`/`.expand()` (default)
  pick the sizing policy; `.on_hit(Fn(Point, Size) -> bool)` narrows hit-testing and gates `.on_tap`/`.on_pointer`;
  `.repaint_key(impl Hash)` requests a repaint when an external fingerprint changes, since the paint closure itself is
  not comparable across a rebuild.
- Pinch flow: `PinchRecognizer::handle(PointerId, &PointerEvent, time_ms) -> Option<ScaleEvent>` reduces a gesture's first two
  contacts (in `Down` order) to the same `ScaleEvent`/`ScalePhase` stream a desktop source produces. `pinch_detector(child)`
  captures the pointer and calls `EventCtx::capture_contacts()` on the first primary `Down`, forwards the first contact to the child
  unchanged, and steals an in-progress child gesture with one synthesized `Cancel` once a pinch begins (`on_scale` never fires from
  a `Cancel`); a desktop `InputEvent::Scale` reaches `on_scale` only when the child ignores it first. Nesting two detectors is
  unsupported. It captures the ambient multi-contact veto cell (`scroll.rs`'s `ambient_scroll_veto`) on its claiming `Down` and
  holds it raised while it tracks a second contact, so a pinch over a single-finger press inside a `ScrollView`/`ListView`
  survives the claimant's travel past slop; it never joins the Down-time nested-scroll claim, unlike `pan_zoom`.
- Pan-zoom flow: `pan_zoom(child)` places its child in a `ChildPod` transformed by `Affine::translate(offset) *
  Affine::scale(scale)` (`PanZoomTransform`), so frust-core's transformed-pod seam (see CORE_ARCHITECTURE.md) inverse-maps the
  child's hit-testing and events. A primary `Down` is offered to the child first; an ignored press pans on the claimant's moves.
  An `InputEvent::Scale` (desktop ctrl/⌘+wheel, trackpad pinch) is likewise offered to the child first; a touch pinch is
  recognised in-widget by a `PinchRecognizer` fed from every primary `Down`'s contacts, so a second finger routes here even when
  the child owns the first. On a `Down` the child ignores, the view publishes the nested-scroll claim (both drag directions,
  unconditionally) so an enclosing scrollable defers to it; when the child owns the press, it captures the multi-contact veto as
  `pinch_detector` does, cleared back down to one contact. A `Scroll` between `Scale` events clears the stale `last_focal` an
  open bracket left (a macOS pinch whose modifier released mid-gesture finishes as wheel events). `.min_scale`/`.max_scale`
  (`0.25`/`8.0`) clamp zoom; `.inertia(bool)` (off by default) glides a released pan on a per-axis `FrictionSimulation`;
  `.on_transform(Fn(&mut State, PanZoomTransform))` notifies after every change; `.controller(PanZoomController)` attaches a
  cloneable handle (`jump_to`/`fit_to_bounds`/`fit_rect`/`transform`/`viewport_size`/`content_size`) whose commands apply at the
  next layout or paint. Panning is unbounded; only panning glides.
## Key Types

| Type | Purpose |
|------|---------|
| `Theme` / `DesignLanguage` / `ThemeBuilder` | `frust-theme`'s aggregate design-token bundle, tagged by baseline (Material3/Cupertino/Glyph), plus a fluent editor |
| `ColorScheme`, `TypeScale`, `ShapeScale`, `Elevation`, `MotionScheme`, `GlassScale`, `StatusPalette` | Individual token-group types composed into `Theme`; `frust-theme` ships only each one's `neutral()` floor — a design system assembles its own values via `ThemeBuilder`'s per-group editors |
| `NativeTypefaces` / `FontFace` | A `ThemeExtensions` payload carrying a design system's own native-control font bytes (button/body face pair); no baseline `frust-theme` value attaches it — `frust-glyph::baseline()`, `frust-material::baseline()`, `frust-shadcn::theme()`, and `frust-beui::theme()` all do — see NATIVE_WIDGETS_ARCHITECTURE.md's theme ladder |
| authoring module (`build_child`/`rebuild_child`/`teardown_child` generic over the child view, `rebuild_children`, `route_event`, `VisitPods`/`visit_children!`) | The sanctioned seam for authoring any widget against `frust-core`, including child-introspection |
| `Navigator` / `Router` / `hero()` | Page-stack and declarative routing plus shared-element transitions; a custom transition is authored through the facade's `Layer` + `resolve_layers` |
| `ButtonStyle`, `ScrollInfo`, `ScrollbarStyle`/`ScrollbarShow`, `IconData`/`IconSource`/`StrokedIconSource`/`IconElement`, `ImageSource`/`ImageFit` | Small per-widget config/state types shared across the baseline widget set (`ScrollbarShow`: `Never`, `WhileScrolling` — the default — `Hover`, `Always`); an icon is a filled `IconSource` or a stroked, multi-element `StrokedIconSource` (round caps/joins, scaled with the icon, inked in the resolved icon colour; an `IconElement` may carry opacity, one dash pattern and its own stroke width, runtime-built through `IconData::from_styled_elements`; `IconData::resolve` returns one fillable outline, each stroke expanded at 0.02 design units, which fill-only slots ink filled, baking the dash and dropping opacity, and ignore `stroke_width` overrides for) |
| `ScrollController` / `ScrollSubscription` / `AnimateTo` / `ItemAlignment` | Programmatic-scroll handle for `ScrollView`/`ListView`, its `on_change` guard, the animate-to duration/curve, and the keyed-row landing alignment — see *Scroll Physics* and *Virtualized ListView* |
| `ListView` / `ListViewWidget` | Baseline virtualized list: windowed rebuild-time materialization, positional or keyed (`ChildKey`) row identity, optional variable (estimated keyed) or exact declared extents, `on_visible_range`, refresh/overscroll parity with `ScrollView` |
| `CustomScrollView` / `ViewportWidget` / `SliverBoxView` / `ScrollAxis` | Sliver viewport and its box adapter (`ViewportWidget`, `SliverBoxWidget` are frust-widgets-only); `ScrollAxis` is re-exported flat from frust-widgets and the facade |
| `NavigatorController::transition()` / `TransitionState` / `PageVisibility`; `route_stack()` / `RouteStack` / `NavChange` | Navigation state observation seams; the route-state observable is the page stack as route identities with a diffed change label (see Data Flow) |
| `RouteNavigator` / `NavRequest` | Off-thread-safe navigation handle (Arc-backed plain data, no reactive types) |
| `shell_route()` | Pathless route binding a subtree to a second, app-owned `NavigatorController` — nested-navigator ("shell") composition |
| `BackPolicy` | Per-page back-press disposition (`Pop`/`DismissAnimated`/`Veto`) gating both a back press and edge-swipe arming |
| `DismissCause` (`Escape`/`Backdrop`/`Back`) / `DismissDecision` (`Dismiss`/`Keep`) | Shared dismiss vocabulary (`dismiss.rs`, re-exported in `frust::authoring`) for a modal host's optional veto `on_dismiss_request(\|state, cause\| -> decision)`: it runs with `&mut State` before `on_dismiss`; `Keep` fires no `on_dismiss`, stages no exit and still consumes the input; no veto means unchanged behaviour. `frust-beui` and `frust-shadcn` implement it, plus the `escape_leaves_field_first` opt-in and default-on `return_focus` (PLUGINS_ARCHITECTURE.md); a veto does not hold a dialog across `Router::go` |
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

`DesignLanguage` is `#[non_exhaustive]` with a `Custom(&'static str)` variant (compared by string content) so a third-party design
system can tag its identity without a breaking enum change; every built-in `==` branch treats an unrecognized tag as the
neutral/System path. The public seam an external design system builds against is three-fold: the `frust_widgets::authoring`
toolkit (container/callback plumbing, `visit_children!`, re-exported as `frust::authoring`), `Theme`'s token bus (`ThemeBuilder`'s
per-group editors, `Theme::neutral()` as a language-free baseline), and `ThemeExtensions` for typed payloads a baseline doesn't
carry (e.g. `NativeTypefaces`, see Key Types) — plus `IconData::resolve`/`same`, `pub` so an out-of-tree catalog paints its own
icon glyphs as a built-in does. `examples/design-system-sample` is the reference proof for a genuinely external crate (a themed
catalog plus installer on `frust`'s public API alone; see ARCHITECTURE.md's Examples table); its one finding, that
`Widget::semantics` cannot be exercised from out of tree, is registered in LIMITATIONS.md. `plugins/shadcn` (55 components) and
`plugins/beui` (81 motion-first upstream ports plus six phone-content parts authored here) are production-scale external-origin
ports on the same seam (PLUGINS_ARCHITECTURE.md's Design-System Plugins). beUI takes its sizes from a `BeuiMetrics` theme extension
selected by a `Density`: `Touch` gives a 44 pt (Apple) / 48 dp (elsewhere) minimum target on the parts that read the metrics, the
rest keep the comfortable sizes, and beUI text follows the OS text scale.

Three more seams are part of the same public authoring surface: opt-in hover claiming
(`EventCtx::claim_hover`/`PaintCtx::is_hovered`) for state-layer-style interaction chrome, cursor
requests (`EventCtx::set_cursor`/`CursorIcon`, also flat-re-exported as `frust::CursorIcon`) for a
design system's own hover/drag affordances, and the overlay portal (`place`/`OverlaySlot`/
`overlay_portal`, via `frust::authoring`; see *Overlay flow*) for a popover, menu, tooltip or context menu.

### Erasure at the API Boundary
Authoring code rarely names `AnyView`: `AnyView::new`/`any()` is idempotent (an argument already
an `AnyView<State>` is returned unchanged), so erasure happens once, inside the API that stores
the child; `build_child`/`rebuild_child`/`teardown_child` are generic over `V: View<State>`, and
only `rebuild_children`'s `view_of` (mixed/keyed lists) and `OverlaySlot` stay `&AnyView` seams.
- **Fluent builders:** `column()`/`row()` (`FlexView`: `child`/`flex`/`keyed`/`children`/`push`/`when`/`when_some`, `gap`, and
  `MainAxisAlignment` `Start`/`Center`/`End`/`SpaceBetween`/`SpaceAround`/`SpaceEvenly`, which distribute leftover space only on a
  bounded main axis with no flexible child) and `stack()` (`StackView`), flat re-exported through `frust_widgets` and `frust`. The
  `Column`/`Row`/`Stack` sugar remains, taking `Vec<V>` with `V: View<State>` (so `Column(iter.collect())` still infers).
- **Parameter contract** (widgets, facade, plugins): a single slot is `impl View<State>`
  (scaffold `app_bar`/`bottom_bar`/`fab`, plugin icon/leading/trailing/title/action); an
  optional slot is `Option<V>` (`app_bar_opt`, `bottom_bar_opt`, `overlay`); a plugin list is
  `impl IntoIterator<Item = impl View<State>>`; a builder closure is `impl Fn(..) -> V`
  (navigator/`overlay_host`, `NavigatorController` push/replace, `Route::new`/`shell_route`/
  `Router::error_builder`, `ListView` item builders, `DraggableView::ghost`; `reorderable_list` takes
  `impl IntoIterator<Item = (K, V)>`).
- **Caller consequences:** a bare `None` on an optional slot becomes `None::<AnyView<S>>`; an
  explicit turbofish gains a `_` for the view type; `.collect()` into an iterator parameter needs `::<Vec<_>>()`.

### Reactive-Free Design
`frust-widgets` is signal-free (no `reactive_graph` symbols); reactive bridging lives in the CORE
facade (`router_glue.rs`, `back_glue.rs`), and nav state sits in plain `Rc<Cell<_>>`/`Arc<Mutex<_>>`.

### Icon Generation
`crates/frust-widgets/src/icons/mod.rs` is **generated** by `scripts/gen_icons.py` from a hardcoded
`STARTER_SET`; hand-edits are lost on the next run — expand the script's list instead.

### Text Widget Alignment
`TextInput` honours `TextStyle::align` within the wrap width on a multi-line field; a single-line
field has no wrap width and draws as Start. `TextView` honors `align`, `.max_lines` and
`.overflow(TextOverflow)`, `.overflow_wrap(OverflowWrap)`, `.locale(..)` (tags the paragraph without marking color/family explicit),
`.font_feature(s)`/`.tabular_figures()` and `.underline()`/`.line_through()`/`.decoration(..)` (painted as rectangle fills).
`rich_text(spans)` (`RichTextView`; `align`, `.overflow_wrap`, `.max_lines`, `.overflow`) lays out `span(text)` pieces (`SpanView`: size,
color, weight, family, italic, letter_spacing, locale, font features, decorations, `themed_family`/`themed_mono_family`/`themed_role`) as
one paragraph (empty spans dropped, equal-style neighbours merged, spans past 65,535 folded into the last group); each span resolves like
`TextView` (themed colour unless explicit, themed family roles, the OS text scale only without `.size()`), paints underline rects, glyph
runs, then line-through rects, and the whole thing is one `Role::Label` node (plus a `Role::Link` child per link, see *Rich Text*). Any cached `TextLayout` (`Text`, `TextInput`, and the plugin
widgets' private ones) stores a `LocaleStamp` (`frust::authoring::text`) read before shaping and is reused only while `is_current`, so a
locale switch re-faces untagged text; plugin caches exempt a style with an explicit locale, but `Text` and `TextInput` stamp
`for_style(None)` and reshape even a tagged style on a default-locale change. `TextInput` defers editor rebuilds (locale, font, theme)
while a preedit is live, firing them on the first layout after it ends.

### Text Selection and the Clipboard
Four gestures reach a `TextInput`'s selection and only two raise the toolbar: a **stationary long-press** selects the word under the press
and opens the bar (the only one a touch-only device has); a **double-tap** selects the word and deliberately opens nothing, since a bar over
a word the user is about to type over is in the way; a **tap inside an existing selection** keeps it and toggles the bar on the release, so
a drag starting inside a selection is still an ordinary caret drag; a **secondary press** claims focus, moves no caret and toggles the bar
(desktop only — no mobile shell delivers `Secondary`). Everything else puts the bar away: any text change, a blur or focus release, a
scroll, Escape, any primary `Down`, and any applied verb. `Cancel` is the one exception, touching neither selection nor bar, per the
never-mutate-on-cancel convention.

The verbs arrive two ways and converge on one handler. A shell dispatches an `InputEvent::EditCommand` (a platform edit menu, a
hardware clipboard key, a chord it decoded itself); or the widget decodes a chord from a plain `Key` — `ctrl` **or** `meta` plus
`c`/`x`/`v`/`a`, case-insensitively, uniform across desktop platforms, plus the legacy spellings (`Copy`/`Cut`/`Paste` named keys,
`Ctrl+Insert`, `Shift+Insert`, `Shift+Delete`). `alt` is never a chord modifier, and any other chorded character is consumed rather
than typed. Refusals are the field's own call and still count as *handled*: an obscured field copies and cuts nothing (neither the
buffer nor its bullet mirror), a collapsed selection makes copy and cut no-ops, a paste sanitised down to nothing inserts nothing,
and a disabled field never holds focus, so never sees a verb (a read-only one does, see *Read-only Text Fields*). Nothing here
touches a host clipboard — see CORE_ARCHITECTURE.md for the write/request slots the shell drains.

The bar itself is somebody else's widget: the field hosts an `OverlaySlot` and fills it from the process-wide selection-toolbar
builder, so `frust-widgets` never names the view that floats and no builder installed means no bar at all. The pod is mounted and
dropped in `rebuild` (the only pass carrying a `BuildCtx`) and only when the enabled verb set changes, then placed in `paint`
against the selection's bounding box, or the caret rect when collapsed. Under the `Native` policy the field floats nothing and only
publishes its request. `selection_toolbar()` is the baseline view that builder yields by default: a pill of text buttons for the
enabled verbs (fire-on-up-inside), cut/copy/select-all riding `EventCtx::dispatch_edit_command` and paste `EventCtx::request_paste`,
since only a shell may read the host clipboard. Its labels are English-only on purpose: an app or design system replaces the whole
view through `set_selection_toolbar_builder` (or the cooperative set-if-unset install), where localisation belongs.

The same verbs are published on the field's **own** semantics node as accesskit custom actions, under the predicates the bar is built from
rather than on whether the bar is up — the floated pod contributes no semantics, so a screen-reader user can reach nothing on the bar
itself. **Those actions are advertised but not invocable today:** the shell-to-core accessibility seam carries `(node_id, action)` and drops
the `ActionRequest::data` a custom-action id rides in, so nothing delivers them; the field's half is complete once one arrives as an
`EditCommand`. The selection range is not published either — accesskit models one as positions into `Role::TextRun` nodes, and this field
contributes a single leaf carrying its text as a plain value.

### Rich Text: Spans, Links and Selection
`SpanView::background`/`themed_background` fill a padded rounded rect per line fragment (paint-only, layout unchanged; an explicit
colour wins over the themed surface role). `SpanView::link(id)` marks a span (adjacent spans with one id are one link) and
`RichTextView::on_link` receives the id on a primary tap that starts and ends on that link within touch slop; a press elsewhere is
not consumed, a scroll takeover cancels it, and a link shows the pointer cursor. A link adds no styling and no focus stop of its own;
semantics are the paragraph label node with one `Role::Link` child per link. Fragments come from the layout geometry API
(`range_rects` over each marked group's byte range on the `line_bounds` boxes; RENDER_ARCHITECTURE.md's *Data Flow*'s text layout geometry bullet), so
no second layout is made.

`RichTextView::selectable(true)` selects and copies text inside **one** block, never across two. **Touch:** a stationary long-press
selects the word, shows a handle at each end and raises the toolbar; a handle drag moves its end by grapheme; a tap inside toggles
the bar, outside clears; any other drag scrolls the parent. **Mouse:** press-drag selects by grapheme (the press vetoes a parent
scroll's takeover), double-click a word, triple-click the block, secondary press toggles the bar. The double-click window is measured on the paint clock, so a block that completed a click requests continuation frames
until it lapses. **Keyboard:** the copy and select-all chords, Escape clears (a bare Escape is declined so a modal still closes);
cut and paste are answered with nothing. Copy goes through `EventCtx::write_clipboard`, the route `TextInput` answers on, and an
`EditCommand` takes the same path. The toolbar is the existing selection-toolbar seam: the block floats the builder's view through
its own `OverlaySlot` under `Framework` policy and publishes its `SelectionToolbarRequest` under either policy, offering Copy and
Select all only. **Focus:** a press keeps another widget's focus session (`EventCtx::keep_focus`), so a tap or scroll leaves a
text field's keyboard up; only a gesture that selects takes focus (through a bound `FocusHandle`), and focus loss clears the
selection. The selection survives scroll and relayout; a disposed block (a virtualized row leaving the window) drops it and its
overlay. Assistive technology gets Copy and Select all as custom actions on the paragraph node under `TextInput`'s ids (advertised,
not invocable, like the field's); the selected range is not published, as the paragraph has no `Role::TextRun` nodes.
`selectable(false)`, the default, builds no gesture state, focus stop, overlay slot or extra semantics.

The private `text_selection` module is what `TextInput` and a selectable block share: the selection colour, the toolbar slot
configuration, `ctrl`-or-`meta` clipboard chord decoding, the drag-handle paint and hit test (a knob hangs below its line, with a
hit area wider than it paints), the paint-clock long-press timer, and the one accessibility custom-action id space (stable ids
1-4: cut, copy, paste, select all).

### Read-only Text Fields
`TextInput::read_only(true)` makes a field uneditable without dimming it (`content_type` is the separate IME hint,
[CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md) `ImeContentType`): dimming stays keyed to `enabled` alone, so a live-styled static mock
does not pop to full alpha when it goes live. Focus and mutation answer separate questions: a read-only field is still **focusable
and copyable** (visible text is text a user can select). It takes focus on a press, drag-selects, long-presses to a toolbar offering
copy and select-all, and answers those verbs in full, while it refuses typed characters, IME composition and commits, and the
editing/caret-motion keys; cut and paste are answered with no effect and no `on_change`, and a paste chord never asks the shell to
read the host clipboard (nor raises iOS's system paste prompt). Escape still ends the session, and its caret is drawn but does not
blink, since a blink advertises an insertion point it does not have. A focused read-only field publishes `ImeState { active: true,
suppress_soft_keyboard: true, .. }` (see CORE_ARCHITECTURE.md's Key Types). `enabled(false)` remains the stronger claim: it refuses
focus outright regardless of `read_only`.

### Text Field Contract
- **Composition and submit.** While an editor-owned preedit is live, Enter confirms the IME candidate: no `on_submit`, no
  newline. A composing range the platform keyboard owns (Android, iOS) does not hold Enter: Enter ends the range, keeping its
  text, and then submits or inserts a newline. `on_composing_change(state, bool)` reports each transition once: `true` before
  the `on_change` of the first preedit update, `false` after the `on_change` of the text that ended it (a desktop empty
  preedit followed by the commit counts as one ending). A cancelled composition reports `false` with the next event the field
  handles; teardown and a callback replaced mid-composition report nothing. `TextEditor::is_composing` is the underlying read.
- **Single line.** Both modes clip selection, text, placeholder and caret to the inner box and report it through
  `InspectDetail::Clip`, so a single-line field always records a clip. A single-line field scrolls horizontally by a retained
  offset that keeps the caret in view, committed after each edit, event and layout; the placeholder is clipped, not ellipsized.
- **Focus setters.** `focus_handle(FocusHandle)`, `autofocus(bool)` (once, on first paint) and `on_focus_change` ride CORE's
  programmatic-focus path; the beUI, shadcn and Material fields forward all four setters (with `on_composing_change`), and
  Material's `SearchView` has them too, its full-screen header field autofocusing unless `autofocus(false)`.

### Scroll Physics
`ScrollView`, `ListView` and the sliver viewport (`scroll.rs`/`list_view.rs`/`viewport.rs`) are thin owners (viewport geometry,
child origins, list windowing) around one `ScrollSurfaceCore` plus the `ScrollSurface` trait in `scroll_core.rs` (crate-internal,
not facade API). The core is the single copy of the drag accumulator and edge pull, the takeover with claim and veto,
fling/settle/ballistic, controller attach/drain/jump/animate/publish, and the `on_scroll` owed-notification rule. Each owner
supplies only its position arithmetic (`pixels`, `displacement`, `set_allowed_position`, `decay_displacement`, `settle_to_edge`,
base offset): `ScrollWidget` keeps an out-of-range offset, `ListViewWidget` a clamped offset plus `overscroll`. A new surface
implements `ScrollSurface` and copies none of the machinery.

The feel is a pluggable strategy (`crate::physics`, mirroring Flutter): a `ScrollPhysics` trait (drag mapping, boundary rejection,
ballistic simulation, fling thresholds, spring; composed with `.chain(parent)`, never inheritance) is orthogonal to
`OverscrollEffect` (how a rejected displacement paints: `Translate`/`Stretch`/`None`) and to `Simulation`
(Friction/Spring/Clamping/Bouncing ports). `default_physics()`/`default_overscroll_effect()` pick the pair per platform (Android
`Clamping` + `Stretch`; elsewhere `Bouncing` + `Translate`) unless the app names its own. `RubberBand` is an explicit opt-in only,
and the only physics still on the legacy fling/settle path (`create_ballistic_simulation` returns `None` by design). `PageSnap`
(`physics/page_snap.rs`; `fraction(f)` or `extent(px)`) chains over `default_physics()` or a named parent, overriding only
`create_ballistic_simulation` so an in-range release ends on a page boundary. Wheel input is a physics-independent hard clamp
offered to the content under the pointer first — the innermost surface that can move takes the notch, one at its pushed edge
passes it outward (`ScrollSurface::route_wheel`). A release carries interrupted momentum forward only when it plainly continues it
(Flutter's `ScrollDragController.end`).

Each drag `Move` hands the physics that move's raw delta against the live position (a path-dependent mapping). The core
accumulates the boundary-rejected excess as its own edge-pull state, so pull-to-refresh and `Stretch` fire under a clamping
physics too. Stretch is a paint-only affine scale about the held edge (`stretch_transform`); no layout pass reads the pull. A
ballistic simulation that ends fully pinned outward stops early, handing the residual pull to the settle path.

**Nested-scroll arbitration** runs the ambient-claim shape of the navigator's edge-swipe arming (`nav::ambient`'s
`with_swipe_claim`/`ambient_swipe_claim`, see *Data Flow*): a surface pushes a fresh claim cell around the `Down` it forwards, the
nearest scrollable underneath reports what it could do with the drag (`InnerScrollState`), and the outer reads that at the
touch-slop takeover, deferring when the inner can consume the direction. A claim registers only with real scroll capacity (a
deliberate deviation from Flutter's `BouncingScrollPhysics`); the report is a `Down`-time snapshot and the defer decision is sticky
for the gesture. All surfaces also run a live **multi-contact veto**, a per-gesture cell replaced on every primary `Down` and
consulted on every `Move` takeover check, so a nested `pinch_detector`/`pan_zoom` reporting a second contact still suppresses the
takeover. Claims are axis-aware: a viewport registers only into a same-axis host.

**Long-press follow-ups.** With any of `on_long_press_move_update`, `on_long_press_end` or `on_long_press_cancel` set, a fired long
press reports its later moves (`LongPressMoveDetails`, any distance, no slop), its release (`LongPressEndDetails`) or its cancel, and
no longer falls back to `on_tap`. Until its `Up` or `Cancel` it vetoes every enclosing scroll surface on both axes (the controller
can still scroll); before it fires, a drag past the slop still scrolls and cancels the press. The veto is one flag per scroll
surface, shared with a nested multi-contact recognizer: the detector re-asserts it on every move and lowers it when the press ends;
teardown lowers it without a callback. `global_position` is the detector's last painted origin plus the local position, so it can
lag when an ancestor moves without repainting and is inexact under a scaled ancestor. `on_long_press_cancel` runs on the next
state-bearing pass after a system cancel; `Tester::long_press_then_drag` drives the sequence.

**Programmatic scroll (`scroll_controller.rs`).** `ScrollController` is a cloneable, reactive-free handle that
`ScrollView`/`ListView`/`CustomScrollView::controller` attaches; one surface holds a handle at a time (the latest attach wins) and
dropping the widget detaches it. With `.scrollbar(..)` and no app handle, a surface binds an internal controller only while the
bar is enabled; bar writes are user-move `jump_to`s, so `on_scroll` hears them. Writes (`jump_to`/`animate_to`, plus `scroll_to_item`
on a keyed list) are recorded, not applied: the surface drains them at its next layout or paint (a `ListView` also at rebuild), so
the clamp lands against fresh extents, and recording raises CORE's pending-result-flush flag so a frame-gated mobile shell runs
the applying frame. The queue is bounded by superseding within a segment (the offset commands since the last item command): a jump
supersedes every earlier offset command, an animate an earlier animate but not a preceding jump, and nothing crosses a queued
`scroll_to_item`; item commands keep an ordered channel capped at 8 (the oldest drops, with a debug log). `jump_to` clamps to `[0,
max_offset]`, stops any fling or settle, and (like `animate_to` and `scroll_to_item`) ends an established live drag, whose
remaining `Move`s and `Up` fall through to the child. `animate_to` runs a `TweenSimulation`
on the release fling's ballistic pump; a user `Down` or wheel interrupts it, a later command replaces it, `reduce_motion`
collapses it to a jump, and a fling, a settle and a tween are never live together. A moving tween owes each frame's Housekeeping
flush, so `on_scroll` arrives with no input event (flings and settles stay one event late). `scroll_to_item` is list-only. A write into an empty queue requests a repaint, so a command
wakes an idle desktop loop (`PanZoomController` does not, and waits for the next frame); the rest is in the controller's rustdoc.

**End commands.** `jump_to_end`/`animate_to_end` resolve the end against the maximum when applied, not when recorded, and supersede
as a jump and an animate; a keyed variable-extent list follows the measured end (stuck under `stick_to_end`, else re-pinned for a
bounded settle). A scrollbar's writes are a user jump: applied like `jump_to`, but the user's move. Two `ScrollSurface` hooks
carry both: `after_end_command` and `after_user_jump`.

### Card Switcher
`motion/card_switcher.rs` (`card_switcher`, `CardSwitcherView`): an app-switcher stage over one live
child. The child shrinks through `ChildPod::set_transform` about the centre under a parent-space
rounded clip, with the shadow and scrim painted at the pattern's own scope; input to the shrunken
child is blocked (a tap closes) but only within the pattern's bounds (`blocked_pointer`). It owns its
ramp (a `make_driver` + `Tween`; `reduce_motion` snaps geometry and cross-fades alphas over
`MotionExtras::reduced_fade`). Peers sit in a horizontal `ScrollSurfaceCore` strip on
`PageSnap::extent(stride)` over the default physics (Android clamps the ends) with tap-only
selection: a fling never selects, taps during a centring tween are ignored, the strip's `forward_wheel` does
not scroll it, and it registers no nested claim. `expanding(i)` fades the slots while a peer opens (the handshake with `on_select`),
`.dots_for(count, current)` fills the indicator slot and `.on_center_changed` follows the strip with
a one-Housekeeping-pass lag. Bounds in `card-switcher-blocking-bounded`,
`card-switcher-tap-only-selection` and `card-switcher-android-strip-ends-clamp`.

### Virtualized ListView (baseline)
`ListView`/`ListViewWidget`/`list_view()` live in `frust-widgets` proper (`list_view.rs`); the facade re-exports
them unconditionally. `frust_material::list_view` is a `#[deprecated]` alias. Each `rebuild` reads the retained
widget's scroll offset and cached viewport to materialize only the visible window plus a buffer; with
`sliver_fixed_extent_list` it is among the few `View`s that read retained element state during `rebuild`. Layout
measures only existing pods and paint builds nothing, so the row builder runs only in `rebuild`. Refresh,
overscroll, claim/veto and physics come from the shared core; the clamped-windowing/paint-only-overscroll split
survives.

**Two row-identity models.** `ListView::builder` reconciles by raw index (right for append-only, truncate-only
or full-replace data; a mid-list mutation reattaches state to whatever now sits at the index). `builder_keyed`
adds a `ChildKey` per row (as `keyed()` does for `Flex`, see WIDGETS_CODE_STANDARDS.md) so state follows the
row, and re-anchors its offset in `rebuild`, before windowing, so a single-sided prepend/removal above the
viewport does not visibly jump the content, except rows inserted into, or the row removed from, the gap between
the window's first buffered row and the viewport-top row, which are not corrected
(`list-view-leading-buffer-edit-not-anchored` in [LIMITATIONS.md](LIMITATIONS.md)). A same-frame mutation on
both sides of the anchor can also miss it (registered). A duplicate key `debug_assert!`s with no positional
fallback in release: the first slot wins.

**Variable extents are keyed-only.** `.estimated_item_extent(px)` switches a keyed list from the closed-form
uniform path to a measured-by-key cache (estimate for unvisited rows, laid-out height once visited); on a
positional list it `debug_assert!`s and is inert. A measurement revising a row above the viewport's top is
committed on the next rebuild so the anchor row never moves; during a fling it accumulates and commits at settle.

**Exact extents.** `.item_extent_of(|index| px)` (positional or keyed) declares every row's extent up front, so
the content extent, row positions and a `jump_to` landing are exact from frame one; rows lay out tight at their
declared extent. A cached prefix sum gives an O(1) total and an O(log N) offset-to-row lookup, rebuilt (O(N)
callback calls) only when `item_count` or `.extent_generation(u64)` changes; closures are not comparable, so the
app bumps the generation when extents change under an unchanged count. It is exclusive with
`estimated_item_extent` (`debug_assert!`; release keeps the exact extents), and the builders' required
`item_extent` is overridden. A keyed exact list keeps its anchor row in place across a count or generation
re-flow above the viewport; a positional one never anchors, and the first exact frame shifts nothing. Pick by
knowledge: **uniform** when every row is the same height, **estimated keyed** when heights are only known by
laying rows out, **exact** when the app can state every height.

**Visible range.** `.on_visible_range(|state, first, last|)` reports the inclusive indexes of rows intersecting
the viewport (cache buffer excluded; a row only touching an edge is not visible) in all three modes, beside
`on_near_start`/`on_near_end`: computed at layout, delivered one event late through the same flush, and only on change.

**Scroll-to-item.** `scroll_to_item(key, ItemAlignment, animated)` (`Start`/`Center`/`End`/`Nearest`) resolves
`key` from measured extents plus the estimate for unlaid rows; a key outside the window costs one O(item count)
`key_of` scan, not per frame. The jump lands on the estimate first when rows ahead were never measured, then
settle-and-correct re-resolves each following layout, bounded to 4 layouts or within 0.5px, so an animated
request over unmeasured rows can end with a small correction snap. An unknown key, or any key on a positional
list, is a no-op (debug log).

**Stick to end.** `.stick_to_end(true)` (default off) keeps a keyed variable-extent list on its live edge, e.g. a transcript;
exact-extent, unkeyed and `sliver_list` lists are not covered and ignore it. The list opens at its end and is *stuck*: whenever a
layout finds the maximum offset moved (last row grew, rows appended or prepended, viewport height or width changed) it places the
content at the new end in the same frame, so the end is the anchor instead of the viewport-top row. The pin rides the
measured-correction channel (recorded as the pending correction, so layout's only offset write stays the range clamp) and is
suspended while a drag or fling/spring/tween is live. A user's move (drag, wheel, scrollbar, fling frames) leaving the content more
than `STICK_TO_END_THRESHOLD_PX` short of the end releases it; growth and programmatic commands never do; coming back within the
threshold by any means sticks it again. A `jump_to` or `scroll_to_item` away while stuck is pulled back at the next layout that
finds the end moved, so an app turns the mode off first. `on_stick_change(|state, stuck|)` reports transitions from an event pass
(late ones ride the Housekeeping flush; the initial state is no transition) and `ListViewWidget::is_stuck_to_end` reads the flag.

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
horizontal viewport builds its core on its own axis (only the stretch overscroll transform is transposed). A vertical `ScrollView` holding the focused widget scrolls its caret into view (`reveal_focused_caret(bool)`, default on) and can end the focus session on a downward drag (`dismiss_keyboard_on_drag(bool)`, default off); `ListView`/`CustomScrollView` do neither.

| Sliver | Behavior |
|--------|----------|
| `sliver_box(child)` | Adapts a box child; the geometry every box fallback uses |
| `sliver_padding(insets, child)` | Main-axis insets scroll before/after the child, cross insets narrow it; the cache window is recomputed from the incoming one |
| `sliver_fill_remaining(child)` | `has_scroll_body(true)`: tight to the remaining paint extent. `false`: fills viewport minus preceding scroll extent, or the child's extent if larger |
| `sliver_fixed_extent_list` / `_keyed` | Lazy list of equal-extent rows, closed-form geometry |
| `sliver_list(...)` | Lazy keyed rows that size themselves; measured-by-key extents shared with `ListView` |
| `sliver_grid` / `_keyed` | Lazy grid; `SliverGridDelegate::FixedCrossAxisCount` or `MaxCrossAxisExtent`; `delegate.offset_of_index(i, cross_extent)` is a cell's row offset in the grid's own coordinates (add the preceding slivers' extent, then `jump_to`/`animate_to`) |
| `sliver_persistent_header(delegate)` | `{min_extent, max_extent, build(shrink_offset, overlaps_content)}`, `.pinned`/`.floating` |
| `keep_alive(child)` | Row scope of `sliver_list` only (a request in a nested list's row pins the enclosing `sliver_list` row): parked rows sit uncapped in a `ChildKey` side map, re-adopted when the key returns, released when it leaves the data or the list tears down; parking cancels a gesture |

**Cache-window rule.** A lazy sliver materialises exactly the rows covering
`[scroll_offset + cache_origin, + remaining_cache_extent)`; the viewport's `cache_extent`
(`DEFAULT_CACHE_EXTENT`, frust-widgets-only) is the buffer, with no separate constant. Rows are built only at rebuild
from the last layout's constraints; paint requests one more frame until the rows cover the window
(`ListView`'s catch-up). `ListView` and the sliver lists share the windowed reconcilers and the
`probe_key`/`locate_key` lookup in `slivers/reconcile.rs`. Near-edge callbacks are checked at layout,
only once the list has moved, and delivered through the Housekeeping broadcast.

**Correction-based anchoring.** `sliver_list` anchors on a row. Keyed prepends and removals
above the anchor (and a leading edge disagreeing with the list's start) are reported as
`scroll_offset_correction` and applied by the retry loop, not a private nudge. It applies only while the
list's start is at or above the viewport's leading edge; `locate_key` scans up to 64 items around the anchor.

**Persistent header.** The child is laid out tight at `max(min, max - shrink)`,
`shrink = min(scroll_offset, max)`. Pinned: `paint_origin = overlap` (pinned headers stack), the
header paints at least `min_extent` while its layout extent shrinks to 0, so the next sliver scrolls
under it (`overlaps_content = overlap > 0`). Floating: it keeps its own effective offset and reveals
only under a Forward `user_scroll_direction`, which the viewport sets only during a drag or fling;
floating+pinned never paints below `min_extent`. Layout records shrink and `overlaps_content`; paint
requests a frame when stale; the next `View::rebuild` calls `build` (against the previous view's, as
`ListView` does), so `build` must be pure and the content lags geometry by one frame. A change under a
0.5 px tolerance (`SHRINK_REBUILD_TOLERANCE`, crate-internal) skips the rebuild unless it lands on 0 or
`max_extent`. An outside sliver can hold `SliverPersistentHeaderWidget` (frust-widgets-only; not yet in
the facade) as a geometry core, as Material's `sliver_app_bar` does.

**Item commands.** `ScrollController::scroll_to_item` reaches only a keyed `ListView`; a `CustomScrollView` drops it
(offset commands still work), hence `offset_of_index` for grid cells.

**Forwarding wrappers** forward `Widget::as_sliver`: `Either`/`EitherOf3..8`, `hero`, `shield`,
`auto_scroll_zone`, `GestureDetector`, plus `AnyView`/component boundaries (CORE). Others are boxes.

**Reference surface and limits.** The playground Slivers page (`examples/playground/src/pages/slivers.rs`)
composes a pinned Material `sliver_app_bar`, a floating `sliver_persistent_header`, a variable-extent
`sliver_list`, a 4-column `sliver_grid` and `sliver_fill_remaining` under a `ScrollController`; its
smoke test covers bar heights, wheel, jumps and zero layout-audit mismatches. v1 scope limits:
`sliver-v1-scope`; `ScrollView`/`ListView` are not yet viewport-based.
