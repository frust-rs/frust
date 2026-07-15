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
| `forgekit-shell-desktop` | Desktop preview shell: a winit `ApplicationHandler` event loop that owns the render root, GPU surface, and text context for `cargo run`-based development. Compiled only for non-Android targets. |
| `forgekit-shell-android` | Android platform shell: the JNI runtime behind the fixed `Java_dev_forgekit_ForgeKitSurfaceView_native*` symbols the generated app's Kotlin `SurfaceView` declares, plus the `android_app!` macro that binds a generated app's `State`/`app_logic` to those exports. Composes the same core+scene+render+text stack as the desktop shell; real on Android only, inert elsewhere. |
| `forgekit` | Facade crate: the public app-author API (`App`, `View`, the widget vocabulary, the re-exported `android_app!` macro) that composes the crates above into the spec's declarative call shape. Depends on `forgekit-shell-android` unconditionally and on `forgekit-shell-desktop` only for non-Android targets; `App::run` (the desktop preview loop) is likewise non-Android-only — an Android app is driven entirely by `android_app!`/JNI. |
| `forgekit-cli` | Standalone `forgekit` binary: project scaffolding (including a full Gradle/Kotlin Android project template rendered into `<app>/android/`), environment doctor, device discovery, and the `forgekit run` drive pipeline. Depends on none of the framework crates above. |

## Layer Dependencies

```
forgekit-scene  (no vello/wgpu — kurbo + peniko only)
    ├── forgekit-core          (view/widget/layout; depends on scene for the PaintScene bridge)
    ├── forgekit-render        (vello/wgpu — consumes Scene)
    └── forgekit-text          (parley — consumes/produces GlyphRun, no vello/wgpu)
forgekit-widgets       = core + scene + text
forgekit-shell-desktop = core + scene + render + text + winit    (non-Android integration point)
forgekit-shell-android = core + scene + render + text + jni/ndk  (Android integration point; JNI FFI)
forgekit               = core + widgets + shell-android (always) + shell-desktop (non-Android only)

forgekit-cli    (independent binary: clap/anyhow/serde/minijinja/include_dir/thiserror only)
```

**Scene-layer purity rule:** `forgekit-scene`'s and `forgekit-text`'s public
APIs expose only `kurbo` (geometry) and `peniko` (brushes/fonts) types —
`vello`/`wgpu` types are forbidden there so the GPU backend stays swappable.
`vello`/`wgpu` types are confined to `forgekit-render`, surfacing at the
seams `SurfaceRenderer::on_surface_created`/`on_surface_created_from_android_window`
(surface creation) and `encode_scene` (returns a `vello::Scene` for shells
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
   `vello::Scene` and presented to the window surface by `SurfaceRenderer`,
   which is the spec §8.1 surface lifecycle state machine
   (`SurfacePhase::NoSurface/SurfaceReady/SurfaceLost`,
   `FrameOutcome::Rendered/Skipped/Redraw/SurfaceLost`) shared by the desktop
   and Android shells: a surface can be destroyed and recreated at any time
   (window close, or Android rotation/backgrounding), and rendering is a
   no-op outside `SurfaceReady`.

**Android frame pipeline:** the same rebuild/layout/paint pipeline runs
inside JNI callbacks (`forgekit-shell-android`) driven by Kotlin's
`Choreographer`/`SurfaceHolder.Callback` instead of a winit event loop —
`nativeOnFrame` drives one rebuild→layout→paint→render pass per posted
frame, and `nativeOnSurfaceChanged`/`nativeOnSurfaceDestroyed` drive the same
`SurfaceRenderer` state machine as the desktop shell's resize/suspend events.

**CLI flow:** `Cli` (clap) parses into a `Command`, dispatched to a
`commands::*` handler. `create` renders a manifest-listed template tree
(`templates/app/`, embedded at compile time, including a full Gradle/Kotlin
Android project under `android.tmpl/`) against a `TemplateContext` — each
manifest entry is content-rendered (`.tmpl`), copied verbatim
(`.copy.tmpl`), or copied as-is, and path segments matching context keys are
expanded (e.g. an org id into nested directories). `doctor` runs a fixed set
of `Validator`s and `devices` runs a fixed set of `DeviceDiscovery`
implementations, both against a shared `DoctorCtx`/`ProcessRunner` — no
handler ever shells out directly. `run`'s `android_run` module drives a
device-selected Android build: a `BuildInfo` funnel (mode/flavor/`--define`s,
currently debug-only) gates the pipeline — preflight (Rust target, cargo-ndk,
JDK 17+, adb) → `./gradlew assembleDebug` (which invokes `cargo ndk` to build
the Rust `.so`) → `adb install`/`launch` → a pid-scoped `logcat` stream until
Ctrl-C. With no Android device selected, `run` falls back to a streamed
`cargo run` (desktop preview).

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
| `RenderContext` / `SurfaceRenderer` | `RenderContext` owns the wgpu instance/device pool; `SurfaceRenderer` is the §8.1 surface lifecycle state machine (`SurfacePhase`/`FrameOutcome`) that owns surface creation and per-frame presentation. |
| `android_app!` | Facade macro binding a generated app's `State`/`app_logic` to the fixed Android JNI exports; the sole Android app entry point. |
| `BuildInfo` / `BuildArgs` | CLI build-mode funnel (debug/profile/release, flavor, defines); drives both the (unimplemented) `build` command and `run`'s Android pipeline. |
| `ProcessRunner` | Seam for every external tool invocation in the CLI, including streaming invocations (`run_streaming`) for long-running processes like `gradlew`/`logcat`; fakeable in tests. |
| `Validator` / `DeviceDiscovery` | Pluggable `doctor`/`devices` checks, each independent and non-fatal on failure. |
| `TemplateContext` | Render/path substitution variables for `forgekit create`'s scaffold. |
