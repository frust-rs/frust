# Lab 9 — Native Controls & the Platform-View Pipeline

**Concept:** Every lab so far ended at pixels *frust* drew. This one ends at
pixels the **OS** drew. `frust-native-widgets` renders six real platform
controls — `Button`/`Label`/`Switch`/`Slider`/`ProgressBar`/`Image`, a genuine
`android.widget.Switch`, a genuine `UISwitch` — driven entirely from Rust:
positioned by frust's layout, themed by frust's `Theme`, reporting taps into
frust's signals, never entering the engine's scene compiler or `RenderRoot::event`. This
lab follows one control from a builder call to a tapped native view, then asks
how React Native and Flutter answer the same question (very differently, and
both answers cost more).

Half of it runs on your laptop — runtime, differ and theme fold are host-tested
with zero JNI/ObjC. The *visual* half needs a device (see `docs/DEVELOPMENT.md`
for the Simulator/emulator caveats). Anchors below are symbols, not line
numbers: labs 1–8's line anchors drift, names don't.

## Where it lives

| Thing | File | Symbols |
|---|---|---|
| The six builders + the factory name they publish | `plugins/native-widgets/src/api/builders.rs` | `native_button` … `native_image`, `VIEW_TYPE`, `next_local_slot`, `ParamsBody`, `placeholder` |
| Identity encode — and the iOS registration trigger | `plugins/native-widgets/src/runtime.rs` | `with_identity`, `ensure_platform_factory`, `CONTROL_KEY`, `SLOT_KEY`, `Params` |
| The type-erased lifecycle engine | same | `NativeRuntime::{create, update_params, on_event, set_callback, take_matching}`, `with_runtime` |
| Event vocabulary + bit-packed `detail` codec | `plugins/native-widgets/src/events.rs` | `EventPayload`, `EVENT_KIND_*`, `pack_value_changed`/`unpack_value_changed` |
| Events-as-signals wrappers | `plugins/native-widgets/src/api/signals.rs` | `on_click`, `on_toggled`, `on_value_changed`, `EventCallback` |
| The iOS factory class (zero Swift) | `plugins/native-widgets/src/apple/factory.rs` | `FACTORY_CLASS_NAME`, `ensure_registered`, `dead_slot_view` |
| Theme fold L2 (platform-neutral, host-tested) | `plugins/native-widgets/src/api/theme.rs` | `resolve`, `ResolvedTheme` |
| Theme L1, per platform | `plugins/native-widgets/src/android/theme.rs` · `apple/theme.rs` | `night_qualified_context` · `apply_user_interface_style` |
| The differ (paint frames → commands) | `crates/frust-shell-common/src/platform_view.rs` | `PlatformViewState::ingest`, `ViewCommand`, `EPSILON_PX`, `HIDE_AFTER_MISSING_FRAMES`, `DISPOSE_AFTER_MISSING_FRAMES`, `FramePairing` |
| Declared vs resolved surface mode | `crates/frust-shell-common/src/surface_mode.rs` | `declare_host_translucent_surface`, `resolved_surface_mode`, `ResolvedSurfaceMode` |
| Whether a surface actually resolved translucent | `crates/frust-gpu/src/surface.rs` | `resolve_alpha_mode`, `ConfiguredSurface::resolved_translucent` |
| The hole punch | `crates/frust-widgets/src/platform_view.rs` → `crates/frust-engine/src/compile/clear.rs` → `crates/frust-engine/src/schedule/mod.rs` | `PlatformViewWidget::paint`, `clear::punch_rect`, `schedule::cut_at`, `shield` |
| Host-side factory resolution | `platform/android/frust-embedding/…/FrustViewHost.kt` · `platform/ios/FrustEmbedding/…/FrustViewHost.swift` | `resolveFactory`, `interactiveTargetAt` · `interactiveSlotContains` |
| A plugin author's own native subtree | `plugins/native-widgets/src/component.rs` | `NativeComponent`, `ComponentCtx`, `register_component`, `native_component` |
| The device-gate vehicle | `examples/playground/src/pages/native_widgets.rs` | `page`, `theme_toggle_demo`, `gate_harness_block` |

## The pipeline, in seven stages

### 1. Builder → tree

