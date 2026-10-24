# Frust - Code Standards

The shared conventions every crate in the workspace follows. Three unit-specific rule sets hang
off this index — read this plus the one that covers what you are touching:

| Unit | Spoke | Holds |
|------|-------|-------|
| PLUGINS + NATIVE_WIDGETS | [PLUGINS_CODE_STANDARDS.md](PLUGINS_CODE_STANDARDS.md) | Plugin Conventions — backend gating, JNI attach scope, the platform/facade charter lines, theme-token folding, idempotent project mutation |
| WIDGETS | [WIDGETS_CODE_STANDARDS.md](WIDGETS_CODE_STANDARDS.md) | Child Lists — keyed-list uniqueness; Theming & Animation Conventions — token resolution/precedence (including `ThemeExtensions`), animation pacing, design-system installation |
| TUI | [TUI_CODE_STANDARDS.md](TUI_CODE_STANDARDS.md) | TUI Conventions — render/update layering, keyboard parity, the single command registry |

## Language Idioms

- **`unsafe` is confined to a small set of sanctioned platform-FFI boundaries.** Every other
  crate (`frust-core`, `frust-scene`, `frust-text`, `frust-widgets`, `frust-shell-common`,
  `frust-shell-linux`, `frust-shell-web`, `frust`) stays `unsafe`-free, bar `frust-shell-desktop`'s
  one macOS-only block (listed below) — where Masonry/xilem-style code would reach for `unsafe`
  downcasting, bound a trait on `Any` (e.g. `Widget: Any`) and downcast through `&mut dyn Any`;
  the web shell's frame waker is a
  capture-nothing closure over a `thread_local` `EventLoopProxy` slot (`WAKE_PROXY`). The
  sanctioned zones are raw-pointer boundaries a GPU/platform shell or loader cannot avoid, each
  isolated in one function/module with a `# Safety` doc comment:
  - `frust-gpu`'s `create_android_surface`/`create_metal_surface` (`lifecycle.rs`) — turn a raw
    `ANativeWindow*`/`CAMetalLayer*` into a `wgpu::Surface`; `frust-render` only forwards it.
  - `frust-shell-android`'s `jni_glue` and `frust-shell-ios`'s `ffi_glue` modules — the FFI boundary
    (`extern "system"`/`extern "C"` exports, `Box::into_raw`/`from_raw`, the surface hand-off,
    `nativeInitPlatform`'s `JavaVM` stash), the `android_app!`/`ios_app!` generated exports, and the
    render-thread split's `unsafe impl Send` for `SendableWindowPtr`/`SendableMetalLayer` plus the
    bare `libc` priority/QoS call kept for the `FRUST_NO_THREAD_TUNING` arm. Android's zone also
    holds `prepare_overlays`'s one `OverlaySurface::new` call; iOS's holds `take_frame_overlays`'s
    plus the `ios_app!`-emitted `frust_overlay_*` exports (`frust_overlay_take_released` writes
    `count <= capacity` layer pointers into Swift's buffer). Every overlay `OverlaySurface::new`
    site argues SAFETY from the shared `OverlayTable` order: a surface drops before its layer or
    window is handed back. That is ordering and leak-freedom, not memory safety — wgpu-hal retains
    the `CAMetalLayer` itself on Apple; on Android the table's held `ANativeWindow` reference keeps
    the object valid past `surfaceDestroyed` (a late present fails, it does not touch freed memory).
    Android's `adpf::platform` holds the opt-in performance-hint `dlsym` binding (`libandroid.so`).
  - `frust-shell-desktop`'s `render.rs` (macOS only) — `build_overlay_surface`, one
    `OverlaySurface::new(.., OverlayTarget::CoreAnimationLayer(ptr), ..)` block under the same
    table order; the layer crosses threads as an exposed-provenance `usize`, so there is no
    `unsafe impl Send`.
  - `frust-shell-macos`'s `platform_view` (macOS only) — the `FrustOverlaySurfaceView`
    `define_class!`, the `msg_send!` calls building and rescaling the overlay's `CAMetalLayer`
    and view, and the view-handle `from_raw`/`Retained` conversions; each `SAFETY`-noted.
  - `frust-shell-macos`'s `appkit_glue` module — four `SAFETY`-noted sites: the reopen-observer
    `define_class!` block, `msg_send![super(this), init]`, `addObserver_selector_name_object`
    (plus its notification `extern` static) and `removeObserver` in `Drop`.
  - `frust-shell-windows`'s `win32_glue` module — three `SAFETY`-noted sites:
    `SetCurrentProcessExplicitAppUserModelID`, `TranslateAcceleratorW` in the accelerator hook, and
    muda's `Menu::init_for_hwnd`.
  - `frust-plugin`'s `android` module — reconstructs the raw `JavaVM`/`jobject` handles plugins
    need from `ndk-context`, one module, scoped `AttachGuard` per call.
  - `frust-shared-preferences`'s `apple` backend — two `# Safety`-noted `setObject:forKey:` calls;
    `frust-secure-storage`'s — a confined `as_cf` objc→`CFType` Keychain bridge.
  - `frust-clipboard`'s `apple` backend — `UIPasteboard`'s `string`/`setString:`/`setItems_options:`
    calls plus one `extern` static read (`UIPasteboardOptionLocalOnly`), each `# Safety`-noted.
  - `frust-share`'s `apple` backend — three `SAFETY`-noted sites: `MainThreadMarker::new_unchecked()`
    in the main-queue closure, `UIActivityViewController::initWithActivityItems_applicationActivities`,
    and the subject `setValue_forKey` guarded by `respondsToSelector(setSubject:)`.
  - `frust-url-launcher`'s `apple` backend — `MainThreadMarker::new_unchecked()` in the main-queue
    hop and the one `openURL:options:completionHandler:` call objc2-ui-kit marks unsafe; its
    `desktop` backend — the Windows `ShellExecuteW` call (`SAFETY`-noted).
  - `frust-haptics`'s `apple` backend — one `MainThreadMarker::new_unchecked()` in a main-queue callback.
  - `frust-gpu`'s Android `import` zone — the hand-opened Vulkan device (`open_with_callback` +
    `create_device_from_hal`, `context.rs`), `import_hardware_buffer` (`create_texture_from_hal`,
    raw `ash`) and the `dlsym`-resolved `AHardwareBuffer_*` entries, each `# Safety`-noted, behind
    the default-off `FRUST_ANDROID_HWBUFFER_IMPORT` knob, plus a debug thread check.
    `frust-render`'s `native_import::android` makes the one call into it (`NativeHandle::new` contract).
  - `frust-gpu`'s Apple `import::apple` zone — `Device::as_hal::<Metal>`, the `CVMetalTextureCache`
    create and `create_texture_from_image`, `CFRetained::from_raw`/`retain`,
    `wgpu::hal::metal::Device::texture_from_raw` and `Device::create_texture_from_hal`, plus
    `unsafe impl Send for MetalImporter` and `unsafe impl Send + Sync for HeldFrame`.
    `frust-render`'s `native_import::apple` makes one forwarding call (`NativeHandle::new` contract).
  - `frust-plugin`'s `NativeHandle::new` and `TextureFrame::into_parts`, `frust-render`'s
    `SourceFrame::into_parts` (`unsafe fn`s; the bus's own call site is the SAFETY-noted block in
    `SourcePass::record`). The camera and video-player bridges each hold two `SAFETY`-noted sites:
    the split and the renderer-twin `NativeHandle::new`. Their Apple producers build
    `NativeHandle::new(CvPixelBuffer, ptr)` owning a +1 reference released in a single-call
    release callback. `frust-video-player`'s Android backend: its constructor call, the
    `AHardwareBuffer_*` NDK calls and a direct-buffer plane read.
  - `frust-engine`'s Apple-only `text/hvgl.rs` — the CoreText calls (`CTFont::with_name`,
    `post_script_name`, `glyph_count`, `path_for_glyph`, `bounding_rects_for_glyphs`, and the
    `CTFontDescriptor::with_attributes`/`copy_with_attributes` variation-instance pair),
    `CGPath::apply` with the `extern "C-unwind"` `push_element` callback, and `unsafe impl Send +
    Sync for CtFace` (read-only; its SAFETY note cites Apple's Core Text thread-safety statement).
  - `frust-gpu`'s `RenderContext::create_pipeline_cache` — one call building a `wgpu::PipelineCache`
    from a shell-persisted, fingerprint-validated blob.
  - `frust-reactive`'s private `thread_policy::sys` — `gettid`/`getpriority`/`setpriority` (Linux)
    and `pthread_set_qos_class_self_np` (Apple), calling thread only, over the unpinned `libc` 0.2.
  - `frust-drive`'s `process` module — a `kill(2)` shim (std has no `killpg`) group-killing a child.
  - `frust-devtools`'s `token::os_random_bytes` (Windows only) — one `BCryptGenRandom` call; a
    non-zero status yields `TokenSource::Fallback`, which the hot-patch gate refuses.
  - `frust-camera`'s `apple` backend — AVFoundation message sends behind one `QueueBound<T>`
    `unsafe impl Send/Sync` (serial-queue confinement, one `# Safety` note), and
    `frust_camera_session_handle`'s raw-pointer C export (retain contract **+1**); its Android
    backend keeps its single `plane_data` direct-buffer block, texture preview included.
  - `frust-native-widgets`'s Android backend — cached `JMethodID` + `call_method_unchecked` for hot
    per-frame setters, confined to one `# Safety` helper (`call_void_cached`); cold setters use the
    checked `NativeCtx::call_void`. Its Apple backend closes objc2's nullability-unannotated
    `unsafe` behind **safe** property wrappers, plus the `define_class!`/`extern_protocol!`
    registrations, `addTarget:action:` attach/detach, `apple_host::on_main`'s
    `MainThreadMarker::new_unchecked()`, and the alert/sheet controllers and `block2` handlers —
    each site `SAFETY`-noted.
  - `frust-iap`'s `apple` backend — one untyped `msg_send![class, shared]` resolving the Swift
    glue's singleton by runtime-only class name, plus two `RcBlock` completion blocks receiving raw
    `NSString` pointers whose validity only the glue's contract establishes; each site is
    `SAFETY`-noted and `catch_unwind`-wrapped. Its `android` backend holds only its two JNI exports'
    `#[unsafe(no_mangle)]` attributes.
  - `frust-auth-session`'s `apple` backend — the `objc2-authentication-services` calls (the
    deprecated `ASWebAuthenticationSession` initializer, its setters, `-start`), the completion
    handler's raw `NSURL`/`NSError` derefs and error-domain `extern` static, the `define_class!`
    presentation-anchor class, one `MainThreadMarker::new_unchecked()`; each is `SAFETY`-noted, the
    completion body under `catch_unwind`. Its `android` backend, like `frust-iap`'s, holds only a
    JNI export's `#[unsafe(no_mangle)]`.
  - `frust-hotpatch` — `patch.rs`: `load_patch_library` (`libloading`; initialisers run, so only a
    library built for this process), `apply_patch` (a table for this exact executable, anchored on
    `__frust_hotpatch_anchor`) and `get_jump_table` (the leaked, Release-published table);
    `hot_fn.rs`: casting a table address to the hot function's signature; `anchor.rs`: image
    lookup; `android.rs`: memfd + `android_dlopen_ext`. `frust-shell-common` calls the safe
    `apply_from_devtools`; `frust::app!` emits only the anchor's `#[unsafe(no_mangle)]`.
