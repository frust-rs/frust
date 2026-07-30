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
| `frust-core::app` / `tree` | `RenderRoot` owns the widget tree and theme, driving the rebuild → layout → paint → event pass |
| `frust-core::component` | `Component`/`ComponentView`/`ComponentWidget` — a stateful-widget analog with retained state and its own reactive `Owner` |
| `frust-core::event` / `animation` / `semantics` | Input/gesture routing, animation curve vocabulary, window insets, and pull-based accesskit semantics |
| `frust-scene::scene` / `builder` | `Scene`/`SceneBuilder`/`Command` — the renderer-agnostic vector display list consumed by `frust-render` |
| `frust-scene::glyph` / `shader` | Carries shaped text and opaque WGSL shader handles from `frust-text` through the `Scene` |
| `frust-reactive::runtime` | Process-wide `ReactiveRuntime`: background executor, root `Owner`, and the `FrameWaker` rebuild-wake bridge |
| `frust-reactive::tracked` | `TrackedScope` — dependency tracking that wakes the shell when a tracked signal it read later changes |
| `frust-reactive::task` / `deep_link` / `back` | `AsyncValue`/`use_task` heavy-work idiom, plus process-wide deep-link and back-press event sources |
| `frust-paths::lib` | Data/cache-dir resolution and an atomic-write helper for desktop and mobile shells |
| `frust::lib` (facade) | Curates core/widgets/theme/reactive/shells into one flat API via `app!`/`run`/`Component` |

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
for directory resolution — its leaf charter and cross-unit callers are also covered in the index.
The facade's dependency on `frust-widgets`/`frust-theme` (WIDGETS) and the shell crates (SHELLS) is
the facade/plugin boundary described in the index; CORE itself never depends on either.

## Data Flow

- `Component::build` runs each frame inside a tracked reactive scope, producing a `View` tree that
  `RenderRoot` diffs into the retained `Widget` tree.
- `RenderRoot::layout` threads box constraints down and sizes up through the widget tree, kept
  renderer- and text-crate-agnostic via a type-erased text context.
- `RenderRoot::paint` walks widgets into a renderer-agnostic `Scene`, later encoded for the GPU by
  `frust-render` (RENDER unit).
- `RenderRoot::event` routes input through the retained tree, tracking capture/focus without a
  separate registry.
- A tracked signal write wakes the shell via the process-wide `FrameWaker`, triggering the next
  rebuild.
- `Component::init`/`teardown` creates/disposes a per-instance reactive `Owner` nested under its
  parent's.
- The `frust` facade re-exports core/widgets/theme/reactive as one flat API, bridging widgets and
  reactive via its own glue modules so neither depends on the other.
- `frust-paths`' dir/atomic-write helpers back GPU pipeline-cache persistence and preference
  storage in the shells and plugins that consume it.
- A compile-time `Send` assertion on `Scene` guards a future render-thread split.

## Key Types

| Type | Purpose |
|------|---------|
| `View<State>` / `Widget` | The declarative/retained pair every UI element implements |
| `RenderRoot<State, V>` | Owns the widget tree and theme; drives rebuild/layout/paint/event |
| `Component` / `ComponentView` / `ComponentWidget` | Stateful widget analog with a per-instance reactive `Owner` |
| `EventCtx` / `EventOutcome` / `InputEvent` | Event-pass context, result, and input vocabulary |
| `PaintCtx` / `PaintScene` / `PaintOutcome` | Paint-pass context and the renderer-agnostic paint target |
| `Scene` / `SceneBuilder` / `Command` | The renderer-agnostic vector display list |
| `GlyphRun` / `ShaderProgram` | Shaped-text carrier and opaque shader handle riding through `Scene` |
| `ReactiveRuntime` / `TrackedScope` / `FrameWaker` | Process-wide reactive substrate and its rebuild-wake bridge |
| `AsyncValue<T>` / `use_task` / `spawn_blocking` | Blessed heavy-work idiom over the reactive substrate |
| `App` / `app!` / `run` / `Component` | The facade's canonical entry surface binding a root `Component` to all platforms |
| `RwSignal` / `Memo` (re-exported) | Facade-flat reactive primitives app state is typed with |