`native_button("Save").on_press(…)` returns a plain `Clone`-able data struct
implementing `frust-core`'s `Component`, whose `Component::State` is a bare
`SlotId` — a `u64` from `next_local_slot()`, this plugin's **own** atomic
counter, deliberately *not* `frust_core::widget::next_slot_id()`. The builder
value is rebuilt fresh every frame; the slot id is retained, so the same id
round-trips through every create/update the platform ever sees for that widget
instance.

Props ride to the platform as **hand-rolled flat JSON** (`ParamsBody` writes
the body, `with_identity` prefixes it) carrying two reserved keys:
`__frustControl` (`CONTROL_KEY`) names the registered control kind, and
`__frustSlot` (`SLOT_KEY`) carries the slot id — the factory contract has no
parameter for it. No `serde` at the boundary: that's the repo-wide mobile-FFI
rule (`docs/CODE_STANDARDS.md`'s Language Idioms), and it keeps the wire
independent of any Rust-side type change. Six builders, six `platform_view`
slots — "N controls = N slots".

### 2. Slot → commands (the differ)

Each control composes `frust-widgets`' `platform_view(VIEW_TYPE)` slot, which
publishes a `PlatformViewFrame` (rect, clip, visible, params generation) every
paint pass. `PlatformViewState::ingest` turns that per-pass snapshot into a
**generation-stamped, idempotent** `ViewCommand` backlog:

- new slot ⇒ `Create` **then** `Update`, in that order, in one batch;
- rect/clip/shield change past `EPSILON_PX` (0.5 logical px) ⇒ `Update`;
  smaller ⇒ nothing at all, so sub-pixel scroll jitter is free;
- `params_generation` bump ⇒ `UpdateParams`, independent of geometry;
- missing for `HIDE_AFTER_MISSING_FRAMES` (2) ingests ⇒ a Hide;
- missing for `DISPOSE_AFTER_MISSING_FRAMES` (**30**) ⇒ `Dispose`.

That 30-frame streak is a *backstop*, not the main path: a torn-down widget
reports itself through `frust-core`'s retire list and `PlatformViewState::retire`
disposes immediately. The streak covers what teardown can't see — and a merely
scrolled-offscreen slot never reports a retire, which is what keeps a culled
camera preview alive. The backlog itself is dropped only on `acknowledge`, so a
host that misses a poll (surface recreation) re-polls and replays; `FramePairing`
+ `commands_up_to` then release only the prefix whose frust frame is already
presented, or a hosted view scrolls visibly ahead of the content it's pinned to.

### 3. FFI → host (finding the factory)

`VIEW_TYPE` is a compile-time constant per platform:
`dev.frust.nativewidgets.FrustNativeControlFactory` on Android (a Kotlin class
in the plugin's own Gradle module), the bare ObjC runtime name
`FrustNativeControlFactory` on iOS — a Rust `define_class!` class, **zero
Swift**. Once shipped, both strings are frozen.

Both hosts resolve it defensively, and the **order** is the contract:

- **Android** `FrustViewHost.resolveFactory` checks (1) the `dev.frust.`
  package prefix, (2) `FrustPlatformViewFactory::class.java.isAssignableFrom`
  on the loaded *class object* — both **before**
  `getDeclaredConstructor().newInstance()` ever runs. Instantiating an
  attacker-influenced class name first would run a no-arg constructor's side
  effects unconditionally.
- **iOS** does the same shape with different primitives: `NSClassFromString`
  resolves the name, `class_conformsToProtocol` checks the protocol, and only
  then `cls.init()`.

Failure on either side is a **dead slot**, never a crash: Android throws into
`applyCreate`'s `catch (Throwable)`; iOS can't throw, so every failing path of
`createViewWithParamsJson:` returns an empty `dead_slot_view()`.

The iOS arm has one extra trap. objc2 registers a `define_class!` class
**lazily**, on the first Rust `ClassType::class()` call, and nothing on the
Swift side can trigger it — the host only asks by name, and `NSClassFromString`
returns nil for an unregistered class. So `ensure_platform_factory()` runs from
`with_identity`, on the same rebuild that publishes the slot, a whole frame
before the host's post-frame poll looks the class up. Get that ordering wrong
and every iOS control is silently invisible.

### 4. Lifecycle: create, update, dispose

`NativeRuntime` is a `thread_local!` (`with_runtime`), not a `Mutex` global: a
control's state may hold main-thread-only handles (iOS's `Retained<UIView>` is
`!Send`), and every entry point already runs on the platform main thread.
`with_runtime` is also re-entrancy *tolerant* — Android's `setChecked` fires
its own listener synchronously, so a re-entrant call reports `None` and is
dropped with a warning rather than panicking on the borrow. Three properties
worth internalizing:

1. **The props diff gate is Rust-side.** `update_params` decodes the new params
   into the control's typed `Props` and compares them (`props_eq`, the vtable's
   `PartialEq`) *before* any platform call. An unchanged rebuild costs one
   decode plus one comparison and **zero** FFI crossings. Field-level diffing
   inside a changed struct is the control's own job — only it knows which setter
   is ~0.8 µs (invalidate-only) and which ~29 µs (the `setText` re-layout class).
