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
| `forgekit-core` | Layers 1+2: the declarative `View` trait, the retained `Widget` trait, box-constraint layout, the `tree_arena`-backed widget tree, and `RenderRoot` (rebuild/layout/paint/event pass driver). Its `input` module carries the pointer/scroll event types (`InputEvent`/`PointerEvent`/`EventCtx`/`EventOutcome`) and gesture-math constants (slop, fling decay) the interactive widgets build on. |
| `forgekit-scene` | Layer 3: the renderer-agnostic vector scene / display list (`Scene`, `SceneBuilder`, `Command`, `GlyphRun`) — the stable seam between widgets and the GPU backend. |
| `forgekit-render` | Layer 4: the wgpu + Vello GPU backend. Encodes a `Scene` into a `vello::Scene` and presents it to a window surface. |
| `forgekit-text` | Text shaping: wraps Parley font matching/layout into `TextContext`/`TextStyle`/`TextLayout`, converting shaped text into `forgekit-scene::GlyphRun`s. Also home to `TextEditor`, the Parley-`PlainEditor`-based editing engine `TextInput` and the platform IME bridges drive (see Key Types). |
| `forgekit-widgets` | The baseline widget set (spec §6.4): `Text`, `Button`, `Checkbox`, `Slider`, `TextInput`, `Image`, `Row`/`Column` (`Flex`), `Stack`, `Padding`, `Align`, `SizedBox`, `ScrollView`, `GestureDetector` — each a `View`/`Widget` pair over `forgekit-core` + `forgekit-text`. Any `Flex` child list can be reconciled by explicit identity via `keyed`/`ChildKey` instead of position (see Key Types). |
| `forgekit-shell-desktop` | Desktop preview shell: a winit `ApplicationHandler` event loop that owns the render root, GPU surface, and text context for `cargo run`-based development. Compiled only for non-Android targets. |
| `forgekit-shell-android` | Android platform shell: the JNI runtime behind the fixed `Java_dev_forgekit_ForgeKitSurfaceView_native*` symbols the generated app's Kotlin `SurfaceView` declares, plus the `android_app!` macro that binds a generated app's `State`/`app_logic` to those exports. Composes the same core+scene+render+text stack as the desktop shell; real on Android only, inert elsewhere. |
| `forgekit-shell-common` | Platform-agnostic shell plumbing shared by the Android and iOS shells: the `AppTree` type-erasure that lets a non-generic native handle drive any app's `State`/`app_logic`, plus the `guard`/`sanitize_scale`/`logical_size` FFI-boundary helpers. Depends on `forgekit-core`/`forgekit-scene`/`forgekit-text` only — no `jni`/`ndk`/`winit`, no `unsafe`, no FFI — so it compiles unchanged on every target. |
| `forgekit-shell-ios` | iOS platform shell: the C-ABI runtime behind the fixed `forgekit_*` exports the generated Swift app calls, plus the `ios_app!` macro that binds a generated app's `State`/`app_logic` to those exports. Composes the same core+scene+render+text stack as the other shells and reuses `forgekit-shell-common`'s plumbing; real on iOS only, inert (macro expands to nothing) elsewhere. |
| `forgekit` | Facade crate: the public app-author API (`App`, `View`, the widget vocabulary, the re-exported `android_app!`/`ios_app!` macros) that composes the crates above into the spec's declarative call shape. Depends on `forgekit-shell-android` and `forgekit-shell-ios` unconditionally, and on `forgekit-shell-desktop` only for non-Android targets; `App::run` (the desktop preview loop) is likewise non-Android-only — an Android app is driven entirely by `android_app!`/JNI, an iOS app entirely by `ios_app!`/the C-ABI exports. |
| `forgekit-cli` | Standalone `forgekit` binary: project scaffolding (including full Gradle/Kotlin Android and Xcode/Swift iOS project templates rendered into `<app>/android/` and `<app>/ios/`), environment doctor, device discovery, and the `forgekit run` drive pipeline for both platforms. Depends on none of the framework crates above. |

## Layer Dependencies

