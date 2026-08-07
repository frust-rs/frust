# Frust - Code Standards

## Language Idioms

- **`unsafe` is confined to a small set of sanctioned platform-FFI boundaries.** Every other
  crate (`frust-core`, `frust-scene`, `frust-text`, `frust-widgets`, `frust-shell-common`,
  `frust-shell-desktop`, `frust`) stays `unsafe`-free — where Masonry/xilem-style code would
  reach for `unsafe` downcasting, use trait upcasting instead: bound a trait on `Any` (e.g.
  `Widget: Any`) and downcast through `&mut dyn Any`. The sanctioned zones are raw-pointer
  boundaries a GPU/platform shell cannot avoid, each isolated in one function/module with a
  `# Safety` doc comment stating the caller contract:
  - `frust-render`'s `create_android_surface`/`create_metal_surface` (`lifecycle.rs`) and
    `on_surface_created_from_android_window`/ `on_surface_created_from_metal_layer`
    (`renderer.rs`) — turn a caller-owned raw `ANativeWindow*`/`CAMetalLayer*` into a
    `wgpu::Surface`.
  - `frust-shell-android`'s `jni_glue` module — the JNI FFI boundary (`extern "system"`
    exports, `Box::into_raw`/`from_raw`, `ANativeWindow_fromSurface`, `nativeInitPlatform`'s
    `JavaVM` stash), `android_app!`'s generated exports, and the render-thread split's
    `unsafe impl Send` for `SendableWindowPtr` plus a bare `libc::setpriority` self-boost.
  - `frust-shell-ios`'s `ffi_glue` module — the C-ABI FFI boundary (`extern "C"` exports,
    `Box::into_raw`/`from_raw`, the call into `on_surface_created_from_metal_layer`),
    `ios_app!`'s generated exports, and the split's `unsafe impl Send` for
    `SendableMetalLayer` plus a bare `libc::pthread_set_qos_class_self_np` self-boost.
  - `frust-plugin`'s `android` module — reconstructs the raw `JavaVM`/`jobject` plugins need
    from `ndk-context`-stored handles, one sanctioned module, scoped `AttachGuard` per call.
  - `frust-shared-preferences`'s `apple` backend — two `setObject:forKey:` calls (`objc2`
    marks the untyped Foundation setter unsafe), each `# Safety`-noted.
  - `frust-secure-storage`'s `apple` backend — a confined `as_cf` objc→`CFType` bridge for
    the Keychain/`SecAccessControl` calls, `# Safety`-noted.
  - `frust-clipboard`'s `apple` backend — `UIPasteboard`'s `string`/`setString:`/
    `setItems_options:` calls plus one `extern` static read (`UIPasteboardOptionLocalOnly`), each
    `# Safety`-noted; its `android`/`desktop` backends hold no `unsafe`.
  - `frust-haptics`'s `apple` backend — one `MainThreadMarker::new_unchecked()` proving a
    `dispatch_get_main_queue()` callback runs on the actual main thread; every
    `UI*FeedbackGenerator` call itself is a plain safe binding. Its `android`/`desktop` backends
    hold no `unsafe`.
  - `frust-render`'s `RenderContext::create_pipeline_cache` — one call building a
    `wgpu::PipelineCache` from a shell-persisted, adapter-fingerprint-validated blob (see
    `docs/RENDER_ARCHITECTURE.md`'s `RenderContext` module row).
  - `frust-drive`'s `process` module — a `kill(2)` FFI shim (std exposes no `killpg`) that
    group-kills a streamed child's Unix process group.
  - `frust-camera`'s `apple` backend — AVFoundation message sends behind one `QueueBound<T>`
    `unsafe impl Send/Sync` (serial-queue confinement, one `# Safety` note), and
    `frust_camera_session_handle`'s raw-pointer C export (retain contract **+1** — a +0
    borrow proved unhonourable across the FFI boundary).
  - `frust-native-widgets`'s Android backend — cached `JMethodID` + `call_method_unchecked`
    for hot per-frame property setters, confined to one `# Safety`-documented helper
    (`call_void_cached`, `plugins/native-widgets/src/controls/mod.rs`'s `platform` submodule)
    with a per-call-site note pairing the cached id to its class/signature; cold setters use
    the checked, `jni_sig!`-typed `NativeCtx::call_void` path instead. Its Apple backend
    closes objc2's nullability-unannotated `unsafe` (not a memory-safety claim) behind
    **safe** property wrappers, each `// SAFETY:`-noted at its one call site; the same arm's
    `define_class!`/`extern_protocol!` factory/event-target registration and
    `addTarget:action:` attach/detach are the other confined sites.
  - `frust-iap`'s `apple` backend — one untyped `msg_send![class, shared]` resolving the
    Swift glue's singleton by runtime-only class name (no generated binding for it), plus two
    completion blocks (`RcBlock`) receiving raw `NSString` pointers whose validity only the
    glue's own contract establishes; each site is `# Safety`/`SAFETY`-noted and wraps its body
    in `catch_unwind` per the no-unwind rule below. Its `android` backend holds no `unsafe`
    block at all — only its two JNI exports' `#[unsafe(no_mangle)]` attributes.
- **No unwind across FFI.** Every platform export routes through `frust-shell-common`'s
  `guard` helper (`catch_unwind` + log, returning a benign default) rather than unwinding
  into JVM-/Swift-owned stack frames — a panic crossing the FFI boundary is undefined
  behavior, not a bug. `run_guarded_thread` is `guard`'s whole-thread-body counterpart:
  every render-thread `spawn` closure routes through it, so a caught panic exits cleanly and
  drains any orphaned `Ack` instead of poisoning shared state.
- **State-sync, not op-forwarding, across a mobile IME bridge.** Android/iOS platform text
  input doesn't send individual keystrokes across the FFI boundary — the platform owns
  composition (Gboard, CJK marked text) against a local mirror, then hands the framework a
  whole reconciled `EditingState` and reads one back to keep its own IME machinery
  synchronized — never add a per-keystroke op-forwarding path.
- **UTF-16 at the FFI seam, bytes inside.** `EditingState`'s `selection_*`/`composing_*`
  indices are UTF-16 code-unit indexed everywhere they cross a shell boundary; convert
  to/from byte offsets only inside `frust-text`'s `TextEditor`.
- **Hand-roll JSON at the mobile FFI boundary — no `serde` in shell crates.**
  `frust-shell-android`/`frust-shell-ios` serialize `ImeState` with a small hand-written
  escaper/builder for a handful of fixed fields.
- **Type-erase to avoid a downstream crate dependency**, not to avoid writing a type. When a
  lower layer threads a resource owned by a higher layer, pass it as `&mut dyn Any` and
  recover it at the one call site that knows the concrete type via a documented,
  panic-on-mismatch `downcast_mut::<T>()`.
- **Edition-2024 `-> impl Trait` return types capture all in-scope lifetimes by default.**
  When a function returns an `impl Trait` that borrows nothing from its parameters (e.g.
  `app_logic(&mut State) -> impl View<State>`, where views are `'static`), opt out
  explicitly:

  ```rust
  fn app_logic(state: &mut AppState) -> impl frust::View<AppState> + use<> {
      frust::text(state.greeting.clone()).size(32.0)
  }
  ```

- **A platform value with no published spec is a named constant with a source comment, not a
  bare literal.** Where a platform's exact behavior is private (an iOS gesture's edge zone,
  commit threshold, or fling velocity; an animation's duration/curve), name the constant,
  and its doc comment states whether it's a published value or **community-approximate** (a
  reverse-engineered/community-converged estimate) plus what it's approximating:

  ```rust
  /// Left-edge activation zone width for the interactive pop-swipe (logical px).
  ///
  /// **Community-approximate**: UIKit's edge zone isn't published; ~20dp is
  /// where community reimplementations converge.
  const EDGE_SWIPE_ZONE_DP: f64 = 20.0;
  ```