2. **Dispose resolves by native-view identity, not slot id.** Neither platform
   hands its dispose entry point a slot id, so `take_matching` scans for the
   view object. A late dispose arriving after a replacement `Create` reused the
   id finds nothing and is a silent no-op — it can never delete the live
   control.
3. **A callback can be registered before its instance exists.** The api layer
   calls `set_callback` during the rebuild that mounts the slot; `create`
   happens on the *next* post-frame poll. A registration with no live instance
   parks in `pending_callbacks` and is taken by `create`, never dropped —
   nothing schedules a retry frame on an idle screen, so dropping it would
   leave a visible, tappable, permanently dead control.

### 5. Compositing: Mode A vs Mode B

Frust never captures the native view into a texture. The OS composites it as a
**sibling** of the GPU surface, in one of exactly two arrangements, chosen once
at surface creation by the generated host glue (`FRUST_TRANSLUCENT_SURFACE` /
`translucentSurface`):

- **Mode A** — opaque GPU surface, native siblings inserted *above* it.
  `PlatformViewWidget::paint` deliberately paints **nothing** in the slot: the
  native view covers the region anyway, so punching would only risk erasing
  real content. Input hits the native view at the OS level; frust never sees it.
- **Mode B** — translucent surface, native siblings *below* it. Now frust owns
  every touch, and the slot must **actively** clear its rect
  (`PaintScene::clear_rect`, gated on `PaintCtx::is_translucent`) so the hole
  survives an opaque app backdrop painted underneath. The engine hoists that
  `ClearRect` to the frame root and lowers it to destination-out coverage —
  `dst' = dst·(1−src.a)` — issued through the dedicated `StripDestOut`/
  `StripDepthDestOut` GPU pipelines and their `DEST_OUT_BLEND` blend state
  (`crates/frust-engine/src/gpu/pipelines.rs`), weighted by the punch's own
  antialiased coverage so a pixel-aligned edge lands pixel-exact. Deliberately
  *not* a whole-tile clear op: a tile-granularity clear bleeds the punch past a
  tile-unaligned rectangle edge, which destination-out's per-pixel weighting
  avoids (`crates/frust-engine/src/compile/clear.rs`'s module docs spell out
  the hoisting and the reasoning).

Mode B also arbitrates input. An `interactive()` slot (`Button`, `Switch`,
`Slider`) hands a touch-DOWN inside its rect to the native sibling — *except*
inside a **z-shield**, a rect reported by frust chrome painted over the slot
(`frust-widgets`' `shield` wrapper, auto-collected each paint and intersected
against each slot by the differ). Display-only slots (`Label`, `ProgressBar`,
`Image`) never set `interactive`, ship an empty shield list, and never take a
touch at all.