- **No unwind across FFI.** Every platform export routes through `frust-shell-common`'s `guard`
  helper (`catch_unwind` + log, returning a benign default) rather than unwinding into
  JVM-/Swift-owned stack frames — a panic crossing the FFI boundary is undefined behavior, not a
  bug. `run_guarded_thread` is `guard`'s whole-thread-body counterpart: every render-thread `spawn`
  closure routes through it, so a caught panic exits cleanly and drains any orphaned `Ack` instead
  of poisoning shared state. `frust-shell-macos` and `frust-shell-windows` depend on neither
  `frust-shell-common` nor each other, so their native-callback boundaries can't route through
  `guard`; each applies the same contract locally instead — `appkit_glue`'s AppKit notification
  callback hand-rolls its own `catch_unwind` around the observer body. The one exception is muda's
  menu-event handler (both crates' `menu` modules), panic-free by construction: an infallible
  mutex-guarded queue push plus a redraw request. Texture release callbacks
  (`TextureFrame`/`SourceFrame`, run from `Drop`) must not panic: the `catch_unwind` in
  `run_release` is a last line of defence in unwinding builds only.
- **State-sync, not op-forwarding, across a mobile IME bridge.** Android/iOS platform text input
  doesn't send individual keystrokes across the FFI boundary — the platform owns composition
  (Gboard, CJK marked text) against a local mirror, then hands the framework a whole reconciled
  `EditingState` and reads one back — never add a per-keystroke op-forwarding path.
- **UTF-16 at the FFI seam, bytes inside.** `EditingState`'s `selection_*`/`composing_*` indices are
  UTF-16 code-unit indexed wherever they cross a shell boundary; convert to/from byte offsets only
  inside `frust-text`'s `TextEditor`.
- **Hand-roll JSON at the mobile FFI boundary — no `serde` in shell crates.**
  `frust-shell-android`/`frust-shell-ios` serialize `ImeState` with a small hand-written
  escaper/builder for a handful of fixed fields.
- **Type-erase to avoid a downstream crate dependency**, not to avoid writing a type. When a lower
  layer threads a resource owned by a higher layer, pass it as `&mut dyn Any` and recover it at the
  one call site that knows the concrete type via a documented, panic-on-mismatch `downcast_mut::<T>()`.
- **Erase at the API boundary, not at call sites.** A helper returns `-> impl View<S>`
  (`+ use<..>` only when the opaque type must not be tied to a borrow — a navigator closure, a
  stored result; edition 2024 captures correctly by default otherwise); a child list is a tuple
  of up to 12 views (nest beyond; `Vec<V>` when homogeneous or dynamic; `keyed()` lists stay
  `Vec`); an API that takes a child list is written `fn f<M>(.., c: impl ViewSeq<S, M>)` and
  erases once inside. `any()`/`AnyView` remain only for the documented irreducible cases:
  stored view tables (`Vec`/`Option<AnyView>`, `Box<dyn Fn(..) -> AnyView>`, page registries
  incl. nine-plus-arm dispatch), accumulators into a `Vec<AnyView>`, recursive helpers, trait
  extension points, runtime-varying child types and mixed lists, and the hot-patch seam's
  `build_erased`, and the `OverlaySlot` `&AnyView` seam. Branches: two arms `either(..)`, three to
  eight `EitherOfN` from a `match`. A hand-written view stores its child typed (`child: V`) and
  drives it via the generic `build_child`/`rebuild_child`/`teardown_child`. A kept site carries
  `// erasure: keep <why>` (`scripts/ci/erasure-check.sh`, rules T1-T7); a driver closure erases
  `Component::build`: `move |s| AnyView::new(root.build(s))`. No `#[allow(refining_impl_trait)]`.
- **Edition-2024 `-> impl Trait` return types capture all in-scope lifetimes by default.** When a
  returned `impl View<State>` must not borrow a parameter (stored, or returned from a
  navigator/`RenderRoot` closure), opt out explicitly with `+ use<>` — the compiler says so.
- **A platform value with no published spec is a named constant with a source comment, not a bare
  literal.** Where a platform's exact behavior is private (an iOS gesture's edge zone, commit
  threshold, or fling velocity; an animation's duration/curve), name the constant; its doc comment
  states whether it's a published value or **community-approximate** (a reverse-engineered estimate)
  plus what it's approximating:

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
  `.context()` / `.with_context()` at each fallible step so the chain names what was attempted.
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

JNI export names are LAW: the package/class (`dev.frust.FrustSurfaceView`) is fixed across every
generated app, so the mangled symbol stays stable regardless of the app's own package. A
`platform_view` factory's `viewType` naming is LAW too (any class under `dev.frust.*` via the app
classloader on Android, the bare `@objc(<Name>)` name via `NSClassFromString` on iOS). Both hosts
check **prefix (Android) then interface/protocol assignability, before instantiation** —
constructing an untrusted class first runs its side effects unchecked.

