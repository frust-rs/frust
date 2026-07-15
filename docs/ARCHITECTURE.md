# ForgeKit - Architecture

## Overview

ForgeKit is a Rust UI framework: a declarative `View` API over a retained
widget tree, rendered through a renderer-agnostic vector scene into a GPU
backend (Vello/wgpu), plus a `forgekit-cli` tool that scaffolds and drives
apps. The workspace is a Cargo workspace of framework crates (`crates/*`)
consumed by app code via the `forgekit` facade crate, and example apps under
`examples/*`. See `docs/spec.md` for the full design rationale.

## Module Structure

| Crate | Responsibility |
|---|---|
| `forgekit-core` | Layers 1+2: the declarative `View` trait, the retained `Widget` trait, box-constraint layout, the `tree_arena`-backed widget tree, and `RenderRoot` (rebuild/layout/paint pass driver). |
| `forgekit-scene` | Layer 3: the renderer-agnostic vector scene / display list (`Scene`, `SceneBuilder`, `Command`, `GlyphRun`) — the stable seam between widgets and the GPU backend. |
| `forgekit-render` | Layer 4: the wgpu + Vello GPU backend. Encodes a `Scene` into a `vello::Scene` and presents it to a window surface. |
| `forgekit-text` | Text shaping: wraps Parley font matching/layout into `TextContext`/`TextStyle`/`TextLayout`, converting shaped text into `forgekit-scene::GlyphRun`s. |
| `forgekit-widgets` | The baseline widget set (currently `Text`/`TextView`) built on `forgekit-core` + `forgekit-text`. |
| `forgekit-shell-desktop` | Desktop preview shell: a winit `ApplicationHandler` event loop that owns the render root, GPU surface, and text context for `cargo run`-based development. |
| `forgekit` | Facade crate: the public app-author API (`App`, `View`, the widget vocabulary) that composes the crates above into the spec's declarative call shape. |
| `forgekit-cli` | Standalone `forgekit` binary: project scaffolding, environment doctor, device discovery. Depends on none of the framework crates above. |

## Layer Dependencies

```
forgekit-scene  (no vello/wgpu — kurbo + peniko only)
    ├── forgekit-core          (view/widget/layout; depends on scene for the PaintScene bridge)
    ├── forgekit-render        (vello/wgpu — consumes Scene)
    └── forgekit-text          (parley — consumes/produces GlyphRun, no vello/wgpu)
forgekit-widgets      = core + scene + text
forgekit-shell-desktop = core + scene + render + text + winit  (the integration point)
forgekit              = core + widgets + shell-desktop         (app-facing facade)

forgekit-cli    (independent binary: clap/anyhow/serde/minijinja/include_dir/thiserror only)
```

**Scene-layer purity rule:** `forgekit-scene`'s and `forgekit-text`'s public
APIs expose only `kurbo` (geometry) and `peniko` (brushes/fonts) types —
`vello`/`wgpu` types are forbidden there so the GPU backend stays swappable.
`vello`/`wgpu` types are confined to `forgekit-render`, surfacing at exactly
two deliberate seams: `RenderContext::create_surface` (takes a
`wgpu::SurfaceTarget`) and `encode_scene` (returns a `vello::Scene` for shells
that drive their own renderer).

`forgekit-cli` has no compile-time dependency on the rendering stack; it is a
separate tool that generates and inspects ForgeKit projects, not a consumer
of the framework.

## Data Flow

**Frame pipeline (desktop shell):**

1. `app_logic(&mut State) -> impl View<State>` runs fresh every frame,
   producing a cheap view descriptor.
2. `RenderRoot::rebuild` diffs the new view against the previous one and
   builds (first frame) or mutates in place (subsequent frames) the
   corresponding retained `Widget` in the arena-backed `WidgetTree`,
   returning `ChangeFlags` (layout/paint dirtiness).
3. `RenderRoot::layout` hands the root widget window-sized `BoxConstraints`
   (constraints flow down, chosen `Size` flows up). When the tree contains
   text, the shell calls `layout_with_text`, which threads the shell-owned
   `forgekit_text::TextContext` through `LayoutCtx` as `&mut dyn Any` — kept
   type-erased so `forgekit-core` has no dependency on `forgekit-text`; text
   widgets recover it via `LayoutCtx::text_context::<TextContext>()`.
4. `RenderRoot::paint` calls each widget's `paint`, which emits draw commands
   into `&mut dyn PaintScene`. `forgekit-scene::SceneBuilder` implements
   `PaintScene` (an additive bridge, not a signature change): `fill_rect` and
   `draw_glyph_run` become real `Command`s in the `Scene`; the legacy
   unshaped `draw_text` is a no-op on this implementation because real text
   must already be shaped into `GlyphRun`s by `forgekit-text` before it can
   reach the scene.
5. The finished `Scene` is encoded (`forgekit_render::encode_scene`) into a
   `vello::Scene` and presented to the window surface by `SurfaceRenderer`.

**CLI flow:** `Cli` (clap) parses into a `Command`, dispatched to a
`commands::*` handler. `create` renders a manifest-listed template tree
(`templates/app/`, embedded at compile time) against a `TemplateContext` —
each manifest entry is content-rendered (`.tmpl`), copied verbatim
(`.copy.tmpl`), or copied as-is, and path segments matching context keys are
expanded (e.g. an org id into nested directories). `doctor` runs a fixed set
of `Validator`s and `devices` runs a fixed set of `DeviceDiscovery`
implementations, both against a shared `DoctorCtx`/`ProcessRunner` — no
handler ever shells out directly. A `BuildInfo` funnel
(mode/flavor/`--define`s) exists for the future `run`/`build` commands but
isn't consumed by any command yet.

## Key Types

| Type | Purpose |
|---|---|
| `View<State>` | Declarative, cheap UI descriptor with a `build`/`rebuild` lifecycle; produced fresh by `app_logic` each frame. |
| `Widget` | Retained tree element (`Any`-bounded for downcast-on-rebuild); implements `layout`/`paint`. |
| `RenderRoot<State, V>` | Owns the `WidgetTree` and previous `View`; drives rebuild → layout → paint for a single-root app. |
| `PaintScene` | Renderer-agnostic paint target widgets draw into; bridged onto `SceneBuilder`. |
| `Scene` / `SceneBuilder` / `Command` | Layer 3 vector display list — the widget/GPU seam. |
| `GlyphRun` | Shaped-glyph carrier from `forgekit-text` into the scene. |
| `TextContext` / `TextStyle` / `TextLayout` | Parley-backed text shaping surface. |
| `RenderContext` / `SurfaceRenderer` | GPU surface setup and per-frame scene presentation. |
| `BuildInfo` / `BuildArgs` | CLI build-mode funnel (debug/profile/release, flavor, defines). |
| `ProcessRunner` | Seam for every external tool invocation in the CLI; fakeable in tests. |
| `Validator` / `DeviceDiscovery` | Pluggable `doctor`/`devices` checks, each independent and non-fatal on failure. |
| `TemplateContext` | Render/path substitution variables for `forgekit create`'s scaffold. |
