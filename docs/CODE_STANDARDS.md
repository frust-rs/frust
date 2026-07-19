# ForgeKit - Code Standards

## Language Idioms

- **`unsafe` is confined to a small set of sanctioned platform-FFI boundaries.**
  Every other crate (`forgekit-core`, `forgekit-scene`, `forgekit-text`,
  `forgekit-widgets`, `forgekit-shell-common`, `forgekit-shell-desktop`,
  `forgekit`) stays `unsafe`-free — where Masonry/xilem-style code would
  reach for `unsafe` downcasting, use trait upcasting instead: bound a trait
  on `Any` (e.g. `Widget: Any`) and downcast through `&mut dyn Any`. The
  sanctioned zones are raw-pointer boundaries a GPU/platform shell cannot
  avoid, each isolated in one function/module with a `# Safety` doc comment
  stating the caller contract:
  - `forgekit-render`'s `create_android_surface`/`create_metal_surface`
    (`lifecycle.rs`) and the `on_surface_created_from_android_window`/
    `on_surface_created_from_metal_layer` renderer methods (`renderer.rs`)
    — turn a caller-owned raw `ANativeWindow*`/`CAMetalLayer*` into a
    `wgpu::Surface`.
  - `forgekit-shell-android`'s `jni_glue` module — the JNI FFI boundary
    (`extern "system"` exports, `Box::into_raw`/`from_raw` for the opaque
    native handle, `ANativeWindow_fromSurface`) — plus the
    `#[unsafe(no_mangle)]` attributes the `android_app!` macro emits on its
    generated exports.
  - `forgekit-shell-ios`'s `ffi_glue` module — the C-ABI FFI boundary
    (`extern "C"` exports, `Box::into_raw`/`from_raw` for the opaque native
    handle, the call into `on_surface_created_from_metal_layer`) — plus the
    `#[unsafe(no_mangle)]` attributes the `ios_app!` macro emits on its
    generated exports.
  - `forgekit-render`'s `context.rs::RenderContext::create_pipeline_cache`
    — one `unsafe { device.create_pipeline_cache(..) }` call building a
    `wgpu::PipelineCache` from a shell-persisted blob. The blob is
    `unframe`d and adapter-fingerprint-validated (`pipeline_cache::unframe`)
    before this call, and wgpu's own `fallback: true` backstops any
    residual mismatch — see `docs/ARCHITECTURE.md`'s GPU pipeline cache.
- **No unwind across FFI.** Every platform export both shells define routes
  through `forgekit-shell-common`'s `guard` helper, which `catch_unwind`s and
  logs, returning a benign default instead of unwinding into JVM- or
  Swift-owned stack frames — a panic crossing the FFI boundary is undefined
  behavior, not just a bug.
- **State-sync, not op-forwarding, across a mobile IME bridge.** Android/iOS
  platform text input doesn't send individual keystrokes across the FFI
  boundary — the platform owns composition (Gboard, CJK marked text) against
  a local mirror (Kotlin `Editable`/Swift `NSMutableString`), then hands the
  framework a whole reconciled `EditingState` (`nativeImeApply`/
  `forgekit_ime_apply`) and reads a reconciled state back
  (`nativeImeState`/`forgekit_ime_state_json`) to keep its own IME machinery
  (`InputConnection`/`UITextInput`) synchronized. Do not add a per-keystroke
  op-forwarding path for mobile text input — it fights the platform's own
  composition state machine.
- **UTF-16 at the FFI seam, bytes inside.** `EditingState`'s
  `selection_*`/`composing_*` indices are UTF-16 code-unit indexed
  everywhere they cross a shell boundary (JNI, C-ABI, `AppTree`) — the
  platform's native string type. Convert to/from byte offsets only inside
  `forgekit-text`'s `TextEditor` (`byte_to_utf16`/`utf16_to_byte`); a
  shell/glue module passes indices through opaquely and never does this
  conversion itself.
- **Hand-roll JSON at the mobile FFI boundary — no `serde` in shell crates.**
  `forgekit-shell-android`/`forgekit-shell-ios` serialize `ImeState` with a
  small hand-written escaper/builder, not a `serde_json` dependency, keeping
  the always-host-testable half of each shell crate free of a codegen
  dependency for a handful of fixed fields.
