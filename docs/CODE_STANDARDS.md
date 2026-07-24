# Frust - Code Standards

## Language Idioms

- **`unsafe` is confined to a small set of sanctioned platform-FFI boundaries.** Every other
  crate (`frust-core`, `frust-scene`, `frust-text`, `frust-widgets`, `frust-shell-common`,
  `frust-shell-desktop`, `frust`) stays `unsafe`-free — where Masonry/xilem-style code would
  reach for `unsafe` downcasting, use trait upcasting instead: bound a trait on `Any` (e.g.
  `Widget: Any`) and downcast through `&mut dyn Any`. The sanctioned zones are raw-pointer
  boundaries a GPU/platform shell cannot avoid, each isolated in one function/module with a
  `# Safety` doc comment stating the caller contract:
  - `frust-render`'s `create_android_surface`/`create_metal_surface`
    (`lifecycle.rs`) and `on_surface_created_from_android_window`/
    `on_surface_created_from_metal_layer` (`renderer.rs`) — turn a
    caller-owned raw `ANativeWindow*`/`CAMetalLayer*` into a `wgpu::Surface`.
  - `frust-shell-android`'s `jni_glue` module — the JNI FFI boundary
    (`extern "system"` exports, `Box::into_raw`/`from_raw`,
    `ANativeWindow_fromSurface`, `nativeInitPlatform`'s `JavaVM` stash),
    `android_app!`'s generated exports, and the render-thread split's
    `unsafe impl Send` for `SendableWindowPtr` plus a bare
    `libc::setpriority` self-boost.
  - `frust-shell-ios`'s `ffi_glue` module — the C-ABI FFI boundary
    (`extern "C"` exports, `Box::into_raw`/`from_raw`, the call into
    `on_surface_created_from_metal_layer`), `ios_app!`'s generated exports,
    and the split's `unsafe impl Send` for `SendableMetalLayer` plus a bare
    `libc::pthread_set_qos_class_self_np` self-boost.
  - `frust-plugin`'s `android` module — reconstructs the raw
    `JavaVM`/`jobject` plugins need from `ndk-context`-stored handles, one
    sanctioned module, scoped `AttachGuard` per call.
  - `frust-shared-preferences`'s `apple` backend — two `setObject:forKey:`
    calls (`objc2` marks the untyped Foundation setter unsafe), each
    `# Safety`-noted.
  - `frust-secure-storage`'s `apple` backend — a confined `as_cf`
    objc→`CFType` bridge for the Keychain/`SecAccessControl` calls,
    `# Safety`-noted.
  - `frust-render`'s `RenderContext::create_pipeline_cache` — one call
    building a `wgpu::PipelineCache` from a shell-persisted,
    adapter-fingerprint-validated blob (see `docs/ARCHITECTURE.md`'s GPU
    pipeline cache).
  - `frust-drive`'s `process` module — a `kill(2)` FFI shim (std exposes no
    `killpg`) that group-kills a streamed child's Unix process group.
- **No unwind across FFI.** Every platform export routes through
  `frust-shell-common`'s `guard` helper (`catch_unwind` + log, returning a
  benign default) rather than unwinding into JVM-/Swift-owned stack frames
  — a panic crossing the FFI boundary is undefined behavior, not a bug.
  `run_guarded_thread` is `guard`'s whole-thread-body counterpart: every
  render-thread `spawn` closure routes through it, so a caught panic exits
  cleanly and drains any orphaned `Ack` instead of poisoning shared state.
- **State-sync, not op-forwarding, across a mobile IME bridge.** Android/iOS
  platform text input doesn't send individual keystrokes across the FFI
  boundary — the platform owns composition (Gboard, CJK marked text)
  against a local mirror, then hands the framework a whole reconciled
  `EditingState` and reads one back to keep its own IME machinery
  synchronized — never add a per-keystroke op-forwarding path.
- **UTF-16 at the FFI seam, bytes inside.** `EditingState`'s
  `selection_*`/`composing_*` indices are UTF-16 code-unit indexed
  everywhere they cross a shell boundary; convert to/from byte offsets only
  inside `frust-text`'s `TextEditor` — a shell/glue module passes indices
  through opaquely, never converting them.
