# Frust - SHELLS Architecture

## Overview

SHELLS is the seam between the platform-agnostic core+scene+render+text+theme+reactive stack and
each concrete host. `frust-shell-common` is the platform-free plumbing all three concrete shells
share: a type-erased app driver, the frame-gate pacing decision, the render-thread split
vocabulary, the platform-view differ, and a handful of signal-poll seams. `frust-shell-desktop`,
`frust-shell-android`, and `frust-shell-ios` are the real FFI/event-loop integration points — each
owns the event loop or frame callback for its host, drives rebuild → layout → paint → encode →
present every frame, and translates platform-native input/lifecycle/theme/insets/IME/deep-link/
back/platform-view signals into the framework's own vocabulary.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how SHELLS relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `frust-shell-common::app_tree` | Type-erased `AppTree` driver letting a native shell own and drive any app's rebuild/layout/paint/event/semantics cycle without generics |
| `frust-shell-common::frame_gate` | Shared run/skip frame decision and pacing used by both continuous-loop mobile shells |
| `frust-shell-common::render_split` | UI-thread/render-thread split vocabulary (scene handoff, lifecycle commands, completion barrier) all three shells' default frame path uses |
| `frust-shell-common::platform_view` | Differ turning per-paint platform-view frames into an idempotent create/update/dispose backlog for embedding native views |
| `frust-shell-common` (signal-poll seams) | Small process-global slot-plus-poll seams (surface mode, theme, fonts, system UI) each mobile shell drains once per frame |
| `frust-shell-desktop` | winit event loop with a UI-thread/render-thread surface split and pipeline-cache persistence |
| `frust-shell-android` | Sanctioned-unsafe JNI FFI boundary, app-binding macro, and Choreographer-synced frame pipeline |
| `frust-shell-ios` | Sanctioned-unsafe C-ABI FFI boundary, app-binding macro, and CADisplayLink-driven frame pipeline |

## Layer Dependencies

All three concrete shells depend on `frust-core` (`RenderRoot`, widget tree, input, insets,
semantics), `frust-scene` (renderer-agnostic `Scene`/`SceneBuilder`), `frust-text` (a `TextContext`
threaded through layout), and `frust-theme` (theme delivery, brightness/design-language state).
Desktop, Android, and iOS additionally depend on `frust-render` (`SurfaceRenderer`, encode/present,
pipeline cache) and `frust-reactive` (`ReactiveRuntime`, `deep_link`, `back`, `task`); desktop and
Android also depend on `frust-paths` for pipeline-cache and cache-dir persistence, and Android
depends on `frust-plugin` to install the JavaVM/Context handle plugins read.

`frust-shell-common` is a hard platform-free leaf: it carries no `jni`/`ndk`/`winit` dependency, no
unsafe code, and no reactive dependency in its shipped surface, so it can be shared unconditionally
by all three concrete shells. Platform-specific FFI lives only in `frust-shell-desktop` (`winit` +
`accesskit_winit`), `frust-shell-android` (`jni`/`ndk`/`accesskit_android`), and `frust-shell-ios`
(`objc2`/`accesskit_ios`).

Each mobile shell confines all unsafe/FFI code to one named module — `jni_glue` on Android,
`ffi_glue` on iOS — a sanctioned-unsafe-zones convention that keeps the JNI and C-ABI boundaries
auditable as a single surface per platform rather than scattered through the crate. `surface_mode`'s
writer set is pinned by a source-scan conformance test: exactly one call site per platform may
publish a resolved surface mode, a single-writer contract that keeps the desktop/Android/iOS
signal-poll seam race-free without a lock.

## Data Flow

- Desktop: winit events drive `AppTree`'s rebuild → layout → paint, then hand the scene across the
  render-split channel for the render thread to encode and present.
- Android: a Choreographer callback consults `frame_gate` for a run/skip decision, then drives
  rebuild → conditional layout → paint → render-thread present; touch input feeds the app between
  frames.
- iOS: a CADisplayLink tick consults the same `frame_gate`, then unconditionally rebuilds/lays
  out/paints/presents, with optional present-sync gating against platform-view geometry.
- Platform-view embedding: paint-time view frames feed the `platform_view` differ, which exposes a
  command backlog each shell's FFI layer polls and applies to the native view hierarchy, frame-paired
  to keep geometry in sync.
- Cross-cutting host signals (theme, insets, IME, deep-link, back, system UI) arrive via each
  shell's native input path, translate into the framework's vocabulary, and force a relayout/repaint.
- Surface-mode resolution: each mobile shell resolves the host's declared translucency mode against
  actual surface capabilities at configure time and republishes the resolved verdict every frame.
- A set of additive, off-by-default kill-switch env vars (`FRUST_NO_RENDER_THREAD`,
  `FRUST_NO_FRAME_GATE`, `FRUST_NO_ANIM_PACING`, `FRUST_NO_RESAMPLE`, `FRUST_NO_DIRECT_SURFACE`,
  `FRUST_NO_SHADER_EFFECTS`) each revert one frame-pipeline seam independently.

## Key Types

| Type | Purpose |
|------|---------|
| `AppTree` / `new_boxed_app` / `new_boxed_app_with` | Type-erased app driver each shell's FFI/event-loop layer owns and calls into for every lifecycle callback |
| `FrameGate` / `FrameInputs` / `FrameDecision` | Shared run/skip decision the continuous-loop mobile shells consult every tick |
| `RenderCommand` / `Ack` / `AckWaiter` / `SceneFrame` | UI-thread↔render-thread lifecycle and scene-handoff vocabulary underlying each shell's default split frame path |
| `PlatformViewState` / `ViewCommand` | Generation-stamped native-sibling create/update/dispose backlog each mobile shell exposes to its embedding module |
| `SurfaceMode` / `ResolvedSurfaceMode` / `SurfaceModeWatcher` | Host translucency declaration latch plus per-frame resolved-mode publication |
| `android_app!` / `ios_app!` | Facade macros binding a generated app's state/logic to the fixed JNI / C-ABI export set |