This keeps the tunable visible in one place and tells a future reader whether "fixing" it
means matching a spec or adjusting a guess.

## Error Handling

- **`anyhow::Result` in binaries and integration-facing library code** (`frust-cli`
  commands, `frust-render`'s public API, `frust-shell-desktop`/`frust`'s `run`), with
  `.context()` / `.with_context()` at each fallible step so the error chain names what was
  being attempted (e.g. `"failed to spawn `{cmd}`"`, `"reading `{path}`"`).
- **`thiserror`-derived enums for errors callers match on**, i.e. where the error is part of
  a library's API contract rather than a leaf failure report:

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
| Platform-view factory `viewType` | Android: any fully-qualified class under `dev.frust.*` (the embedding owns the root package); iOS: bare `@objc(<Name>)` runtime name | `dev.frust.camera.CameraPreviewFactory` / `@objc(MapFactory)` |

JNI export names are LAW: the package/class (`dev.frust.FrustSurfaceView`) is fixed across
every generated app, so the mangled symbol stays stable regardless of the app's own package.
A `platform_view` factory's `viewType` naming is LAW too, widened from
`dev.frust.<Name>Factory`: **any fully-qualified class under `dev.frust.*`** via the app
classloader (Android; `dev.frust.camera.CameraPreviewFactory` is the precedent) or the bare
`@objc(<Name>)` name via `NSClassFromString` (iOS). Both hosts check in the same order —
**prefix (Android) then interface/protocol assignability, before instantiation** — never the
reverse; constructing an untrusted class before the check passes runs its side effects
unchecked.

## Plugin Conventions

