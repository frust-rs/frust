# Frust - Architecture

## Overview

Frust is a Rust UI framework: a declarative `View` API over a retained
widget tree, rendered through a renderer-agnostic vector scene into a GPU
backend (Vello/wgpu), plus a `frust-cli` tool that scaffolds and drives
apps. The workspace is a Cargo workspace of framework crates (`crates/*`)
consumed by app code via the `frust` facade crate, and example apps under
`examples/*`. See `docs/spec.md` for the full design rationale.

## Module Structure

| Crate | Responsibility |
|---|---|
| `frust-core` | Layers 1+2: the declarative `View` trait, the retained `Widget` trait, box-constraint layout, the `tree_arena`-backed widget tree, and `RenderRoot` (rebuild/layout/paint/event pass driver). Its `input` module carries the pointer/scroll event types (`InputEvent`/`PointerEvent`/`EventCtx`/`EventOutcome`) and gesture-math constants (slop, fling decay) the interactive widgets build on. Its `component` module adds `Component` — a `StatefulWidget` analog with retained local state and a per-component reactive `Owner` (see Key Types and Data Flow's Component state boundary); this is the crate's sole, deliberate `reactive_graph` dependency (see Layer Dependencies) — no executor, no tokio. Its `anim` module is the animation vocabulary (`FrameTime`, `Curve`, `Tween`, `SpringDesc`, `AnimationController` — see Key Types): plain data/math, no clock, no scheduler. Its `semantics` module (spec §9) is a pull-based accessibility pass — `Widget::semantics` defaults to a no-op so existing widgets are unaffected, and `RenderRoot::semantics` collects it post-layout into a flat `SemanticsUpdate` (see Key Types, Data Flow's Semantics pass); its `accesskit` dependency (re-exported as `frust_core::accesskit` so `frust-widgets` needs no direct dependency) is a second deliberate, narrow exception joining the `reactive_graph` one above. Its `insets` module carries `WindowInsets`/`EdgeInsets` (see Key Types) — `RenderRoot::set_insets` stores and threads them through `LayoutCtx`/`PaintCtx` exactly like `set_theme` (see Data Flow's Inset delivery). Its `widget` module also carries `PaintCtx::report_hero`/`HeroFrames`/`HeroDirective`, a tagged-rect vocabulary for shared-element ("hero") transitions threaded through paint like the semantics pass above (see Key Types, Data Flow's Navigation flow), and `PaintScene::push_transform`/`pop_transform` (default no-ops; `SceneBuilder` forwards them to its own transform stack). |
| `frust-scene` | Layer 3: the renderer-agnostic vector scene / display list (`Scene`, `SceneBuilder`, `Command`, `GlyphRun`) — the stable seam between widgets and the GPU backend. |
| `frust-render` | Layer 4: the wgpu + Vello GPU backend. Encodes a `Scene` into a `vello::Scene` and presents it to a window surface — exposed as separate `encode()`/`present()` entry points a shell can time or thread independently (see Key Types' `SurfaceRenderer`); an experimental, feature-gated (`cpu-tier`, non-default) `vello_cpu` CPU tier is a selectable fallback via the same `SceneSink` encode seam (see Key Types' `RenderTier`). Its `pipeline_cache` module frames/validates an opaque, adapter-fingerprinted `wgpu::PipelineCache` blob a shell can persist and replay across launches (Vulkan-only — see Data Flow's GPU pipeline cache). |
| `frust-text` | Text shaping: wraps Parley font matching/layout into `TextContext`/`TextStyle`/`TextLayout`, converting shaped text into `frust-scene::GlyphRun`s. `TextContext` owns a width-independent, bounded-LRU shape cache (stats via `shape_cache_stats()`) — a width change re-runs only line-breaking on the cached shaped layout, never a full re-shape. Also home to `TextEditor`, the Parley-`PlainEditor`-based editing engine `TextInput` and the platform IME bridges drive (see Key Types). Color emoji renders through the same `GlyphRun`/`draw_glyphs` path with no render-path change (vello 0.9's COLR+CPAL and sbix bitmap-strike glyph support, Parley's `GenericFamily::Emoji` fallback); CBDT is unverified pending on-device testing. |
| `frust-theme` | Design-token crate (spec §17): the Material 3 baseline value tables — `ColorScheme` (light/dark role pairs), `TypeScale`, `ShapeScale`, `Elevation`, `MotionScheme` (named springs) — bundled into a `Theme` aggregate, plus the `Theme::from_paint_ctx`/`from_layout_ctx` accessors widgets use to recover a threaded theme (see Key Types, Data Flow's theme delivery). A `DesignLanguage` tag (`Material3`/`Cupertino`) selects which baseline a `Theme` targets, filled by `ColorScheme::cupertino_light`/`cupertino_dark` and `Theme::cupertino_baseline`/`m3_baseline`. A `GlassScale` (spec 6f) — `chrome`/`bar`/`control` tiers, each a `GlassMaterial` recipe of per-brightness `GlassFill` wash stacks, a specular hairline alpha, a drop shadow, and a `blur_radius_intent` future-backend contract — ships as `Theme.glass`, populated by `GlassScale::ios27`/`opaque_material` per baseline (see Data Flow's Glass material tokens). Pure data + constructors: no scene/reactive dependency. |
| `frust-widgets` | The baseline widget set (spec §6.4): `Text`, `Button`, `Checkbox`, `Radio`, `Slider`, `TextInput`, `Image`, `Icon`, `Row`/`Column` (`Flex`), `Stack`, `Padding`, `Align`, `SizedBox`, `ScrollView`, `GestureDetector`, `SafeArea` — each a `View`/`Widget` pair over `frust-core` + `frust-text` + `frust-theme`. Widgets resolve theme tokens at paint/layout time with an unthemed-fallback constant per resolved value (see `docs/CODE_STANDARDS.md`); `TextInput` joins `Text` in resolving its glyph color layout-time-baked (see Data Flow's Theme delivery). `SafeArea` is the v1 consumer of the inset channel (see Key Types, Data Flow's Inset delivery). Any `Flex` child list can be reconciled by explicit identity via `keyed`/`ChildKey` instead of position (see Key Types). `Icon` paints a `kurbo::BezPath` from an `IconData` source, either a user-built path or a generated `icons` module entry (41 Material Symbols vendored as SVG-path `IconSource` consts, Apache-2.0, regenerated from a real Material Symbols checkout by `scripts/gen_icons.py`). `GestureDetector` adds a paint-clock-timed long-press (fires on release or on the first post-threshold move once a press has been held past the threshold). `ScrollView` supports iOS-style drag overscroll with rubber-band resistance, observable via `on_scroll`'s `ScrollInfo` snapshot and triggerable via `on_refresh_release` (pull-to-refresh). `ListView` adds `on_near_start` for near-start-edge pagination. `TextInput` adds a `.multiline(max_visible_lines)` wrap-width mode over its `TextEditor`, with `.submit_on_enter` controlling Enter-key behavior. Its `nav` module (spec §19) adds an imperative page-stack `Navigator` + page transitions, a declarative go_router-subset `Router`, and a `hero(tag, child)` shared-element wrapper morphing a tagged child between two pages during a transition (see Data Flow's Navigation flow) — kept in-crate (not a separate crate) because these need the same crate-private container plumbing (`ChildPod`, `build_child`/`teardown_child`, `cancel_pod`) every other container widget uses; `frust-widgets` itself stays reactive-free, so the router's deep-link signal glue lives in the `frust` facade (see Data Flow's Navigation/Deep-link flow). A `material`/`cupertino` widget catalog (AppBar, Card, Chips, Dialog, FAB, ListView/ListItem, NavigationBar, BottomSheet, Switch, progress indicators, plus Cupertino counterparts for the subset with an iOS equivalent) shares the `material::state_layer` interaction-overlay helper — same `View`/`Widget` pattern as the baseline set, still reactive-free. The `material` catalog's M3-Expressive layer (spec 6f) adds a `shape_morph` primitive (`RoundedPolygon`/`morph_path`, radial-function corner rounding) driving `LoadingIndicator`'s shape-cycling spinner and `ButtonGroup`'s press-emphasis overlay, plus `SplitButton`, `FabMenu`, floating/docked `Toolbar`, and a wavy variant of the progress indicators. The `cupertino` catalog's chrome (`navbar`, `tabbar`, capsule `button`, `switch`, `slider`'s Cupertino branch, `alert_dialog`, `action_sheet`) paints from `Theme.glass` on the Cupertino baseline, degrading to an opaque Material surface fill when `GlassMaterial::is_opaque()` (see Data Flow's Glass material tokens); `tabbar` is the first `GlassScale` consumer and drives a minimize-on-scroll state machine (the event pass sets a target off scroll-delta sign, paint drives the theme's motion spring toward it). |
| `frust-reactive` | Leaf reactive substrate: the process-wide `ReactiveRuntime` (a background tokio runtime, a custom `any_spawner` executor routing `spawn`/`spawn_local`, a UI-thread local task pump, and the root reactive `Owner`) plus `TrackedScope`, the rebuild-dependency-tracking bridge that wakes a shell when a tracked signal changes (see Key Types, Data Flow's Signal-driven wake). Its `deep_link` module is a process-wide deep-link source (spec §19): a shell delivers a platform link via `push_deep_link`, app code reads it via `deep_links()`/`DeepLinks` (facade re-exports) — see Data Flow's Deep-link flow. Its `back` module is the process-wide back-press source, mirroring `deep_link`'s shape: `push_back_press`/`back_presses()` plus a `set_handles_back`/`handles_back` flag — see Data Flow's Back flow. Its `task` module is the blessed heavy-work idiom (`AsyncValue<T>`/`use_task`, see Key Types): a background+UI-thread-coordinator split scoped to a component's `Owner`, keeping every signal write on the UI thread; `spawn_blocking` is a thin facade over the runtime's blocking pool for one-off CPU work, alongside `spawn`/`spawn_local` above. Depends only on `reactive_graph`/`any_spawner`/`tokio` — no `frust-core`, no `winit`/`vello`/`wgpu`; consumed by the three shells and the `frust` facade (see Layer Dependencies). |
| `frust-shell-desktop` | Desktop preview shell: a winit `ApplicationHandler` event loop that owns the render root, GPU surface, and text context for `cargo run`-based development. Compiled only for non-Android targets. |
| `frust-shell-android` | Android platform shell: the JNI runtime behind the fixed `Java_dev_frust_FrustSurfaceView_native*` symbols the generated app's Kotlin `SurfaceView` declares, plus the `android_app!` macro that binds a generated app's `State`/`app_logic` to those exports. Composes the same core+scene+render+text stack as the desktop shell; real on Android only, inert elsewhere. |
| `frust-shell-common` | Platform-agnostic shell plumbing shared by the Android and iOS shells: the `AppTree` type-erasure that lets a non-generic native handle drive any app's `State`/`app_logic`, plus the `guard`/`sanitize_scale`/`logical_size`/`logical_insets` FFI-boundary helpers. Depends on `frust-core`/`frust-scene`/`frust-text`/`frust-theme` (its `theme_override` module is the app-facing `set_app_theme`/`clear_app_theme` seam — see Data Flow's Theme delivery; stays reactive-free in its shipped graph, see Layer Dependencies) — no `jni`/`ndk`/`winit`, no `unsafe`, no FFI — so it compiles unchanged on every target. Its `frame_gate` module is the whole-frame skip gate both mobile shells consult before rebuilding (see Data Flow's Frame gate). Its `resample` module is a pure-logic `PointerResampler` that interpolates buffered pointer moves to the frame boundary (Down/Up/Cancel pass through losslessly) plus deadline-overrun helpers both mobile shells use for instrumentation, disabled via `FRUST_NO_RESAMPLE` (see `docs/DEVELOPMENT.md`). |
| `frust-shell-ios` | iOS platform shell: the C-ABI runtime behind the fixed `frust_*` exports the generated Swift app calls, plus the `ios_app!` macro that binds a generated app's `State`/`app_logic` to those exports. Composes the same core+scene+render+text stack as the other shells and reuses `frust-shell-common`'s plumbing; real on iOS only, inert (macro expands to nothing) elsewhere. |
| `frust` | Facade crate: the public app-author API — the canonical `Component`/`app!`/`run` entry surface (spec §5.5), plus `App`/`View`/the widget vocabulary as the lower-level layer `app!` desugars to — composing the crates above into the spec's declarative call shape. Also home to `decode_image_async` (an off-thread `spawn_blocking` wrapper around `frust-widgets`' synchronous `ImageSource::decode`, composing with `use_task`) — it lives here rather than in `frust-widgets` because that crate stays reactive/tokio-free by charter (see Layer Dependencies). Depends on `frust-shell-android`, `frust-shell-ios`, and `frust-reactive` unconditionally, and on `frust-shell-desktop` only for non-Android targets; `run`/`App::run` (the desktop preview loop) are likewise non-Android-only — an Android app is driven entirely by `android_app!`/JNI, an iOS app entirely by `ios_app!`/the C-ABI exports. |
| `frust-drive` | Drive library: the `ProcessRunner` seam (`run`/`run_streaming`/cancellable `spawn_streaming` + `StreamHandle`), device discovery, environment doctor checks, the `BuildInfo` mode/flavor/defines funnel (sans the clap-derived `BuildArgs`, which stays CLI-side), the Android (Gradle/cargo-ndk) and iOS (xcodebuild/devicectl) run/build pipelines, and the project scaffold/template renderer (full Gradle/Kotlin Android and Xcode/Swift iOS project templates). `spawn_streaming`'s lines ride a bounded, drop-oldest ring buffer (a `LineReceiver` counting `dropped_lines()`) with the child's stderr merged into the same stream, so a slow consumer can't stall the producer and no output is silently inherited to a caller's own tty. Every build/run core is print-free — output flows only through an `on_line` sink argument passed down from the front-end, never a bare `println!` (see `docs/CODE_STANDARDS.md`'s Anti-patterns); the CLI front-end owns printing, the TUI routes the same sink into a session's log tab. Consumed by `frust-cli` and `frust-tui`; no framework-crate or clap dependency. |
| `frust-tui` | Mouse-first ratatui TEA workbench: `engine` (pure `AppState`/`Message`/`update`, returning an `Effect` for the impure actions `update` itself can't perform — stop/launch a session, clipboard copy — that the runner enacts; an mpsc-channeled `Engine`), `ui` (layout/views/theme/a per-frame mouse-region registry with hover, an ANSI-aware session log view — renders `&AppState` only, never mutates the engine), `supervise` (the session-supervision layer: a session is keyed by project root × device target × build mode, moving through a `Configuring→Building→Installing→Running→Exited`/`Killed` lifecycle inferred tolerantly from streamed output; a std drain thread per session bridges `frust-drive`'s blocking `spawn_streaming` handle into a bounded tokio mpsc the engine selects on, coalescing each ready burst of lines into one `Lines` batch send and applying a drop-newest + counted-overflow policy on a full channel — one layer above `frust-drive`'s drop-oldest ring, see the `ProcessRunner` row below), and a tokio-driven `runner` (terminal lifecycle, crossterm event loop, dirty-frame skip, effect enactment). Depends on `frust-drive` only; no other framework crate. |
| `frust-cli` | Standalone `frust` binary: a thin clap front-end. `commands::dispatch` constructs one `RealProcessRunner` and injects it into every handler, which calls `frust-drive` for scaffolding, doctor/device checks, and the run/build/clean drive pipelines, and `frust-tui` for the `tui` subcommand. Depends on `frust-drive` + `frust-tui`; no direct dependency on the framework crates above. |
| `frust-plugin` | Leaf plugin substrate (like `frust-reactive`): owns the Android `(JavaVM, application Context)` platform-handle install a plugin needs to reach the OS through FFI — the shell's `nativeInitPlatform` calls `android::initialize` once, which writes the handles into `ndk-context`'s process-wide slot and sets an atomic ready flag; a plugin reads them back via `android::with_jni_env` (a scoped JNI attach), gated on that flag first — see Data Flow's Plugin flow. No `frust-*` dependencies; inert (the flag never sets, so calls always report `NotInitialized`) on non-Android targets so it stays an unconditional plugin dependency. On Apple there is nothing to publish — the ObjC runtime is globally reachable via `objc2`. |
| `frust-shared-preferences` (`plugins/shared-preferences`) | The first **platform plugin**: a synchronous, thread-safe key-value store (`bool`/`i64`/`f64`/`String`/`Vec<String>`) behind one `SharedPreferences` API, routed by `#[cfg(target_os)]` to three backends — `apple` (`NSUserDefaults` via `objc2`, also serving macOS desktop preview), `android` (`Context.getSharedPreferences` via `frust-plugin`), `file` (a JSON file on Linux/Windows). Depends on `frust-plugin` plus FFI crates only, never on a `frust-*` framework crate. |
| `clean-signals-frust` (`plugins/clean-signals-frust`) | The first **facade-tier plugin**: a glue crate binding the (separately published) `clean-signals` clean-architecture core to Frust — nothing more than a crate depending on `frust`, sitting above the whole framework graph. A standalone workspace excluded from the root Cargo workspace (like `examples/huddle`), since its `clean-signals` dependency is a sibling-checkout path dep until `clean-signals` publishes to crates.io (see `docs/DEVELOPMENT.md`). |

## Layer Dependencies

```
frust-scene  (no vello/wgpu — kurbo + peniko only)
    ├── frust-core          (view/widget/layout; depends on scene for the PaintScene bridge, and on reactive_graph for Component's per-instance Owner)
    ├── frust-render        (vello/wgpu — consumes Scene)
    └── frust-text          (parley — consumes/produces GlyphRun, no vello/wgpu)
frust-reactive       (leaf: reactive_graph + any_spawner + tokio only — no core/scene/render/text/winit)
frust-theme          = peniko + frust-text (+ frust-core, a reverse edge for its from_paint_ctx/from_layout_ctx accessors only — core never depends on theme, so no cycle)
frust-widgets       = core + scene + text + theme
frust-shell-common  = core + scene + text + theme             (platform-agnostic; no jni/ndk/winit, no unsafe, reactive-free in shipped deps)
frust-shell-desktop = core + scene + render + text + theme + winit + reactive    (non-Android integration point)
frust-shell-android = core + scene + render + text + theme + reactive + shell-common + jni/ndk + frust-plugin (Android-gated)  (Android integration point; JNI FFI)
frust-shell-ios     = core + scene + render + text + theme + reactive + shell-common           (iOS integration point; C-ABI FFI)
frust  = core + widgets + reactive + shell-android (always) + shell-ios (always) + shell-desktop (non-Android only)

frust-drive  (leaf: no frust-* deps, no framework-crate deps — process/devices/doctor/build/scaffold + android/ios pipelines)
frust-cli (bin) → frust-tui → frust-drive  (clap front-end → ratatui/crossterm TEA workbench → drive library; no other framework crate)

frust-plugin  (leaf: no frust-* deps; Android-only jni/ndk-context, target-gated; inert stub elsewhere)
plugins/*     (beside the facade, never inside it — the facade never depends on or re-exports a plugin)
    ├── frust-shared-preferences  = frust-plugin + FFI crates only   (platform plugin)
    └── clean-signals-frust      = frust alone                      (facade plugin)
```

**Plugins are a tier beside the facade, not inside it.** `frust-plugin` is
a leaf by charter — no outgoing `frust-*` edges — though `frust-shell-android`
now depends on it (Android-gated) to install platform handles via its
`initialize` entry; a plugin only reads them back. A **platform plugin**
depends on `frust-plugin` plus FFI crates and never on a `frust-*` framework
crate (keeps it tiny and cycle-free); a **facade plugin** depends on `frust`
alone and sits above the whole graph. Either way the facade never depends on
or re-exports a plugin — an app adds one directly to its own `Cargo.toml`,
the Flutter-pubspec model (see `docs/CODE_STANDARDS.md`'s Plugin
Conventions).

**`frust-reactive` is a leaf substrate**, consumed by the three shells and
the `frust` facade — never by `frust-core`/`frust-scene`/
`frust-widgets`. **`frust-core` depends on `reactive_graph` directly**
(not on `frust-reactive`) for exactly one purpose — `Component`'s
per-instance `Owner` — a deliberate, narrow layering exception rather than a
general reactive dependency: `frust-core` has no runtime, executor, or
tokio dependency. **`frust-shell-common` stays reactive-free in its
shipped dependency graph** (core + scene + text + theme; a test-only
`reactive_graph` dev-dependency backs the root-owner regression test and
never reaches consumers); the mobile shells own their own reactive
wiring directly (`ReactiveRuntime::init`/`pump_local` called from
`frust-shell-android`/`-ios`), keeping shell-common's zero-`unsafe`,
compiles-everywhere charter intact.

**Scene-layer purity rule:** `frust-scene`'s and `frust-text`'s public
APIs expose only `kurbo` (geometry) and `peniko` (brushes/fonts) types —
`vello`/`wgpu` types are forbidden there so the GPU backend stays swappable.
`vello`/`wgpu` types are confined to `frust-render`, surfacing at the
seams `SurfaceRenderer::on_surface_created`/`on_surface_created_from_android_window`/
`on_surface_created_from_metal_layer` (surface creation) and `encode_scene`
(returns a `vello::Scene` for shells that drive their own renderer).

`frust-cli` has no compile-time dependency on the rendering stack; it is a
separate tool that generates and inspects Frust projects, not a consumer
of the framework.

## Data Flow

**Frame pipeline (desktop shell):**

1. `app_logic(&mut State) -> impl View<State>` runs fresh every frame,
   producing a cheap view descriptor. On the desktop shell this runs inside a
   `TrackedScope::track` closure, itself run under the process-wide
   `ReactiveRuntime`'s root `Owner` (`runtime.with_owner(|| scope.track(||
   ...))`): every signal read during the rebuild is recorded as a tracked
   source, which is what lets a later write to one wake the shell (see
   Signal-driven wake below).
2. `RenderRoot::rebuild` diffs the new view against the previous one and
   builds (first frame) or mutates in place (subsequent frames) the
   corresponding retained `Widget` in the arena-backed `WidgetTree`,
   returning `ChangeFlags` (layout/paint dirtiness).
3. `RenderRoot::layout` hands the root widget window-sized `BoxConstraints`
   (constraints flow down, chosen `Size` flows up). When the tree contains
   text, the shell calls `layout_with_text`, which threads the shell-owned
   `frust_text::TextContext` through `LayoutCtx` as `&mut dyn Any` — kept
   type-erased so `frust-core` has no dependency on `frust-text`; text
   widgets recover it via `LayoutCtx::text_context::<TextContext>()`.
4. `RenderRoot::paint(scene, frame_time)` calls each widget's `paint`, which
   emits draw commands into `&mut dyn PaintScene`; `frust-scene::SceneBuilder`
   implements `PaintScene` (fills, rounded rects, strokes, clip/transform
   push/pop, glyph runs) as real `Command`s in the `Scene`. The `frame_time`
   (a shell-supplied `FrameTime` — desktop's `Instant`-since-epoch, Android's
   `Choreographer` tick, iOS's `CADisplayLink` timestamp; never
   `Instant::now()` inside `frust-core`/widgets, see `docs/CODE_STANDARDS.md`)
   and the render root's stored theme (see Theme delivery below) thread down
   through `PaintCtx` unchanged to every descendant, letting a widget advance
   an `anim::AnimationController` (or fling spring) and call
   `PaintCtx::request_frame` to ask for another frame without external
   input; this bubbles up as `PaintOutcome::needs_frame`, honored by the
   desktop shell as an extra `window.request_redraw()` and fed into the
   mobile frame-gate decision instead (see Frame gate below) — an in-flight
   animation is never skipped. A focused editable similarly calls
   `PaintCtx::publish_ime_state` during paint (mirroring `request_frame`'s
   bubbling), refreshing `RenderRoot::ime_state`/`AppTree::ime_state` for an
   app-driven change that never crossed an event (e.g. a controlled
   clear-on-submit). `PaintCtx` also carries `has_focus`, seeded from the
   pod's recorded focus path: a focused editable gates its focus chrome and
   this republish on it, and `RenderRoot::paint` only accepts a bubbled
   `ime_state` while `focus_active` — a guard against resurrecting a surface
   a blur already cleared.
5. The finished `Scene` is encoded (`frust_render::encode_scene`) into a
   `vello::Scene` and presented to the window surface by `SurfaceRenderer`,
   which is the spec §8.1 surface lifecycle state machine
   (`SurfacePhase::NoSurface/SurfaceReady/SurfaceLost`,
   `FrameOutcome::Rendered/Skipped/Redraw/SurfaceLost`) shared by every
   shell: a surface can be destroyed and recreated at any time (window
   close, or Android rotation/backgrounding), and rendering is a no-op
   outside `SurfaceReady`.

**Semantics pass:** `RenderRoot::semantics` (spec §9) walks the tree
post-layout via `ChildPod::semantics_child` (mirroring paint's origin
threading) into a flat `SemanticsUpdate` (node map, root/focus ids), each
node keyed by a stable per-pod id so a platform adapter can track it across
rebuilds; `semantics_generation` lets a shell skip re-pushing an unchanged
tree (iOS additionally serves a cached snapshot to a newly-activated screen
reader so gating never starves a VoiceOver connect). `RenderRoot::perform_accessibility_action`
routes a platform action back through the **normal event path** by
synthesizing pointer events at the target node's bounds, recomputing
semantics fresh per call, so any fire-on-up-inside widget is operable with
no widget-side changes. All three shells push `SemanticsUpdate`s into a
platform accesskit adapter (`accesskit_winit`/`accesskit_android`'s
`InjectingAdapter`/`accesskit_ios`'s `SubclassingAdapter` — the last
source-complete but not yet compiled on this non-macOS host). A modal flag
plus a keyboard-operability convention (claim focus on a first pointer
interaction, dismiss on a focus-routed `Key(Escape)`, desktop only) cover
the catalog's five modal/menu widgets.

**Signal-driven wake:** a write to a tracked signal fires the process-wide
`FrameWaker` (`frust-reactive`; coalesced — N writes between tracked
rebuilds produce one wake); every `spawn_local` task wake, including one
arriving from a background thread (e.g. a tokio timer), fires the same
`FrameWaker` through the executor's composite waker. On desktop the waker
sends a `ShellUserEvent::SignalsDirty` through the winit `EventLoopProxy`,
which pumps the reactive runtime's UI-thread local task queue
(`ReactiveRuntime::pump_local`) and requests a redraw; the next
`RedrawRequested` re-tracks from scratch. The mobile shells need no nudge
step — `Choreographer`/`CADisplayLink` already drive a continuous per-frame
loop — but both pump local tasks once per frame before the
surface-readiness gate, and again at touch/IME entry points.
`ReactiveRuntime::init` is idempotent (a re-init swaps the waker, not the
runtime). **The set side of this contract is a persistent `TrackedScope`
each mobile `AppHandle` wraps its per-frame rebuild in**: only a rebuild
run *inside* a `TrackedScope` subscribes its signal reads, so only then
does a later write flip the process-wide `signals_dirty` flag the mobile
shells drain once per frame as a frame-gate input (see Frame gate below).

**Component state boundary:** a `Component` (`frust-core::component`) is a
`StatefulWidget` analog — retained local state living in the widget tree
behind a `ComponentView<C>` that implements `View<Outer>` for any outer
state, so the hosted subtree diffs against the component's own `C::State`
instead of the ambient app state. `ComponentWidget` builds an inner `EventCtx`
over that local state and routes events into its child through the same
capture/focus/IME contract a single-child container uses, then mirrors the
inner outcome (redraw/capture/focus/IME) back onto the *outer* `EventCtx` —
the boundary a container observes effects through, never mutates across (a
synthesized `Cancel` crossing it still never touches state — see
`docs/CODE_STANDARDS.md`). Each component owns a child reactive `Owner`
(nested under its parent component's, or the shell's root `Owner` at the
top), scoping `provide_context`/`use_context`/`on_cleanup`. Rebuild always
re-runs `C::build` — local state can change independent of the component
value — and reconciles the result exactly like `RenderRoot::rebuild_view`;
teardown tears down the child element, disposes the owner (running its
`on_cleanup`s), and drops the state.

**Theme delivery:** a shell owns the active `frust_theme::Theme` and
delivers it two ways, mirroring the text-context pattern above. To widgets:
boxed type-erased (`RenderRoot::set_theme(Box<dyn Any>)`) and lent as
`Option<&dyn Any>` into every `LayoutCtx`/`PaintCtx`; a themed widget recovers
it with `theme_as::<Theme>()` (or the `Theme::from_paint_ctx`/`from_layout_ctx`
wrappers) — `frust-core` never depends on `frust-theme`. To app code: the
shell `provide_context`s a cloned `Theme` under the reactive root `Owner`, read
reactively via `use_context::<Theme>()` in `Component::build`. A shell
re-pushes both paths together on a brightness change (desktop's
`WindowEvent::ThemeChanged`, or the mobile appearance exports below). An
app can also force the active `Theme` directly via
`frust::set_app_theme`/`clear_app_theme` (`frust-shell-common::theme_override`)
— a process-global override slot each shell polls once per frame
and pushes through both delivery paths; the override wins over the
platform's own light/dark preference until `clear_app_theme` runs, letting
an app (e.g. the widget catalog's design-language toggle) force a specific
`Theme` regardless of appearance changes.
Resolution timing is asymmetric: most themed widgets re-read the theme from
`PaintCtx` every paint pass and self-refresh on a live swap for free, but
`Text` and `TextInput` both bake their resolved glyph color into the shaped
layout at LAYOUT time and `paint` only replays that brush. This stays
correct under the mobile shells' layout-skip gate (see Frame gate below) by
contract, not coincidence: `RenderRoot::set_theme` marks `ChangeFlags::LAYOUT
| PAINT` pending, so a theme swap always forces the relayout `Text`/
`TextInput`'s baked color depends on, even on a frame that would otherwise
skip layout (see `docs/CODE_STANDARDS.md`'s Theming conventions).

**Inset delivery:** a shell converts platform-reported system-bar/cutout/IME
occlusion into logical `WindowInsets` (`view_padding`/`view_insets`) via the
FFI-boundary `logical_insets` helper (mirroring `logical_size`) and delivers
it two ways, mirroring Theme delivery above: to widgets, threaded concrete
(not type-erased — `WindowInsets` is `Copy` scalar data core owns) into
every `LayoutCtx`/`PaintCtx` via `RenderRoot::set_insets` as
`window_insets()`; to app code, `provide_context`d under the reactive root
`Owner` on each push. `WindowInsets::padding()` derives the actually-safe
edge as `max(0, view_padding − view_insets)` per edge (Flutter's
`ViewPadding`/`ViewInsets` provenance), so an IME overlap zeroes the
affected edge toward the occluding surface rather than double-padding.
`set_insets` is `PartialEq`-guarded (a no-op push is free) and marks
`ChangeFlags::LAYOUT | PAINT` pending only on an actual change — the same
forced-relayout contract Theme delivery's brightness swap uses, letting a
mobile frame-gate `Skip` decision still relayout the one time an inset
genuinely moves. `SafeArea` (see Key Types) is the sole widget consumer of
`padding()` in v1.

**Glass material tokens:** `frust-theme::glass` (spec 6f) is pure data — no
scene/reactive dependency, and no blur is rendered here. `GlassScale` bundles
three tiers (`chrome`/`bar`/`control`), each a `GlassMaterial` recipe of a
`blur_radius_intent` (a **future-backend contract**, not a rendered pixel
radius — `0.0` means the opaque path, since the pinned vello 0.9 has no
backdrop-blur primitive), light/dark fill-wash stacks, a specular hairline
alpha, and a drop shadow. `Theme.glass` carries one `GlassScale` per baseline
(`ios27` for Cupertino, `opaque_material` for M3), so a widget branches on
`GlassMaterial::is_opaque()` rather than switching on `DesignLanguage`
directly. The `cupertino` chrome widgets are the first consumers, compositing
the wash/hairline/shadow on the glass branch and degrading to an opaque
Material fill otherwise — a **deliberate static approximation** of real
backdrop blur pending a future render-backend upgrade.

**Event pipeline:** an `InputEvent` (a `PointerEvent` down/move/up/cancel, or
a scroll delta — already translated into **logical**, density-independent
coordinates by the shell before it crosses into `frust-core`) enters the
tree through `RenderRoot::event`, which builds a root `EventCtx` over the
type-erased app state and dispatches to the root widget. Containers own
their children directly as `ChildPod`s — a `Vec` or named fields, not arena
nodes — and route an event down by translating it into each child's local
space (`ChildPod::event_child`); a deliberate divergence from the
arena-backed `WidgetTree`, which stays single-root. Pointer **capture** and
**focus** are each a recorded path (not a global registry) that
`EventCtx::capture_pointer`/`request_focus` set on `Down`/claim and that
subsequent moves or `Key`/`Ime` events route straight back through with no
hit test; a structural container rebuild force-releases a path (a
synthetic `Cancel` for capture, dropped IME state for focus) only for a
child whose identity was actually lost — a stable-prefix or key-matched
survivor (see Key Types' `ChildKey` row) keeps its recorded path across the
rebuild (see `docs/CODE_STANDARDS.md`'s Interaction Semantics). The focused
widget's `ImeState` (current `EditingState` plus caret rect) is published
via `EventCtx`/`PaintCtx::publish_ime_state` and surfaced to shells as
`RenderRoot::ime_state()`/`AppTree::ime_state()`; a platform IME bridge
pushes a reconciled `EditingState` back via `AppTree::ime_apply` (see Key
Types' `EditingState`/`ImeState` row). Interactive widgets hold their
view-declared callback as an **erased closure** (`Rc<dyn Fn(&mut State)>`
boxed into `Box<dyn FnMut(&mut EventCtx)>`), mirroring the `&mut dyn Any`
erasure `LayoutCtx`'s text context uses, so `frust-core`/`frust-widgets`
carry no knowledge of the concrete app-state type. The pass never rebuilds
or repaints — it returns `EventOutcome { handled, needs_redraw }`, and the
shell runs rebuild→layout→paint afterward only if warranted. The desktop
shell is **dirty-driven** (`needs_redraw` becomes one
`window.request_redraw()`, `winit`'s `ControlFlow::Wait` keeps idle CPU
near zero) while Android/iOS run a **continuous** per-frame loop
regardless of the outcome — same `event` call, different redraw
scheduling.

**Frame gate:** the mobile shells' Choreographer/`CADisplayLink` callbacks
keep firing every tick; `frust-shell-common::frame_gate`'s `FrameGate`
decides whether one actually reproduces a frame, from a small OR-list of
dirtiness signals (pending input, pending `ChangeFlags`, an in-flight
`PaintOutcome::needs_frame`, a theme/appearance change, the reactive
`signals_dirty` flag above, pointer capture/focus, an accessibility action,
or post-resume warmup) bundled into `FrameInputs`; any true signal forces
`Run`, else `Skip` — a `Skip` does no rebuild/layout/paint/present work but
still counts as a skipped frame in `FrameStats`. `FRUST_NO_FRAME_GATE=1`
(see `docs/DEVELOPMENT.md`) forces every tick to `Run`. On a `Run`, Android
also skips layout unless `AppTree::take_change_flags` reports
`needs_layout()`, first frame, or resize (the theme-swap contract is Theme
delivery's, above); iOS still relayouts every `Run`, the finer skip not yet
wired there.

**GPU pipeline cache:** `frust-render::pipeline_cache` frames an opaque
`wgpu::PipelineCache` blob behind an adapter-fingerprint header, so a
foreign or driver-updated blob is rejected as a cache miss before it reaches
the sanctioned-`unsafe` pipeline-cache creation call (Vulkan-only — a silent
no-op elsewhere; see `docs/CODE_STANDARDS.md`). `SurfaceRenderer` lets a
shell load a blob before surface creation and persist a larger one back
after compiling its pipelines, both atomically on a background thread so
the first frame never waits on disk.

**Android frame pipeline:** the same rebuild/layout/paint pipeline runs
inside JNI callbacks (`frust-shell-android`) driven by Kotlin's
`Choreographer`/`SurfaceHolder.Callback` instead of a winit event loop: the
frame callback consults the frame gate above and, on a `Run`, drives
rebuild → (layout iff dirty/first/resized) → paint → render per posted
frame; a touch callback feeds one pointer contact into the same
`RenderRoot::event` path between frames; surface-changed/destroyed callbacks
drive the same `SurfaceRenderer` state machine as the desktop shell's
resize/suspend events (rotation recreates the surface). The full JNI export
surface (frame/touch/lifecycle/IME/theme/deep-link/accessibility/insets/
back, plus the plugin `nativeInitPlatform` export — see Plugin flow below)
is the `android_app!` row in Key Types.

**iOS frame pipeline:** `frust-shell-ios` is driven by the generated Swift
app instead of an event loop: a UIKit `CADisplayLink` tick calls the
render-frame export once per frame (its timestamp, in nanoseconds, as
`FrameTime`), consulting the same frame gate as Android before running
rebuild→layout→paint→render; unlike Android, a `Run` still relayouts
unconditionally (the intra-frame layout skip isn't wired here yet). A
resize export resizes the existing `SurfaceRenderer` surface in place (the
`CAMetalLayer` Swift owns survives the app lifetime); a lost surface
recreates itself from the retained layer pointer on the next call.
Pause/resume exports gate frame rendering into a no-op while backgrounded
(Metal submission from a suspended app can get the process killed); a
touch-dispatch export feeds the same `RenderRoot::event` path. The full
C-ABI export surface (init/resize/render/touch/lifecycle/IME/theme/
deep-link/accessibility/insets) mirrors Android's shape and is the
`IosAppHandle`/`ios_app!` row in Key Types (iOS gets no back export — back
is a navigation-bar affordance there, see Back flow below).

**Plugin flow:** a plugin reaches the OS the way any in-process Rust code
would — through FFI crates directly, with no per-plugin native wrapper and
no message-channel bridge. On Android, `JNI_OnLoad` captures the process
`JavaVM`, then the generated Kotlin `FrustSurfaceView` calls the fixed
`nativeInitPlatform(applicationContext)` export once, `Once`-guarded, which
calls `frust_plugin::android::initialize(vm, ctx)` — frust-plugin writes
`(JavaVM, Context)` into `ndk-context`'s process-wide slot, then sets an
atomic ready flag that `with_jni_env` checks first, returning
`PlatformHandleError::NotInitialized` (no panic machinery touched) before
`ndk-context` is ever read; once ready it reconstructs the `JavaVM`,
attaches the calling thread for the closure's scope only (detaching on
return — Frust doesn't own the thread), and hands back a live JNI env plus
the context. On Apple there is no init step: the ObjC runtime is globally
reachable via `objc2`. A project scaffolded before this plumbing existed
just gets `PlatformHandleError::NotInitialized` on first plugin call, never
a panic (see `docs/DEVELOPMENT.md`'s manual gate).

**Deferred: build-time contribution manifests.** A plugin needing an
OS-side contribution (an Android manifest entry, a Gradle dependency, an
Info.plist key) would declare it in a per-platform `frust-plugin.toml`,
discovered via `cargo metadata` at `run`/`build` preflight and merged into
the generated project through anchored marker-comment merges — never a
wholesale re-render, since generated projects are user-editable (the
`local.properties` merge-write precedent). **Implemented: no** — deferred
until the first plugin needs a contribution; `frust-shared-preferences`
needs none.

**Navigation flow:** `nav::navigator()`'s retained page stack is driven by
`NavigatorController`, a cloneable handle that only *records* requested ops
(`push`/`pop`/`replace`) — never self-mutates mid-event, the same
controlled-component convention interactive widgets follow (see
`docs/CODE_STANDARDS.md`). Queued ops drain and apply in
`NavigatorView::rebuild` (a `BuildCtx` pass), so a push/pop lands whether
triggered by a gesture or a background task; a page switch cancels any
in-flight capture on the outgoing top page, clears its focus, and publishes
a cleared `ImeState` so a platform keyboard hides deterministically. Only
the topmost settled **opaque** page (plus any transparent pages above it)
is laid out and painted — covered pages keep their retained widgets (state
survives) but are culled until revealed. A push/pop carrying a
`TransitionSpec` runs a `TransitionDriver` the navigator advances during
paint; while a non-interactive transition is in flight the navigator blocks
pointer routing to pages under the **capture-cancel-before-block
contract** (both pages' captures are synthetically cancelled at transition
start, so the block never leaves a dangling capture behind) — an
edge-swipe pop is the deliberate exception, driving the same transition
interactively from its own pointer stream. `push_transparent_for_result`
composes a transparent push with a result callback — the modal shape
`Dialog`/`BottomSheet` build on. During any transition, both pages'
`hero(tag, child)` wrappers report their bounds through the navigator's
`HeroFrames` registry every paint; where the same tag appears on both
pages, the topmost page's hero paints the position+scale morph onto the
interpolated rect while the counterpart is suppressed — uniform for a
driven transition or an interactive edge-swipe pop.

**Deep-link flow:** a platform delivers a link (cold-start intent data, or
a running app's warm re-delivery) through the fixed FFI export each mobile
shell defines (Android's `nativeOnDeepLink`, iOS's `frust_on_deep_link`,
both above), which calls `frust_reactive::push_deep_link` — the
process-wide source `frust-reactive::deep_link` owns (an
`RwSignal<Option<DeepLink>>` plus a set-once `initial` snapshot, uniform
cold-start/warm delivery through the same signal). App code reads it via
`deep_links()`, tracking `DeepLinks::latest` from `Component::build` like
any other signal (see Signal-driven wake above). The facade's
`RouterDeepLinks` glue (the one seam allowed to see both `Router` and the
deep-link source together, keeping `frust-widgets` itself reactive-free)
dedupes by the last-consumed link and calls `Router::handle_location` on a
new one.

**Back flow:** a hardware/gesture back press enters through Android's
`nativeOnBackPress` export (iOS has no equivalent — back is a
navigation-bar affordance there) and calls `frust_reactive::push_back_press`,
the process-wide counter source `frust-reactive::back` owns (mirrors
`deep_link`'s shape above, counting presses instead of carrying a payload).
The facade's `BackHandler` dedupes by the last-consumed count and pops the
attached `NavigatorController` when `can_pop()`. `handles_back()` — read by
the shell *before* it decides whether to consume a press itself, mirroring
Flutter's `setFrameworkHandlesBack` pre-registration — prefers a **live
can-pop provider** (`set_can_pop_provider`, reading the navigator's current
depth at press time) over a once-per-rebuild `set_handles_back` flag,
closing the stale-by-one-frame window where a poppable stack could
otherwise fall through to activity-finish; the navigator's own `len > 1`
guard makes any residual mis-prediction a safe no-op pop.

**CLI flow:** `Cli` (clap) parses into a `Command`, dispatched by
`commands::dispatch`, which builds one `RealProcessRunner` and injects it
into every thin handler's `run_in` core — the CLI's sole `Real`
construction site. `create` renders `templates/app/` via
`frust-drive::scaffold` against a `TemplateContext` (see Module Structure).
`doctor`/`devices` run fixed `Validator`/`DeviceDiscovery` sets against the
injected runner, so no handler ever shells out directly. `run`/`build`
convert a clap `BuildArgs` into `frust-drive`'s `BuildInfo` funnel
(mode/flavor/defines — debug-default for `run`, release-default for
`build`) and dispatch to `frust-drive`'s per-platform pipelines (see Module
Structure's `frust-drive` row). `run` installs and streams device output
until Ctrl-C, or falls back to a streamed `cargo run` desktop preview —
threading `BuildInfo`'s mode into the `cargo` profile arg and every
`--define` into the spawned env, matching on-device behavior. `build`
resolves an `AndroidArtifact`/`IosArtifact` target into a `BuiltArtifacts`
path list; release builds gate on platform signing (see Key Types'
`BuildInfo` row). `clean` removes Cargo/Gradle build output; `tui` hands
off to `frust-tui`'s entry point from its own tokio runtime, bridging the
async workbench into the CLI's synchronous dispatch.

## Key Types

| Type | Purpose |
|---|---|
| `View<State>` | Declarative, cheap UI descriptor with a `build`/`rebuild` lifecycle; produced fresh by `app_logic` each frame. |
| `Widget` | Retained tree element (`Any`-bounded for downcast-on-rebuild); implements `layout`/`paint`. |
| `Component` / `ComponentView` / `ComponentWidget` | `frust-core::component`'s `StatefulWidget` analog: `Component` declares retained `State` (`init`/`build`); `ComponentView<C>` is the `View<Outer>` adapter usable under any outer state; `ComponentWidget` is the retained element owning `State` + a per-component `Owner` and routing events across the state boundary — see Data Flow's Component state boundary. |
| `ReactiveRuntime` / `TrackedScope` / `FrameWaker` | `frust-reactive`'s process-wide substrate: `ReactiveRuntime` owns the background tokio runtime, the custom `any_spawner` executor, and the root `Owner`; `TrackedScope` runs a rebuild with dependency tracking and fires the swappable `FrameWaker` (coalesced) when a tracked signal later changes — see Data Flow's Signal-driven wake. |
| `AsyncValue<T>` / `use_task` / `spawn_blocking` | `frust-reactive::task`'s blessed heavy-work idiom: `use_task` runs a fetch under a component's `Owner`, tracking result state as `AsyncValue<T>` (`Idle`/`Loading(Option<T>)`/`Ready`/`Error`); a UI-thread coordinator awaits the background half and is the sole signal writer, so writes never race across threads, and owner cleanup aborts the coordinator. `spawn_blocking` is the one-off-CPU-work counterpart to `spawn`/`spawn_local` (see `docs/CODE_STANDARDS.md`'s heavy-work routing table); an already-running closure can't be interrupted. |
| `RenderRoot<State, V>` | Owns the `WidgetTree`, previous `View`, and the boxed active theme (`set_theme`); drives rebuild → layout → paint for a single-root app, and routes an `InputEvent` to it via `event`. |
| `Theme` / `ColorScheme` / `TypeScale` / `ShapeScale` / `Elevation` / `MotionScheme` | `frust-theme`'s design-token bundle (spec §17): `Theme` pairs a light/dark `ColorScheme` (M3 color roles) with a `TypeScale` (`frust_text::TextStyle` per M3 type role), `ShapeScale` (corner-radius tokens), `Elevation` (shadow specs per level), and `MotionScheme` (named M3 spring presets) — see Data Flow's Theme delivery. A `DesignLanguage` tag (`Material3`/`Cupertino`) selects `Theme::m3_baseline`/`cupertino_baseline`; `set_app_theme`/`clear_app_theme` force the active `Theme` app-side, overriding the platform's own preference until cleared. |
| `WindowInsets` / `EdgeInsets` | `frust-core::insets`'s platform-occlusion model (Flutter `ViewPadding`/`ViewInsets` provenance): `WindowInsets` carries `view_padding`/`view_insets` per edge (`EdgeInsets`, re-exported as `WindowEdgeInsets` to avoid a name clash with `frust-widgets`' `Padding`-family `EdgeInsets`); `padding()` derives `max(0, view_padding − view_insets)` per edge — see Data Flow's Inset delivery. |
| `GlassFill` / `GlassMaterial` / `GlassScale` | `frust-theme::glass`'s translucent-material tokens (spec 6f): `GlassFill` is one alpha-washed color; `GlassMaterial` pairs a `blur_radius_intent` (future-backend blur contract, `0.0` = opaque) with light/dark fill-wash stacks, a specular `hairline_alpha`, and a `ShadowSpec`; `GlassScale` bundles the `chrome`/`bar`/`control` tiers `Theme.glass` carries on both baselines — see Data Flow's Glass material tokens. |
| `FrameTime` / `Curve` / `Tween<T>` / `SpringDesc` / `AnimationController` | `frust-core::anim`'s animation vocabulary: `FrameTime` is an opaque shell-supplied clock reading (difference-only, never absolute); `Curve` is a named/cubic-Bézier easing function; `Tween<T>` interpolates a `Lerp` value type; `SpringDesc` parameterizes a damped spring; `AnimationController` drives a `0.0..=1.0` value by duration+curve or by a spring fling, advanced during paint via `PaintCtx::frame_time` — see Data Flow's Frame pipeline. |
| `InputEvent` / `PointerEvent` / `KeyEvent` / `ImeEvent` / `EventCtx` / `EventOutcome` | Layer 2 input (spec §9): `InputEvent` is a `PointerEvent`, a scroll delta, `Key(KeyEvent)`, or `Ime(ImeEvent)` — `Clone` but not `Copy` (`Key`/`Ime` carry owned `String` payloads) — entering at `RenderRoot::event`; `EventCtx` is the erased-state context a handler mutates (capture pointer, request/release focus, publish IME state, request redraw); `EventOutcome` is the pass's `handled`/`needs_redraw` summary — see Data Flow's Event pipeline. |
| `EditingState` / `ImeState` | The IME state-sync contract at the shell seam (see Data Flow's Event pipeline). `EditingState` is text plus selection/composing indices, UTF-16 code-unit indexed at this boundary (`frust-text`'s `TextEditor` owns byte conversion); a platform bridge pushes one in via `AppTree::ime_apply`/`Ime(ApplyEditingState)` and reads a reconciled one back via `RenderRoot::ime_state`/`AppTree::ime_state`, which returns the focused widget's published `ImeState` (`EditingState` + caret rect). |
| `SemanticsCtx` / `SemanticsUpdate` | `frust-core::semantics`'s pull-based accessibility pass (spec §9): `SemanticsCtx` collects `accesskit::Node`s during `RenderRoot::semantics`, assigning each a stable per-pod id; `SemanticsUpdate` is the resulting flat node map plus root/focus ids, pushed each frame by a per-shell platform adapter — see Data Flow's Semantics pass. |
| `RenderTier` / `TierCaps` / `TierSelection` / `TierOutcome` | `frust-render::tier`'s render-backend selection vocabulary: `RenderTier` is `Gpu` (default, vello 0.9) or `Cpu` (experimental `vello_cpu` fallback, only selectable behind the non-default `cpu-tier` feature); `select_render_tier` picks one from probed `TierCaps` plus an optional override (`FRUST_RENDER_TIER` / `frust run --render-tier`, which always wins), returning a `TierSelection` (`TierOutcome::Available`/`Unavailable` plus a diagnosis string) that `RenderContext` consults at device-init time. |
| `ChildPod` | A container's owned child: boxed widget + layout geometry + capture-active/focused bookkeeping — how `frust-widgets`' containers and interactive widgets own children without the arena (single-root-arena divergence; see Data Flow). |
| `AnyView<State>` | Type-erased `View` (element `Box<dyn Widget>`) used wherever children are heterogeneous (a container's child list); mirrors the xilem `AnyView` pattern. |
| `ChildKey` / `keyed` | Explicit child identity for a `Flex` child list (spec §6.3): `keyed(key, view)` attaches a `ChildKey` the reconciler matches old↔new children by, relocating a matched child's widget (preserving its internal state) across a reorder/insert/remove instead of rebuilding it. Keys are all-or-nothing and unique per list; a mixed or duplicate key set falls back to positional matching. A keyed reorder is a structural change like any other (see Data Flow's Event pipeline). |
| `Text` / `Button` / `Checkbox` / `Radio` / `Slider` / `TextInput` / `Image` / `Icon` / `Flex` (`Row`/`Column`) / `Stack` / `Padding` / `Align` / `SizedBox` / `ScrollView` / `GestureDetector` / `SafeArea` | `frust-widgets`' baseline vocabulary — each a `View`/`Widget` pair over the `AnyView`/`ChildPod` substrate; the interactive ones are controlled components (see `docs/CODE_STANDARDS.md`). `TextInput` owns a `TextEditor` and drives it from focus-routed `Key`/`Ime` events; `Image` wraps a decode-once `ImageSource` (an `Arc`-backed `peniko::ImageData` handle) and a fit mode (`ImageFit::Fill`/`Contain`/`Cover`), painted via the new `Command::Image` scene command; `Radio` reports a requested selection via `on_select` but never self-owns it, the same controlled-component contract as `Checkbox`; `Align` fills a bounded axis and shrink-wraps an unbounded one, per axis independently (Flutter `RenderPositionedBox`/`RenderAligningShiftedBox` parity); `SafeArea` (`.left`/`.top`/`.right`/`.bottom(bool)`, `.minimum(EdgeInsets)`) deflates by `WindowInsets::padding()` per enabled edge, floored by `.minimum` even on a disabled edge (Flutter parity — see Data Flow's Inset delivery). |
| `IconData` / `IconSource` | `Icon`'s vector-path source: `IconSource` is a generated `crate::icons` const (an SVG path `d` string plus its design-box size); `IconData::from_path` accepts a user-built `kurbo::BezPath` instead. Parsed to a `BezPath` and cached on the widget at build/rebuild. |
| `NavigatorController<State>` / `NavigatorView` / `PopResult` | `nav::navigator`'s app-facing handle for the retained page stack — an op queue (`push`/`pop`/`replace`) drained at rebuild, never self-mutating (see Data Flow's Navigation flow); `PopResult` is a type-erased pop payload a pusher's `on_result` callback receives. |
| `HeroFrames` / `HeroDirective` | The shared-element ("hero") transition vocabulary a navigator installs over `PaintCtx` during a page transition: a `hero(tag, child)` wrapper reports its bounds via `PaintCtx::report_hero`, getting back `HeroDirective::Normal` (no transition in flight), `Morph` (repaint under a rect→rect transform to the interpolated position, on the topmost page), or `Suppress` (skip painting, on the counterpart) — see Data Flow's Navigation flow. |
| `Route<State>` / `Router<State>` / `Resolution` | `nav::router`'s go_router-subset declarative layer over the navigator: `Route` pairs a path pattern with a page builder, optional redirect, and nested children; `Router::resolve` is pure location-matching (param capture, redirects under a loop guard) into a `Resolution`; `go`/`push`/`pop` drive the owned `NavigatorController`. |
| `PageTransition` / `TransitionSpec` / `TransitionDriver` | `nav::transition`'s page-transition vocabulary: named presets (M3 shared-axis/fade-through, `SlideUp`, iOS push/modal) plus the progress driver the navigator advances during paint (see Data Flow's Navigation flow). |
| `DeepLink` / `DeepLinks` / `deep_links()` | `frust-reactive::deep_link`'s process-wide deep-link source (facade: `frust::deep_links()`); `DeepLinks::latest` is a trackable signal, `initial` a set-once cold-start snapshot — see Data Flow's Deep-link flow. |
| `PaintScene` | Renderer-agnostic paint target widgets draw into; bridged onto `SceneBuilder`. `fill_path`/`stroke_path` and `push_transform`/`pop_transform` (default no-ops, overridden by `SceneBuilder`) paint arbitrary filled/stroked `kurbo::BezPath` shapes and push/pop an `Affine` onto the scene's transform stack, respectively. |
| `PaintCtx` / `PaintOutcome` | Paint-pass context and result: `PaintCtx::frame_time` is the shell-fed clock reading a widget advances animation state with; `PaintCtx::theme_as::<T>()` recovers the type-erased threaded theme (`None` if none was set — see Data Flow's Theme delivery); `PaintCtx::request_frame`/`needs_frame` let a widget ask to be re-invoked without external input; `PaintCtx::has_focus`, seeded from the pod's recorded focus path, lets a focused editable gate its focus chrome and IME republish on it, mirroring `EventCtx::has_focus`; `PaintOutcome::needs_frame` surfaces the former through `RenderRoot::paint`/`AppTree::paint` — see Data Flow's Frame pipeline. |
| `Scene` / `SceneBuilder` / `Command` | Layer 3 vector display list — the widget/GPU seam. `Command::Path` carries a `BezPath` plus a fill-or-stroke `PathStyle`. |
| `GlyphRun` | Shaped-glyph carrier from `frust-text` into the scene. |
| `TextContext` / `TextStyle` / `TextLayout` | Parley-backed text shaping surface. |
| `RenderContext` / `SurfaceRenderer` | `RenderContext` owns the wgpu `Instance` and lazily creates/holds the logical `wgpu::Device` itself (adapter-derived limits, not a thin `vello::util` wrapper) so it can request the real adapter limits vello's own device pool cannot; `ensure_device_headless` creates the device with no surface, the Android pre-init entry (see Data Flow's Android frame pipeline). `SurfaceRenderer` is the §8.1 surface lifecycle state machine (`SurfacePhase`/`FrameOutcome`) that owns surface creation and per-frame presentation, plus the persisted-pipeline-cache seam (`set_initial_pipeline_cache_data`/`pipeline_cache_data` — see Data Flow's GPU pipeline cache). Its `encode()`/`present()` split GPU encode from swapchain-acquire+present as separate public calls (`render()` retained as a combined wrapper), returning an `EncodeOutcome` (`Encoded`/`Skipped`) — the seam a render-thread split would time or move independently. |
| `FrameGate` / `FrameInputs` / `FrameDecision` | `frust-shell-common::frame_gate`'s whole-frame skip gate a mobile shell consults every tick: `FrameInputs` bundles the dirtiness signals, `FrameGate::decide` returns `Run`/`Skip`, gated by a `FRUST_NO_FRAME_GATE` kill switch — see Data Flow's Frame gate. |
| `FrameStats` / `StartupSpans` | `frust-shell-common::perf`'s instrumentation: `FrameStats` records a ring buffer + running counters of per-pass frame timings and emits a rate-limited summary; `StartupSpans` records named cold-start milestones and emits once on first frame presented. Both gated behind `perf::enabled()`/`FRUST_TRACE` — see `docs/DEVELOPMENT.md`. A second, independent gate (`FRUST_TRACE_RAW`, also requiring `FRUST_TRACE`) puts `FrameStats` into a raw-export mode emitting one parseable line per frame instead of periodic summaries, splitting `encode`/`present` timing into separate fields (v2 format); `mark_scenario_start`/`mark_scenario_end` stamp named scenario boundaries into the same stream for a benchmark harness to slice by — see `docs/DEVELOPMENT.md`. `SPAN_FIRST_ENCODE_DONE` marks first-encode on all three shells; `SPAN_PIPELINE_CACHE_RESTORED` marks a warm pipeline-cache hit but is wired on the desktop shell only (not yet on the mobile FFI init seams); each mobile shell also stamps `font_preinit_started`/`font_preinit_joined` around its background `TextContext` prewarm join, overlapped with GPU pre-init. |
| `android_app!` | Facade macro binding a generated app's `State`/`app_logic` to the fixed, sixteen-export Android JNI surface (init/frame/touch/resume/pause/destroy/surface-changed/surface-destroyed, the `nativeImeApply`/`nativeImeState`/`nativeImeAction` IME state-sync trio, `nativeSetAppearance`, `nativeOnDeepLink`, `nativeInitAccessibility`, `nativeOnInsetsChanged`, and `nativeOnBackPress`); the sole Android app entry point. `nativeInit` additionally carries a `cacheDir` string (pipeline-cache seam, see Data Flow); a process-wide `JNI_OnLoad` (native-library load, not macro-generated) starts the GPU pre-init thread `nativeInit` joins — see Data Flow's Android frame pipeline. A 2-arg (`State: Default`) and a 3-arg state-factory arm both funnel through `new_boxed_app_with`. |
| `IosAppHandle` / `ios_app!` | `IosAppHandle` (`frust-shell-ios`) is the opaque native handle behind the fourteen `frust_*` C exports (init/resize/render-frame/dispatch-touch/pause/resume/destroy, the `frust_ime_apply`/`frust_ime_state_json`/`frust_string_free` IME state-sync trio, `frust_set_appearance`, `frust_on_deep_link`, `frust_init_accessibility`, and `frust_set_insets`), mirroring `AndroidAppHandle` (retains the Swift-owned `CAMetalLayer` pointer, guaranteed to outlive the handle until `frust_destroy`, so a lost surface can be recreated; a `paused` flag gates frame submission). `ios_app!` is the facade macro binding a generated app's `State`/`app_logic` to those exports, mirroring `android_app!`'s 2-/3-arg arms; the sole iOS app entry point. |
| `app!` | The canonical facade entry macro (spec §5.5): binds a `Component + Default` root to all three platforms in one call — `android_app!`/`ios_app!` under the hood, plus a hidden desktop `__frust_main` calling `frust::run`. A generated `lib.rs` calls it once; `main.rs` calls the generated `__frust_main`; `frust::run(root)` is the same desktop path called directly, for apps with no mobile target. |
| `new_boxed_app_with` | `frust-shell-common`'s app-construction seam: builds an `AppTree` from a state *factory* closure (`FnOnce() -> State`) rather than a pre-built value, letting the entry macros bind `Component::init`; `new_boxed_app` (`State: Default`) is the convenience wrapper over it. |
| `BuildInfo` / `BuildArgs` | CLI build-mode funnel (debug/profile/release, flavor, defines, build name/number); drives both the `build` command (release-default) and `run`'s Android/iOS pipelines (debug-default). |
| `AndroidArtifact` / `IosArtifact` / `BuiltArtifacts` | Artifact-selection targets for the `build` command (Android APK with optional ABI splits, or an appbundle; iOS an `.app`, optionally unsigned, or an archived `.ipa` with an export method) and the resulting built-artifact path list `android_build`/`ios_build` hand back. |
| `ProcessRunner` | `frust-drive`'s seam for every external tool invocation: blocking (`run_streaming`) and cancellable (`spawn_streaming` → `StreamHandle`) streaming for long-running processes like `gradlew`/`logcat`, the latter's lines delivered through a bounded ring-buffered `LineReceiver` (drop-oldest, `dropped_lines()` counted, stderr merged in); fakeable in tests. Each `spawn_streaming` child is spawned into its own process group on Unix (`StreamHandle::kill` group-kills the whole tree, e.g. `cargo run` plus the preview binary it forks; Windows stays direct-child-only, tracked fast-follow). |
| `Validator` / `DeviceDiscovery` | Pluggable `doctor`/`devices` checks, each independent and non-fatal on failure. |
| `TemplateContext` | Render/path substitution variables for `frust create`'s scaffold. |