```
forgekit-scene  (no vello/wgpu — kurbo + peniko only)
    ├── forgekit-core          (view/widget/layout; depends on scene for the PaintScene bridge)
    ├── forgekit-render        (vello/wgpu — consumes Scene)
    └── forgekit-text          (parley — consumes/produces GlyphRun, no vello/wgpu)
forgekit-widgets       = core + scene + text
forgekit-shell-common  = core + scene + text                     (platform-agnostic; no jni/ndk/winit, no unsafe)
forgekit-shell-desktop = core + scene + render + text + winit    (non-Android integration point)
forgekit-shell-android = core + scene + render + text + shell-common + jni/ndk  (Android integration point; JNI FFI)
forgekit-shell-ios     = core + scene + render + text + shell-common           (iOS integration point; C-ABI FFI)
forgekit  = core + widgets + shell-android (always) + shell-ios (always) + shell-desktop (non-Android only)

forgekit-cli    (independent binary: clap/anyhow/serde/minijinja/include_dir/thiserror only)
```

**Scene-layer purity rule:** `forgekit-scene`'s and `forgekit-text`'s public
APIs expose only `kurbo` (geometry) and `peniko` (brushes/fonts) types —
`vello`/`wgpu` types are forbidden there so the GPU backend stays swappable.
`vello`/`wgpu` types are confined to `forgekit-render`, surfacing at the
seams `SurfaceRenderer::on_surface_created`/`on_surface_created_from_android_window`/
`on_surface_created_from_metal_layer` (surface creation) and `encode_scene`
(returns a `vello::Scene` for shells that drive their own renderer).

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
   `PaintScene` (an additive bridge, not a signature change): filled rects,
   rounded rects, stroked lines, push/pop clip, and glyph runs all become real
   `Command`s in the `Scene`; the legacy unshaped `draw_text` stays a no-op on
   this implementation because real text must already be shaped into
   `GlyphRun`s by `forgekit-text` before it can reach the scene. Paint is also
   the animation driver: a widget may advance animation state (e.g. a fling
   in progress) and call `PaintCtx::request_frame` to ask for another frame
   without waiting on external input; this bubbles up as
   `PaintOutcome::needs_frame` from `RenderRoot::paint`/`AppTree::paint`. The
   desktop shell honors it with an extra `window.request_redraw()`; the
   Android/iOS continuous loops below ignore it since they already produce
   every frame regardless. A focused editable can similarly call
   `PaintCtx::publish_ime_state` during paint, mirroring `request_frame`'s
   bubbling through `ChildPod::paint_child`: `EventCtx::publish_ime_state`
   refreshes `RenderRoot::ime_state`/`AppTree::ime_state` on every edit
   during the event pass, but an app-driven change applied only by the next
   rebuild (e.g. a controlled clear-on-submit) never crosses an event, so
   paint is the other place that surface gets refreshed.
5. The finished `Scene` is encoded (`forgekit_render::encode_scene`) into a
   `vello::Scene` and presented to the window surface by `SurfaceRenderer`,
   which is the spec §8.1 surface lifecycle state machine
   (`SurfacePhase::NoSurface/SurfaceReady/SurfaceLost`,
   `FrameOutcome::Rendered/Skipped/Redraw/SurfaceLost`) shared by every
   shell: a surface can be destroyed and recreated at any time (window
   close, or Android rotation/backgrounding), and rendering is a no-op
   outside `SurfaceReady`.