**And Mode B can be refused.** The `frust-engine` render path itself never refuses a translucent
surface — every backend/alpha-mode pair renders straight into the acquired swapchain or, on a
straight-alpha compositor, through one un-premultiply pass on the way out
(`choose_engine_render_path`, [RENDER_ARCHITECTURE.md](../RENDER_ARCHITECTURE.md)'s Data Flow).
What can still refuse is the *platform*: when its own compositor offers no translucent alpha mode
for the surface frust requested, the surface resolves opaque regardless of what the engine could
have drawn — `ResolvedSurfaceMode::RefusedTranslucent` from `resolved_surface_mode()`
(`crates/frust-shell-common/src/surface_mode.rs`). Every builder consults it and renders a
labelled frust-drawn `placeholder` rather than an invisible, untappable slot; the refusal is
logged **once**, crate-wide.

### 6. Events-as-signals

The platform listener (Android's one `FrustNativeListener`, iOS's target-action)
fires on the platform main thread — the same thread the rest of frust runs on.
`NativeRuntime::on_event` routes by slot id, the control decodes a raw
`(kind, detail)` pair into a typed `EventPayload`, `api::signals`' wrapper
unwraps that into the plain closure shape the builder took, and the closure
writes an `RwSignal`. One write wakes exactly one frame. `RenderRoot::event` is
**bypassed entirely, by design**: no pointer capture, no focus, none of frust's
fire-on-up-inside press semantics — a native control's interaction is
platform-owned end to end.

The wire is primitive, not JSON: `detail` is a `jlong` with the value in the
low 32 bits and `fromUser` in bit 32 (`pack_value_changed`). A slider listener
fires at drag rate, and allocating a JSON string per event on the main thread
is what the no-JSON-on-the-hot-path rule exists to prevent. The `EVENT_KIND_*`
constants are pinned against their Kotlin twins by
`plugins/native-widgets/tests/kotlin_conformance.rs` — drift fails the build.

### 7. The theme ladder

`api::theme::resolve` folds the ambient `Theme` into packed primitives (`u32`
ARGB, `f32` dp/sp) written into `params_json` every rebuild — so stage 4's diff
gate makes an unchanged theme cost zero FFI. The fold is **selective per
control**:

| Token | Folded into |
|---|---|
| `dark` | all six controls |
| corner radius | `Button` only |
| text size | `Button`, `Label` |
| typeface (only under the Glyph design language) | `Button`, `Label`, `Switch` |
| accent tints | `Button`, `Switch`, `Slider`, `ProgressBar` |
| explicit background | `Label`, `ProgressBar` only — an explicit fill would replace `Switch`/`Slider`'s ripple drawable |

`Image` folds `dark` and nothing else: tinting an app-supplied photo would
corrupt its content.

Now the asymmetry that is *not* a bug. L1 — the platform's own chrome (ripple
colour, thumb/track resting colour) — bakes at construction on Android:
`night_qualified_context` builds a night-qualified `Context`, and a `Context`
is consumed once, at `new Button(context)`. A live brightness flip re-themes
L2's explicit setters but leaves L1 chrome pinned to whichever brightness the
control was born under. iOS has no such constraint —
`overrideUserInterfaceStyle` is a plain mutable `UIView` property, so
`apply_user_interface_style` runs from **both** create and update and iOS
re-themes live. `docs/CODE_STANDARDS.md` states the rule: *a platform
capability gap is recorded per-platform, never "corrected" onto the platform
that doesn't have it.* Mirroring one platform's constraint onto the other
already caused a real device defect — a recreated `UISwitch` pinned to a stale
brightness among its peers.

## How the other two frameworks solve this

External facts below were **retrieved 2026-08-06** — treat versions and
defaults as of that date.

### React Native (Fabric)

RN inverts the question: native views **are** the primitives. Fabric (the
default renderer since 0.76, October 2024) runs three phases — *Render* (JS
builds an immutable C++ shadow tree), *Commit* (layout on a background thread),
*Mount* (diff the trees, then apply native mutations **synchronously** on the
UI thread). JSI's synchronous JS↔C++ access replaced the old asynchronous JSON
bridge, so a mount is no longer a serialized message hop. It still isn't 1:1
with the host tree, though: **View Flattening** merges layout-only `<View>`s so
no host view is created for them, and nested `<Text>` collapses into a single
`NSAttributedString`/`SpannableString`. RN pays frust's problem in reverse —
the whole tree is native views, and the optimization work is *avoiding*
creating them.

### Flutter (PlatformViews)

Flutter is the near-neighbour: widgets self-draw (Skia/Impeller), and native
views arrive only through a PlatformView — the awkward case. Android alone has
**four** modes:

