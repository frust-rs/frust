# Frust - CORE Architecture

## Overview

CORE is the framework's declarative-to-retained spine and public API surface. It covers five
crates: `frust-core` defines the `View`/`Widget` lifecycle, layout, event routing, and the
`Component` state boundary; `frust-scene` is the renderer-agnostic vector display-list seam;
`frust-reactive` is the leaf signals/tasks/executor substrate; `frust-paths` is a tiny leaf
resolving data/cache directories; and the `frust` facade curates all of the above into the one
declarative `app!`/`Component`/`View` API that application code depends on.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how CORE relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `frust-core::view` / `widget` | `View` (per-frame declarative descriptor) and `Widget` (retained tree element: layout/paint/event) — the core declarative/retained trait pair |
| `frust-core::app` / `tree` | `RenderRoot` owns the widget tree and theme, driving the rebuild → layout → paint → event pass; `WidgetTree`/`RenderRoot::inspect()` expose a read-only pre-order snapshot for external tooling |
| `frust-core::component` | `Component`/`ComponentView`/`ComponentWidget` — a stateful-widget analog with retained state and its own reactive `Owner` |
| `frust-core::event` / `animation` / `semantics` | Input/gesture routing, animation curve vocabulary, window insets, and pull-based accesskit semantics |
| `frust-scene::scene` / `builder` | `Scene`/`SceneBuilder`/`Command` — the renderer-agnostic vector display list consumed by `frust-render`. `Command::SceneTexture { id, dest, transform }` draws a caller-owned GPU texture scaled to fill `dest`; `id` is opaque scene-layer data (the same precedent `ShaderProgram`'s own id sets) only the render backend resolves — an unregistered id draws nothing. `Command::ShaderQuad`'s fragment-shader output is treated as premultiplied alpha and rendered by the engine into such a texture ahead of the scene pass |
| `frust-scene::glyph` / `shader` | Carries shaped text and opaque WGSL shader handles from `frust-text` through the `Scene` |
| `frust-reactive::runtime` | Process-wide `ReactiveRuntime`: background executor, root `Owner`, and the `FrameWaker` rebuild-wake bridge |
| `frust-reactive::tracked` | `TrackedScope` — dependency tracking that wakes the shell when a tracked signal it read later changes |
| `frust-reactive::task` / `deep_link` / `back` | `AsyncValue`/`use_task` heavy-work idiom, plus process-wide deep-link and back-press event sources |
| `frust-paths::lib` | Per-platform data/cache-dir resolution (including a macOS legacy-XDG read-through fallback) and an atomic-write helper for desktop and mobile shells |
| `frust::lib` (facade) | Curates core/widgets/theme/reactive/shells into one flat API via `app!`/`run`/`Component`, plus `frust::authoring` — the widget-authoring vocabulary (trait lifecycle, child/event plumbing, geometry) an app needs to implement its own `View`/`Widget` pair without a direct dependency on `frust-core`/`frust-scene`/`frust-text`/`kurbo`/`peniko` |

## Layer Dependencies

Within CORE, dependencies form a strict internal chain: `frust-paths` and `frust-reactive` are
leaves (no dependency on any other CORE crate); `frust-scene` depends only on `kurbo` and `peniko`;
`frust-core` depends on `frust-scene` plus `tree_arena`, `kurbo`, `peniko`, `accesskit`, and
`reactive_graph`; the `frust` facade sits on top, depending on `frust-core`, `frust-reactive`, and
(conditionally) the shell crates.

`frust-core`'s dependency on `reactive_graph` is direct rather than routed through the
`frust-reactive` wrapper — the one place a CORE crate other than `frust-reactive` itself touches
the underlying reactive library. This keeps `frust-reactive` a true leaf while letting `frust-core`
drive the tracked-scope machinery its render loop needs.

`frust-scene` exposes only `kurbo`/`peniko` types in its public API; this is the CORE side of the
scene-layer purity boundary enforced against `frust-render` (see
[ARCHITECTURE.md](ARCHITECTURE.md)). `frust-paths` is consumed by shells and plugins outside CORE
for directory resolution — its leaf charter and cross-unit callers are also covered in the index. On
macOS, `data_dir()`/`cache_dir()` resolve `~/Library/Application Support`/`~/Library/Caches` as the
base (XDG vars never override it) but read through to the pre-macOS-arm legacy XDG location when
only it holds the app's `app_stem()`-named directory — never migrating, copying, or deleting. That
built-in probe is `app_stem()`-granular, so the two differently-shaped in-repo callers
(`plugins/database`, `frust-shell-desktop`'s pipeline cache) instead resolve the legacy base
themselves via the public `legacy_data_dir()`/`legacy_cache_dir()` and do their own file-level
read-through with the same never-migrate contract. See `paths-macos-legacy-fallback-runtime-unverified`
in [LIMITATIONS.md](LIMITATIONS.md) for runtime-verification status. The facade's dependency on
`frust-widgets`/`frust-theme` (WIDGETS) and the shell crates (SHELLS) is
the facade/plugin boundary described in the index; CORE itself never depends on either.

## Data Flow

- `Component::build` runs each frame inside a tracked reactive scope, producing a `View` tree that
  `RenderRoot` diffs into the retained `Widget` tree.
- `RenderRoot::layout` threads box constraints down and sizes up through the widget tree, kept
  renderer- and text-crate-agnostic via a type-erased text context.
- `RenderRoot::paint` walks widgets into a renderer-agnostic `Scene`, later encoded for the GPU by
  `frust-render` (RENDER unit). `PaintCtx::origin` is the widget's **absolute window-space** origin:
  each `ChildPod::paint_child` accumulates it by adding the child's parent-relative offset to the
  parent's already-absolute origin; `ChildPod::origin` itself stays parent-relative.
- A paint pass also carries a frame-request contract: `PaintCtx::request_frame` (`TickClass::Transition`,
  unpaced) marks the pass every-vsync (a max-lattice — any `Transition` request wins over a concurrent
  `CosmeticLoop` one); `request_frame_paced`/`request_frame_paced_at(interval)` (`TickClass::CosmeticLoop`)
  marks it merely throttleable and folds `interval` onto a separate MIN-lattice (tightest interval wins,
  so a fast shimmer beside a slow caret paces the frame at the shimmer's rate; a bare
  `request_frame_paced` folds in `Duration::ZERO`, meaning "at the theme's own rate"). Both aggregates
  surface on `PaintOutcome` (`needs_frame_paced_only`, `paced_interval`) for the shell's frame gate to
  resolve against the active theme's `cosmetic_loop_rate` (see SHELLS_ARCHITECTURE.md).
- `RenderRoot::event` routes input through the retained tree, tracking capture/focus without a
  separate registry. Events fall into three routing classes: hit-tested (pointer/scroll),
  focus-routed (`Key`/`Ime`, delivered down the recorded focus path), and **broadcast**
  (`InputEvent::Housekeeping`) — a non-input event every container forwards to every child
  unconditionally, ahead of its capture/focus/hit-test logic, and never consumes.
- `RenderRoot::rebuild` dispatches a `Housekeeping` broadcast when a thread-local flag
  (`mark_pending_result_flush`/`take_pending_result_flush`) is set — the seam a widget uses to run
  a deferred callback that needs `&mut State` but was queued during the state-free view diff (a
  navigator's pop-result and `frust-widgets`' gesture long-press latch are the shipped producers;
  see WIDGETS_ARCHITECTURE.md). It then re-runs `app_logic` + the view diff so the same frame
  reflects the mutated state, bounded at `MAX_PENDING_RESULT_FLUSH_PASSES` (3) passes; a remainder
  past the cap folds into `paint`'s `needs_frame` so a dirty-driven desktop loop still wakes for it
  next frame. The dispatch's `EventOutcome` is propagated as part of the same contract: a
  `needs_redraw` it reports folds into `ChangeFlags::PAINT` and the deferred-frame flag, so a
  flushed callback whose only effect is `EventCtx::request_redraw` (no state the view diff can see)
  still wakes both the mobile frame gate and the desktop `Wait` loop. A non-draining peek,
  `has_pending_result_flush`, lets the mobile frame gate's `deferred_callbacks_pending` input force
  a run before a skipped frame ever reaches the `rebuild` that would otherwise drain the mark.
- A tracked signal write wakes the shell via the process-wide `FrameWaker`, triggering the next
  rebuild.
- `Component::init`/`teardown` creates/disposes a per-instance reactive `Owner` nested under its
  parent's.
- The `frust` facade re-exports core/widgets/theme/reactive as one flat API, bridging widgets and
  reactive via its own glue modules so neither depends on the other — e.g. `router_glue`'s
  `RouteObserver`, a `Copy`/`Send`/`Sync` reactive face over `frust-widgets`' signal-free route-state
  publish (`NavigatorView::on_route_change`), reachable via `provide_context` and attached with
  `.observe(view)`; `RouterDeepLinks::routes()` hands out the one wired to its own router.
- `frust-paths`' dir/atomic-write helpers back GPU pipeline-cache persistence and preference
  storage in the shells and plugins that consume it.
- A compile-time `Send` assertion on `Scene` guards a future render-thread split.
- **Introspection is read-only and zero cost when unused.** `WidgetTree::roots()`/`children()`
  return the arena's own insertion order (the arena stays authoritative); `WidgetTree::inspect()`
  (and `RenderRoot::inspect()`, the same walk over the live tree) produce a plain, owned
  `InspectNode` snapshot per node — id, type name, debug label, absolute bounds, children —
  descending through both the arena's own children and each widget's `Widget::visit_children`
  (default-empty, so a container that hasn't opted in reads as a leaf), naming each node via
  `Widget::type_name` (vtable dispatch) and `ChildPod::inspect_id`. A `WidgetPod`'s type name is
  instead captured once at `WidgetPod::new_typed` construction, since a root pod's element type
  cannot swap. Core carries no serde or devtools-specific knowledge; the DEVTOOLS unit owns
  turning the snapshot into wire types (see DEVTOOLS_ARCHITECTURE.md).

## Window Metrics and Context Delivery

`WindowMetrics` (logical size, device-pixel scale, derived orientation, and window insets) is
delivered via `provide_context` as a **plain value, not a signal** — exactly like `Theme` and
`WindowInsets` already are. Only `deep_link` and `back` are true `RwSignal`s in the host-signal
layer; theme, insets, and now metrics are re-provided plain values each time they change. A widget
or component reads them inside `Component::build` via `use_context::<WindowMetrics>()` without any
signal subscription.

**Contexts are visible inside both build and event passes.** A root-level `provide_context` (the
shell's root `Owner`) makes its context available to every `Component::build` **and** every
`Widget::event` handler, since the shell runs both passes under the same owner. This includes
`Theme`, `WindowMetrics`, and a root component's own `init`-installed contexts. A **nested**
component's own `provide_context` is invisible from its subtree's event handlers: `ComponentWidget::event`
routes the event through an inner `EventCtx` over the component's local state but does not install
the component's owner, so `use_context` inside a handler cannot resolve a sibling's ancestor-provided
value. This is an accepted residual: event handlers can read root-level context but not nested
component context.

**Orientation is derived, not platform-sourced:** no platform callback in either mobile shell carries
an orientation enum — the shell furnishes only `(width, height, scale)`. `Orientation` is computed
via `Orientation::from_size()` (portrait when `height >= width`, including exact squares as
portrait) and never tracks a device orientation-lock. This is a stable guarantee pinned by tests.

**Context is not reactive.** `provide_context` is a plain insert into the owner's context map — it
notifies nothing — and `use_context` creates no subscription, so a metrics write neither marks the
tracked scope dirty nor wakes a frame. A new value therefore becomes visible only on the **next**
rebuild, which the resize or inset change that produced it already drives. Delivery is
pull-on-next-frame, not push; do not write code that assumes writing a context triggers a rebuild.
(Signal writes are the reactive path and do wake — but only `deep_link` and `back` are signals.)

A shell must still re-provide only on actual change, guarded by `WindowMetricsPublisher`'s check.
The reason is cost at the FFI boundary, not a rebuild storm: an unconditional per-frame re-provide
would burn a lock write plus an allocation every frame on the mobile path. Separately, when a rebuild
does run it rebuilds the whole app — there is no per-component skipping — which is affordable only
because builds are cheap by construction.

## Focus/IME Lifecycle

`RenderRoot` caches only two focus-adjacent values — `focus_active` (root-level mirror of "some
widget holds focus") and `ime_state` (the last-published IME surface) — and they are always either
both cleared or paired into one **active** session; there is no at-rest inactive surface. A widget
publishing an *inactive* IME surface therefore reads as a full session release, not a value update:
one primitive clears `focus_active` and `ime_state` together, exactly like an outside-tap blur or an
explicit `EventCtx::release_focus`.

A monotonic `focus_ime_generation` counter bumps exactly once per release (a paired clear counts as
one edge, not two) and once per focus/IME change otherwise. `RenderRoot::focus_ime_generation()`
(exposed to shells via `AppTree::focus_ime_generation`) is what the mobile frame gate caches and
diffs each tick to derive `FrameInputs::focus_or_ime_changed` (see SHELLS_ARCHITECTURE.md) — a
same-value republish, such as the paint pass re-publishing an unchanged surface every frame a field
stays focused, never moves it.

A structural rebuild that tears down, type-swaps, or clears the `focused` flag of a pod holding the
recorded focus path — *on the live focus chain* — runs inside a state-free view diff with no
`RenderRoot` handle to release the session itself. Liveness is tracked through the rebuild pass by
a `BuildCtx` effective-focus AND-chain (`has_focus`/`with_focus_link`), seeded at the root from the
session mirror (`focus_active || ime_state.is_some()`) and threaded through pod descent and the
component boundary, so a stale `focused` flag under an already-blurred ancestor marks nothing. A
live loss instead raises a thread-local `mark_focus_orphaned` flag — mirroring
`mark_pending_result_flush` above, including the same idempotent, thread-affine, data-free shape —
and `RenderRoot::rebuild` drains it (`take_focus_orphaned`) after its deferred-callback loop and
releases the session, so the root's cached focus/IME state can never outlive the widget it described.

## Hover and Cursor

Hover has no Enter/Leave phase; it is an opt-in claim a widget records from its own uncaptured
`Move` handler (`EventCtx::claim_hover`), not a state the pipeline infers. `RenderRoot` caches two
values for it — `hover_active` (root-level mirror, the hover analog of `focus_active`) and
`hover_epoch` (the identity of the last completed hover pass) — and every `ChildPod` carries a
stamp (`hover_epoch()`, no setter) rather than a flag: a claim stamps the live epoch up the pod
chain, and a link counts as hovered only while its stamp still matches the live epoch, ANDed with
the chain exactly like focus. `RenderRoot::event` advances the epoch once per hover pass — an
uncaptured `Move` (which may record a claim) or the `Down`/`Up`/`Cancel` that ends a hover outright
— which strands the previous claimant's stamp with no leave event or explicit clearing required. A
structural rebuild that drops the pod holding the *live* claim (not merely a stale, already-stranded
one) has no hover pass to strand it with — `RenderRoot::rebuild` releases it explicitly instead, via
the same thread-local mark/drain shape (`mark_hover_orphaned`/`take_hover_orphaned`) as the
focus-orphan release above, so an unmounted claimant can never leave the root's hover mirror standing
on a widget that no longer exists.

What is recorded is a **path**, so `EventCtx::is_hovered`/`PaintCtx::is_hovered` answer "this
widget or a descendant of it holds the link" (CSS `:hover` semantics — an enclosing container reads
hovered while the pointer is over a claiming child; siblings and off-path widgets read `false`).
The paint-time read is authoritative, since a pointer that left a widget never delivers that widget
another event. Repaints split between the two sides: a consumer's own latched hover flag, redraw-
gated on its change, is what produces the frame for hover *gain* and for a claimant-to-claimant
handoff (the root's mirror is identity-free and cannot see either), while the root manufactures the
one frame nobody can ask for — a hover ending with no new claimant. See `docs/CODE_STANDARDS.md`'s
Interaction Semantics for the consumer contract.

The cursor is hover's sibling channel and deliberately not derived from it: `EventCtx::set_cursor`
writes a per-pass, thread-local request that `RenderRoot::event` resolves into the cached `cursor`
field on any pointer `Move` (captured included, so a drag keeps its own shape), last writer wins,
and absence resolves to `CursorIcon::Default`. Every other pass leaves `cursor` standing. The request
slot is bracketed per pass by a `CursorPass` guard rather than a bare clear/take pair, so a dispatch
that re-enters `RenderRoot::event` (nothing in this workspace does today, but the guard makes it safe
regardless) resolves its own nested pass independently and hands the slot back to the enclosing one
on exit, instead of the inner pass clobbering a request the outer pass had already collected. A shell
reads the resolved cursor via `RenderRoot::cursor()` — see
[SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md) for the desktop-only apply path.

## Key Types

| Type | Purpose |
|------|---------|
| `View<State>` / `Widget` | The declarative/retained pair every UI element implements |
| `RenderRoot<State, V>` | Owns the widget tree and theme; drives rebuild/layout/paint/event |
| `WidgetTree` / `InspectNode` | Read-only tree accessors (`roots`/`children`/`inspect`) and the plain owned snapshot node (id, type name, debug label, absolute bounds, children) they produce |
| `Component` / `ComponentView` / `ComponentWidget` | Stateful widget analog with a per-instance reactive `Owner` |
| `EventCtx` / `EventOutcome` / `InputEvent` | Event-pass context, result, and input vocabulary — including the `Housekeeping` broadcast variant (see Data Flow), opt-in hover claiming (`claim_hover`/`is_hovered`), and cursor requests (`set_cursor`) |
| `PaintCtx` / `PaintScene` / `PaintOutcome` | Paint-pass context and the renderer-agnostic paint target; `PaintCtx::is_hovered` is the authoritative hover read, `PaintCtx::origin` the absolute window-space origin (see Data Flow). `PaintScene` additionally carries `fill_rounded_rect_radii`/`push_clip_rounded_radii`/`stroke_path_dashed` — default, delegating methods, so no third-party sink is forced to implement per-corner rounding or dashing itself. `draw_scene_texture(id, dest)` composites an externally bound texture, `id` a `SceneTextureId::get()` from the `frust::gpu` `ExternalPass` seam (see RENDER_ARCHITECTURE.md); an unbound id draws nothing and is warned once by the engine, the paint is always blended, and the method is itself defaulted to a no-op |
| `DiscardScene` | frust-core's all-no-op `PaintScene` sink: runs a subtree's paint pass purely for its side effects (hero-rect capture through `PaintCtx::with_hero_registry`, paint-time widget state) with no scene command emitted — see WIDGETS_ARCHITECTURE.md's "Alpha-zero paint redirect" for the consumer-side detail |
| `CornerRadii` / `DashPattern` / `TextAlign` / `TextOverflow` | Re-exported through `frust-core` and `frust::authoring`; the per-corner-rounding, dashed-stroke, and text-overflow vocabulary `PaintScene`/`frust-text` consume (see RENDER_ARCHITECTURE.md) |
| `CursorIcon` | Non-exhaustive pointer-shape request vocabulary (`Default`/`Pointer`/`Text`/`Grab`/`Grabbing`/`ColResize`/`RowResize`/`NotAllowed`) a widget asks for via `EventCtx::set_cursor`, resolved into `RenderRoot::cursor()` |
| `Scene` / `SceneBuilder` / `Command` | The renderer-agnostic vector display list |
| `GlyphRun` / `ShaderProgram` | Shaped-text carrier and opaque shader handle riding through `Scene` |
| `ReactiveRuntime` / `TrackedScope` / `FrameWaker` | Process-wide reactive substrate and its rebuild-wake bridge |
| `AsyncValue<T>` / `use_task` / `spawn_blocking` | Blessed heavy-work idiom over the reactive substrate |
| `App` / `app!` / `run` / `Component` | The facade's canonical entry surface binding a root `Component` to all platforms |
| `WindowMetrics` / `Orientation` | Window shape delivered as a plain `provide_context` value (not a signal) — see "Window Metrics and Context Delivery" |
| `RwSignal` / `Memo` (re-exported) | Facade-flat reactive primitives app state is typed with |
| `RouteObserver` | Facade-level reactive face over a navigator's published route stack — see Data Flow |
| `ImeState` / `ImeContentType` / `EditingState` | IME surface state: focus, caret, editing text, and a content-type hint (Normal/Password/NoSuggestions/Terminal) the shell uses to configure the platform IME. **Residual exposure:** the core publishes the real text even for secret fields; leak-closure depends on shells honouring the hint and has not yet been device-verified. `Debug` impl redacts text to prevent accidental logging. |