- **Type-erase to avoid a downstream crate dependency**, not to avoid writing
  a type. When a lower layer needs to thread a resource owned by a higher
  layer (e.g. `forgekit-core`'s `LayoutCtx` carrying the shell's
  `forgekit-text::TextContext`), pass it as `&mut dyn Any` rather than adding
  the dependency, and recover it at the one call site that knows the
  concrete type with a documented, panic-on-mismatch `downcast_mut::<T>()`
  — the panic message should say this is a wiring bug, not a runtime-data
  condition.
- **Edition-2024 `-> impl Trait` return types capture all in-scope
  lifetimes by default.** When a function returns an `impl Trait` that
  borrows nothing from its parameters (e.g. `app_logic(&mut State) -> impl
  View<State>`, where views are `'static`), opt out explicitly:

  ```rust
  fn app_logic(state: &mut AppState) -> impl forgekit::View<AppState> + use<> {
      forgekit::text(state.greeting.clone()).size(32.0)
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
  /// Left-edge activation zone width for the interactive pop-swipe, in
  /// logical px.
  ///
  /// **Community-approximate**: UIKit's `interactivePopGestureRecognizer`
  /// edge zone is not a published constant; ~20dp is the value the
  /// community-reverse-engineered reimplementations converge on.
  const EDGE_SWIPE_ZONE_DP: f64 = 20.0;
  ```
  This keeps the tunable visible and re-tunable in one place instead of
  buried inline, and tells a future reader whether "fixing" a value means
  matching a spec or just adjusting a guess. The `cupertino::*` widget catalog
  applies this to every iOS-tunable it introduces (alert/action-sheet
  dimensions, switch geometry, spinner spoke layout, hairline widths) — grep
  for `Community-approximate` to find them all.

## Error Handling