- **Virtual Display** — deprecated; renders the view to a texture, which broke
  touch and accessibility;
- **Hybrid Composition** — a real view in the hierarchy, but pre-Android-10 it
  copied memory GPU↔main every frame, and it merges the raster thread onto the
  platform thread;
- **TLHC** (Texture Layer Hybrid Composition, SDK 23+) — a `FrameLayout` whose
  drawing is redirected into a Flutter `Texture`. `SurfaceView`s bypass the
  redirect and draw *over* Flutter, with automatic fallback only when the
  `SurfaceView` already exists at platform-view creation time;
- **Hybrid Composition++** — opt-in in Flutter 3.44, API 34+ with
  Vulkan/Impeller, giving each view its own `Surface` composited by
  SurfaceFlinger.

iOS is always hybrid: a real `UIView`, a hole punched in the Flutter surface,
and a *mutator stack* replaying Flutter's clips/transforms onto `CALayer`s.

### Where Frust sits

Frust self-draws everything like Flutter — but hosts native controls as OS
**sibling views**, over the GPU surface (Mode A) or under it with a
`DestOut`-punched hole (Mode B). No per-frame texture capture, no thread
merging; the costs land elsewhere instead — surface-mode constraints
(`ResolvedSurfaceMode::RefusedTranslucent`, when the platform's own compositor
offers no translucent mode), explicit input arbitration (shields), and stage 3's
one-frame create-timing dance.

| | React Native (Fabric) | Flutter | Frust |
|---|---|---|---|
| Rendering model | Native views are the primitives | Self-drawn (Skia/Impeller) | Self-drawn (frust-engine/wgpu) |
| Native-widget mechanism | The whole tree; synchronous UI-thread mount from a C++ shadow tree | PlatformView: texture capture, hybrid composition, or a per-view Surface | OS sibling view over/under the GPU surface; Rust-side differ emits create/update/dispose |
| Known costs | Tree isn't 1:1 (view flattening, text collapsing); JSI/UI-thread coupling | Mode zoo, per-frame copies or thread merging, `SurfaceView` z-order surprises | Mode B needs a translucent surface (refusable); input arbitration is explicit; lazy iOS class registration must be forced a frame early |

## Experiments

### 9.1 — Watch the props diff gate, with no device

```bash
cargo test -p frust-native-widgets --lib
```

Read two of those with the source open:
`runtime::tests::unchanged_props_never_reach_the_platform` and
`changed_props_apply_once_and_rebase_the_gate`. The test `NativeCtx` records
every platform call into a `Vec<String>`, so the gate's whole guarantee is an
assertion on an empty vector — no emulator, no JNI. Now break it: delete the
`props_eq` early return in `NativeRuntime::update_params` so every update
applies unconditionally, re-run, and watch those assertions fail. That vector is
what a real device would spend an FFI crossing per control per frame on.

### 9.2 — Bend the differ's two constants

```bash
cargo test -p frust-shell-common platform_view
```

46 tests, all pure logic. Set `EPSILON_PX` to `0.0` and watch
`sub_epsilon_rect_change_emits_nothing` fail — that's every sub-pixel scroll
frame becoming a native-side `Update`. Set `DISPOSE_AFTER_MISSING_FRAMES` to `2`
and read `a_merely_culled_slot_is_never_disposed_by_the_retire_path`: you just
made a scrolled-offscreen camera preview tear down and restart. Revert both.

### 9.3 — Mode A vs Mode B, on a device

[The Native Widgets section now lives in `examples/playground` only —
`examples/glyph-catalog` was stripped to a Glyph-theme-only showcase after this
lab was written. Substitute `examples/playground`, package `it.f0x.playground`,
and `MainActivity.kt`/`SceneDelegate.swift` under that app's tree when running
the steps below.]

`examples/glyph-catalog` is the vehicle — a standalone workspace, run from its
own directory, and it ships in **Mode B**:

```bash
cd examples/glyph-catalog
frust run -d <device-id>          # → the "Native Widgets" section
```