Both plugin tiers under `plugins/` (see `docs/ARCHITECTURE.md`'s Doc Map, PLUGINS + NATIVE_WIDGETS rows) follow
these conventions:

- **Backends are `#[cfg]`-gated modules** (`apple`/`android`/`file`) behind one
  platform-independent public API; FFI deps are target-gated in the crate's `Cargo.toml`,
  never unconditional.
- **Errors are `thiserror` enums callers match on** (`PrefsError`, `PlatformHandleError`),
  per the Error Handling rule above.
- **A JNI attach is scoped per call, never permanent** — threads don't auto-detach on exit,
  so `attach_permanently` leaks the attachment; use `frust-plugin::android::with_jni_env`'s
  scoped `AttachGuard`.
- **No panics/unwinds near an FFI boundary**, the same rule as shell exports (Language
  Idioms, above).
- **Platform plugins never depend on `frust-*` framework crates** (`frust-plugin` +
  `frust-paths` + FFI crates only); **facade plugins depend on `frust` alone**. A plugin
  needing both splits into a platform-core plus facade-glue crate — **or** ships as one
  crate with a default-on `frust-api` feature gating the optional
  `frust`/`frust-core`/`frust-theme` deps (`frust-native-widgets`'s shape), so `cargo check
  -p <crate> --no-default-features` still resolves to the platform-plugin charter line.
  Splitting would have left an almost-empty platform-core crate here; the charter line is
  what actually matters, and it is mechanically checkable either way (`cargo tree -p <crate>
  --no-default-features -e normal`). `frust-native-widgets` needs `frust-core`/`kurbo`
  directly alongside `frust` (not the facade alone) because its builders hand-implement
  `View<Outer>`/`Widget` in the plugin's own crate — the plugin tier's sanctioned, permanent
  exception to the app-tier facade-first rule (State & Reactivity Conventions below);
  plugins sit beside the facade, never inside it (`docs/ARCHITECTURE.md`'s facade/plugin
  boundary), and are not expected to migrate onto `frust::authoring`.
- **A store shared with the OS namespaces its keys `frust.`** (NSUserDefaults, Android
  SharedPreferences) so plugin keys can't collide with other libraries'.
- **`dev.frust` is the embedding module's exclusive package; a plugin's Android Kotlin ships
  as a subpackage inside its own Gradle module (`plugins/<name>/platform/android`, e.g.
  `dev.frust.securestorage`), wired by `Contribution::GradleModule` — never rendered from a
  template or copied into the app.** AGP's `namespace` scopes generated `R`/`BuildConfig`
  per module (a plugin's namespace must differ from `dev.frust`), and Kotlin's `internal` is
  module-scoped (an `internal` embedding type stays invisible to a plugin module even under
  the same source package) — two confirmed side-constraints this rule is built around.
- **A plugin's theme-token folding pins a representative subset, never chases full
  design-token fidelity.** `frust-native-widgets`' theme ladder resolves a fixed, documented
  mapping of `Theme` roles into each control's props — no elevation, motion, glass, or
  per-state (hover/pressed/disabled) variants, which the platform's own drawables already
  provide for free; widening the mapping is additive, never a breaking republish.
- **A platform capability gap is recorded per-platform, never "corrected" onto the platform
  that doesn't have it.** `frust-native-widgets`' theme-ladder L1 (brightness) is
  deliberately asymmetric: Android bakes it at control-construction time
  (`createConfigurationContext` yields a `Context` consumed once, so a live brightness
  toggle needs a rebuilt view), while iOS re-pins `overrideUserInterfaceStyle` on every
  `update` and re-themes live. Mirroring one platform's constraint onto the other is a
  real defect class — match each platform's own capability, not its sibling's.
- **A native listener callback is wrapped into a plain signal-writing closure, not routed
  through frust's event machinery.** `frust-native-widgets`' events-as-signals convention
  (`crate::api::signals`) decodes each control's raw platform callback (Android JNI, iOS
  target-action) into the closure shape a builder's `.on_press`/`.on_toggle`/`.on_change`
  takes; since that callback runs on the platform main thread — the same thread the rest of
  frust runs on — an app closure just writes an `RwSignal` (`move |v| sig.set(v)`) and wakes
  the next frame normally, with no `TrackedScope`/`EventCtx` involved.
- **A generated-project mutation is idempotent, never a blind overwrite.** Adding an OS-side
  contribution (a Cargo dependency, a manifest permission, an Info.plist key, or a Gradle
  module include) checks first and no-ops if already present — the contract
  `frust-drive::plugin::add_plugin` implements (see `docs/CLI_ARCHITECTURE.md`'s plugin-add data flow).
- **A call that blocks pairs with `spawn_blocking`, and every blockable path is
  UI-thread-guarded.** A gated/platform-answer call (secure-storage's biometric prompt;
  camera's `request_permission`/`take_picture`) fails fast with a typed error
  (`CameraError::UiThread`) on the platform's UI thread instead of parking there. A call
  that answers without waiting (e.g. an already-decided permission status) is exempt and
  stays callable from anywhere.

## Platform-View Conventions

- **Mode B is a build-time host configuration, never an app Rust opt-in.** The embedding
  module's overridable `translucentSurface` seam (Android: `FrustSurfaceView`'s constructor
  parameter, fed by `FrustActivity`'s `open val`; iOS: `FrustViewController`'s `open var` —
  different override mechanisms per platform, not a symmetric API) must drive the window
  pixel format, the host's native-sibling z-order, and the
  `declare_host_translucent_surface` call *together*, in the same branch — splitting them is
  the exact defect that once shipped black rectangles. App Rust has no matching call; only the two shells' own
  FFI-glue may declare it (see `docs/ARCHITECTURE.md`'s Platform-view flow).
- **Mode B paint contract: an unpainted region is a window, not a compositor bug.**
  `platform_view` punches its own slot rect automatically; any other chrome region left
  unpainted by a Mode B host shows raw OS content behind the frust surface — pair
  translucency with an explicit opaque app-root background (the catalog's `AppBackground`
  precedent).
- **A `platform_view` slot never receives `Widget::event` (v1).** Native-view input is
  OS-routed through the host's own view hierarchy, not `EventCtx` — there is no
  hit-test/dispatch seam for a hosted view; don't add pointer handling to
  `PlatformViewWidget`.

## TUI Conventions

- **`ui` renders `&AppState`, never mutates the engine** — all state changes happen in
  `engine::update`; a render fn taking `&mut` state is a layering violation.
- **Every mouse action has keyboard parity** (e.g. the welcome Create button: `Enter`/`c`) —
  mouse support is additive, never the sole path.
- **Commands/keybindings have one registry**, `engine::palette::commands` — the palette and
  help overlay both render from it; never a parallel list.
- **fdemon is a pattern source, not a copy source** — it is BSL-1.1 licensed; study its
  patterns but never copy a file verbatim.

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
Every external tool invocation (`rustc`, `adb`, `xcrun`, `cargo ndk`, …) goes through
`ProcessRunner`, so `doctor`/`devices` logic tests via `FakeProcessRunner`.

### Leaking `vello`/`wgpu` types outside `frust-render`

**BAD:** a `frust-scene` or `frust-text` public function taking or returning a
`vello::*`/`wgpu::*` type.

**GOOD:** public APIs above `frust-render` speak only `kurbo`/`peniko`; this is what lets
the GPU backend be swapped later without touching widget or text code.

**The one named exception:** `frust-render::DetachedSurface` is a deliberate **opaque**
escape valve, not a leak — its only accessor, `into_surface`, is crate-private, so the
wrapped `wgpu::Surface` is never nameable outside `frust-render`; follow the same pattern
for any future value crossing this boundary.

### Printing directly from a `frust-drive` build/run core

**BAD:** a `println!`/`print!` inside `android_build`/`ios_build`/ `android_run`/`ios_run` —
reaches the caller's stdout unconditionally, garbling a TUI session's raw-mode terminal with
raw pipeline output.

**GOOD:** thread an `on_line: &mut dyn FnMut(&str)` sink through the core instead — the CLI
passes `&mut |line| println!("{line}")`, the TUI routes it into a session's log tab.
`frust-drive/tests/print_free_cores.rs` (a source-scan conformance test) enforces this
against every drive core outside a small CLI-entry allowlist.

## Interaction Semantics

Conventions for `Widget::event` implementations, followed by every interactive
widget in `frust-widgets`:

- **Fire on up-inside, not down.** A press captures the pointer on `Down` and tracks a
  `pressed` visual on `Move`, but the callback fires only on `Up` and only if the release
  lands inside the widget's bounds; `Cancel` (platform gesture steal) clears the pressed
  state without firing:

  ```rust
  PointerPhase::Up => {
      if inside(p.position, ctx.size()) { (self.on_press)(ctx); }
      self.pressed = false;
  }
  ```

- **Controlled components never self-mutate.** `Checkbox`/`Slider` report the *requested*
  value through `on_toggle`/`on_change` and leave `checked`/`value` untouched until the next
  `rebuild` feeds the app-confirmed value back down — never flip `self.checked` inline in a
  handler. `TextInput` is controlled too: `rebuild` applies the view's `value`
  set-if-different, so an app that rejects/transforms input in `on_change` sees its own
  value win next frame.

- **A container checks `InputEvent::is_broadcast()` before anything else.** A broadcast
  (`InputEvent::Housekeeping`) is not user input: every routing helper forwards it to *every*
  child unconditionally, ahead of the capture/focus/hit-test branches below, and always reports
  `Ignored` regardless of what children returned — a broadcast is never consumed and never
  short-circuited by a captured or focused child (`docs/CORE_ARCHITECTURE.md`'s event-routing data
  flow).

- **Focus routes by recorded path, like capture; `Key`/`Ime` events never hit-test.** A
  container simply forwards `Key`/`Ime` events to its focused child; a `Down` that doesn't
  (re)claim focus on the child it hits blurs the chain. **A structural container rebuild
  clears capture and focus only where identity is actually lost** —
  stable-prefix/key-matched, not a blanket clear. A reconciler that tears down (or type-swaps) a
  focused pod cannot reach `RenderRoot` itself (no handle inside a `BuildCtx` pass), so it marks the
  pod orphaned instead, but only *on the live focus chain* — gate the mark on
  `ctx.has_focus() && pod.is_focused()`, never on the recorded `focused` flag alone, since a
  hand-rolled container's own teardown/type-swap path can hit a stale flag under an
  already-blurred ancestor; `RenderRoot`'s cached `focus_active`/`ime_state` release on the same
  rebuild that raised the mark, not merely "eventually" on a later event pass
  (`docs/CORE_ARCHITECTURE.md`'s Focus/IME Lifecycle). **The same gate binds a container
  that publishes a cleared IME surface** (the navigator's `needs_ime_clear` producers,
  `PatternSwitcher`'s): an inactive publish *is* a session release, so raise it only for an
  outgoing/covered subtree that was itself on the live chain — on a push the outgoing pod is
  the page being **covered**. Two severing paths are known to be **uncovered** and are
  registered rather than fixed: a type swap through a doubly-erased pod, which no reconciler
  can observe (`focus-double-erasure-swap-blind`), and the hand-rolled navbar/tabbar item
  lists, which never clear or mark a truncated item's own focus link
  (`focus-navbar-item-truncation-unmarked`). See `docs/LIMITATIONS.md`.

- **Keyed lists are all-or-nothing, and keys must be unique.** `keyed(key, view)` marks a
  `Flex` child list for identity-based reconciliation; a mixed or duplicate key set
  `debug_assert!`s and falls back to positional matching in release (never panics live). A
  matched reorder relocates the existing widget rather than rebuilding it.

- **Input constants have one source.** Gesture thresholds (`TOUCH_SLOP`, `MOUSE_SLOP`),
  scroll/fling tuning (`WHEEL_LINE_PX`, `FLING_DECAY`, `FLING_STOP`, `VELOCITY_WINDOW_MS`),
  and `VelocityTracker` live in `frust-core::input`; widgets import them rather than
  hardcoding a local threshold.

- **Events are logical-coordinate by the time they cross `AppTree`.** Every platform
  boundary (winit, JNI `nativeOnTouch`, the C `frust_dispatch_touch`) converts to
  density-independent logical pixels before building an `InputEvent` — widget/container code
  never divides by scale factor; only the shell's FFI-boundary helpers do.

- **Insets follow the same physical-at-FFI, logical-inside rule as coordinates.** A platform
  delivers occlusion in physical px (Android `Insets`) or already-logical points (iOS
  `safeAreaInsets`); either way the FFI-boundary `logical_insets` helper reconciles it —
  widget code only ever sees a resolved `WindowInsets` in logical px
  (`docs/SHELLS_ARCHITECTURE.md`'s cross-cutting host-signal flow).

- **A `Cancel` arm must never call `EventCtx::state_mut`.** It may only clear internal flags
  (`self.pressed`/`self.captured`/`self.armed`) and request a redraw. A structural container
  rebuild can synthesize a `Cancel` to a still-captured child delivered over a throwaway
  `()` state (`docs/CORE_ARCHITECTURE.md`'s event-routing data flow) — a handler reaching for real state
  there panics on the `()` downcast, a deliberate tripwire.

- **A container that suppresses routing to its children must cancel their capture, clear
  their focus, and publish a cleared IME surface — in that order, with no bypass.** This
  binds any container that stops forwarding events to an already-interactive child (the
  navigator's mid-transition input block is the reference impl). Skipping this leaves a
  child armed or a stale focus/IME surface behind after the block lifts. Publishing a cleared
  surface here means the same thing it means everywhere: a full root focus/IME session release,
  not a value update (`docs/CORE_ARCHITECTURE.md`'s Focus/IME Lifecycle).

## Semantics Conventions

Conventions for `Widget::semantics` (see `docs/CORE_ARCHITECTURE.md`'s `semantics` module):

- **The method defaults to a no-op.** Only override it if the widget has a role/label/state
  worth reporting; a widget with nothing to say about itself needs no impl at all.
- **A container MUST forward to every child via `ChildPod::semantics_child`**, never by
  calling a child's `semantics` directly. Skipping a child here silently drops its whole
  subtree from the accessibility tree with no compile-time or test signal, so every new
  container widget needs a `semantics` impl even a transparent one that just forwards
  (`self.child.semantics_child(ctx)`).
- **The one carve-out: a container that gates input may forward only the children input
  can reach.** The navigator (rule R23) contributes no node of its own and forwards
  *exactly* the pages `NavigatorWidget::event` would route to — one shared
  `input_routed_pages()`, so the two reaches cannot drift — omitting covered pages and the
  page under a modal entirely rather than flagging them `hidden` (which needs a synthetic
  wrapper node and publishes the stale bounds of a page `layout` skipped). The
  justification is **input parity**, not "it isn't painted": offering a screen reader a
  control the user cannot activate is worse than omitting it. A container that does *not*
  gate input has no such exception — forward every child.
- **Keep it minimal: role, label, state, and bounds only.** This gives the per-shell adapter
  just enough to build on — no live-region announcements, custom actions, or adapter wiring
  belong here; that integration lives in a shell, not `frust-core`.
- **A platform adapter gates its pushes on `semantics_generation`, not on pushing every
  frame unconditionally.** All three shipping adapters compare the last-seen generation
  before rebuilding a `TreeUpdate`.

## Instrumentation & Frame-Gate Conventions

- **Perf recording is always gated behind `perf::enabled()`, never unconditional.**
  `frust-shell-common::perf`'s `FrameStats`/`StartupSpans` no-op internally when disabled,
  but a shell should still read `perf::enabled()` once per frame into a local bool and gate
  every `Instant::now()` read behind it (`bool::then(Instant::now)`) on FFI-sensitive paths
  (Android/iOS) so a disabled build takes zero clock reads. Span names are `perf::SPAN_*`
  consts, not string literals.
- **Frame-gate inputs default to must-run, never to skip.** A `FrameInputs` field with no
  precise signal should stay `true`/fed conservatively rather than guessed `false` —
  over-running costs a wasted frame, over-skipping drops real work (see
  `docs/SHELLS_ARCHITECTURE.md`'s `frame_gate` module). `FRUST_NO_FRAME_GATE=1` (parsed like `FRUST_TRACE`)
  forces every tick to `Run`; reach for it first when diagnosing a suspected stuck-UI
  report.

## State & Reactivity Conventions

- **Local state lives in the retained `Component` element, not signals, unless it needs
  cross-tree reactivity.** `Component::State` is plain data `ComponentWidget` retains across
  rebuilds — reach for an `RwSignal` field only when something outside the component's own
  `build` (a background task, a sibling, a nested component) needs to observe a write.
- **Teardown disposes the component's `Owner`; register cleanup via `on_cleanup`, not
  `Drop`.** `ComponentWidget::teardown` tears down the child element, then disposes the
  owner (running every `on_cleanup` registered under it since `init`) before dropping
  `State`. Register disposal (timers, controller/subscription teardown) with `on_cleanup`
  inside `init`/`build` — never hand-roll a `Drop` impl on `State`; `Drop` order across the
  state/element/owner triple is not a contract, `on_cleanup` is.
- **A `Cancel` arm still never touches state, even the component's own** — the
  Cancel-never-mutates-state rule above binds every handler a component hosts, including one
  reading `ComponentWidget`'s own `State` (a synthesized `Cancel` crossing a component
  boundary carries real state, unlike the throwaway `()` at the root, but the contract is
  identical).
- **Heavy work routes by shape: `spawn` (async IO) / `spawn_local` (UI-thread `!Send`) /
  `spawn_blocking` (one-off CPU) / rayon (an app-level choice, not bundled).** Calling
  `spawn_local` off the UI thread panics; a backgrounded iOS app pauses `CADisplayLink`, so
  a `spawn_local` timer stalls until `frust_resume`'s next pump. `use_task` composes
  `AsyncValue<T>` over this routing: a UI-thread coordinator hands work to
  `spawn`/`spawn_blocking` and is the sole signal writer; owner cleanup aborts it and its
  `JoinHandle`.
- **`Component::State` holds `RwSignal`s directly; app code depends on the `frust` facade
  only, never `reactive_graph`/`any_spawner`/`frust-reactive` directly.** A reactive field
  is typed `RwSignal<T>`, read/written through the facade's `Get`/`Set`/`Update` traits.
- **An app authors a custom `View`/`Widget` pair through `frust::authoring`, never a direct
  `frust-core`/`frust-scene`/`frust-text`/`accesskit`/`kurbo`/`peniko` dependency.**
  `frust::authoring` (plus its `text`/`scene` submodules) re-exports the full trait
  lifecycle, child/event/callback plumbing, semantics, and geometry/paint types a custom
  widget needs, so an `examples/*`/app crate's `Cargo.toml` depends on `frust` plus plugin
  crates only. For the long tail, reach through the whole-crate valves `frust::kurbo`,
  `frust::peniko`, `frust::accesskit` rather than re-declaring the dependency — each of
  those crates is version-pinned in exactly one place (`docs/DEVELOPMENT.md` §
  Version-Pin Policy). Mechanically enforced
  across `benchmarks/frust_bench` and the four in-repo example apps by
  `crates/frust/tests/authoring_seam_conformance.rs`; the plugin tier is exempt (Plugin
  Conventions above).
- **A rebuild must run inside a `TrackedScope` for a signal write to wake it later — an
  untracked read is a silent wake hazard, not a stale value.** `.get()` subscribes only from
  *inside* a live `TrackedScope::track` closure; both shells guarantee this for their
  per-frame rebuild. A render-relevant read taken via `*_untracked`/`get_untracked` anywhere
  in that path never subscribes, so a later write flips no dirty flag and the shell may
  never repaint — reserve `*_untracked` for genuine non-rendering reads, never a value a
  `build` return depends on.

## Theming & Animation Conventions

- **Paint-time resolution is always safe; layout-time-baked resolution is only safe under
  the `set_theme` → `ChangeFlags` contract.** Most themed widgets resolve tokens from
  `PaintCtx` every paint pass and self-refresh on a live theme swap for free.
  `Text`/`TextInput` instead bake resolved glyph color into the shaped layout at LAYOUT
  time; a widget adding layout-time-baked resolution depends on relayout actually happening,
  so treat a theme change as forcing `ChangeFlags::LAYOUT`, not just `PAINT`.
- **Resolve theme tokens with an unthemed-fallback constant per resolved value.** A themed
  widget looks up `Theme::from_paint_ctx(ctx)`/`from_layout_ctx(ctx)`, falling back to a
  local constant (e.g. `Button`'s `FILL`/`RADIUS`) when no theme is threaded. Precedence is
  **explicit builder value > theme > fallback** (see `Text`'s `color_explicit` flag).
- **Token-not-hardcode: a widget authors against a `Theme` field first; a bare local
  constant is the documented fallback, not the default.** A hardcoded metric/color is a
  defect once a matching `ColorScheme`/`ShapeScale`/`Elevation`/`GlassScale`/`MotionScheme`
  field exists. Only a genuine token-scale gap earns a hand-tuned constant, and it stays
  named, doc-commented, and states *why* no token applies.
- **A contested or unsourced design fact is resolved against a primary source and cited with
  a retrieval date, not left as a guess** — record `<source>, retrieved <date>` in the
  module doc, alongside the **Community-approximate** marker (above) for values that stay
  genuinely unsourced.
- **Event-pass code never reads a theme — `EventCtx` carries none.** Only
  `LayoutCtx`/`PaintCtx` thread a theme; a metric an event handler also needs stays a plain
  constant read from both passes (`TextInput`'s `PAD_X`/`PAD_Y`/`CARET_W` precedent).
- **No `Instant::now()` in `frust-core`/`frust-widgets`.** Time enters the framework only
  from a shell, as the `FrameTime` passed into `RenderRoot::paint`/`PaintCtx::frame_time` —
  desktop reads its own `Instant` epoch; Android/iOS pass through the platform's frame
  clock. Widget code only *differences* two `FrameTime`s, never reads a wall clock directly.
- **An animation controller advances during paint, not on a timer.** A widget holds an
  `anim::AnimationController`, calls `advance(ctx.frame_time())` once per paint and, while
  it returns `true`, calls `PaintCtx::request_frame()`. **A layout-affecting animation calls
  `request_layout()` instead** (implies `request_frame`) — reserve bare `request_frame` for
  a paint-only animation, or the mobile intra-frame layout skip leaves it unresized
  (`docs/SHELLS_ARCHITECTURE.md`'s frame-pipeline data flow).
- **State-layer opacity has one source: `material::state_layer`'s constants**
  (`HOVER_OPACITY`/`FOCUS_OPACITY`/`PRESSED_OPACITY`/ `DRAGGED_OPACITY`, M3 `StateTokens`) —
  a catalog widget imports them rather than hardcoding overlay opacity, taking the
  **maximum** of concurrently-active states, never their sum.
- **Glyph's token set adds three resolution precedents.** A per-status color with no
  `ColorScheme` field (Success/Warning/Info) resolves `Theme::extension::<StatusPalette>()`
  first, before a role that already has one (Error) resolves it directly. `GlyphInk` is
  never brightness-swapped like a scheme role. Accent role split: `primary`/`on_primary` is
  accent text/icon ink, `primary_container`/`on_primary_container` is the bright fill —
  conflating the two is the catalog's most common accent bug.
- **A transition pattern's default timing resolves from `Theme.motion`, never a hand-rolled
  duration, and collapses under `reduce_motion`.**
  `PatternSwitcher`/`AnimatedOpacity`/`AnimatedScale` resolve `MotionScheme`'s
  duration/easing/spring tokens by default (an explicit `.timing(...)` call always wins);
  every pattern substitutes a short linear crossfade under `reduce_motion` instead of a
  bespoke variant.
- **A perpetual decorative loop calls `PaintCtx::request_frame_paced`
  (`TickClass::CosmeticLoop`), never bare `request_frame`.** `request_frame` stays
  `TickClass::Transition` (unpaced) — correct for a spring or any transition with a
  user-visible endpoint. A shimmer/spinner/pulse with no endpoint requests the paced class
  instead, letting the mobile frame gate throttle it to `MotionScheme::cosmetic_loop_rate`,
  and must still honor `reduce_motion` (freeze in place, stop requesting frames).
  **Input-driven frames are never paced.** A loop far slower than the cap (a ~500ms caret
  blink against a 30Hz shimmer) names its own cadence with `request_frame_paced_at(interval)`
  instead of the bare call — still `CosmeticLoop`-classified and still `reduce_motion`-honoring,
  just at an explicit interval rather than the theme's default rate. `TextInput`'s caret is the
  shipped example, with one deliberate exception to the freeze-in-place rule above: it freezes
  **visible** rather than hidden (a position cue must stay legible) while still dropping all
  frame requests when frozen.
- **Design-system code targets `frust_widgets::authoring`, never a catalog module.** A baseline
  widget never imports `material`/`cupertino`/`glyph`; the container/callback plumbing, event
  routing, and callback erasure every widget needs live in the public `authoring` module
  instead — the same surface the three built-in, feature-gated catalogs themselves consume,
  pinned `authoring`-only by `authoring_only_conformance.rs`; `PRESSED_OPACITY`'s
  `material::state_layer` re-export is compatibility-only. `PageTransition::Custom` needs an
  explicit `Timing::Duration`/`Timing::Spring` (`Timing::ThemeDefault` falls back to the M3
  default, 300ms + `Curve::Emphasized`); `reduce_motion` collapses it only programmatically,
  and an interactive edge-swipe pop calls it like every preset.
- **A design system installs itself via `set_default_theme` + `register_app_fonts` from an
  `app!` `setup` block — never `Component::init` (no kept ordering contract) or
  `set_app_theme` (pins brightness, breaking platform dark/light following).**
  `frust::glyph_theme::install()` is the built-in Glyph caller.

## Testing Patterns

- **Fixture-driven tests for parsers/validators**: register canned
  `ProcessRunner`/`EnvLookup` responses keyed by the exact invocation, then assert the
  resulting `Status`/`Validation`/`DoctorReport`.
- **Injectable hook seams for process-global side effects**: a function installing a real
  handler in production (`ctrlc::set_handler`, a filesystem watcher) takes a small `Hooks`
  struct defaulted to the real installers, with a `::fake()` (`#[cfg(test)]`) no-op pair a
  test injects instead — exercising dispatch logic without installing a real process-wide
  handler.
- **`#[ignore = "<reason>"]` for GPU-dependent or slow end-to-end tests.** The reason string
  must say how to run it (`cargo test -p ... --ignored`) and why it's excluded by default
  (needs a real GPU; compiles a full generated dependency graph; etc.).
- **Recording fakes for paint assertions**: a minimal `PaintScene` impl pushing `(origin,
  size)`/`(origin, text)` tuples into `Vec`s lets widget `layout`/`paint` behavior be
  asserted without any GPU dependency.
- **Template rendering uses `minijinja::UndefinedBehavior::Strict`**: an unresolved `{{
  placeholder }}` is a hard render-time error, catching template/context drift in tests
  instead of a generated project.

## Comment Conventions

- **A comment documents the code as it is — its contract, its rationale — never the
  development process that produced it.** Banned in comments: plan/phase references
  (`Phase 9.B step 1`, `(phase 5.5)`), plan-task references (`task-12`), workflow findings
  ledger numbers (`FINDINGS #43`), review-round/fix-round references (`re-review round 1`,
  `cfix-2`), plan requirement numbers (`req 4`), and internal PR numbers. Git history and the
  private workflow ledger own that process context; a ledger number in a comment is a
  dangling reference for every reader without that private repo.
- **Sanctioned citations** stay fine: `docs/LIMITATIONS.md` stable ids (e.g.
  `` `cam-blit-opaque` ``) — the register's documented purpose; named semantic rules (`R23`,
  `R44-back`) — they name behavior, not a ledger entry; pointers to doc sections; and commit
  SHAs/version pins of *external* repos (e.g. clean-signals' `910f626`).
- **Substance over ledger number.** When the process context carried real meaning, keep the
  substance and drop the number: `// re-created finding #44` becomes `// re-created the
  root-modal double-claim bug`. If a `docs/LIMITATIONS.md` entry covers it, cite its stable id
  instead.
- **Domain vocabulary is not residue.** A render lifecycle's encode/present phases, an
  animation/oscillator's phase, a device name in a benchmark note, and a doc comment
  referring to its own numbered steps are all legitimate — the ban targets
  development-process phases/rounds, not the word itself.
- **Header budget scales with the module.** A simple module gets a 1–3 line header; a complex
  module documents contract + rationale only — history lives in git, and content a unit spoke
  doc already owns gets one pointer there, not a restatement. Inline comments state
  constraints the code can't show on its own (a `# Safety` contract, a magic-number source).
- **Mechanically checked.** `crates/frust/tests/comment_residue_conformance.rs` catches
  plan-phase (dotted `9.B`, parenthesized, hyphenated `Phase-N`, "the Phase N"), plan-task
  (`task-NN`), findings-ledger numbers, review-round (`re-review`, `cfix-N`, gated `round-N`),
  plan-document (`PLAN <tag>`, `workflow/plans/`), phase-adjacent `req N`, and `review finding
  <id>` refs — sanctioned citations (LIMITATIONS ids, R-rules, external rev pins) exempt only
  their match span, not the line. Bare internal PR numbers (vs. upstream wgpu's `#7057`) and
  bare plan tags (`T04`/`D6a` vs. `M3`/`R8`) stay human-reviewed — still banned, swept on sight
  when review finds them.