**Event pipeline:** an `InputEvent` (a `PointerEvent` down/move/up/cancel, or
a scroll delta — already translated into **logical**, density-independent
coordinates by the shell before it crosses into `forgekit-core`) enters the
tree through `RenderRoot::event`, which builds a root `EventCtx` over the
type-erased app state and dispatches to the root widget. Containers
(`Flex`/`Stack`/`Padding`/`Align`/`ScrollView`, and the interactive widgets'
own label/track children) own their children directly as `ChildPod`s — a
`Vec` or named fields, not arena nodes — and route an event down by
translating it into each child's local space (`ChildPod::event_child`); this
is a deliberate divergence from the arena-backed `WidgetTree`, which stays
single-root (arena-backed children, for damage tracking or global a11y
access, are deferred to a later phase). Capture is **by recorded path, not a
global registry**: on `Down` a widget calls `EventCtx::capture_pointer`, and
the enclosing `ChildPod`/`RenderRoot` records it as the active child so
subsequent moves/releases route straight back, auto-releasing on
`Up`/`Cancel`; outside this normal flow, a structural container rebuild
(a child-count change, an `AnyView` type swap at a captured index, or a
keyed reorder — see Key Types' `ChildKey` row) force-releases the capture by
synthesizing a `Cancel` to a still-armed surviving widget (see
`docs/CODE_STANDARDS.md`'s Interaction Semantics for the contract this
relies on). **Focus is a second recorded path, mirroring capture:**
`EventCtx::request_focus`/`release_focus` record/clear the focused child the
same way `capture_pointer` records the active one, and `Key`/`Ime` events
route down that recorded chain with no hit test — a container just forwards
to its focused child. A pointer `Down` that lands on a child which doesn't
(re)claim focus clears the chain (blur-on-outside-tap); a structural rebuild
clears both the capture and focus paths, and `RenderRoot`'s cached
`focus_active`/`ime_state` are not pushed at rebuild time — they self-correct
on the next event pass instead. The focused widget's `ImeState` (its current
`EditingState` plus caret rect) is published through `EventCtx`/`PaintCtx`'s
`publish_ime_state` (see the Frame pipeline) and surfaced to shells as
`RenderRoot::ime_state()`/`AppTree::ime_state()`; a platform IME bridge
pushes a reconciled `EditingState` back in via `AppTree::ime_apply` (see Key
Types' `EditingState`/`ImeState` row). Interactive widgets hold their view-declared
callback as an **erased closure** — an `Rc<dyn Fn(&mut State)>` boxed at
build time into a `Box<dyn FnMut(&mut EventCtx)>` the widget invokes
directly — mirroring the `&mut dyn Any` erasure `LayoutCtx`'s text context
uses, so `forgekit-core` and `forgekit-widgets` carry no knowledge of the
concrete app-state type. The pass never rebuilds or repaints: it returns an
`EventOutcome { handled,
needs_redraw }`, and the shell runs rebuild→layout→paint afterward only if
warranted. The desktop shell is **dirty-driven** — `needs_redraw` becomes a
single `window.request_redraw()`, and `winit`'s `ControlFlow::Wait` keeps
idle CPU near zero with no pending input or animation — while the
Android/iOS shells run a **continuous** per-frame loop
(`Choreographer`/`CADisplayLink`) regardless of the outcome; same `event`
call, different redraw scheduling.

**Android frame pipeline:** the same rebuild/layout/paint pipeline runs
inside JNI callbacks (`forgekit-shell-android`) driven by Kotlin's
`Choreographer`/`SurfaceHolder.Callback` instead of a winit event loop —
`nativeOnFrame` drives one rebuild→layout→paint→render pass per posted
frame, `nativeOnTouch` feeds one pointer contact into the same
`RenderRoot::event` path between frames, and
`nativeOnSurfaceChanged`/`nativeOnSurfaceDestroyed` drive the same
`SurfaceRenderer` state machine as the desktop shell's resize/suspend events
— eleven JNI exports in total, three of which
(`nativeImeApply`/`nativeImeState`/`nativeImeAction`) carry the soft-keyboard
state-sync contract (see Key Types' `EditingState`/`ImeState` row): Kotlin's
`InputConnection` owns text composition against a mirror `Editable`, pushes
whole `EditingState`s to Rust and polls the reconciled state back to keep
the IMM (`updateSelection`) and soft-keyboard visibility synchronized;
hardware/injected Enter also routes through `nativeImeAction`. On
rotation/surface-config changes Android recreates the surface
(`surfaceChanged` tears down and calls
`on_surface_created_from_android_window` again).

**iOS frame pipeline:** `forgekit-shell-ios` is driven by the generated
Swift app instead of an event loop: a UIKit `CADisplayLink` tick calls the
`forgekit_render_frame` C export once per frame, which runs the same
rebuild→layout→paint→render pass. Unlike Android, rotation/bounds changes
call `forgekit_resize` to resize the existing `SurfaceRenderer` surface in
place (`on_surface_changed`) rather than recreate it, since the `CAMetalLayer`
Swift owns survives the whole app lifetime. A `SurfaceLost` surface is
recoverable rather than terminal: the handle retains the layer pointer, and
the next `forgekit_render_frame`/`forgekit_resize` call recreates the surface
from it before proceeding. `forgekit_pause`/`forgekit_resume` (wired to UIKit's resign/become-active
notifications) gate `forgekit_render_frame` into a no-op while backgrounded,
since Metal command submission from a suspended app can get the process
killed. `forgekit_dispatch_touch` feeds one touch contact into the same
`RenderRoot::event` path, translating Swift's fixed phase code (`0`=began,
`1`=moved, `2`=ended, `3`=cancelled) to `PointerPhase`. Three more exports
(`forgekit_ime_apply`/`forgekit_ime_state_json`/`forgekit_string_free`)
carry the same state-sync contract as Android's, driving a full
`UITextInput` conformance on the generated `ForgeKitView` (marked-text and
selection mirror, autocorrect, CJK composition, dictation) — ten
`forgekit_*` C exports in total.

**CLI flow:** `Cli` (clap) parses into a `Command`, dispatched to a
`commands::*` handler. `create` renders a manifest-listed template tree
(`templates/app/`, embedded at compile time, including a full Gradle/Kotlin
Android project under `android.tmpl/` and a full Xcode/Swift project under
`ios.tmpl/`) against a `TemplateContext` — each manifest entry is
content-rendered (`.tmpl`), copied verbatim (`.copy.tmpl`), or copied as-is,
and path segments matching context keys are expanded (e.g. an org id into
nested directories). `doctor` runs a fixed set of `Validator`s and `devices`
runs a fixed set of `DeviceDiscovery` implementations, both against a shared
`DoctorCtx`/`ProcessRunner` — no handler ever shells out directly. `run`
dispatches on the selected device's platform/kind: `android_run` drives a
`BuildInfo`-gated (mode/flavor/`--define`s, currently debug-only) Android
build — preflight (Rust target, cargo-ndk, JDK 17+, adb) →
`./gradlew assembleDebug` (invokes `cargo ndk` to build the Rust `.so`) →
`adb install`/`launch` → a pid-scoped `logcat` stream until Ctrl-C. `ios_run`
drives the iOS Simulator equivalent: preflight (Xcode, Rust sim target, a
booted simulator) → `xcodebuild build` (its run-script build phase invokes
`cargo build` for the simulator/device triple to produce the Rust staticlib)
→ `simctl install` → `simctl launch --console-pty` (streamed) until Ctrl-C,
with a best-effort `simctl terminate` cleanup. A physical iOS device is not
yet driven by `run` — it bails with a Phase 5 (signing pipeline) sentinel.
With no device selected, `run` falls back to a streamed `cargo run` (desktop
preview).

## Key Types

| Type | Purpose |
|---|---|
| `View<State>` | Declarative, cheap UI descriptor with a `build`/`rebuild` lifecycle; produced fresh by `app_logic` each frame. |
| `Widget` | Retained tree element (`Any`-bounded for downcast-on-rebuild); implements `layout`/`paint`. |
| `RenderRoot<State, V>` | Owns the `WidgetTree` and previous `View`; drives rebuild → layout → paint for a single-root app, and routes an `InputEvent` to it via `event`. |
| `InputEvent` / `PointerEvent` / `KeyEvent` / `ImeEvent` / `EventCtx` / `EventOutcome` | Layer 2 input (spec §9): `InputEvent` is a `PointerEvent`, a scroll delta, `Key(KeyEvent)`, or `Ime(ImeEvent)` — `Clone` but not `Copy` (`Key`/`Ime` carry owned `String` payloads) — entering at `RenderRoot::event`; `EventCtx` is the erased-state context a handler mutates (capture pointer, request/release focus, publish IME state, request redraw); `EventOutcome` is the pass's `handled`/`needs_redraw` summary — see Data Flow's Event pipeline. |
| `EditingState` / `ImeState` | The IME state-sync contract at the shell seam (see Data Flow's Event pipeline). `EditingState` is text plus selection/composing indices, UTF-16 code-unit indexed at this boundary (`forgekit-text`'s `TextEditor` owns byte conversion); a platform bridge pushes one in via `AppTree::ime_apply`/`Ime(ApplyEditingState)` and reads a reconciled one back via `RenderRoot::ime_state`/`AppTree::ime_state`, which returns the focused widget's published `ImeState` (`EditingState` + caret rect). |
| `ChildPod` | A container's owned child: boxed widget + layout geometry + capture-active/focused bookkeeping — how `forgekit-widgets`' containers and interactive widgets own children without the arena (single-root-arena divergence; see Data Flow). |
| `AnyView<State>` | Type-erased `View` (element `Box<dyn Widget>`) used wherever children are heterogeneous (a container's child list); mirrors the xilem `AnyView` pattern. |
| `ChildKey` / `keyed` | Explicit child identity for a `Flex` child list (spec §6.3): `keyed(key, view)` attaches a `ChildKey` the reconciler matches old↔new children by, relocating a matched child's widget (preserving its internal state) across a reorder/insert/remove instead of rebuilding it. Keys are all-or-nothing and unique per list; a mixed or duplicate key set falls back to positional matching. A keyed reorder is a structural change like any other (see Data Flow's Event pipeline). |
| `Text` / `Button` / `Checkbox` / `Slider` / `TextInput` / `Image` / `Flex` (`Row`/`Column`) / `Stack` / `Padding` / `Align` / `SizedBox` / `ScrollView` / `GestureDetector` | `forgekit-widgets`' baseline vocabulary — each a `View`/`Widget` pair over the `AnyView`/`ChildPod` substrate; the interactive ones are controlled components (see `docs/CODE_STANDARDS.md`). `TextInput` owns a `TextEditor` and drives it from focus-routed `Key`/`Ime` events; `Image` wraps a decode-once `ImageSource` (an `Arc`-backed `peniko::ImageData` handle) and a fit mode (`ImageFit::Fill`/`Contain`/`Cover`), painted via the new `Command::Image` scene command. |
| `PaintScene` | Renderer-agnostic paint target widgets draw into; bridged onto `SceneBuilder`. |
| `PaintCtx` / `PaintOutcome` | Paint-pass context and result: `PaintCtx::request_frame`/`needs_frame` let a widget advance animation state during paint and ask to be re-invoked without external input; `PaintOutcome::needs_frame` surfaces that through `RenderRoot::paint`/`AppTree::paint` — see Data Flow's Frame pipeline. |
| `Scene` / `SceneBuilder` / `Command` | Layer 3 vector display list — the widget/GPU seam. |
| `GlyphRun` | Shaped-glyph carrier from `forgekit-text` into the scene. |
| `TextContext` / `TextStyle` / `TextLayout` | Parley-backed text shaping surface. |
| `RenderContext` / `SurfaceRenderer` | `RenderContext` owns the wgpu `Instance` and lazily creates/holds the logical `wgpu::Device` itself (adapter-derived limits, not a thin `vello::util` wrapper) so it can request the real adapter limits vello's own device pool cannot; `SurfaceRenderer` is the §8.1 surface lifecycle state machine (`SurfacePhase`/`FrameOutcome`) that owns surface creation and per-frame presentation. |
| `android_app!` | Facade macro binding a generated app's `State`/`app_logic` to the fixed, eleven-export Android JNI surface (init/frame/touch/resume/pause/destroy/surface-changed/surface-destroyed, plus the `nativeImeApply`/`nativeImeState`/`nativeImeAction` IME state-sync trio); the sole Android app entry point. |
| `IosAppHandle` / `ios_app!` | `IosAppHandle` (`forgekit-shell-ios`) is the opaque native handle behind the ten `forgekit_*` C exports (init/resize/render-frame/dispatch-touch/pause/resume/destroy, plus the `forgekit_ime_apply`/`forgekit_ime_state_json`/`forgekit_string_free` IME state-sync trio), mirroring `AndroidAppHandle` (retains the Swift-owned `CAMetalLayer` pointer, guaranteed to outlive the handle until `forgekit_destroy`, so a lost surface can be recreated; a `paused` flag gates frame submission). `ios_app!` is the facade macro binding a generated app's `State`/`app_logic` to those exports; the sole iOS app entry point. |
| `BuildInfo` / `BuildArgs` | CLI build-mode funnel (debug/profile/release, flavor, defines); drives both the (unimplemented) `build` command and `run`'s Android/iOS pipelines. |
| `ProcessRunner` | Seam for every external tool invocation in the CLI, including streaming invocations (`run_streaming`) for long-running processes like `gradlew`/`logcat`; fakeable in tests. |
| `Validator` / `DeviceDiscovery` | Pluggable `doctor`/`devices` checks, each independent and non-fatal on failure. |
| `TemplateContext` | Render/path substitution variables for `forgekit create`'s scaffold. |