- **Hand-roll JSON at the mobile FFI boundary — no `serde` in shell crates.**
  `frust-shell-android`/`frust-shell-ios` serialize `ImeState` with a small
  hand-written escaper/builder for a handful of fixed fields.
- **Type-erase to avoid a downstream crate dependency**, not to avoid
  writing a type. When a lower layer threads a resource owned by a higher
  layer, pass it as `&mut dyn Any` and recover it at the one call site that
  knows the concrete type via a documented, panic-on-mismatch
  `downcast_mut::<T>()` (the panic should say wiring bug, not runtime-data
  condition).
- **Edition-2024 `-> impl Trait` return types capture all in-scope
  lifetimes by default.** When a function returns an `impl Trait` that
  borrows nothing from its parameters (e.g. `app_logic(&mut State) -> impl
  View<State>`, where views are `'static`), opt out explicitly:

  ```rust
  fn app_logic(state: &mut AppState) -> impl frust::View<AppState> + use<> {
      frust::text(state.greeting.clone()).size(32.0)
  }
  ```

- **A platform value with no published spec is a named constant with a
  source comment, not a bare literal.** Where a platform's exact behavior is
  private (an iOS gesture's edge zone, commit threshold, or fling velocity;
  an animation's duration/curve), name the constant, and its doc comment
  states whether it's a published value or **community-approximate** (a
  reverse-engineered/community-converged estimate) plus what it's
  approximating:

  ```rust
  /// Left-edge activation zone width for the interactive pop-swipe (logical px).
  ///
  /// **Community-approximate**: UIKit's edge zone isn't published; ~20dp is
  /// where community reimplementations converge.
  const EDGE_SWIPE_ZONE_DP: f64 = 20.0;
  ```
  This keeps the tunable visible in one place and tells a future reader
  whether "fixing" it means matching a spec or adjusting a guess — grep
  for `Community-approximate` to find every instance.

## Error Handling