- **`anyhow::Result` in binaries and integration-facing library code**
  (`forgekit-cli` commands, `forgekit-render`'s public API,
  `forgekit-shell-desktop`/`forgekit`'s `run`), with `.context()` /
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
| Crate names | `forgekit-<layer>` | `forgekit-scene`, `forgekit-shell-desktop` |
| Fallible constructor errors | `<Type>Error` enum, `thiserror`-derived | `BuildInfoError`, `NameError` |
| Test-only fakes | `Fake<Trait>` | `FakeProcessRunner`, `FakeEnv` |
| Widget pairs | `<Name>View` (declarative) / `<Name>Widget` (retained) | `TextView` / `TextWidget` |
| JNI exports | `Java_<fixed_package>_<FixedClass>_native<Name>` | `Java_dev_forgekit_ForgeKitSurfaceView_nativeOnFrame` |

JNI export names are LAW: the package/class (`dev.forgekit.ForgeKitSurfaceView`)
is fixed across every generated app, not app-specific, so the mangled symbol
stays stable regardless of the app's own package.

## Anti-patterns

### Shelling out directly instead of through `ProcessRunner`

**BAD:**
```rust
let out = std::process::Command::new("rustc").arg("--version").output()?;
```
Untestable without actually invoking `rustc`, and every caller re-implements
its own spawn-failure handling.

**GOOD:**
```rust
fn validate(&self, ctx: &DoctorCtx) -> Validation {
    match ctx.runner.run("rustc", &["--version"]) { /* ... */ }
}
```
Every external tool invocation (`rustc`, `adb`, `xcrun`, `cargo ndk`, …) goes
through the `ProcessRunner` trait, so `doctor`/`devices` logic is exercised
in `cargo test` with `FakeProcessRunner` and never shells out during a test
run.

### Leaking `vello`/`wgpu` types outside `forgekit-render`

**BAD:** a `forgekit-scene` or `forgekit-text` public function taking or
returning a `vello::*`/`wgpu::*` type.

**GOOD:** public APIs above `forgekit-render` speak only `kurbo`/`peniko`;
this is what lets the GPU backend be swapped later without touching widget
or text code.

## Interaction Semantics

Conventions for `Widget::event` implementations (spec §9), followed by every
interactive widget in `forgekit-widgets`:

- **Fire on up-inside, not down.** A press captures the pointer on `Down` and
  tracks a `pressed` visual on `Move`, but the callback fires only on `Up`
  and only if the release lands inside the widget's bounds; `Cancel`
  (platform gesture steal) clears the pressed state without firing:

  ```rust
  PointerPhase::Up => {
      if inside(p.position, ctx.size()) {
          (self.on_press)(ctx);
      }
      self.pressed = false;
  }
  ```

- **Controlled components never self-mutate.** `Checkbox`/`Slider` report the
  *requested* value through `on_toggle`/`on_change` and leave `checked`/
  `value` untouched until the next `rebuild` feeds the app-confirmed value
  back down — the widget is not its own source of truth. Never flip
  `self.checked` (or similar) inline in an event handler. `TextInput` is a
  controlled component too, reconciled rather than mutated: `rebuild`
  applies the view's `value` to the widget's `TextEditor` with a
  set-if-different (preserving the live selection when the text is
  unchanged), so an app that rejects or transforms input in `on_change` sees
  its own value win on the next frame.

- **Focus routes by recorded path, like capture; `Key`/`Ime` events never
  hit-test.** `EventCtx::request_focus`/`release_focus` record/clear the
  focused child exactly like `capture_pointer` records the active one, and a
  container simply forwards `Key`/`Ime` events to its focused child. A
  `Down` that doesn't (re)claim focus on the child it hits blurs the chain
  (blur-on-outside-tap). A structural container rebuild — a child-count
  change, an `AnyView` type swap, or a keyed reorder — clears both the
  capture and focus paths; `RenderRoot`'s cached `focus_active`/`ime_state`
  are not pushed at rebuild time and self-correct on the next event pass,
  the same convergence contract the capture-cancel case below already uses.

- **Keyed lists are all-or-nothing, and keys must be unique.** `keyed(key,
  view)` marks a `Flex` child list for identity-based reconciliation; once
  any child in a list is keyed, every child in that list must be (a mixed
  keyed/unkeyed set, or duplicate keys, `debug_assert!`s and falls back to
  positional matching in release builds — never panics live). A matched
  reorder relocates the existing widget, preserving its internal state,
  rather than rebuilding it.

- **Input constants have one source.** Gesture thresholds (`TOUCH_SLOP`,
  `MOUSE_SLOP`), scroll/fling tuning (`WHEEL_LINE_PX`, `FLING_DECAY`,
  `FLING_STOP`, `VELOCITY_WINDOW_MS`), and `VelocityTracker` live in
  `forgekit-core::input`; widgets import them rather than hardcoding a local
  threshold, so tuning changes in one place and stays consistent everywhere.

- **Events are logical-coordinate by the time they cross `AppTree`.** Every
  platform boundary (winit, JNI `nativeOnTouch`, the C `forgekit_dispatch_touch`)
  converts to density-independent logical pixels before building an
  `InputEvent` — widget/container code never divides by scale factor; only
  the shell's FFI-boundary helpers do.

- **A `Cancel` arm must never call `EventCtx::state_mut`.** It may only clear
  internal flags (`self.pressed`/`self.captured`/`self.armed`) and request a
  redraw. A structural container rebuild can synthesize a `Cancel` to a
  still-captured child with no application state in scope, delivered over a
  throwaway `()` state (see `docs/ARCHITECTURE.md`'s Event pipeline); a
  handler that reached for real state there panics on the `()` downcast — a
  deliberate tripwire, not silent corruption.

- **A container that suppresses routing to its children must cancel their
  capture, clear their focus, and publish a cleared IME surface — in that
  order — before the block takes effect; there is no bypass.** This binds any
  container that can stop forwarding events to an already-interactive child
  (the navigator's mid-transition input block is the reference
  implementation — see `docs/ARCHITECTURE.md`'s Navigation flow), not just
  `Navigator`:

  ```rust
  fn cancel_top(&mut self) {
      if let Some(top) = self.pages.last_mut() {
          if top.pod.is_active() {
              crate::cancel_pod(&mut top.pod); // synthetic Cancel first
              top.pod.set_active(false);
          }
          if top.pod.is_focused() {
              top.pod.set_focused(false); // then focus
          }
      }
      // then: publish a cleared ImeState on the next paint
  }
  ```
  Skipping this leaves a child armed (a later `Up` it never receives fires
  against a widget the container has stopped routing to) or a stale
  focus/IME surface behind after the block lifts.

## Semantics Conventions

Conventions for `Widget::semantics` (spec §9 — see `docs/ARCHITECTURE.md`'s
Semantics pass):

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
  fn semantics(&self, ctx: &mut SemanticsCtx) {
      self.child.semantics_child(ctx);
  }
  ```

- **Keep it minimal: role, label, state, and bounds only.** This gives the
  per-shell platform adapter (`accesskit_winit`/`accesskit_android`/
  `accesskit_ios` — see `docs/ARCHITECTURE.md`'s Semantics pass) just enough
  to build on — no live-region announcements, no custom actions, and no
  adapter wiring belong here; that integration lives in a shell, not
  `forgekit-core`.
- **A platform adapter gates its pushes on `semantics_generation`/
  `semantics_if_changed`, not on pushing every frame unconditionally.** All
  three shipping adapters (desktop, Android, iOS) compare the last-seen
  generation before rebuilding a `TreeUpdate`; the iOS adapter additionally
  serves a cached tree snapshot to a newly-activated screen reader so
  gating never starves a VoiceOver connect against an already-settled
  screen — the reference pattern a new adapter's activation handling
  should follow.

## Instrumentation & Frame-Gate Conventions

- **Perf recording is always gated behind `perf::enabled()`, never
  unconditional.** `forgekit-shell-common::perf`'s `FrameStats`/
  `StartupSpans` no-op internally when disabled, but a shell should still
  read `perf::enabled()` once per frame into a local bool and gate every
  `Instant::now()` read behind it (`bool::then(Instant::now)`) on
  FFI-sensitive paths (Android/iOS) so a disabled build takes zero clock
  reads, not just zero recording; desktop's frame budget is generous enough
  to skip this extra branch. Span names are `perf::SPAN_*` consts, not
  string literals, so every shell logs the same names; a shell-local
  milestone `perf.rs` has no const for (e.g. Android's on-disk cache-load
  checkpoint) may still pass a `&'static str` literal straight to
  `StartupSpans::record` rather than editing `perf.rs`.
- **Frame-gate inputs default to must-run, never to skip.** A `FrameInputs`
  field a shell doesn't have a precise signal for should stay `true`/be
  fed conservatively rather than guessed `false` — over-running costs a
  wasted frame, over-skipping drops real work (see
  `docs/ARCHITECTURE.md`'s Frame gate). `FORGEKIT_NO_FRAME_GATE=1` (parsed
  the same compile-time-or-runtime way as `FORGEKIT_TRACE`) forces every
  tick to `Run`; reach for it first when diagnosing a suspected stuck-UI
  report before assuming a widget bug.

## State & Reactivity Conventions

- **Local state lives in the retained `Component` element, not signals,
  unless it needs cross-tree reactivity.** A `Component::State` is plain data
  `ComponentWidget` retains across rebuilds — reach for an `RwSignal` field
  only when something outside the component's own `build` (a background
  task, a sibling, a nested component) needs to observe a write. Most
  `Component::State` should be plain structs/enums, not signal-wrapped.
- **Teardown disposes the component's `Owner`; register cleanup via
  `on_cleanup`, not `Drop`.** `ComponentWidget::teardown` tears down the child
  element, then disposes the owner (running every `on_cleanup` registered
  under it since `init`) before dropping `State`. Register disposal (timers,
  controller/subscription teardown) with `on_cleanup` inside `init`/`build` —
  never by hand-rolling a `Drop` impl on `State`; `Drop` order across the
  state/element/owner triple is not a contract, `on_cleanup` is.
- **A `Cancel` arm still never touches state, even the component's own.** The
  Cancel-never-mutates-state rule above binds every widget a component hosts,
  including handlers reading `ComponentWidget`'s own `State` — a synthesized
  `Cancel` crossing a component boundary carries real state (unlike the
  throwaway `()` at the root), but the contract not to reach for it is
  identical.
- **`spawn` is for `Send` background work; `spawn_local` only ever runs on
  the UI thread.** `forgekit::spawn` hands a `Send` future to the background
  tokio runtime; `forgekit::spawn_local` queues a `!Send` future the shell
  drains via `ReactiveRuntime::pump_local` once per frame. Calling
  `spawn_local` off the UI thread is a wiring bug, not a runtime-data
  condition, and panics with a message saying so (the same convention as the
  `downcast_mut` panic message above). On iOS, a backgrounded app pauses
  `CADisplayLink`, so nothing pumps and any timer-driven local task (e.g. an
  in-flight `tokio::time::sleep`) stalls until `forgekit_resume` fires the
  next pump on foreground — don't assume a `spawn_local` timer completes
  promptly while backgrounded.
- **`Component::State` holds `RwSignal`s directly; app code depends on the
  `forgekit` facade only, never `reactive_graph`/`any_spawner`/
  `forgekit-reactive` directly.** A state field that needs reactive
  read-tracking is typed `RwSignal<T>` and read/written through the
  `Get`/`Set`/`Update` traits the facade re-exports — an app crate should
  never add `reactive_graph`/`any_spawner`/`forgekit-reactive` to its own
  `Cargo.toml`; every symbol an app needs is already flat-re-exported from
  `forgekit` (see `docs/ARCHITECTURE.md`'s Key Types). **This is the general
  rule for every `examples/*` crate, not just reactive types**: an example's
  `Cargo.toml` should depend on `forgekit` alone. `examples/huddle` — the sole
  example — mostly holds to this but carries a **documented**
  `forgekit-core`/`kurbo`/`peniko` escape-hatch dependency (see its
  `Cargo.toml`'s comment) for the handful of custom app widgets no facade
  widget covers: `ui/swipeable`'s swipe-to-action row, `ui/sheet`'s modal
  sheet, `screens/home`'s avatar-fill/`Shimmer` loading effect, and
  `ui/toast`'s `toast_entrance` slide-up/fade entrance widget. A new example
  reaching for this escape hatch should first check whether the gap belongs
  in the facade instead.

## Theming & Animation Conventions

- **Paint-time resolution is always safe; layout-time-baked resolution is
  only safe under the `set_theme` → `ChangeFlags` contract.** Most themed
  widgets resolve tokens from `PaintCtx` on every paint pass and so
  self-refresh on a live theme swap for free. `Text` instead bakes its
  resolved glyph color into the shaped layout at LAYOUT time (see
  `docs/ARCHITECTURE.md`'s Theme delivery); a widget adding layout-time-baked
  resolution depends on relayout actually happening, so any dirty-tracking
  work must treat a theme change as forcing `ChangeFlags::LAYOUT`, not just
  `PAINT`.
- **Resolve theme tokens with an unthemed-fallback constant per resolved
  value.** A themed widget looks up
  `Theme::from_paint_ctx(ctx)`/`from_layout_ctx(ctx)`, falling back to a local
  constant (e.g. `Button`'s `FILL`/`RADIUS`) when no theme is threaded
  (bare-core tests, pre-theme apps) — a widget never assumes a theme is
  present. Precedence is **explicit builder value > theme > fallback**: an
  app-set value (`.color(...)`, `.style(...)`) always wins over both (see
  `Text`'s `color_explicit` flag).
- **Token-not-hardcode: a widget authors against a `Theme` field first; a
  bare local constant is the documented fallback, not the default.** A
  hardcoded metric/color in paint or layout code is a defect once a matching
  `ColorScheme`/`ShapeScale`/`Elevation`/`GlassScale`/`MotionScheme` field
  exists (see `docs/ARCHITECTURE.md`'s Key Types) — resolve it through
  `Theme::from_paint_ctx`/`from_layout_ctx` per the fallback bullet above.
  Only a genuine token-scale gap earns a hand-tuned constant (`forgekit-theme`
  ships no spacing scale and no fixed-dimension scale for switch-track/button-
  padding metrics), and that constant stays named, doc-commented, and states
  *why* no token applies (`button.rs`'s `PAD_X`/`PAD_Y`, `switch.rs`'s
  `TRACK_W`/`TRACK_H` are the reference cases) rather than being left as a
  silent magic number.
- **A contested or unsourced design fact is resolved against a primary
  source and cited with a retrieval date, not left as a guess.** When a
  research doc's claim lacks (or conflicts with) a citable primary source,
  fetch the primary source and record `<source>, retrieved <date>` in the
  module doc, alongside the existing **Community-approximate** marker
  (above) for values that stay genuinely unsourced. Reference cases:
  `typography.rs`'s M3-Expressive emphasized-role count, resolved against
  `androidx.compose.material3`'s `TypeScaleTokens.kt` (retrieved
  2026-07-18); `color.rs`'s Cupertino palette refresh and several
  `cupertino::*` metrics (`switch.rs`'s 64×28 track, `button.rs`'s
  size-class heights), each citing a specific named record in the mined
  `kit-colors-type-metrics.json`/`glass-recipes.json` research ledgers.
- **Event-pass code never reads a theme — `EventCtx` carries none.** Only
  `LayoutCtx`/`PaintCtx` thread a theme; a metric an event handler also needs
  (hit-test padding, caret geometry) stays a plain constant read from both
  passes — the precedent is `TextInput`'s `PAD_X`/`PAD_Y`/`CARET_W`,
  deliberately never resolved from theme.
- **No `Instant::now()` in `forgekit-core`/`forgekit-widgets`.** Time enters
  the framework only from a shell, as the `FrameTime` passed into
  `RenderRoot::paint` and threaded via `PaintCtx::frame_time` — desktop reads
  its own `Instant` epoch; Android/iOS pass through the platform's own frame
  clock (`Choreographer`/`CADisplayLink`). Widget code only *differences* two
  `FrameTime`s (`saturating_sub`), never reads a wall clock directly.
- **An animation controller advances during paint, not on a timer.** A widget
  holds an `anim::AnimationController`, calls `advance(ctx.frame_time())` once
  per paint, reads `value()`, and — while `advance` returns `true` — calls
  `PaintCtx::request_frame()` so the shell schedules the next frame; there is
  no ambient ticker (see `docs/ARCHITECTURE.md`'s Frame pipeline).
- **State-layer opacity has one source: `material::state_layer`'s constants.**
  `HOVER_OPACITY`/`FOCUS_OPACITY`/`PRESSED_OPACITY`/`DRAGGED_OPACITY` (M3
  `StateTokens`) live in that one module; a catalog widget imports them rather
  than hardcoding its own overlay opacity. When more than one interaction
  state is active at once, the overlay opacity is the **maximum** of the
  active states', never their sum — M3 shows the strongest state, not a
  stacked blend. Of the four, only `pressed` is currently driven by any
  shipping widget: `hovered` awaits a pointer-hover `PointerPhase` (none
  exists yet); `focused` awaits a widget that both participates in focus
  routing *and* paints its own `StateLayer` surface — the four modal widgets
  now claim keyboard focus (see `docs/ARCHITECTURE.md`'s Semantics pass) but
  paint no `StateLayer` chrome, so this state stays unwired; `dragged` awaits
  a consumer calling `set_dragged`. See `material/state_layer.rs`'s "Live vs.
  aspirational states" module doc for the authoritative statement.

## Testing Patterns

- **Fixture-driven tests for parsers/validators**: register canned
  `ProcessRunner`/`EnvLookup` responses (`FakeProcessRunner::with(...)`,
  `.missing(...)`, `FakeEnv::set(...)`) keyed by the exact invocation, then
  assert the resulting `Status`/`Validation`. This is the pattern used
  throughout `forgekit-cli`'s `doctor`/`devices` validators.
- **`#[ignore = "<reason>"]` for GPU-dependent or slow end-to-end tests.**
  The reason string must say how to run it (`cargo test -p ... --
  --ignored`) and why it's excluded by default (needs a real GPU; compiles a
  full generated dependency graph; etc.) — see
  `forgekit-render/tests/gpu_smoke.rs` and
  `forgekit-cli/tests/create_e2e.rs`.
- **Recording fakes for paint assertions**: a minimal `PaintScene`
  implementation that pushes `(origin, size)`/`(origin, text)` tuples into
  `Vec`s lets widget `layout`/`paint` behavior be asserted without any GPU
  or `forgekit-render` dependency (see `forgekit-core::widget` and `app`
  unit tests).
- **Template rendering uses `minijinja::UndefinedBehavior::Strict`**: an
  unresolved `{{ placeholder }}` is a hard render-time error rather than a
  silently emitted `undefined`, so template/context drift is caught by the
  test suite instead of shipping into a generated project.