## Plugin Conventions

Both plugin tiers under `plugins/` carry additional conventions of their own —
[PLUGINS_CODE_STANDARDS.md](PLUGINS_CODE_STANDARDS.md).

## Platform-View Conventions

- **Mode B is a build-time host configuration, never an app Rust opt-in, and Android/iOS only.** The
  embedding module's overridable `translucentSurface` seam (Android: `FrustSurfaceView`'s
  constructor parameter, fed by `FrustActivity`'s `open val`; iOS: `FrustViewController`'s `open
  var`) must drive the window pixel format, the host's native-sibling z-order and the
  `declare_host_translucent_surface` call *together*, in the same branch — splitting them ships
  black rectangles. App Rust has no matching call; only the two mobile shells' own FFI glue may
  declare it (see `docs/ARCHITECTURE.md`'s Platform-view flow).
- **A desktop host is Mode A: `DesktopExtensions`' platform-view hooks, never an FFI poller.** It
  resolves a slot's `view_type` through `frust_plugin::desktop` rather than naming a plugin crate,
  leaves rects and clips in **logical points**, and treats `on_platform_views_suspended` as
  remove-not-hide — the surface-recreate replay is the way back.
- **A desktop-hosted `NSControl` owns its input; `interactive` means nothing there.** AppKit's
  responder chain routes it, so `frust-shell-macos`' Mode-A host ignores `interactive` and shields —
  add no shielding or hit-test forwarding for `plugins/native-widgets/src/appkit/`'s controls.
- **Mode B paint contract: an unpainted region is a window, not a compositor bug.** `platform_view`
  punches its own slot rect automatically; any other chrome region a Mode B host leaves unpainted
  shows raw OS content — pair translucency with an opaque app-root background (`AppBackground`).
- **A `platform_view` slot never receives `Widget::event` (v1).** Native-view input is OS-routed
  through the host's own view hierarchy, not `EventCtx` — there is no hit-test/dispatch seam for a
  hosted view; don't add pointer handling to `PlatformViewWidget`.
- **A modal is never a slot.** An alert or sheet is host-owned, imperatively requested modal UI — no
  `platform_view` slot, rect or Mode, and no slot inside it (`plugins/native-widgets/src/present/mod.rs`).

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

**GOOD:** public APIs above `frust-render` speak only `kurbo`/`peniko`; this is what lets the
GPU backend be swapped later without touching widget or text code. **The one named exception:**
`frust-render::DetachedSurface` is a deliberate **opaque** escape valve — its only accessor,
`into_surface`, is crate-private, so the wrapped `wgpu::Surface` is never nameable outside
`frust-render`; follow it for any future crossing.

### Reaching into a design-system plugin's internals from a design-system crate

**BAD:** a `crates/frust-drive/templates/design-system/`-derived crate's `Cargo.toml` declaring a
dependency on `frust-glyph`/`frust-material`/`frust-cupertino` to reach one convenience symbol, or
depending on `frust-widgets`/`frust-core` directly instead of the facade.

**GOOD:** depend on `frust::authoring` only (`frust`, `default-features = false`), and file a gap
against `authoring` instead. `frust` carries no catalog cargo feature at all — the three built-ins
are ordinary sibling plugin crates ([PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md)'s
Design-System Plugins). `design-system-sample`'s `cargo tree -e features -i frust -p sample-app`
gate (see [DEVELOPMENT.md](DEVELOPMENT.md)) proves out-of-tree resolution realism.

### Printing directly from a `frust-drive` build/run core

**BAD:** a `println!`/`print!` inside `android_build`/`ios_build`/ `android_run`/`ios_run` —
reaches the caller's stdout unconditionally, garbling a TUI session's raw-mode terminal with
raw pipeline output.

**GOOD:** thread an `on_line: &mut dyn FnMut(&str)` sink through the core — the CLI prints it,
the TUI routes it into a session's log tab. `frust-drive/tests/print_free_cores.rs` (a
source-scan test) enforces this outside a small CLI-entry allowlist.

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
  (`Housekeeping`, `Overlay`, `Shortcut`, a file drop's `Ended`) is not user input: routing helpers
  forward it ahead of the capture/focus/hit-test branches below, never consume or short-circuit it,
  and report `Ignored` whatever children returned. It reaches *every* child except a `Shortcut`,
  which a hidden or covered child must not receive; only the owner an `Overlay` quotes acts on it,
  and a declarer answers only its own `ShortcutId` `Handled`, never a blanket `Handled`
  (`docs/CORE_ARCHITECTURE.md`'s event-routing data flow).

- **Focus routes by recorded path, like capture; `Key`/`Ime`/`EditCommand` events never hit-test.**
  A container forwards them to its focused child; a `Down` that doesn't (re)claim focus on the child
  it hits blurs the chain. **A structural container rebuild clears capture and focus only where
  identity is actually lost** — stable-prefix/key-matched, not a blanket clear. A reconciler that
  tears down (or type-swaps) a focused pod has no `RenderRoot` handle inside a `BuildCtx` pass, so it
  marks the pod orphaned instead, gated on the *live* focus chain — `ctx.has_focus() &&
  pod.is_focused()`, never the recorded `focused` flag alone, which a hand-rolled container's
  teardown can hit stale under an already-blurred ancestor; the root's cached focus/IME state
  releases on the same rebuild (`docs/CORE_ARCHITECTURE.md`'s Focus/IME Lifecycle). **The same gate
  binds a container that publishes a cleared IME surface** (the navigator's `needs_ime_clear`
  producers, `PatternSwitcher`'s): an inactive publish *is* a session release, so raise it only for
  an outgoing/covered subtree that was itself on the live chain — on a push, the page being
  **covered**. Two severing paths are registered rather than fixed: a type swap inside a wrapper view
  re-boxing an `AnyView` under its own `Box<dyn Widget>` (`focus-wrapper-erasure-swap-blind`) and
  the hand-rolled navbar/tabbar item lists, which never clear or mark a truncated item's focus link
  (`focus-navbar-item-truncation-unmarked`). See `docs/LIMITATIONS.md`.

- **A focusable control answers `Widget::is_focusable` and activates only on a bare, non-repeat
  Space/Enter while focused;** its keyboard-focus look reads `PaintCtx::is_focused_for_ring`, while
  a text field keeps `has_focus` (WIDGETS_CODE_STANDARDS.md's Keyboard Focus and Shortcuts).

- **Input constants have one source.** Gesture thresholds (`TOUCH_SLOP`, `MOUSE_SLOP`), scroll/fling
  tuning (`WHEEL_LINE_PX`, `FLING_DECAY`, `FLING_STOP`, `VELOCITY_WINDOW_MS`) and `VelocityTracker`
  live in `frust-core::input`; widgets import them rather than hardcoding a local threshold.

- **Events are logical-coordinate by the time they cross `AppTree`.** Every platform boundary
  (winit, JNI `nativeOnTouch`, the C `frust_dispatch_touch`) converts to logical pixels before
  building an `InputEvent` — widget/container code never divides by scale factor.

- **Insets follow the same physical-at-FFI, logical-inside rule as coordinates.** A platform
  delivers occlusion in physical px (Android `Insets`) or logical points (iOS `safeAreaInsets`);
  the FFI-boundary `logical_insets` helper reconciles it — widget code only ever sees a resolved
  `WindowInsets` in logical px (`docs/SHELLS_ARCHITECTURE.md`'s cross-cutting host-signal flow).

- **A self-sizing chrome widget consumes its own window inset exactly once, in its own
  `layout`, never pre-inset by its container** (`Scaffold`'s R-B4-inset: `app_bar`/`bottom_bar`
  self-size for top/bottom this way, so a bar that doesn't self-inset must wrap itself in
  `safe_area(...)` instead — see `docs/WIDGETS_ARCHITECTURE.md`'s Scaffold flow). `safe_area` removes what it consumed, so wrapping a self-insetting bar in it is harmless but leaves the band unpainted — pick one.

- **A `Cancel` arm must never call `EventCtx::state_mut`.** It may only clear internal flags
  (`self.pressed`/`self.captured`/`self.armed`) and request a redraw. A rebuild can synthesize a
  `Cancel` to a still-captured child over a throwaway `()` state (`docs/CORE_ARCHITECTURE.md`'s
  event-routing data flow) — reaching for real state there panics on the `()` downcast, a tripwire.

- **A container that suppresses routing to its children must cancel their capture, clear their
  focus, and publish a cleared IME surface — in that order, with no bypass.** This binds any
  container that stops forwarding events to an already-interactive child (the navigator's
  mid-transition input block is the reference impl); skipping it leaves a child armed or a stale
  focus/IME surface behind after the block lifts. A cleared surface is a full root focus/IME
  session release, not a value update (`docs/CORE_ARCHITECTURE.md`'s Focus/IME Lifecycle).

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
  stateless like `request_redraw`: re-ask each move (a captured drag from its captured `Move` arm,
  keeping its cursor outside its bounds) rather than latching a shape. A press-specific cursor keys
  off the widget's pressed state read in the `Move` arm — only `Move` resolves or resets the cursor,
  so a shape set on `Down` would stand for the whole click with no chance to restate it.

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
- **A rebuild must run inside a `TrackedScope` for a signal write to wake it later — an
  untracked read is a silent wake hazard, not a stale value.** `.get()` subscribes only from
  *inside* a live `TrackedScope::track` closure; both shells guarantee this for their
  per-frame rebuild. A render-relevant read taken via `*_untracked`/`get_untracked` anywhere
  in that path never subscribes, so a later write flips no dirty flag and the shell may
  never repaint — reserve `*_untracked` for genuine non-rendering reads, never a value a
  `build` return depends on.
- **An app authors a custom `View`/`Widget` pair through `frust::authoring`, never a direct
  `frust-core`/`frust-scene`/`frust-text`/`accesskit`/`kurbo`/`peniko` dependency.**
  `frust::authoring` (plus its `text`/`scene` submodules) re-exports the trait lifecycle,
  child/event/callback plumbing, semantics, and geometry/paint types a custom widget needs, so
  an app crate's `Cargo.toml` depends on `frust` plus plugin crates only. For the long tail use
  the whole-crate valves `frust::kurbo`, `frust::peniko`, `frust::accesskit` — each is
  version-pinned in one place (`docs/DEVELOPMENT.md` § Version-Pin Policy). Enforced across
  `benchmarks/frust_bench` and the five in-repo example apps (`huddle`, `shadertoy`,
  `glyph-catalog`, `playground`, `native-widgets-demo`) by
  `crates/frust/tests/authoring_seam_conformance.rs`; the plugin tier is exempt
  ([PLUGINS_CODE_STANDARDS.md](PLUGINS_CODE_STANDARDS.md)).

## Theming & Animation Conventions

Token resolution and precedence (including `ThemeExtensions` payloads), animation pacing, and
design-system installation are the WIDGETS unit's rules — [WIDGETS_CODE_STANDARDS.md](WIDGETS_CODE_STANDARDS.md).

## Testing Patterns

- **Fixture-driven tests for parsers/validators**: canned `ProcessRunner`/`EnvLookup` responses
  keyed by the exact invocation.
- **Injectable hook seams for process-global side effects**: a function installing a real
  handler (`ctrlc::set_handler`, a filesystem watcher) takes a `Hooks` struct defaulted to the
  real installers, with a `#[cfg(test)]` `::fake()` no-op pair a test injects instead.
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
  development process that produced it.** Banned: plan/phase references (`Phase 9.B step 1`,
  `(phase 5.5)`), plan-task references (`task-12`), findings-ledger numbers (`FINDINGS #43`),
  review/fix-round references (`re-review round 1`, `cfix-2`), plan requirement numbers
  (`req 4`), and internal PR numbers; a ledger number dangles for readers without that repo.
- **Sanctioned citations** stay fine: `docs/LIMITATIONS.md` stable ids (e.g.
  `` `engine-metal-postmultiplied-truth-bug` ``), named semantic rules (`R23`, `R44-back`),
  doc-section pointers, and commit SHAs/version pins of *external* repos. Keep a ledger entry's
  meaning, drop its number (`// re-created finding #44` → `// re-created the root-modal
  double-claim bug`). Domain vocabulary (render phases, numbered steps) is not residue.
- **Header budget scales with the module.** A simple module gets a 1–3 line header; a complex
  one gives contract + rationale plus one pointer to the owning spoke doc. Inline comments
  state what the code can't show (a `# Safety` contract, a magic-number source).
- **Rust source comment budget.** Per region (production head before the first top-level
  `#[cfg(test)]` module; test tail) at most 25 % comment lines (regions under 80 lines exempt);
  `///`/`//` runs at most 12 lines (across blank lines), `//!` at most 24. The gate
  `crates/frust/tests/comment_density_budget.rs` covers `frust-core` with tighten-only per-file caps
  (measured + 3 points, never above 25); other crates follow it, human-reviewed.
- **TOML manifests follow these too** (`Cargo.toml`, `*.tmpl`, `frust.toml`, `.cargo/config.toml`):
  only what the TOML cannot show (LAW pin + lockstep site, must-not-re-add feature, cfg reason), at most 3 lines, linking the owning doc.
- **Mechanically checked.** `comment_residue_conformance.rs` scans source and manifest `#` comments
  for banned plan/phase, plan-task, findings-ledger, review-round, plan-document, `req N` and
  `review finding <id>` refs (citations exempt only their match span); bare PR numbers and plan
  tags (`T04`/`D6a` vs. `M3`/`R8`) stay human-reviewed. `manifest_comment_budget.rs` fails a
  manifest comment block over 3 lines, or over 12 prose `#` lines (commented-out TOML excluded)
  with a prose/setting ratio over 0.9 (Cargo, `.cargo/config.toml`) or 3.3 (`frust.toml`); no allowlist.