Six real controls sit beside their frust-drawn twins, same size, same `Theme`,
same signals. Now flip the mode: `translucentSurface` is `true` in
`android/app/src/main/kotlin/it/f0x/glyphcatalog/MainActivity.kt` (iOS:
`ios/Runner/SceneDelegate.swift`). Set it `false`, rebuild, and watch the
z-order invert — natives composite *above* the frust surface, the slot stops
emitting `clear_rect` entirely (Mode A paints nothing), and any frust chrome
you drew over a control vanishes behind it. While you're there, read
`gate_harness_block`'s live `live_slot_count()`: six at rest, **seven** with
the composite `NativeComponent` toggled on — one slot for a whole native card,
however many native children it owns.

**Desktop note:** platform views are mobile-only. On the desktop preview
(`cargo run` in the same directory) there is no host and no factory: the slot
lays out and publishes a `PlatformViewFrame` every paint that nobody consumes,
so every native control renders *nothing*. `platform_view(…)`'s `debug_fill()`
(debug builds only) paints translucent magenta so you can see where it went.

### 9.4 — (Historical) Forcing the translucency refusal on demand

This experiment used to force the deleted `FRUST_NO_DIRECT_SURFACE` render-path knob to make an
Android surface resolve non-translucent on a capable platform, watch every native control swap to
the frust-drawn `placeholder` banner, and confirm logcat carried exactly one `translucency
refused` warning. That knob and the render path it forced are gone (Phase 8) — the
`frust-engine` render path never itself refuses a translucent surface (stage 5 above). Refusal now
depends only on a real platform fact — whether the compositor advertises a translucent alpha mode
for the surface — which is not something to force on demand from a build flag; on a Pixel/iPhone
that supports translucency you will not see `ResolvedSurfaceMode::RefusedTranslucent` fire at all.
The `placeholder` fallback itself is still live and worth reading
(`plugins/native-widgets/src/api/builders.rs`'s `placeholder()`) — it is just no longer something
this lab can reproduce on a normal device.

**The other failure mode**, if you want to watch the trust check: change
Android's `VIEW_TYPE` in `api/builders.rs` to a name outside the `dev.frust.`
prefix (`com.example.Factory`), rebuild, and read logcat — `resolveFactory`
rejects it at step (1), before any class is loaded and long before any
constructor runs, and the slots go dead instead of crashing the frame loop.
Revert; that constant is frozen for a reason.

### 9.5 — Feel the theme ladder's platform asymmetry

On the same page, use `theme_toggle_demo`'s local light/dark row. On **both**
platforms the L2 folds re-theme live through the ordinary `UpdateParams` path —
text colour, accent tints, corner radius, background — no remount, no flicker
of a fresh view.

Now watch platform-owned chrome: press-and-hold a native `Switch` or drag the
`Slider` either side of a flip. On **iOS** the ripple/thumb resting colours
follow, because `apply_user_interface_style` re-pins on every update. On
**Android** they stay pinned to the brightness the control was *created* under
— flip the theme, scroll it off screen and back (forcing a recreate through the
differ's dispose/create path), and it snaps to the new brightness. That's L1's
`Context` consumed once at construction: documented behaviour, not a bug.

## What to notice before moving on

- The stage-4 diff gate and the stage-2 epsilon are one idea at two layers:
  *never cross a boundary you don't have to*. Together they're why a native
  control costs nothing on an idle frame — which is what lets Mode B keep its
  zero-frames-at-rest property while hosting native siblings.
- Lab 7's frame gate sits upstream of all of it: the shell calls `ingest` only
  on a `Run`, so a `Skip` produces no commands by construction — and an
  unmounted control's missing streak only advances on frames that actually ran.
- The `NativeComponent` seam (`component.rs`) lets a **plugin author** mount a
  whole native subtree through this same runtime as one slot. An app crate
  can't implement it — that means naming raw `jni`/`objc2-ui-kit` types the
  plugin doesn't re-export — so app code stays on the six builders, the boundary
  [NATIVE_WIDGETS_ARCHITECTURE.md](../NATIVE_WIDGETS_ARCHITECTURE.md) charters.
- The comparison cuts both ways: RN's flattening and Flutter's four Android
  modes exist because *their* default made the other case hard. Frust's sibling
  model makes the common case cheap and pushes the hard part into surface
  configuration — which is why `resolved_surface_mode()` is app-facing at all.