- **`anyhow::Result` in binaries and integration-facing library code**
  (`frust-cli` commands, `frust-render`'s public API,
  `frust-shell-desktop`/`frust`'s `run`), with `.context()` /
  `.with_context()` at each fallible step so the error chain names what was
  being attempted (e.g. `"failed to spawn `{cmd}`"`, `"reading `{path}`"`).
- **`thiserror`-derived enums for errors callers match on**, i.e. where the
  error is part of a library's API contract rather than a leaf failure
  report:

  ```rust
  #[derive(thiserror::Error, Debug, PartialEq, Eq)]
  pub enum BuildInfoError {
      #[error("--debug, --profile, and --release are mutually exclusive")]
      ConflictingModes,
      #[error("invalid --define '{0}': expected KEY=VALUE with a non-empty key")]
      InvalidDefine(String),
  }
  ```

## Naming Conventions

| Element | Convention | Example |
|---|---|---|
| Crate names | `frust-<layer>` | `frust-scene`, `frust-shell-desktop` |
| Fallible constructor errors | `<Type>Error` enum, `thiserror`-derived | `BuildInfoError`, `NameError` |
| Test-only fakes | `Fake<Trait>` | `FakeProcessRunner`, `FakeEnv` |
| Widget pairs | `<Name>View` (declarative) / `<Name>Widget` (retained) | `TextView` / `TextWidget` |
| JNI exports | `Java_<fixed_package>_<FixedClass>_native<Name>` | `Java_dev_frust_FrustSurfaceView_nativeOnFrame` |

JNI export names are LAW: the package/class (`dev.frust.FrustSurfaceView`)
is fixed across every generated app, so the mangled symbol stays stable
regardless of the app's own package.

## Plugin Conventions

Both plugin tiers under `plugins/` (see `docs/ARCHITECTURE.md`'s Module
Structure) follow these conventions:

- **Backends are `#[cfg]`-gated modules** (`apple`/`android`/`file`) behind
  one platform-independent public API; FFI deps are target-gated in the
  crate's `Cargo.toml`, never unconditional.
- **Errors are `thiserror` enums callers match on** (`PrefsError`,
  `PlatformHandleError`), per the Error Handling rule above.
- **A JNI attach is scoped per call, never permanent** — threads don't
  auto-detach on exit, so `attach_permanently` leaks the attachment; use
  `frust-plugin::android::with_jni_env`'s scoped `AttachGuard`.
- **No panics/unwinds near an FFI boundary**, the same rule as shell exports (Language Idioms, above).
- **Platform plugins never depend on `frust-*` framework crates**
  (`frust-plugin` + `frust-paths` + FFI crates only); **facade plugins depend
  on `frust` alone**. A plugin needing both splits into a platform-core plus
  facade-glue crate.
- **A store shared with the OS namespaces its keys `frust.`** (NSUserDefaults,
  Android SharedPreferences) so plugin keys can't collide with other libraries'.
- **An Android plugin's app-side Kotlin lives under `dev.frust`, beside
  the generated `FrustSurfaceView.kt`.** The package/class name is a hard
  JNI lookup contract; ship the canonical file for a caller to copy in
  (`plugins/<name>/platform/`), never render it from a template.
- **A generated-project mutation is idempotent, never a blind overwrite.**
  Adding an OS-side contribution (dependency, manifest permission, Kotlin
  file, plist key) checks first and no-ops if already present — the
  contract `frust-drive::plugin::add_plugin` implements (see `docs/ARCHITECTURE.md`'s Plugin flow).

## TUI Conventions

- **`ui` renders `&AppState`, never mutates the engine** — all state
  changes happen in `engine::update`; a render fn taking `&mut` state is a
  layering violation.
- **Every mouse action has keyboard parity** (e.g. the welcome Create
  button: `Enter`/`c`) — mouse support is additive, never the sole path.
- **Commands/keybindings have one registry**, `engine::palette::commands`
  — the palette and help overlay both render from it; never a parallel list.
- **fdemon is a pattern source, not a copy source** — it is BSL-1.1
  licensed; study its patterns but never copy a file verbatim.

## Anti-patterns

### Shelling out directly instead of through `ProcessRunner`

**BAD:**
```rust
let out = std::process::Command::new("rustc").arg("--version").output()?;
```
Untestable without invoking `rustc`; every caller re-implements spawn-failure handling.

**GOOD:**
```rust
fn validate(&self, ctx: &DoctorCtx) -> Validation {
    match ctx.runner.run("rustc", &["--version"]) { /* ... */ }
}
```
Every external tool invocation (`rustc`, `adb`, `xcrun`, `cargo ndk`, …) goes
through `ProcessRunner`, so `doctor`/`devices` logic tests via `FakeProcessRunner`.

### Leaking `vello`/`wgpu` types outside `frust-render`

**BAD:** a `frust-scene` or `frust-text` public function taking or
returning a `vello::*`/`wgpu::*` type.

**GOOD:** public APIs above `frust-render` speak only `kurbo`/`peniko`;
this is what lets the GPU backend be swapped later without touching widget
or text code.

**The one named exception:** `frust-render::DetachedSurface` is a
deliberate **opaque** escape valve, not a leak — its only accessor,
`into_surface`, is crate-private, so the wrapped `wgpu::Surface` is never
nameable outside `frust-render`; follow the same pattern for any future
value crossing this boundary.

### Printing directly from a `frust-drive` build/run core

**BAD:** a `println!`/`print!` inside `android_build`/`ios_build`/
`android_run`/`ios_run` — reaches the caller's stdout unconditionally,
garbling a TUI session's raw-mode terminal with raw pipeline output.

**GOOD:** thread an `on_line: &mut dyn FnMut(&str)` sink through the core
instead — the CLI passes `&mut |line| println!("{line}")`, the TUI routes it
into a session's log tab. `frust-drive/tests/print_free_cores.rs` (a
source-scan conformance test) enforces this against every drive core
outside a small CLI-entry allowlist.

## Interaction Semantics

Conventions for `Widget::event` implementations (spec §9), followed by every
interactive widget in `frust-widgets`:

- **Fire on up-inside, not down.** A press captures the pointer on `Down` and
  tracks a `pressed` visual on `Move`, but the callback fires only on `Up`
  and only if the release lands inside the widget's bounds; `Cancel`
  (platform gesture steal) clears the pressed state without firing:

  ```rust
  PointerPhase::Up => {
      if inside(p.position, ctx.size()) { (self.on_press)(ctx); }
      self.pressed = false;
  }
  ```

- **Controlled components never self-mutate.** `Checkbox`/`Slider` report the
  *requested* value through `on_toggle`/`on_change` and leave `checked`/
  `value` untouched until the next `rebuild` feeds the app-confirmed value
  back down — never flip `self.checked` (or similar) inline in a handler.
  `TextInput` is controlled too, reconciled rather than mutated:
  `rebuild` applies the view's `value` to the widget's `TextEditor`
  set-if-different (preserving the live selection when unchanged), so an
  app that rejects/transforms input in `on_change` sees its own value win
  next frame.

- **Focus routes by recorded path, like capture; `Key`/`Ime` events never
  hit-test.** `EventCtx::request_focus`/`release_focus` record/clear the
  focused child exactly like `capture_pointer`, and a container simply
  forwards `Key`/`Ime` events to its focused child; a `Down` that doesn't
  (re)claim focus on the child it hits blurs the chain (blur-on-outside-tap).
  **A structural container rebuild clears capture and focus only where
  identity is actually lost** — stable-prefix/key-matched, not a blanket
  clear: positional reconciliation clears a path only at/after the first
  index whose concrete type changed; keyed reconciliation clears it only
  for a removed/type-swapped child (a key-matched reorder relocates the
  widget, and its path, intact). `RenderRoot`'s cached
  `focus_active`/`ime_state` self-correct on the next event pass.

- **Keyed lists are all-or-nothing, and keys must be unique.** `keyed(key,
  view)` marks a `Flex` child list for identity-based reconciliation; once
  any child in a list is keyed, every child must be (a mixed or duplicate
  key set `debug_assert!`s and falls back to positional matching in
  release builds — never panics live). A matched reorder relocates the
  existing widget, preserving its state, rather than rebuilding it.

- **Input constants have one source.** Gesture thresholds (`TOUCH_SLOP`,
  `MOUSE_SLOP`), scroll/fling tuning (`WHEEL_LINE_PX`, `FLING_DECAY`,
  `FLING_STOP`, `VELOCITY_WINDOW_MS`), and `VelocityTracker` live in
  `frust-core::input`; widgets import them rather than hardcoding a local
  threshold.

- **Events are logical-coordinate by the time they cross `AppTree`.** Every
  platform boundary (winit, JNI `nativeOnTouch`, the C `frust_dispatch_touch`)
  converts to density-independent logical pixels before building an
  `InputEvent` — widget/container code never divides by scale factor; only
  the shell's FFI-boundary helpers do.

- **Insets follow the same physical-at-FFI, logical-inside rule as
  coordinates.** A platform delivers occlusion in physical px (Android
  `Insets`) or already-logical points (iOS `safeAreaInsets`); either way the
  FFI-boundary `logical_insets` helper reconciles it — widget code only
  ever sees a resolved `WindowInsets` in logical px (`docs/ARCHITECTURE.md`'s
  Inset delivery).

- **A `Cancel` arm must never call `EventCtx::state_mut`.** It may only clear
  internal flags (`self.pressed`/`self.captured`/`self.armed`) and request a
  redraw. A structural container rebuild can synthesize a `Cancel` to a
  still-captured child delivered over a throwaway `()` state
  (`docs/ARCHITECTURE.md`'s Event pipeline) — a handler reaching for real
  state there panics on the `()` downcast, a deliberate tripwire.

- **A container that suppresses routing to its children must cancel their
  capture, clear their focus, and publish a cleared IME surface — in that
  order, with no bypass.** This binds any container that stops forwarding
  events to an already-interactive child (the navigator's mid-transition
  input block is the reference impl, via `cancel_pod`/`set_active`/
  `set_focused` — see `docs/ARCHITECTURE.md`'s Navigation flow), not just
  `Navigator`. Skipping this leaves a child armed (a later `Up` it never
  receives fires against a widget the container has stopped routing to) or
  a stale focus/IME surface behind after the block lifts.

## Semantics Conventions

Conventions for `Widget::semantics` (spec §9, `docs/ARCHITECTURE.md`'s Semantics pass):

- **The method defaults to a no-op.** Only override it if the widget has a
  role/label/state worth reporting; a widget with nothing to say about itself
  needs no impl at all.
- **A container MUST forward to every child via `ChildPod::semantics_child`**,
  never by calling a child's `semantics` directly — this threads the absolute
  origin the same way `paint_child`/`event_child` do. Skipping a child here
  silently drops its whole subtree from the accessibility tree with no
  compile-time or test signal, so every new container widget needs a
  `semantics` impl even a transparent one that just forwards:

  ```rust
  fn semantics(&self, ctx: &mut SemanticsCtx) { self.child.semantics_child(ctx); }
  ```

- **Keep it minimal: role, label, state, and bounds only.** This gives the
  per-shell adapter (`accesskit_winit`/`accesskit_android`/`accesskit_ios`
  — see `docs/ARCHITECTURE.md`'s Semantics pass) just enough to build on —
  no live-region announcements, custom actions, or adapter wiring belong
  here; that integration lives in a shell, not `frust-core`.
- **A platform adapter gates its pushes on `semantics_generation`/
  `semantics_if_changed`, not on pushing every frame unconditionally.** All
  three shipping adapters compare the last-seen generation before rebuilding
  a `TreeUpdate`; iOS also serves a cached snapshot to a newly-activated
  screen reader so gating never starves a VoiceOver connect.

## Instrumentation & Frame-Gate Conventions

- **Perf recording is always gated behind `perf::enabled()`, never
  unconditional.** `frust-shell-common::perf`'s `FrameStats`/`StartupSpans`
  no-op internally when disabled, but a shell should still read
  `perf::enabled()` once per frame into a local bool and gate every
  `Instant::now()` read behind it (`bool::then(Instant::now)`) on
  FFI-sensitive paths (Android/iOS) so a disabled build takes zero clock
  reads. Span names are `perf::SPAN_*` consts, not string literals.
- **Frame-gate inputs default to must-run, never to skip.** A `FrameInputs`
  field with no precise signal should stay `true`/fed conservatively rather
  than guessed `false` — over-running costs a wasted frame, over-skipping
  drops real work (see `docs/ARCHITECTURE.md`'s Frame gate).
  `FRUST_NO_FRAME_GATE=1` (parsed like `FRUST_TRACE`) forces every tick to
  `Run`; reach for it first when diagnosing a suspected stuck-UI report.

## State & Reactivity Conventions

- **Local state lives in the retained `Component` element, not signals,
  unless it needs cross-tree reactivity.** `Component::State` is plain data
  `ComponentWidget` retains across rebuilds — reach for an `RwSignal` field
  only when something outside the component's own `build` (a background
  task, a sibling, a nested component) needs to observe a write.
- **Teardown disposes the component's `Owner`; register cleanup via
  `on_cleanup`, not `Drop`.** `ComponentWidget::teardown` tears down the child
  element, then disposes the owner (running every `on_cleanup` registered
  under it since `init`) before dropping `State`. Register disposal (timers,
  controller/subscription teardown) with `on_cleanup` inside `init`/`build` —
  never hand-roll a `Drop` impl on `State`; `Drop` order across the
  state/element/owner triple is not a contract, `on_cleanup` is.
- **A `Cancel` arm still never touches state, even the component's own** —
  the Cancel-never-mutates-state rule above binds every handler a component
  hosts, including one reading `ComponentWidget`'s own `State` (a
  synthesized `Cancel` crossing a component boundary carries real state,
  unlike the throwaway `()` at the root, but the contract is identical).
- **Heavy work routes by shape: `spawn` (async IO) / `spawn_local` (UI-thread `!Send`) /
  `spawn_blocking` (one-off CPU) / rayon (an app-level choice, not bundled).** Calling
  `spawn_local` off the UI thread panics (a wiring bug, not a runtime-data condition); a
  backgrounded iOS app pauses `CADisplayLink`, so a `spawn_local` timer stalls until
  `frust_resume`'s next pump. `use_task` composes `AsyncValue<T>` over this routing: a
  UI-thread coordinator (under the calling component's `Owner`) hands work to
  `spawn`/`spawn_blocking` and is the sole signal writer, ruling out a cross-thread write
  race; owner cleanup aborts it and its `JoinHandle` — an already-running `spawn_blocking`
  closure can't be interrupted, only its result delivery dropped.
- **`Component::State` holds `RwSignal`s directly; app code depends on the
  `frust` facade only, never `reactive_graph`/`any_spawner`/`frust-reactive`
  directly.** A reactive field is typed `RwSignal<T>`, read/written through
  the facade's `Get`/`Set`/`Update` traits. **An `examples/*`/app crate's
  `Cargo.toml` depends on `frust` plus plugin crates only** — the facade
  never re-exports plugins (Plugin Conventions below); a documented
  `frust-core`/`kurbo`(/`peniko`) escape hatch (`examples/huddle`,
  `examples/glyph-catalog`) is sanctioned, for gaps no facade widget covers.
- **A rebuild must run inside a `TrackedScope` for a signal write to wake it later — an
  untracked read is a silent wake hazard, not a stale value.** `.get()` subscribes only from
  *inside* a live `TrackedScope::track` closure; both shells guarantee this for their
  per-frame rebuild (desktop's `scope.track(|| root.rebuild(..))`, mirrored on mobile by a
  persistent per-`AppHandle` `TrackedScope`). A render-relevant read taken via
  `*_untracked`/`get_untracked` anywhere in that path never subscribes, so a later write flips
  no dirty flag and the shell may never repaint — reserve `*_untracked` for genuine
  non-rendering reads, never a value a `build` return depends on.

## Theming & Animation Conventions

- **Paint-time resolution is always safe; layout-time-baked resolution is
  only safe under the `set_theme` → `ChangeFlags` contract.** Most themed
  widgets resolve tokens from `PaintCtx` on every paint pass and so
  self-refresh on a live theme swap for free. `Text` and `TextInput` instead
  bake their resolved glyph color into the shaped layout at LAYOUT time (see
  `docs/ARCHITECTURE.md`'s Theme delivery); a widget adding layout-time-baked
  resolution depends on relayout actually happening, so any dirty-tracking
  work must treat a theme change as forcing `ChangeFlags::LAYOUT`, not just
  `PAINT`.
- **Resolve theme tokens with an unthemed-fallback constant per resolved
  value.** A themed widget looks up
  `Theme::from_paint_ctx(ctx)`/`from_layout_ctx(ctx)`, falling back to a local
  constant (e.g. `Button`'s `FILL`/`RADIUS`) when no theme is threaded
  (bare-core tests, pre-theme apps). Precedence is **explicit builder value >
  theme > fallback**: an app-set value (`.color(...)`, `.style(...)`) always
  wins over both (see `Text`'s `color_explicit` flag).
- **Token-not-hardcode: a widget authors against a `Theme` field first; a
  bare local constant is the documented fallback, not the default.** A
  hardcoded metric/color in paint or layout code is a defect once a matching
  `ColorScheme`/`ShapeScale`/`Elevation`/`GlassScale`/`MotionScheme` field
  exists — resolve it through `Theme::from_paint_ctx`/`from_layout_ctx` per
  the fallback bullet above. Only a genuine token-scale gap earns a
  hand-tuned constant, and that constant stays named, doc-commented, and
  states *why* no token applies rather than being a silent magic number.
- **A contested or unsourced design fact is resolved against a primary
  source and cited with a retrieval date, not left as a guess.** When a
  research doc's claim lacks (or conflicts with) a citable primary source,
  fetch it and record `<source>, retrieved <date>` in the module doc,
  alongside the **Community-approximate** marker (above) for values that
  stay genuinely unsourced.
- **Event-pass code never reads a theme — `EventCtx` carries none.** Only
  `LayoutCtx`/`PaintCtx` thread a theme; a metric an event handler also needs
  (hit-test padding, caret geometry) stays a plain constant read from both
  passes — the precedent is `TextInput`'s `PAD_X`/`PAD_Y`/`CARET_W`,
  deliberately never resolved from theme.
- **No `Instant::now()` in `frust-core`/`frust-widgets`.** Time enters
  the framework only from a shell, as the `FrameTime` passed into
  `RenderRoot::paint` and threaded via `PaintCtx::frame_time` — desktop reads
  its own `Instant` epoch; Android/iOS pass through the platform's own frame
  clock. Widget code only *differences* two `FrameTime`s (`saturating_sub`),
  never reads a wall clock directly.
- **An animation controller advances during paint, not on a timer.** A widget
  holds an `anim::AnimationController`, calls `advance(ctx.frame_time())` once
  per paint, reads `value()`, and — while `advance` returns `true` — calls
  `PaintCtx::request_frame()` so the shell schedules the next frame; there is
  no ambient ticker. **A layout-affecting animation calls `request_layout()`
  instead** (it implies `request_frame`) — reserve bare `request_frame` for an
  animation that only repaints (color fade, caret blink), or the mobile
  intra-frame layout skip silently leaves it unresized (see
  `docs/ARCHITECTURE.md`'s Frame pipeline).
- **State-layer opacity has one source: `material::state_layer`'s constants.**
  `HOVER_OPACITY`/`FOCUS_OPACITY`/`PRESSED_OPACITY`/`DRAGGED_OPACITY` (M3
  `StateTokens`) live in that one module; a catalog widget imports them
  rather than hardcoding overlay opacity, taking the **maximum** of
  concurrently-active states, never their sum. Only `pressed` is currently
  wired by any shipping widget.
- **Glyph's token set adds three resolution precedents.** A per-status color
  with no `ColorScheme` field (Success/Warning/Info) resolves
  `Theme::extension::<StatusPalette>()` first, before a role that already has
  one (Error) resolves it directly. `GlyphInk` is never brightness-swapped
  like a scheme role — it reads identically on Light/Dark `glyph_baseline()`.
  Accent role split: `primary`/`on_primary` is accent text/icon ink,
  `primary_container`/`on_primary_container` is the bright fill — conflating
  the two (Glyph light mode forces them apart) is the catalog's most common
  accent bug.
- **A transition pattern's default timing resolves from `Theme.motion`,
  never a hand-rolled duration, and collapses under `reduce_motion`.**
  `PatternSwitcher`/`AnimatedOpacity`/`AnimatedScale` resolve
  `MotionScheme`'s duration/easing/spring tokens for their default
  `Timing` (an explicit `.timing(...)` call always wins); every
  pattern/wrapper substitutes a short linear crossfade under
  `reduce_motion` instead of a bespoke reduced variant.
- **A perpetual decorative loop calls `PaintCtx::request_frame_paced`
  (`TickClass::CosmeticLoop`), never bare `request_frame`.** `request_frame`
  stays `TickClass::Transition` (unpaced) — correct for a spring, a finite
  transition, or anything with a user-visible endpoint (`request_layout`
  always implies `Transition`). A shimmer/spinner/pulse with no endpoint
  requests the paced class instead, letting the mobile frame gate throttle
  it to `MotionScheme::cosmetic_loop_rate` (`docs/ARCHITECTURE.md`'s Frame
  gate), and must still honor `reduce_motion` like any other loop (freeze
  in place, stop requesting frames). **Input-driven frames are never
  paced** — a paced request only ever comes from a perpetual paint-time
  loop, never from `EventCtx`.

## Testing Patterns

- **Fixture-driven tests for parsers/validators**: register canned
  `ProcessRunner`/`EnvLookup` responses keyed by the exact invocation, then
  assert the resulting `Status`/`Validation`/`DoctorReport`.
- **Injectable hook seams for process-global side effects**: a function
  installing a real handler in production (`ctrlc::set_handler`, a
  filesystem watcher) takes a small `Hooks` struct defaulted to the real
  installers, with a `::fake()` (`#[cfg(test)]`) no-op pair a test injects
  instead — exercising dispatch logic without installing a real
  process-wide handler.
- **`#[ignore = "<reason>"]` for GPU-dependent or slow end-to-end tests.**
  The reason string must say how to run it (`cargo test -p ... --ignored`)
  and why it's excluded by default (needs a real GPU; compiles a full
  generated dependency graph; etc.).
- **Recording fakes for paint assertions**: a minimal `PaintScene` impl
  pushing `(origin, size)`/`(origin, text)` tuples into `Vec`s lets widget
  `layout`/`paint` behavior be asserted without any GPU dependency.
- **Template rendering uses `minijinja::UndefinedBehavior::Strict`**: an
  unresolved `{{ placeholder }}` is a hard render-time error, catching
  template/context drift in tests instead of a generated project.
