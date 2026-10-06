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
| `frust-core::event` / `animation` / `semantics` | Input/gesture routing, animation curve vocabulary, window insets (incl. window-control corners), and pull-based accesskit semantics |
| `frust-scene::scene` / `builder` | `Scene`/`SceneBuilder`/`Command` — the renderer-agnostic vector display list consumed by `frust-render`. `Command::SceneTexture { id, dest, transform }` draws a caller-owned GPU texture scaled to fill `dest`; `id` is opaque scene-layer data (the same precedent `ShaderProgram`'s own id sets) only the render backend resolves — an unregistered id draws nothing. `Command::ShaderQuad`'s fragment-shader output is treated as premultiplied alpha and rendered by the engine into such a texture ahead of the scene pass |
| `frust-scene::glyph` / `shader` | Carries shaped text and opaque WGSL shader handles from `frust-text` through the `Scene` |
| `frust-reactive::runtime` | Process-wide `ReactiveRuntime`: background executor, root `Owner`, and the `FrameWaker` rebuild-wake bridge |
| `frust-reactive::tracked` | `TrackedScope` — dependency tracking that wakes the shell when a tracked signal it read later changes |
| `frust-reactive::task` / `deep_link` / `back` | `AsyncValue`/`use_task` heavy-work idiom, plus process-wide deep-link and back-press event sources — each delivered `DeepLink` carries a monotonic per-process `sequence`; consumers dedupe a delivery by comparing `sequence`, not the URL text |
| `frust-paths::lib` | Per-platform data/cache-dir resolution (including a macOS legacy-XDG read-through fallback and an Android install slot the Android shell fills from `Context.getFilesDir()`/`getCacheDir()` at `nativeInitPlatform`) and an atomic-write helper for desktop and mobile shells |
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
in [LIMITATIONS.md](LIMITATIONS.md) for runtime-verification status. On Android, `data_dir()`/
`cache_dir()` instead resolve the slot the host shell installs via `install_android_dirs`
(first-wins, both paths validated absolute) and answer `None` until the Android shell's
`nativeInitPlatform` has installed it; `HOME`/`XDG_*` are never consulted on that target. The
facade's dependency on `frust-widgets`/`frust-theme` (WIDGETS) and the shell crates (SHELLS) is
the facade/plugin boundary described in the index; CORE itself never depends on either.

## Data Flow

- `Component::build` runs each frame inside a tracked reactive scope, producing a `View` tree that
  `RenderRoot` diffs into the retained `Widget` tree. It returns `impl View<Self::State>`;
  `ComponentView`/`ComponentWidget` erase the result with `AnyView::new` before storing it.
  - **Erasure sits at API boundaries, not call sites.** `AnyView::new`/`any()` is idempotent (an
    argument already an `AnyView<State>` is returned unchanged via a `'static` downcast), so
    `any(any(v))` is one box. Authoring code writes `any()` only where branches of different
    concrete types must unify; widget APIs take `V: View<State>` and erase internally (see
    WIDGETS_ARCHITECTURE.md's *Erasure at the API Boundary*).
  - **Driver closures erase explicitly.** The opaque `build` return captures the `&self`/
    `&mut State` borrows, so a closure that hands the result out (the facade's `run*` drivers, the
    `app!` android/ios/web arms, test drivers) wraps it: `move |s| AnyView::new(root.build(s))`.
  - **Reconciliation identity is unchanged:** position/key plus a downcast to the concrete element
    `TypeId`. That `TypeId` is the concrete widget type only while no wrapper view re-boxes an
    `AnyView` (`type Element = Box<dyn Widget>` forwarding to an inner `AnyView`; in-tree only
    `ReorderableListView`): such a wrapper hides inner type swaps from the check — see
    LIMITATIONS.md `focus-wrapper-erasure-swap-blind`.
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
  focus-routed (`Key`/`Ime`/`EditCommand`, delivered down the recorded focus path to whatever
  widget holds focus *at delivery* — a clipboard verb belongs here because it is *about the
  selection*, which lives wherever focus is), and **broadcast** — `InputEvent::Housekeeping`,
  `InputEvent::Overlay` and a file drop's `FileDropPhase::Ended` follow-up, events every container
  forwards to every child unconditionally, ahead of its capture/focus/hit-test logic, and never
  consumes.
- `RenderRoot::rebuild` dispatches a `Housekeeping` broadcast when a thread-local flag
  (`mark_pending_result_flush`/`take_pending_result_flush`) is set — the seam a widget uses to run
  a deferred callback that needs `&mut State` but was queued during the state-free view diff (a
  navigator's pop-result and `frust-widgets`' gesture long-press latch are the shipped producers;
  see WIDGETS_ARCHITECTURE.md). It then re-runs the root component's `build` + the view diff so the same frame
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
- **Multi-contact pointer routing.** A `PointerId` (`PointerSource::Mouse`/`Touch` plus a slot)
  names a live contact; `PointerEvent` itself carries no identity. A shell reports a non-mouse
  contact as `InputEvent::PointerContact { pointer_id, event }`; the root unwraps it and delivers
  the plain `InputEvent::Pointer` a widget already handles, with `EventCtx::pointer_id()` reporting
  which contact it was (`PointerId::MOUSE` for a bare `Pointer`). With no live capture, slot `0` is
  hit-tested exactly like the mouse and a capture taken on its `Down` latches that id as the
  claimant; slot ≥ 1 is dropped. While a capture is live, the claimant's events take the captured
  path as usual; another contact reaches only the captor — the widget that called
  `EventCtx::capture_contacts()` on its capturing `Down` — and nothing above it on the active path:
  each intervening container is handed an inert `InputEvent::Overlay` broadcast instead of running
  its own pointer handling, while the real event rides alongside, re-based into each pod's space
  (`ChildPod::event_child`/`walk_secondary`/`dispatch_local`); where a container does not forward
  broadcasts to its children (an overlay owner whose captured pod is a floated surface), the real
  event falls back to ordinary delivery to that child alone. Only the claimant's `Up`/`Cancel`
  releases the capture. A thread-local `ContactPass` guard brackets each root dispatch with the
  contact identity and opt-in flag so both survive a `ComponentWidget` boundary's fresh `EventCtx`.
  A container that takes a gesture over from a captured child releases it through
  `EventCtx::release_captured_child`, not `ChildPod::set_active(false)` directly: the claimant
  keeps the capture until its own `Up`/`Cancel`, but the captor's contact opt-in ends
  (`capture_contacts` clears) when the released subtree held it.
- **Scale gestures.** `InputEvent::Scale(ScaleEvent { phase, scale_delta, focal, velocity })`
  (`ScalePhase::Begin`/`Update`/`End`) is hit-tested and bubbles exactly like `Scroll`;
  `OverlayEventKind::Scale` is its floated-surface mirror, and `InputEvent::position`/`translated`/
  `transformed` cover both new variants alongside the existing ones.
- **Desktop file drop.** `InputEvent::FileDrop(FileDropEvent { phase, position, paths })`
  (`FileDropPhase::Hover`/`Drop`/`Cancel`) is hit-tested and bubbles by `position` exactly like
  `Scroll`, so a container routes it with no change of its own. It is deliberately **not** routed
  through the overlay pre-pass: a native OS drag is a window-level signal with no floated-surface
  concept on the platform side, so it is hit-tested straight against the main tree — a drop target
  living inside a popover is a gap this does not close (see `file-drop-desktop-only` in
  [LIMITATIONS.md](LIMITATIONS.md)). Only `frust-shell-desktop` publishes it today (see
  [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)); Android, iOS and web publish nothing. Every
  `Drop` or `Cancel`, handled or not — it may resolve over a region with no drop target, or be
  handled by one widget while the drag opened state in another — is followed by
  `RenderRoot::event` in the same call with `FileDropPhase::Ended`, a positionless broadcast
  (`is_broadcast()`), so all state the drag opened (`frust-widgets`' external drag sessions, one
  per coordinator) ends; answering it must be idempotent. Dropped `paths` arrive verbatim from the
  OS drag source and are untrusted input.
- **Transformed pods.** `ChildPod::set_transform(Option<kurbo::Affine>)` places a child under an
  arbitrary affine, opt-in and unset by default. While set, `ChildPod::contains`/`event_child` map a
  point or a positioned event (`InputEvent::transformed`) through the inverse before hit-testing or
  delivery, paint brackets the child in `PaintScene::push_transform`/`pop_transform`, and
  `ChildPod::semantics_child` reports the subtree at the transformed rect's axis-aligned bounding
  box (an approximation — see `transformed-subtree-semantics-aabb` in
  [LIMITATIONS.md](LIMITATIONS.md)). `frust_core::hit::point_in_transformed_rect` is the public
  helper a canvas hit closure or a pan/zoom container tests against directly (see
  WIDGETS_ARCHITECTURE.md). A transform with no inverse (singular, non-finite) hits and paints
  nothing; a pod with no transform takes the exact pre-existing path.
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
signal subscription. `WindowMetrics.insets` carries the window-control corners too; a corner change
republishes both `WindowInsets` and `WindowMetrics` through the same guarded path.

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

**A session has an identity, not just a flag.** `RenderRoot` carries a `focus_epoch` — focus's
analog of the hover epoch below — and a `ChildPod` stamps it beside the link it records
(`ChildPod::set_focused`), so a link names *which* session it belongs to rather than merely that one
existed. The epoch moves forward around a dispatch and is put back unless that dispatch actually
claimed, and a release advances it outright, which strands every older link by arithmetic:
`ChildPod::holds_live_focus` is the falsifiable read every routing decision takes, while the raw
`is_focused` flag survives for the few things that legitimately want it (a paint-time cull
exemption, a container's own sweep). Arithmetic rather than a sweep is what makes it work off-tree —
a pod floated through the Overlay Portal is reachable by no container's blur sweep, and the root
retires its stale link the next time it holds the pod. The paint seed is composed the same way, per
link: `paint_child` ANDs the pod's own flag, its stamp against the live epoch, and the ancestor
chain, so a branch proves its own claim instead of the root vouching for it from a frame behind.

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
slot is bracketed per pass by the shared `RequestPass` guard rather than a bare clear/take pair, so a
dispatch that re-enters `RenderRoot::event` (nothing in this workspace does today, but the guard
makes it safe regardless) resolves its own nested pass independently and hands the slot back to the
enclosing one on exit, instead of the inner pass clobbering a request the outer pass had already
collected. A shell reads the resolved cursor via `RenderRoot::cursor()` — see
[SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md) for the desktop-only apply path.

The clipboard write and the paste request are the cursor's siblings in that same guard — one bracket
over every pass-scoped request channel, opened and closed per dispatch. `EventCtx::write_clipboard`
answers a `Copy`/`Cut` by putting the text in a last-writer-wins slot; `EventCtx::request_paste`
raises a data-free flag, since two widgets asking in one pass still owe exactly one host read. Both
resolve at the root and a shell drains them after every dispatch
(`RenderRoot::take_clipboard_write`/`take_paste_request`) — **destructive**, where `cursor()` is a
standing level, so a caller that drains and drops loses the edge. A paste is answered with a *new*
focus-routed `EditCommand::Paste(text)` dispatch rather than a return value: the host read may be
asynchronous, and the pass that asked is over by the time it lands. That dispatch carries no identity
of its own — focus routing hands it to whatever field holds focus *at delivery*, which is the field
that asked only if focus never moved in between. A synchronous read has no in-flight window and needs
no guard; an asynchronous one binds its answer to the session that asked, snapshotting the session
identity above (`RenderRoot::focus_epoch`, reached from a shell as `AppTree::focus_epoch`) at the
request and dropping an answer whose epoch no longer matches — never `focus_ime_generation`, which an
edit or a caret move inside one session also moves. See
[SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md) for which tier reads which way. A third channel is a
FIFO rather than a slot — `EventCtx::dispatch_edit_command`/`take_edit_commands`, pass-scoped,
order-preserving, and dropped rather than carried if nobody drains it (a stale `Cut` applied two
gestures later would destroy text). It exists because a floated toolbar and the field it acts on have
no container path between them (see Overlay Portal).

## Overlay Portal

A popover, menu, tooltip or selection toolbar must escape its owner's bounds three ways at once, and
`frust-core::overlay` splits those three between the owner and the root.

**Owner-hosted.** The surface is a plain `ChildPod` the owner builds, keeps and lays out loosely
against `LayoutCtx::window_size` — never against its own constraints, which say nothing about a box
the pod has left. Nothing is re-parented and no second tree exists: the pod keeps its widget state
and its place in the owner's reactive context.

**Root-painted.** The owner does not paint the pod. It registers an `OverlayEntry` from its own
`paint` (`PaintCtx::register_overlay`) carrying an absolute `window_rect` derived from
`PaintCtx::origin`, and `RenderRoot::paint` paints every registered pod *after* the main tree in
band order (`Floating` below `Tooltip`, registration order breaking ties inside a band). Painting
last is the only way a surface escapes its owner's paint order and every ancestor's clip;
recomputing the rect each paint is what makes an anchored surface follow its owner with no
subscription of any kind. Overlay paint outcomes merge into the frame's own, so an animating surface
keeps frames coming exactly like an animating widget in the tree.

An entry's rect need not derive from the owner's own paint origin, either: `OverlayAnchor::Window`
states it directly in window space — a point a caller updates from its own `event` pass rather than
waiting for the next `paint`, for content (a dragged ghost) whose position is itself only ever known
there. A pod registered `Transparent`/`Tooltip` this way is painted last but never hit-tested, so the
pointer riding under it still reaches whatever drop target or widget the main tree has underneath;
`frust-widgets`' drag ghost rides exactly this combination, keeping its session's real semantics on
the in-tree source/target nodes rather than the floated pod (see `drag-ghost-pod-no-semantics` in
[LIMITATIONS.md](LIMITATIONS.md)).

**Root-routed.** Hit testing is bounds-gated, so the root tests the registered rects **first**,
topmost band first, and on a hit dispatches `InputEvent::Overlay` as a broadcast in place of the
original event. The broadcast quotes the owner's `OverlayKey`; the owner alone consumes it and
forwards the window-space payload into its pod, and every other widget ignores it. Because a
broadcast is not user input at the root, the main tree's focus session is untouched — tapping a
popover does not blur the field that opened it — while a capture or focus claim raised inside the
pod bubbles through the owner like any other child's. An entry may declare itself `Transparent`
(painted, never hit-tested, what a tooltip wants) and may ask for the light-dismiss notification a
primary press outside *every* registered rect produces: `OutsideTap::Notify { consume }`, per
surface, where consuming closes the surface without also activating what sits under it and
non-consuming lets the press continue into the main tree.

Entries live exactly one paint pass — the registry is cleared when `RenderRoot::paint` opens and
drained once the main tree has painted. A surface stays alive only while its owner keeps
registering, so a kept-mounted exit animation is just "keep registering while it runs", an owner
that stops (or is unmounted) drops out of the routing table after the next paint, and there is
nothing to unregister and no way to leak an entry. Between passes the root retains the routing half
only — key, band, input class, outside-tap policy, rect — never the pod, so a pod's lifetime stays
exactly its owner's.

v1 deliberately excludes four things: no focus trap (focus inside a pod behaves like focus anywhere
else); no declined-key forwarding (key/IME/edit-command events stay focus-routed and are never
re-offered to an owner that took no focus); no nesting (a surface that itself needs one registers
both from the single owner); and no visibility to `RenderRoot::inspect()` or the semantics pass — a
floated pod publishes no accessibility node and devtools sees the owner, not the surface, so an
assistive-technology user reaches a floated surface through the owner's own node.

The selection toolbar is the first consumer. A field publishes one `SelectionToolbarRequest` per
paint (`PaintCtx::publish_selection_toolbar` — the selection's window-space anchor plus the verb set
that applies) under **both** routes, so the routes diverge downstream of one code path.
`SelectionToolbarPolicy` picks which: `Framework` floats a pod built by the process-wide
`SelectionToolbarBuilder` a design system installs, `Native` floats nothing and leaves the request
on `RenderRoot::selection_toolbar` with a generation a shell diffs to drive the platform's own edit
menu (see [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)). Both slots are process-global and
last-writer-wins; the builder additionally has a set-if-unset install, so two catalogs linked into
one binary cannot fight over it and an app's explicit choice survives a catalog initialising later.

## Key Types

| Type | Purpose |
|------|---------|
| `View<State>` / `Widget` | The declarative/retained pair every UI element implements |
| `RenderRoot<State, V>` | Owns the widget tree and theme; drives rebuild/layout/paint/event |
| `WidgetTree` / `InspectNode` | Read-only tree accessors (`roots`/`children`/`inspect`) and the plain owned snapshot node (id, type name, debug label, absolute bounds, children) they produce |
| `Component` / `ComponentView` / `ComponentWidget` | Stateful widget analog with a per-instance reactive `Owner`; `build` returns `impl View<Self::State>` |
| `AnyView<State>` / `any()` | Type-erased `View` (`Element = Box<dyn Widget>`); construction is idempotent, so erasure happens once at an API boundary |
| `EventCtx` / `EventOutcome` / `InputEvent` | Event-pass context, result, and input vocabulary — including both broadcast variants (`Housekeeping`, `Overlay`; a file drop's `Ended` phase also broadcasts) and the focus-routed `EditCommand` (see Data Flow), opt-in hover claiming (`claim_hover`/`is_hovered`), the multi-contact pair (`pointer_id`/`capture_contacts`), and the per-pass request channels (`set_cursor`, `write_clipboard`, `request_paste`, `dispatch_edit_command`) |
| `PointerId` / `PointerSource` | A pointer contact's identity (device plus slot), riding beside a `PointerEvent` and read via `EventCtx::pointer_id()` — see Data Flow's Multi-contact pointer routing |
| `ScaleEvent` / `ScalePhase` | A pinch/zoom gesture event, hit-tested and bubbling like `Scroll` — see Data Flow's Scale gestures |
| `FileDropEvent` / `FileDropPhase` | A desktop OS file-drag event (`Hover`/`Drop`/`Cancel`), hit-tested and bubbling like `Scroll`, bypassing the overlay pre-pass, plus the root's broadcast-only `Ended` follow-up to every `Drop`/`Cancel` — see Data Flow's Desktop file drop |
| `frust_core::hit::point_in_transformed_rect` / `checked_inverse` | Hit-testing helpers for content drawn under an arbitrary `Affine` — what `ChildPod::set_transform` uses internally, exposed for a canvas hit closure or pan/zoom container |
| `EditCommand` | The four clipboard/selection verbs a shell or a floated toolbar hands the focused editable: `Copy`/`Cut` carry nothing (the widget owns the selection and answers into the clipboard slot), `Paste(text)` carries text already read by the shell, `SelectAll` is pure selection. `Debug` redacts the paste payload |
| `OverlayEntry` / `OverlayKey` / `OverlayBand` / `OverlayInput` / `OutsideTap` | One floated surface's registration and the four rules the root reads back from it — owner identity, z-band, whether it hit-tests at all, and what a press outside every surface delivers (see Overlay Portal) |
| `OverlayEvent` / `OverlayEventKind` | What a routed overlay broadcast carries, always in absolute window space: `Pointer`, `Scroll`, or the positionless `OutsideDown` light-dismiss notification |
| `SelectionToolbarRequest` / `SelectionToolbarActions` / `SelectionToolbarPolicy` | What a field with a selection publishes each paint (window-space anchor plus the enabled verb set) and who draws the bar for it — a `Framework` pod or the platform's `Native` menu |
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
| `ImeState` / `ImeContentType` / `EditingState` | IME surface state: focus, caret, editing text, a content-type hint (Normal/Password/NoSuggestions/Terminal) the shell uses to configure the platform IME, and `suppress_soft_keyboard` (default `false` — a publisher that says nothing behaves exactly as before). A shell that raises an on-screen keyboard must, when this is set, keep the IME surface wired (editing and clipboard routes) while raising none of it. **Residual exposure:** the core publishes the real text even for secret fields; leak-closure depends on shells honouring `content_type` and has not yet been device-verified. No shell reads `suppress_soft_keyboard` yet either. `Debug` impl redacts text to prevent accidental logging. |
