# Frust - Code Standards

The shared conventions every crate in the workspace follows. Three unit-specific rule sets hang
off this index — read this plus the one that covers what you are touching:

| Unit | Spoke | Holds |
|------|-------|-------|
| PLUGINS + NATIVE_WIDGETS | [PLUGINS_CODE_STANDARDS.md](PLUGINS_CODE_STANDARDS.md) | Plugin Conventions — backend gating, JNI attach scope, the platform/facade charter lines, theme-token folding, idempotent project mutation |
| WIDGETS | [WIDGETS_CODE_STANDARDS.md](WIDGETS_CODE_STANDARDS.md) | Theming & Animation Conventions — token resolution/precedence, animation pacing, design-system installation |
| TUI | [TUI_CODE_STANDARDS.md](TUI_CODE_STANDARDS.md) | TUI Conventions — render/update layering, keyboard parity, the single command registry |

## Language Idioms

- **`unsafe` is confined to a small set of sanctioned platform-FFI boundaries.** Every other
  crate (`frust-core`, `frust-scene`, `frust-text`, `frust-widgets`, `frust-shell-common`,
  `frust-shell-desktop`, `frust-shell-linux`, `frust-shell-web`, `frust`) stays `unsafe`-free —
  where Masonry/xilem-style code would reach for `unsafe` downcasting, use trait upcasting
  instead: bound a trait on `Any` (e.g. `Widget: Any`) and downcast through `&mut dyn Any`.
  `frust-shell-web`'s frame waker in particular needs no sanctioned zone of its own: it is a
  capture-nothing closure over a `thread_local` `EventLoopProxy` slot
  (`crates/frust-shell-web/src/app_handler.rs`'s `install_wake_proxy`/`WAKE_PROXY`), chosen over
  the Phase-0 probe's `unsafe impl Send + Sync` precedent. The
  sanctioned zones are raw-pointer boundaries a GPU/platform shell cannot avoid, each isolated
  in one function/module with a `# Safety` doc comment stating the caller contract:
  - `frust-gpu`'s `create_android_surface`/`create_metal_surface` (`lifecycle.rs`) — turn a
    caller-owned raw `ANativeWindow*`/`CAMetalLayer*` into a `wgpu::Surface`, each
    `# Safety`-noted. `frust-render`'s `on_surface_created_from_android_window`/
    `on_surface_created_from_metal_layer` (`renderer.rs`) are the entry points a shell calls;
    their own `unsafe` is confined to forwarding the raw pointer into the `frust-gpu` pair above.
  - `frust-shell-android`'s `jni_glue` module — the JNI FFI boundary (`extern "system"`
    exports, `Box::into_raw`/`from_raw`, `ANativeWindow_fromSurface`, `nativeInitPlatform`'s
    `JavaVM` stash), `android_app!`'s generated exports, and the render-thread split's
    `unsafe impl Send` for `SendableWindowPtr` plus a bare `libc::setpriority` self-boost.
  - `frust-shell-ios`'s `ffi_glue` module — the C-ABI FFI boundary (`extern "C"` exports,
    `Box::into_raw`/`from_raw`, the call into `on_surface_created_from_metal_layer`),
    `ios_app!`'s generated exports, and the split's `unsafe impl Send` for
    `SendableMetalLayer` plus a bare `libc::pthread_set_qos_class_self_np` self-boost.
  - `frust-shell-macos`'s `appkit_glue` module — four sites: the whole `define_class!` block
    counted as one (its `#[unsafe(super(NSObject))]`/`#[unsafe(method(…))]` attributes
    declaring the reopen-notification observer class, AND the `unsafe impl NSObjectProtocol`
    conformance it also carries, share the block's one `SAFETY:` note at the macro head),
    `msg_send![super(this), init]` (`NSObject`'s designated initializer),
    `NSNotificationCenter::addObserver_selector_name_object` plus the
    `NSApplicationDidBecomeActiveNotification` `extern` static read that registers it, and the
    matching `removeObserver` in `Drop`. Each is `SAFETY`-noted.
  - `frust-shell-windows`'s `win32_glue` module — three sites:
    `SetCurrentProcessExplicitAppUserModelID` (taskbar identity), the `TranslateAcceleratorW`
    call inside the menu-accelerator message hook, and muda's `Menu::init_for_hwnd` attaching
    the native menu bar to a live `HWND`. Each is `SAFETY`-noted; `frust-shell-linux` holds no
    `unsafe` at all.
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
  - `frust-gpu`'s `RenderContext::create_pipeline_cache` — one call building a
    `wgpu::PipelineCache` from a shell-persisted, adapter-fingerprint-validated blob (see
    `docs/RENDER_ARCHITECTURE.md`'s `frust-gpu::context` module row).
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
  drains any orphaned `Ack` instead of poisoning shared state. `frust-shell-macos` and
  `frust-shell-windows` depend on neither `frust-shell-common` nor each other, so their
  native-callback boundaries can't route through `guard`; each applies the same contract
  locally instead — `appkit_glue`'s AppKit notification callback hand-rolls its own
  `catch_unwind` around the observer body. The one exception is muda's menu-event handler
  (shared by both crates' `menu` modules): it is a native-dispatch boundary held panic-free
  by construction rather than by `catch_unwind` — the callback body is an infallible
  mutex-guarded queue push plus a redraw request, nothing that can panic.
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

Both plugin tiers under `plugins/` carry additional conventions of their own —
[PLUGINS_CODE_STANDARDS.md](PLUGINS_CODE_STANDARDS.md).

## Platform-View Conventions

- **Mode B is a build-time host configuration, never an app Rust opt-in, and Android/iOS only.** The
  embedding module's overridable `translucentSurface` seam (Android: `FrustSurfaceView`'s constructor
  parameter, fed by `FrustActivity`'s `open val`; iOS: `FrustViewController`'s `open var` — per-platform
  mechanisms, not a symmetric API) must drive the window pixel format, the host's native-sibling z-order
  and the `declare_host_translucent_surface` call *together*, in the same branch — splitting them is the
  exact defect that once shipped black rectangles. App Rust has no matching call; only the two mobile
  shells' own FFI glue may declare it (see `docs/ARCHITECTURE.md`'s Platform-view flow).
- **A desktop host is Mode A: `DesktopExtensions`' platform-view hooks, never an FFI poller.** It
  resolves a slot's `view_type` through `frust_plugin::desktop` rather than naming a plugin crate, leaves
  rects and clips in **logical points** (no physical conversion, unlike the mobile FFI boundary), and
  treats `on_platform_views_suspended` as remove-not-hide — the surface-recreate replay is the way back.
- **Mode B paint contract: an unpainted region is a window, not a compositor bug.** `platform_view`
  punches its own slot rect automatically; any other chrome region a Mode B host leaves unpainted shows
  raw OS content — pair translucency with an opaque app-root background (the `AppBackground` precedent).
- **A `platform_view` slot never receives `Widget::event` (v1).** Native-view input is OS-routed
  through the host's own view hierarchy, not `EventCtx` — there is no hit-test/dispatch seam for a
  hosted view; don't add pointer handling to `PlatformViewWidget`.

## GPU / Render-Engine Rules

Downlevel design rules (E1-E18, `crates/frust-gpu/src/lint.rs`) bind `frust-gpu`/`frust-engine`
— the frust-owned strip pipeline `frust-render` always contains, not a cargo feature. Four bind
every change to either crate:

- **WGSL lives in a `.wgsl` file under `crates/frust-engine/shaders/`, reached via
  `include_str!` — never an inline string literal.** The downlevel lint
  (`frust_gpu::lint::lint_wgsl_dir`) only scans that directory; an inline shader string
  compiles but escapes the scan unseen.
- **No compute shaders, no storage buffers/textures, anywhere on the frame path (E1/E2).**
  The tier's whole downlevel posture assumes a WebGL2/GLES3.0 ceiling that offers neither —
  even a present-side format conversion (`frust-engine::gpu::present::UnpremultiplyPass`) is an
  ordinary `RENDER_ATTACHMENT` fragment write, never a compute dispatch.
- **Every draw pipeline blends and writes premultiplied alpha; convert only at present, never
  in a draw pass.** A pass emitting straight alpha belongs in `gpu::present` alone — converting
  anywhere else double-corrects a Metal `PostMultiplied` swapchain (`docs/LIMITATIONS.md`'s
  `engine-metal-postmultiplied-truth-bug`).
- **A render-path error returns `EngineError`; it never panics (E17).** Every module on the
  frame path — compile, schedule, cache, filters, the renderer itself — treats a refused frame
  as a value the caller skips cleanly, never an `unwrap`/`expect`/`panic!` (a source-scan test
  greps `cache/`, `compile/`, `filters/`, `gpu/`, `text/`, `schedule/`, and `renderer.rs` for it).

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

### Leaking `wgpu` types outside `frust-render`/`frust-gpu`/`frust-engine`

**BAD:** a `frust-scene` or `frust-text` public function taking or returning a `wgpu::*` type.

**GOOD:** public APIs above `frust-render` speak only `kurbo`/`peniko`; this is what lets
the GPU backend be swapped later without touching widget or text code.

**The one named exception:** `frust-render::DetachedSurface` is a deliberate **opaque**
escape valve, not a leak — its only accessor, `into_surface`, is crate-private, so the
wrapped `wgpu::Surface` is never nameable outside `frust-render`; follow the same pattern
for any future value crossing this boundary.

### Reaching into a design-system plugin's internals from a design-system crate

**BAD:** a `crates/frust-drive/templates/design-system/`-derived crate's `Cargo.toml` declaring a dependency on
`frust-glyph`/`frust-material`/`frust-cupertino` to reach one convenience symbol, or depending on
`frust-widgets`/`frust-core` directly instead of the facade.

**GOOD:** depend on `frust::authoring` only (`frust`, `default-features = false`), and file a gap
against `authoring` instead. The historical Cargo-feature-unification hazard this anti-pattern used
to warn about (one `features = ["glyph"]` line silently turning a catalog back on for every
dependent app) no longer applies — `frust` carries no catalog cargo feature at all; the three
built-ins are ordinary sibling plugin crates ([PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md)'s
Design-System Plugins). `design-system-sample`'s `cargo tree -e features -i frust -p sample-app`
gate (see [DEVELOPMENT.md](DEVELOPMENT.md)) now proves out-of-tree resolution realism rather than
a catalog-off contract.

### Printing directly from a `frust-drive` build/run core

**BAD:** a `println!`/`print!` inside `android_build`/`ios_build`/ `android_run`/`ios_run` —
reaches the caller's stdout unconditionally, garbling a TUI session's raw-mode terminal with
raw pipeline output.

**GOOD:** thread an `on_line: &mut dyn FnMut(&str)` sink through the core instead — the CLI
passes `&mut |line| println!("{line}")`, the TUI routes it into a session's log tab.
`frust-drive/tests/print_free_cores.rs` (a source-scan conformance test) enforces this
against every drive core outside a small CLI-entry allowlist.

## Interaction Semantics

Conventions for `Widget::event` implementations in every interactive `frust-widgets` widget:

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

- **Only `PointerButton::Primary` starts a press/capture/activation.** A secondary (or other
  non-primary) press is a context gesture, not an activation — the sanctioned consumers are a
  context-menu trigger and the baseline text input's selection toolbar (a secondary press on an
  interactive `TextInput` claims focus, moves no caret, and toggles that toolbar), never the
  widget's own `on_press`/`on_toggle`/`on_change`. A barrier or light-dismiss layer may still
  consume any button to close, but must not enter press state from a non-primary one, while
  focus/IME session re-claims stay button-agnostic. Every widget tree carries its own crate-private
  predicate for the rule — `frust_widgets::authoring::presses`, `frust_shadcn::hit::presses`, and
  `press::presses` in each catalog crate — no cross-crate dependency.

- **Controlled components never self-mutate.** `Checkbox`/`Slider` report the *requested*
  value through `on_toggle`/`on_change` and leave `checked`/`value` untouched until the next
  `rebuild` feeds the app-confirmed value back down — never flip `self.checked` inline in a
  handler. `TextInput` is controlled too: `rebuild` applies the view's `value`
  set-if-different, so an app that rejects/transforms input in `on_change` sees its own
  value win next frame.

- **A container checks `InputEvent::is_broadcast()` before anything else.** A broadcast
  (`InputEvent::Housekeeping`, `InputEvent::Overlay`) is not user input: every routing helper
  forwards it to *every* child unconditionally, ahead of the capture/focus/hit-test branches below,
  and always reports `Ignored` whatever children returned — it is never consumed and never
  short-circuited by a captured or focused child, and only the owner whose `OverlayKey` an `Overlay`
  quotes acts on it (`docs/CORE_ARCHITECTURE.md`'s event-routing data flow).

- **Focus routes by recorded path, like capture; `Key`/`Ime`/`EditCommand` events never hit-test.**
  A container simply forwards them to its focused child; a `Down` that doesn't (re)claim focus on
  the child it hits blurs the chain. **A structural container rebuild clears capture and focus only
  where identity is actually lost** — stable-prefix/key-matched, not a blanket clear. A reconciler
  that tears down (or type-swaps) a focused pod cannot reach `RenderRoot` itself (no handle inside a
  `BuildCtx` pass), so it marks the pod orphaned instead, but only *on the live focus chain* — gate
  the mark on `ctx.has_focus() && pod.is_focused()`, never on the recorded `focused` flag alone,
  since a hand-rolled container's own teardown/type-swap path can hit a stale flag under an
  already-blurred ancestor; `RenderRoot`'s cached `focus_active`/`ime_state` release on the same
  rebuild that raised the mark, not merely "eventually" on a later event pass
  (`docs/CORE_ARCHITECTURE.md`'s Focus/IME Lifecycle). **The same gate binds a container that
  publishes a cleared IME surface** (the navigator's `needs_ime_clear` producers,
  `PatternSwitcher`'s): an inactive publish *is* a session release, so raise it only for an
  outgoing/covered subtree that was itself on the live chain — on a push the outgoing pod is the
  page being **covered**. Two severing paths are known to be **uncovered** and are registered rather
  than fixed: a type swap through a doubly-erased pod, which no reconciler can observe
  (`focus-double-erasure-swap-blind`), and the hand-rolled navbar/tabbar item lists, which never
  clear or mark a truncated item's own focus link (`focus-navbar-item-truncation-unmarked`). See
  `docs/LIMITATIONS.md`.

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

- **A self-sizing chrome widget consumes its own window inset exactly once, in its own
  `layout`, never pre-inset by its container** (`Scaffold`'s R-B4-inset: `app_bar`/`bottom_bar`
  self-size for top/bottom this way, so a bar that doesn't self-inset must wrap itself in
  `safe_area(...)` instead — see `docs/WIDGETS_ARCHITECTURE.md`'s Scaffold flow).

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

- **A hover consumer claims from the uncaptured `Move` arm, latches its own flag, and
  self-corrects that flag at paint time.** All three, because each covers a case the others
  cannot:
  - **Claim** with `EventCtx::claim_hover` on every qualifying move (hit-test the position
    against the widget's own bounds first), not just on entry — the claim is per-pass, so a
    widget that stops claiming stops being hovered on the next pass, with no leave event to
    react to.
  - **Latch** the same hit test into an internal hover flag and gate `request_redraw` on its
    *changed*-return. This is the only frame source for hover *gain* and for a link moving from
    one claimant to another: `claim_hover` requests no redraw, and the root manufactures one
    only when a hover ends with nothing taking it (its mirror is identity-free, so a claimant
    handoff is invisible to it). A widget without the flag shows no hover chrome on entry.
  - **Self-correct** from `PaintCtx::is_hovered` in `paint` — it is authoritative, and fixes the
    flag whenever no event could (the pointer left, or a container cleared/lapsed the link).
    `frust_material::list_item` is the reference consumer for all three.

  `PaintCtx::is_hovered`/`EventCtx::is_hovered` report **path** membership, not claimant
  identity: "this widget or a descendant of it holds the link", so a container reads `true` while
  the pointer is over a claiming child (CSS `:hover` semantics, which is what a web-derived
  design system expects) and a widget that never claims can still read `true`; siblings and
  off-path widgets read `false`. A `Down`, `Up`, or `Cancel` ends the link outright — consumers
  re-claim on the next `Move` rather than expecting hover chrome to survive a click
  (`docs/CORE_ARCHITECTURE.md`'s Hover and Cursor section).

  **A container with hover chrome of its own claims *after* routing the `Move` to its
  children, never before.** One claim per pass is recorded and the first one recorded wins,
  so an ancestor claiming first makes every descendant ineligible for that pass — the child
  under the pointer never reads hovered, while the latch rule above still has it repaint on
  every move. Claiming after routing makes the container's claim a fallback: a descendant's
  claim wins and the container still reads hovered through the path, and a container over no
  claiming child still gets its own chrome.

- **Ask for a cursor on every qualifying `Move`, never on `Down`.** `EventCtx::set_cursor` is
  stateless like `request_redraw`: a widget re-asks each move rather than latching a shape, and
  a captured drag re-asks from its own captured `Move` arm to keep its cursor outside its
  bounds. A press-specific cursor keys off the widget's own pressed state read in the `Move`
  arm, not off `Down` — no pass other than `Move` resolves or resets the cursor, so a `Down`
  handler that sets one would leave it standing for the length of a click with no chance to
  restate it.

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
- **Devtools frame-stats publish is deliberately outside the `perf::enabled()` gate.**
  `FrameStats::record` fans out to the devtools frame-stats bus *before* checking
  `perf::enabled()` — subscribing a devtools client is its own opt-in (gated by the
  `devtools` cargo feature and the running service), not tied to `FRUST_TRACE`. Do not fold
  the two gates together; they answer different questions (see
  [DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md)).
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
  state/element/owner triple is not a contract, `on_cleanup` is. That guarantee is itself
  route-dependent: a process-exit shutdown (macOS AppKit `terminate:`) runs neither `on_cleanup`
  nor `Drop` — see `docs/LIMITATIONS.md`'s `desktop-macos-quit-skips-executor-drop`. A plugin
  handle that owns an OS resource (e.g. `frust-camera`'s `AppleSession`) must not assume
  Drop-at-exit on macOS when held in app `State`.
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
  `crates/frust/tests/authoring_seam_conformance.rs`; the plugin tier is exempt
  ([PLUGINS_CODE_STANDARDS.md](PLUGINS_CODE_STANDARDS.md)).
- **A rebuild must run inside a `TrackedScope` for a signal write to wake it later — an
  untracked read is a silent wake hazard, not a stale value.** `.get()` subscribes only from
  *inside* a live `TrackedScope::track` closure; both shells guarantee this for their
  per-frame rebuild. A render-relevant read taken via `*_untracked`/`get_untracked` anywhere
  in that path never subscribes, so a later write flips no dirty flag and the shell may
  never repaint — reserve `*_untracked` for genuine non-rendering reads, never a value a
  `build` return depends on.

## Theming & Animation Conventions

Token resolution and precedence, animation pacing, and design-system installation are the
WIDGETS unit's rules — [WIDGETS_CODE_STANDARDS.md](WIDGETS_CODE_STANDARDS.md). The same
**explicit builder value > theme > fallback constant** precedence binds a `ThemeExtensions`
payload identically to a plain token: a consumer (a design-system plugin or a fully external
catalog) checks its own explicit override first, then `Theme::extension::<T>()`, and only then a
hardcoded fallback — see
[NATIVE_WIDGETS_ARCHITECTURE.md](NATIVE_WIDGETS_ARCHITECTURE.md)'s typeface ladder for the
shipped instance.

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
  `` `engine-metal-postmultiplied-truth-bug` ``) — the register's documented purpose; named semantic rules (`R23`,
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
