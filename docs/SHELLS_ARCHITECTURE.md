# Frust - SHELLS Architecture

## Overview

SHELLS is the seam between the platform-agnostic core+scene+render+text+theme+reactive stack and
each concrete host. `frust-shell-common` is the platform-free plumbing every shell shares: a
type-erased app driver, the frame-gate pacing decision, the render-thread split vocabulary, the
platform-view differ, and a handful of signal-poll seams. Above it sit four host tiers:

- **Desktop** — `frust-shell-desktop` is the shared winit core (event loop, frame pipeline,
  input/IME/theme translation, accessibility) for *every* desktop host, plus the
  `DesktopExtensions` seam and the `DesktopConfig`/`MenuSpec` vocabulary. Three thin per-OS
  crates — `frust-shell-macos`, `frust-shell-windows`, `frust-shell-linux` — implement that seam
  with native integration the core neither knows nor should know about.
- **Android** and **iOS** — `frust-shell-android` and `frust-shell-ios` are FFI/frame-callback
  integration points that own their host's lifecycle end to end.
- **Web** — `frust-shell-web` (`crates/frust-shell-web`) is a winit host like Desktop, but not a
  *desktop* host, so it sits directly on `frust-shell-common` — the way the two mobile shells do —
  rather than depending on `frust-shell-desktop`, whose core is entangled with `accesskit_winit`,
  `pollster::block_on` surface bring-up, a render thread and `frust-paths` cache I/O, none of which
  exist on `wasm32-unknown-unknown`. It carries its own copy of that core's winit-*generic* halves
  — input mapping, the reactive-owner event wrap (`event_under_owner`), the change-guarded
  `WindowMetrics` publish, and brightness-follow — under a documented must-not-diverge contract
  (`crates/frust-shell-web/src/app_handler.rs`'s header).

Every concrete shell drives rebuild → layout → paint → encode → present and translates
platform-native input/lifecycle/theme/insets/IME/deep-link/back/platform-view signals into the
framework's own vocabulary.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how SHELLS relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `frust-shell-common::app_tree` | Type-erased `AppTree` driver letting a native shell own and drive any app's rebuild/layout/paint/event/semantics cycle without generics |
| `frust-shell-common::frame_gate` | Shared run/skip frame decision and pacing used by both continuous-loop mobile shells |
| `frust-shell-common::render_split` | UI-thread/render-thread split vocabulary (scene handoff, lifecycle commands, completion barrier) every shell's default frame path uses |
| `frust-shell-common::platform_view` | Differ turning per-paint platform-view frames into an idempotent create/update/dispose backlog for embedding native views |
| `frust-shell-common` (signal-poll seams) | Small process-global slot-plus-poll seams (surface mode, theme override, fonts, system UI) drained once per frame — surface mode and system UI on mobile only; theme override and fonts on every shell, desktop included |
| `frust-shell-common::devtools` (feature `devtools`) | Shell-side `DevtoolsBackend` implementation plus the per-frame UI-thread hop and pump each shell drives; see [DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md) |
| `frust-shell-common::gpu` (feature `gpu`) | Process-wide install-once slot (`install_gpu_handle`/`gpu_handle`) a shell publishes its live GPU device handle into, read back through the facade's `frust::gpu::with_context` (see [RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md)'s GPU Seam). `frust-shell-desktop` publishes at both of its device-creation sites; the Android and iOS shells forward the feature but do not install a handle yet, so `with_context` answers `None` there |
| `frust-shell-desktop` | The shared winit core: event loop, UI-thread/render-thread surface split, accessibility adapter, paced wake, pipeline-cache persistence — plus the `DesktopExtensions` seam and the `DesktopConfig`/`MenuSpec` vocabulary the per-OS crates read. Also the zero-config dev-preview entry point |
| `frust-shell-desktop::platform_view` | The OS-neutral desktop platform-view host: owns the shared differ, ingesting each paint's frames right after `RenderRoot::paint` and draining the batch to the per-OS hook right after the frame is submitted, then acknowledging it; plus the two lifecycle resets (replay on surface re-create, drop-everything on suspend) |
| `frust-shell-macos` | AppKit integration: the native menu bar (a standard application menu plus the app's own spec), hide-on-close and Dock-reopen lifecycle, the reopen observer in its `appkit_glue` unsafe zone, and the platform-view host below |
| `frust-shell-macos::platform_view` | The AppKit (Mode A) native-view host: resolves a slot's `view_type` to a plugin factory, parents the returned `NSView` above winit's content view, and places/shows/disposes it from every later command. Its slot bookkeeping, geometry mapping and retain accounting sit behind a `NativeViewOps` seam and compile on every host; only the AppKit implementation is `cfg(target_os = "macos")` |
| `frust-shell-windows` | Win32 integration: taskbar identity, window/taskbar icons, an `HMENU` menu bar with keyboard accelerators, and titlebar brightness — every `unsafe` and every `windows-sys` call confined to `win32_glue` |
| `frust-shell-linux` | Wayland `app_id`/X11 `WM_CLASS` plus the X11 window icon, through winit's own cross-platform API. Deliberately thin: no GTK/X11 binding, no native menu (the menu is widget-drawn), no `unsafe`, one hook implemented |
| `frust-shell-android` | Sanctioned-unsafe JNI FFI boundary, app-binding macro, and Choreographer-synced frame pipeline; carries the theme precedence ladder and the conformance-pinned resolved-surface-mode publish site |
| `frust-shell-ios` | Sanctioned-unsafe C-ABI FFI boundary, app-binding macro, and CADisplayLink-driven frame pipeline, with optional present-sync gating against platform-view geometry |
| `frust-shell-web::app_handler` | Winit-generic host-signal translation ported from `frust-shell-desktop` (input mapping, the reactive-owner event wrap, theme delivery, window-metrics publish) plus, gated to `wasm32`, the `browser_loop` submodule owning the canvas-bound event loop and frame turn (`spawn_app`/`run_app`) |
| `frust-shell-web::pacing` | Pure paced-wake decision logic composing the browser's two wake mechanisms (`requestAnimationFrame` redraws, `ControlFlow::WaitUntil` deadlines) |
| `frust-shell-web::render` | The single-thread inline frame executor (`WebFrameExecutor`) over the wgpu surface: swapchain reconciliation, encode/acquire/submit, and the cold-page adapter-retry bring-up |
| `frust-shell-web::input` | Single-contact `TouchTracker`, matching the mobile shells' v1 touch contract — the one input mapping with no `frust-shell-desktop` twin |
| `frust-shell-web::ime` | The hidden-`<input>` overlay bridging real browser composition into `ImeEvent::Compose`/`Commit`, and the canvas re-dispatch that keeps plain typing on winit's existing key path — see Cross-cutting host signals below |
| `frust-shell-web::logging` | Routes the `log` facade to the browser console (`console_log`/`console_error_panic_hook`), idempotent against the facade's own install, plus a `?log=` query-param level knob |

## Layer Dependencies

Every concrete shell depends on `frust-core` (`RenderRoot`, widget tree, input, insets,
semantics), `frust-scene` (renderer-agnostic `Scene`/`SceneBuilder`), `frust-text` (a
`TextContext` threaded through layout), `frust-theme` (theme delivery, brightness/design-language
state), `frust-render` (`SurfaceRenderer`, encode/present, pipeline cache) and `frust-reactive`
(`ReactiveRuntime`, `deep_link`, `back`, `menu`, `task`). Desktop and Android also depend on
`frust-paths` for cache-dir persistence, and Android on `frust-plugin` to install the
JavaVM/Context handle plugins read — the same `nativeInitPlatform` call installs the app's
files/cache directories into `frust-paths` (`install_android_dirs`) first, which is what makes
`data_dir()`/`cache_dir()` resolve there.

`frust-shell-common` is a hard platform-free leaf: no `jni`/`ndk`/`winit` dependency, no unsafe
code, and no reactive dependency in its shipped surface, so every concrete shell can share it
unconditionally. Platform FFI lives only in the concrete shells — `winit` + `accesskit_winit` in
`frust-shell-desktop`, `jni`/`ndk`/`accesskit_android` in `frust-shell-android`,
`objc2`/`accesskit_ios` in `frust-shell-ios`, `winit` alone (no AccessKit — there is no web
adapter) in `frust-shell-web`, and the per-OS desktop bindings below.

**The three per-OS desktop crates depend only on `frust-shell-desktop`** plus `winit`, whatever
narrow seam they push into or are handed (`frust-reactive` for menu activations, `frust-theme` for
the Windows brightness hook, and on macOS `frust-shell-common` for the `ViewCommand` vocabulary its
platform-view hook receives) and their own native bindings — never on each other, and nothing in
the shared core depends on them. The core is compiled for every desktop host; the native half is
chosen above it.

`frust-shell-macos` additionally depends on `frust-plugin`, to resolve a slot's `view_type` through
the desktop view-factory registry: the **second shell → substrate edge** after `frust-shell-android`'s,
in the same direction and for the same reason — a shell reads the leaf substrate so it can host a
plugin's native view without depending on any plugin, and the edge only ever points that way (see
[PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md)). Both that edge and the `frust-shell-common`
one are unconditional rather than macOS-gated, so the host's OS-neutral core stays compiled and
unit-tested on every build host.

**`frust-shell-web` depends only on `frust-shell-common`**, never on `frust-shell-desktop`, for the
Overview's reasons. Its `winit` edge rides the identical workspace pin the desktop tier uses — a
second `winit` identity between the two shell tiers would be a resolution hazard, not a
convenience.

### Target gating

The `frust` facade is the only place that names a per-OS or per-tier shell crate. It gates each
dependency and the matching extension-selection/entry-point arm under the identical
`cfg(target_os = …)` — or `cfg(target_arch = "wasm32")` for the browser tier — so a build resolves
only its own host's native bindings: `crates/frust/Cargo.toml`'s `[target.'cfg(not(any(target_os =
"android", target_os = "ios", target_arch = "wasm32")))'.dependencies]` table carries
`frust-shell-desktop` (excluding it from `wasm32`), and a separate `[target.'cfg(target_arch =
"wasm32")'.dependencies]` table carries `frust-shell-web` instead.

| Target | Shell composition |
|--------|-------------------|
| macOS / Windows / Linux | shared desktop core + that OS's extension crate |
| other desktop hosts (BSDs) | shared desktop core + the whole-set no-op extensions — window, input, theme and accessibility all work; only native identity/menu integration is absent |
| Android | `frust-shell-android` only; the desktop core is excluded (winit's `android-activity` edge does not build there and the app is JNI-driven) |
| iOS | `frust-shell-ios` only; the desktop core is excluded, keeping winit and `accesskit_winit` out of an iOS build entirely. The `app!` macro still emits a `__frust_main` **stub** on iOS, because the generated `main.rs` binary target Xcode builds calls it — it logs and exits non-zero rather than looking like an app that started and vanished |
| Web (`wasm32-unknown-unknown`) | `frust-shell-web` only; the desktop core is excluded (see the Overview, and `crates/frust-shell-web/Cargo.toml`'s own header). `frust::web_app!` mirrors `android_app!`/`ios_app!`: an unconditional, self-gating invocation whose generated `#[wasm_bindgen(start)]` shim expands to nothing off `wasm32`, and `app!` emits it under the identical gate |

### Sanctioned-unsafe zones

Each concrete shell confines all unsafe/FFI code to one named module — `jni_glue` on Android,
`ffi_glue` on iOS, `appkit_glue` on macOS, `win32_glue` on Windows — so each platform boundary is
auditable as a single surface. `frust-shell-common`, `frust-shell-desktop` and
`frust-shell-linux` hold no `unsafe` at all. The register of what each zone is permitted to do,
and the `# Safety`/no-unwind rules that govern it, live in
[CODE_STANDARDS.md](CODE_STANDARDS.md) § Language Idioms.

`surface_mode`'s writer set is pinned by a source-scan conformance test: exactly one call site per
platform may publish a resolved surface mode, a single-writer contract that keeps the signal-poll
seam race-free without a lock.

## The Desktop Extension Seam

`DesktopExtensions` is an eight-hook trait, every method defaulted to a no-op, invoked at the
points in the loop where a native integration has something to say. `NoExtensions` implements none
of them and is what the zero-config preview installs, so that path pays nothing for a seam it does
not use.

| Hook | When | Motivating use |
|------|------|----------------|
| `on_event_loop_builder` | before the loop is built | builder-only winit extensions — the Windows accelerator message hook |
| `on_window_attributes` | before the window is created | attributes winit accepts only at creation: Linux's `with_name`, a window icon |
| `on_window_created(&Arc<Window>)` | once, after creation, before the window is shown | attaching a native menu to the live handle; retaining the window |
| `pump` | once per frame, at the top of the redraw pass | draining the extension's own queue so what it delivers is visible to *this* frame's rebuild |
| `on_theme_brightness_changed` | whenever the resolved brightness actually moves | Windows titlebar theming |
| `on_platform_view_commands(&Arc<Window>, scale, &[ViewCommand])` | once per frame, right after the frame is submitted | placing plugin-provided native views — macOS's AppKit host |
| `on_platform_views_suspended` | the window or its surface is going away | removing every hosted native view before it is orphaned |
| `on_close_requested() -> CloseAction` | on a close request | macOS hide-instead-of-quit |

Two contracts shape it. **Static dispatch, not `dyn`:** the builder hook hands out a
`DesktopEventLoopBuilder` that a platform crate extends through winit's own `EventLoopBuilderExt*`
traits (object safety would bite), and there is exactly one extension per binary anyway. **Only
two hooks receive the window:** `on_window_created`, as an `&Arc` the extension clones and retains
if a later hook needs it, and `on_platform_view_commands`, which fires from inside the frame path
and must parent a hosted view into that exact window. That keeps the other six unit-testable with
no live event loop.

`CloseAction::Exit` (the default) exits the loop, which is what runs the real shutdown path:
`run_app` returns, the frame executor drops, the render thread joins after a final present and its
best-effort cache persist. `CloseAction::KeepRunning` hands the eventual exit to the extension —
nothing in the core will call `exit()` on its behalf.

### Configuration vocabulary

`DesktopConfig` (app name, reverse-DNS id, window icon, menu spec, last-window-close policy) and
the `MenuSpec`/`MenuItemSpec`/`MenuRole` tree are plain data defined in `frust-shell-desktop` —
the lowest crate both the core (which carries the spec) and the per-OS shells (which build a
native menu from it) share. They cannot live in the facade, which depends on the shell crates and
never the reverse; the facade **re-exports** them instead, exactly as it re-exports the reactive
seams it likewise cannot own. `IconData` is re-exported *renamed* as `DesktopIconData`, because
the facade already carries `frust-widgets`' `IconData` (an in-UI vector icon).

The shared core itself consumes exactly one field, the window title. Everything else is carried
for the per-OS shells. `DesktopConfig::default()` reproduces the zero-config preview window
exactly, so an app that configures nothing is unaffected by the seam's existence. The preview
window's initial logical size and maximized state are a separate, `DesktopConfig`-independent pair
of measurement knobs (`FRUST_WINDOW_SIZE`/`FRUST_WINDOW_MAXIMIZED`, applied by `app_handler`'s
`apply_window_size`) — see [DEVELOPMENT.md](DEVELOPMENT.md) § Instrumentation.

An icon is a decoded, tightly-packed RGBA8 buffer whose dimensions are an invariant checked at
construction — this tier carries no image decoder, and an inconsistent icon would reach a platform
API as a buffer overrun rather than a wrong picture. Decoding is the tooling tier's job (see
[CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md)).

### Menu activations

Menus are built with `muda` — an `NSMenu` tree on macOS, an `HMENU` on Windows — and activations
travel a **push bridge**, not a poll. The shared core is dirty-driven (`ControlFlow::Wait`) and
calls `pump` only from a redraw, so draining muda's process-global channel alone would leave an
activation sitting until some unrelated event produced a frame; Windows accelerators are the sharp
case, since the message hook reports the keystroke as handled and winit never sees an event to
wake on. So each per-OS crate installs muda's handler, filters the activation against the ids its
own spec declared, queues it in a bounded (oldest-dropped) queue, and requests a redraw through a
`MenuWaker` over the retained window.

`pump` then hands **exactly one** activation per frame to `frust_reactive::push_menu_event` and
re-wakes while more remain: the reactive menu source is a single-slot signal a frame reads once,
so a batch pushed between two frames would coalesce to its last entry. The queue also keeps the
push on the UI thread, which `push_menu_event` requires while muda's handler thread is the
platform's business. App code reads the other end through the facade's `menu_events()`, whose
`MenuEvent` carries the app's own item id plus a monotonic sequence so choosing the same item
twice reads as two activations. That read side compiles on **every** target (nothing pushes on
mobile), unlike the desktop-only spec vocabulary. See [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md)
for the reactive source itself.

macOS and Windows keep separate copies of this strategy — each owns its own platform's menu
vocabulary — and must not diverge in behavior.

### Per-OS desktop integration

| Concern | macOS | Windows | Linux |
|---------|-------|---------|-------|
| App identity | resolved app name titles the application menu (configured name → bundle display name → executable stem → default) | `app_id` becomes the process AppUserModelID, claimed before the first window exists so the taskbar honors it | `app_id` becomes the Wayland `app_id` and X11 `WM_CLASS`, via winit's `with_name` |
| Icon | not attached (macOS wants an application icon, which the bundle carries) | both `ICON_SMALL` (titlebar/alt-tab) and `ICON_BIG` (taskbar) | X11 window icon; Wayland has no window-icon protocol and sources it from the `.desktop` entry instead |
| Menu | always builds the standard application menu (About/Hide/Hide Others/Show All/Quit) and appends the app's spec after it, so even a menu-less app gets a conventional bar with a working ⌘Q | builds only the app's own spec, with `TranslateAcceleratorW` accelerators; a role this platform cannot perform is dropped rather than faked, and the quit role is realized by the shell itself (an owned item mapped to `WM_CLOSE` — muda's predefined quit is inert under winit) | none — the menu is widget-drawn |
| Theme | winit handles it | titlebar brightness follows the app's resolved theme through winit's `Window::set_theme`, re-issued on every change the core signals; a resolution arriving before the window exists is held pending | winit handles it |
| Close / quit | `quit_on_last_window_closed = false` hides the window and keeps the loop running; a Dock re-activation brings it back | default exit-on-close | default exit-on-close |
| Activation policy | winit's, deliberately: it sets `NSApplicationActivationPolicy` while launching, so a competing call here could only disagree with it | n/a | n/a |

Two macOS shapes are load-bearing. **Reopen** is observed through
`NSApplicationDidBecomeActiveNotification` on a notification observer registered beside winit's
delegate, because winit owns `NSApplication`'s delegate and `applicationShouldHandleReopen:` is
therefore unreachable. **Quit** stays on AppKit's own `terminate:` for both ⌘Q and the menu item,
because it must work while the window is hidden and no frames are being produced — which means
that route skips the whole handler `Drop` chain. Both carry accepted limitations
(`desktop-macos-reopen-gap-already-active`, `desktop-macos-quit-skips-executor-drop`,
and `desktop-macos-app-menu-title-process-name` for the unbundled menu title) in
[LIMITATIONS.md](LIMITATIONS.md).

Every per-OS crate compiles **inert off its own target**: native bindings sit in a
`cfg(target_os)` dependency table and each native entry point pairs its real body with a
do-nothing stand-in, so the whole workspace builds on any host while only the platform-free halves
(menu planning, the titlebar latch, the lifecycle state machine) stay host-testable. Failure is
always degradation — a refused menu install, an unparseable accelerator, or an unattachable icon
logs and leaves the app running without that one integration.

## Data Flow

### Desktop frame path

winit events drive `AppTree`'s rebuild → layout → paint, then hand the scene across the
render-split channel for the render thread to encode and present (or run the tail inline, under
the `FRUST_NO_RENDER_THREAD` kill switch). The loop is dirty-driven rather than continuous: a
frame runs when something asks for one, and a paced request resolves its next wake straight from
that paint's outcome.

The desktop shell persists a pipeline cache through `frust-paths`, so second-and-later launches
skip pipeline compilation on adapters that advertise `PIPELINE_CACHE` — in wgpu 30.0.1 that is any
Vulkan adapter (Linux, and Windows when the selected backend is Vulkan; verified in wgpu-hal
30.0.1's unconditional Vulkan feature set), never Metal or DX12 — so macOS never writes a cache.
Loading is unconditional file I/O at startup on every desktop OS and reads through to a legacy
macOS cache base when the current-location file is absent, migrating nothing — inert by
construction, since the legacy base is `Some` only on macOS, where a save never fires. The whole
path is best-effort: startup never fails on cache I/O. See
[CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md) for `frust-paths` itself.

### Mobile frame path

Both mobile shells gather an identical `FrameInputs` OR-list each tick — input-for-input
comparable across platforms — and consult the shared `frame_gate` for a run/skip decision before
rebuild → layout → paint → present. Android drives it from a Choreographer callback with a
conditional layout; iOS from a CADisplayLink tick, unconditionally.

`focus_or_ime_changed` is an *edge* (derived from a focus/IME generation counter, cached per
handle, committed only on a frame that actually runs), not a level: a steady focus session no
longer forces a frame every tick and no longer blocks pacing, so a paced (`CosmeticLoop`) frame
like a blinking caret throttles to the theme's `cosmetic_loop_rate` even while focused, while a
focus/IME transition still forces exactly one frame. `deferred_callbacks_pending` is the other
run-forcing input, a peek at CORE's pending-result-flush flag so a Housekeeping flush owed with
nothing else dirty still runs its frame.

**Per-request paced interval:** a paced request may name its own cadence, which reaches the shell
as `PaintOutcome::paced_interval` — core's own MIN-fold across every paced request in the pass.
Each mobile shell latches it for the *next* tick's pre-paint decision; desktop instead reads the
same value straight into its next wake, synchronously after the paint that produced it, and so
needs no latch. Either path resolves the interval through the identical `max(cap, requested)`
contract: the theme's `cosmetic_loop_rate` is a **ceiling, not a target**, so a request tighter
than the cap clamps up to it and only a slower request widens the cadence. The mobile pacing
anchor re-anchors to `now` when the interval changes, so a cadence flip cannot double-fire off an
anchor laid down under the old one, and a tick carrying the focus/IME edge tightens pacing back to
the bare theme cap regardless of any longer per-request interval (see
[LIMITATIONS.md](LIMITATIONS.md)).

### iOS render-thread split

`frust-shell-ios`'s default (split) executor overlaps startup rather than serializing it: the
dedicated render thread is spawned, and starts GPU bring-up (adapter/device/renderer-ready) on its
own, *before* the UI thread joins the background font-preinit thread it spawned at entry — so the
GPU work and the font wait, the two longest serial stretches of iOS cold start, run concurrently
instead of back to back. The two threads share one startup-span recorder (`SharedStartupSpans`)
behind a `StartupRecorder` that is `Owned` for the inline (`FRUST_NO_RENDER_THREAD`) executor and
`Shared` for the split one; every `Shared` method locks only for the body of its own record/take
call, never across the blocking GPU tail (drawable acquire plus submit), which would be a UIKit
watchdog hazard. Once the startup line is taken and emitted on the first present, the render thread
drops to a lock-free `Owned` handle for every later frame. Because the two threads race to record
onto one shared line, the spans do not print in causal order — read each span's own delta, not its
position.

Frame pacing itself is unchanged: the surface's desired maximum frame latency stays at its constant
(two frames in flight, `crates/frust-gpu/src/surface.rs`), and pre-acquiring a drawable ahead of the
frame that needs it was measured and rejected — a drawable parked across a `Pause` is a UIKit
watchdog hazard. What exists instead is a `perf-trace`-only diagnostic behind the `FRUST_PACE_TRACE`
dial: `ios-pace`, one `frust-perf ios-pace` line per rendered frame decomposing wake/idle/acquire/
submit/loop time plus `p2p_us` — submit-to-submit cadence between frames whose GPU work was actually
submitted for presentation. A frame that presented nothing reports `p2p_us=NA` and leaves the
cadence base untouched; under armed present-sync the UI thread commits the drawable later than the
render thread's own submit, so the render thread never observes that later commit. `FramePasses::total`
is not an interval to read a dropped vsync out of: it sums the UI and render threads'
concurrently-running spans, so it is a cost.

### Web frame path

The single-thread inline executor (`WebFrameExecutor`, `crates/frust-shell-web/src/render.rs`) —
mirroring the desktop core's `FrameExecutor::Inline` encode→acquire→submit tail — is this tier's
**first-class** path, not a fallback: `wasm32-unknown-unknown` has no OS threads and wgpu's handle
types are not `Send`/`Sync` there, so there is no split executor to fall back from. Redraws ride
`requestAnimationFrame`; a paced wake instead rides winit's `ControlFlow::WaitUntil`, serviced by
the browser's Prioritized Task Scheduling API (falling back to `setTimeout`) rather than the
heavily-clamped bare-`setTimeout` chain a naive implementation would hit — every deadline computed
on `web_time::Instant` (`crates/frust-shell-web/src/pacing.rs`), the identical type winit itself
declares `ControlFlow::WaitUntil` over on this target. The swapchain is reconciled against the
canvas's real size every frame, because winit reports `inner_size` as 0x0 until its
`ResizeObserver` has fired at least once. Surface bring-up is `async` all the way out to the
caller — no `pollster::block_on`, which would block the page's one JS thread — and retries a cold
page's `requestAdapter()`, which can spuriously answer `null` once on a machine with a perfectly
good adapter (a GPU-process warm-up race). There is no pipeline-cache I/O: `frust-paths` writes
files and a page has no filesystem, so the renderer runs on its empty initial cache every launch.

The tier's gate vehicle is the standalone [examples/web-gallery](../examples/web-gallery)
workspace — an ordinary `frust::web_app!` app exercising input, theme-follow, resize/DPR and a
signal-driven repaint in the browser; see its own README for the milestone evidence.

### Cross-cutting host signals

- **Event pass under root owner:** every shell routes input through `AppTree::event` inside a
  reactive `Owner.with` wrap, so handlers see root-level contexts (`Theme`, `WindowMetrics`,
  root-component `init`-installed values). `frust-reactive` is a leaf crate and cannot wrap
  itself. The pass is deliberately **not tracked** — entering a tracked scope resets dependencies
  and clears the dirty flag, which would unsubscribe the frame loop from pending wakes. Desktop
  forwards the secondary mouse button as `PointerButton::Secondary` through a delivery-latched
  gate — an Up dispatches iff its Down did — guaranteeing every Down/Up pair reaches `AppTree`
  paired even under pointer capture.
- **Window metrics** (logical size, scale, derived orientation, insets snapshot) are published
  from the points where the window's shape actually changes, guarded by the shared
  `WindowMetricsPublisher` against per-frame churn (see
  [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md)). Desktop seeds insets as the default, since winit
  0.30 offers no cross-platform safe-area accessor.
- **Android edge-to-edge:** `FrustActivity`, not the manifest theme, owns edge-to-edge — `onCreate`
  calls androidx `enableEdgeToEdge` with transparent status/navigation-bar `SystemBarStyle`s, then
  on API < 35 adds `FLAG_DRAWS_SYSTEM_BAR_BACKGROUNDS` and (API 29+) clears the status/nav-bar
  contrast scrim, so transparent bars and real window insets arrive regardless of the manifest
  theme; API 35+ enforces edge-to-edge platform-side regardless of any of this. The scaffold
  template names `@android:style/Theme.Material.NoActionBar`; a still-legacy theme (missing
  `windowDrawsSystemBarBackgrounds`) keeps working — the added flag covers it — but logs one
  `frust`-tagged warning naming the migration recipe (see
  [SHELLS_DEVELOPMENT.md](SHELLS_DEVELOPMENT.md)). Every real inset change logs `frust-insets
  view_padding l=… t=… r=… b=… view_insets l=… t=… r=… b=… scale=…`
  (`crates/frust-shell-android/src/app/surface.rs`) — debug builds only (`cfg!(debug_assertions)`),
  since the Android logger is capped at Info in every build so an unconditional `log::debug!` would
  never surface.
- **Theme and brightness** reach widgets through `RenderRoot::set_theme` and app code through a
  re-provide under the root owner; on desktop a third path fires the per-OS brightness hook, gated
  on the resolved brightness actually moving so an override swapping one dark theme for another
  fires nothing. Android additionally reads the app's resolved brightness back over JNI — after
  every appearance change and once per frame — so a theme forced from Rust reaches the status bar
  without waiting for a platform event.
- **Accessibility:** `frust-core`'s semantics pass reports every node's `bounds` in logical
  pixels — the same layout unit paint scales from — while accesskit expects physical pixels. The
  desktop shell's `build_tree_update` stamps a root `Affine::scale(window.scale_factor())`
  transform onto the accesskit `TreeUpdate` it pushes (left `None` at scale 1.0, matching
  accesskit's own identity-transform guidance), converting the whole tree through accesskit's
  ancestor-transform rule so an assistive-technology client reads physical-pixel bounds matching
  the DPR-scaled paint pass. The mobile shells own their own accessibility bridges
  (`accesskit_android`, `accesskit_ios`) rather than sharing this path.
- **Reduced motion** is the one host signal whose sensor is not the appearance sensor: Android
  watches the animator duration scale via a `ContentObserver` plus an `onResume` re-read, iOS the
  UIKit reduce-motion notification plus an activation re-read, and desktop has no source at all (a
  checked gap — winit exposes nothing). Delivery bypasses the theme-override path on purpose,
  which would otherwise latch an override and pin brightness; each shell edits the motion field
  directly and marks appearance dirty so the frame gate cannot skip the carrying tick.
- **IME content type:** each focused widget publishes a content type (Normal/Password/
  NoSuggestions/Terminal) and each shell destructures the editing state into its platform's
  payload per that platform's capabilities — Android into `EditorInfo` input-type and IME flags
  (with a restart when the type changes on a steady-focused field, since Android never re-queries
  a bound connection), iOS into the `UITextInputTraits` matrix over its shared responder (every
  managed trait assigned in every arm, so no prior classification survives a switch), desktop into
  winit's `ImePurpose`, which is a cosmetic hint on Wayland only and never a secure-text-entry
  boundary — a desktop password field gets no platform protection from it. Both mobile shells
  reconcile the platform's IME mirror on a per-frame poll behind a divergence guard rather than
  on focus edges alone. The iOS Swift half remains compile- and device-unverified on the Linux
  build host — see [LIMITATIONS.md](LIMITATIONS.md) `ime-ios-content-type-unverified`.
- **Cursor:** desktop reads `RenderRoot::cursor()` after every event dispatch, beside
  `sync_ime` (`ShellHandler::sync_cursor`), and applies it via winit's `Window::set_cursor` only
  when it differs from the shape last pushed. `winit_cursor_for` (`frust-shell-desktop`) is the one
  platform touch point mapping the framework's `CursorIcon` onto winit's own vocabulary; a mobile
  shell never reads the resolved value. Web carries the identical request/resolve contract and a
  real implementation (winit's web backend writes the canvas's CSS `cursor` property), unlike its
  semantics/devtools seams below. See [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md)'s Hover
  and Cursor section for the request/resolve contract.
- **Clipboard:** the framework side is two one-shot slots plus one inbound verb —
  `AppTree::take_clipboard_write`/`take_paste_request`, drained after a dispatch, and
  `InputEvent::EditCommand`, dispatched back. [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md) owns
  those slots and the rule an asynchronous read inherits: the answer is a later focus-routed
  dispatch carrying no identity, so it is bound to the session (`focus_epoch`) recorded when the
  paste request is drained and dropped when that session has gone. Which tier reads which way is
  this tier's half: desktop and Android asynchronously, iOS synchronously.
  **Desktop** drains both slots beside `sync_ime`/`sync_cursor`, because either can fill as a side
  effect of any event, and hands each to a lazily spawned worker thread owning the process's single
  `arboard::Clipboard` for the run's lifetime: on X11 and Wayland a copy is served *live* by the
  process that claimed the selection, so an instance created per call takes the copy with it when it
  drops, and a read blocks for as long as that owner takes to answer (24 s, measured, against a
  stopped one) — which is why the read may not run on the event loop, and why both operations
  serialise on one thread rather than sharing an instance behind a lock. A finished read comes back
  through the event-loop proxy as a fresh top-level dispatch on a later turn, never nested in the
  pass that asked. A request raised while a read is outstanding folds into that read rather than
  queueing a second such wait, and re-points its answer at the most recent asker — the only asker an
  answer can still reach. That gate re-opens as the worker *posts* an answer, not as the UI thread
  consumes it, so a further read can start inside that window; the record is taken on arrival, so a
  surplus answer finds nothing addressed to it and is dropped, the deliberate cost being that two
  presses close enough to race it yield one paste rather than two. Shutdown waits, briefly, for the
  worker to drop the instance so a clipboard manager can adopt the copy.
  **Web** cannot go through winit here either, because a browser hands clipboard access to the
  focused editable and nobody else: the hidden IME overlay below carries the `copy`/`cut`/`paste`
  listeners (plus `beforecopy`/`beforecut`, which claim the verbs for a collapsed selection),
  reading `clipboardData` directly on a paste and writing with `setData` on a copy or cut, inside
  the callback, where neither a secure context nor a permission is needed. A paste keystroke is
  withheld from the canvas re-dispatch, since the DOM event carries text no key event could; a copy
  or cut keeps the key path — an engine may raise no event at all for this overlay, and a bridge
  waiting for one would drop the gesture — and marks a handoff, so the widget's own answer is what
  the callback writes and one gesture stays one write. A write no callback is coming for (an app- or
  toolbar-driven copy) and a toolbar paste take `navigator.clipboard`, undefined outside a secure
  context: an `http://` page loses those outright, warning once, while the keyboard's paste is
  unaffected.
  **Android** owns the clipboard on the JVM side (`ClipboardManager` has no Rust-reachable binding),
  so the channel is a JNI trio: the Kotlin view drains `nativeTakeClipboardWrite` and
  `nativeTakePasteRequest` once per frame, and pushes verbs back in through `nativeEditCommand` from
  two host routes — the IME's own affordances through
  `FrustInputConnection.performContextMenuAction` (a soft keyboard never sends chords) and hardware
  `Ctrl+C`/`X`/`V`/`A` decoded in `onKeyDown`. Both are gated on a focused editable, since an edit
  command is focus-routed and consuming the press for nobody would take it from the host activity
  for nothing. A clip whose text must be coerced resolves off the view's thread under the same
  session guard as desktop, bounded additionally by a 2 s deadline stamped at request time, since
  nothing can interrupt a thread already parked inside another app's `ContentProvider`. One
  resolution is outstanding at a time here too: a request raised while one runs folds into it,
  re-pointed at the most recent asker with its deadline re-stamped from that press, rather than
  starting a second. That record is held until the UI thread *takes* the answer rather than
  released as it is posted, so no second resolution can start inside that window at all. Two system
  behaviours are lived with rather than engineered around: `getPrimaryClip` yields nothing unless
  the app holds input focus (Android 10+), indistinguishable here from an empty clipboard on
  purpose; and reading another app's clip raises the system "pasted from" toast (Android 12+), which
  is why the cheap `hasPrimaryClip` gate runs first and only a real, user-initiated paste ever
  crosses that line.
  **iOS** *locks* the `Native` toolbar policy at start-up — an app's later
  `set_selection_toolbar_policy` is refused, not obeyed, because the exemption below depends on the
  native route being the only one — so a field floats no pod and UIKit's own edit menu is the only
  menu on screen, presented at the published anchor through `UIEditMenuInteraction` (iOS 16+,
  `UIMenuController` on 15) and answered from the published verb set, with the framework's view as
  the app's single responder for both the menu and a hardware `Cmd+C`/`X`/`V`/`A` arriving over
  `UIResponderStandardEditActions`. A focused field publishes those verbs whether or not any menu is
  up, because the responder chain is asked for them with nothing on screen. Native is the platform's
  requirement rather than a preference: `UIPasteboard.general.string` is exempt from iOS's paste
  notice and per-app permission alert only when the read is system-initiated, which `paste(_:)` is
  and a framework-drawn Paste button — answerable only by reading the pasteboard off a display-link
  tick — is not. That read runs synchronously on the main thread inside `paste(_:)`, so this tier
  has no in-flight window to guard at all. Both mobile tiers' clipboard paths are device-unverified.
- **Web IME bridge:** winit's web backend never emits `WindowEvent::Ime` (its `web_sys` backend
  implements none of the IME setters — upstream issue 4424 is open with no timeline), so
  `frust-shell-web::ime` bypasses it with one hidden `<input id="frust-ime-overlay">` under
  `document.body` (`opacity: 0`, `pointer-events: none`), opened when the focused widget publishes
  an active `ImeState` (`focus_ime_generation`) and removed when it stops. Its attributes come from
  the focused field's `ImeContentType`: a secret field gets `type="password"` plus
  `autocomplete="new-password"`, a suggestion-refusing field adds `inputmode="text"`, and a hint
  that moves tears the element down and rebuilds it rather than mutating it, since a browser only
  reads `type` when it classifies an element it has not seen before — checked from a dispatched
  event and from the frame loop alike, so a focus move a signal drove (no input event behind it) is
  answered on the next frame: the replacement inherits the DOM focus the old element held, an
  element that had lost focus is closed instead (a frame never takes focus for the user), and the
  element's value is cleared on every pass no composition owns. Composition tracking is an explicit
  three-state session, because an *empty* `compositionend` is not reliably a cancel: several
  browsers end a composition that way and deliver its text on the `input` behind it, so the preedit
  is held one signal longer and resolved as that commit when one arrives, as a clear otherwise.
  winit's own `keydown`/`keyup` listeners stay on the canvas rather than `document`, so the overlay
  re-dispatches each plain keystroke onto the canvas as a copy — except a `keydown` the browser
  reports as `Unidentified` (what a mobile soft keyboard sends for most of its keys), whose edit is
  taken from the `input` signal behind it and delivered once as a key event instead. A soft
  keyboard's Backspace is the known gap: the element is emptied every frame, so the deletion has
  nothing to consume and raises no `input`. Removing the element mid-composition ends the session
  and retracts the preedit, the same as a blur. The same element is the page's clipboard surface —
  its verb listeners and the exclusion keeping a clipboard keystroke from reaching a widget twice
  are the Clipboard bullet above. Not yet wired: the mobile visual-viewport jump when a soft
  keyboard opens, and multiple simultaneous editables. The bridge is compile- and
  unit-tested but device-unverified — this build host has no browser rig — see
  [LIMITATIONS.md](LIMITATIONS.md) `web-ime-residual-gaps`.
- **Web host signals with no browser counterpart:** accessibility (AccessKit ships no web
  adapter — a canvas app needs its semantics mirrored into real DOM/ARIA elements) and the in-app
  devtools UI-thread hop (a `wasm32` build has no sockets for its loopback listener, so
  `frust-shell-web` forwards no `devtools` cargo feature at all) are each a documented no-op rather
  than a silent gap (`crates/frust-shell-web/src/app_handler.rs`'s `push_semantics`/`pump_devtools`).
  See [LIMITATIONS.md](LIMITATIONS.md) `web-a11y-devtools`.
- **Platform-view embedding:** paint-time view frames feed the `platform_view` differ, which
  exposes an idempotent create/update/dispose backlog. The two mobile shells **poll** it across
  their FFI boundary and apply it frame-paired against present, converting each rect to physical px
  on the way. Desktop has neither boundary nor poller: the differ is driven straight from the winit
  loop — ingest after paint, then **push** the batch to
  `DesktopExtensions::on_platform_view_commands` after the frame is submitted, acknowledged as the
  hook returns, rects left in **logical points** (the hook also gets the window's scale factor, for
  a host that does need physical px; AppKit does not). macOS is the one host implementing it,
  parenting the plugin-provided `NSView` above winit's content view — Mode A, so nothing is
  forwarded into it and the frust slot behind it is simply covered. Pushing before present means a
  hosted view can lead its surroundings by **at most one frame**
  (`desktop-platform-view-frame-lead` in [LIMITATIONS.md](LIMITATIONS.md)); pairing would need a
  presented-frame id the desktop executor does not publish. A suspend removes every hosted view
  rather than hiding it, and every surface bring-up replays `Create` + `Update` per live slot.
- **Surface-mode resolution:** each mobile shell resolves the host's declared translucency mode
  against actual surface capabilities at configure time and republishes the resolved verdict every
  frame.
- **Devtools UI-thread hop** (feature-gated): a backend call needing live app state queues onto a
  process-wide request each shell drains once per frame — desktop wakes via its winit event-loop
  proxy, the mobile shells have no wake and pump at the top of every tick since their display loop
  runs continuously while resumed. An injected event dispatches through the shell's real input
  path, never directly against widget code. `FRUST_DEVTOOLS=0` is the one security-relevant runtime
  kill switch here — it skips starting the service even in a `devtools`-featured build. Full detail
  in [DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md).
- **Kill switches:** a set of additive, off-by-default env vars (`FRUST_NO_RENDER_THREAD`,
  `FRUST_NO_FRAME_GATE`, `FRUST_NO_ANIM_PACING`, `FRUST_NO_RESAMPLE`) each revert one
  frame-pipeline seam independently.

## Key Types

| Type | Purpose |
|------|---------|
| `AppTree` / `new_boxed_app` / `new_boxed_app_with` | Type-erased app driver each shell's FFI/event-loop layer owns and calls into for every lifecycle callback. Beside the lifecycle passes it carries the host-signal drains a shell polls: `take_clipboard_write()`/`take_paste_request()` (destructive, one-shot) and `selection_toolbar()`/`selection_toolbar_generation()` (a level plus a counter to diff), each delegating to the matching `RenderRoot` accessor |
| `FrameGate` / `FrameInputs` / `FrameDecision` | Shared run/skip decision the continuous-loop mobile shells consult every tick |
| `RenderCommand` / `Ack` / `AckWaiter` / `SceneFrame` | UI-thread↔render-thread lifecycle and scene-handoff vocabulary underlying each shell's default split frame path |
| `PlatformViewState` / `ViewCommand` | Generation-stamped native-sibling create/update/dispose backlog — polled by a mobile shell's embedding module, pushed by the desktop shell to its per-OS host |
| `SurfaceMode` / `ResolvedSurfaceMode` / `SurfaceModeWatcher` | Host translucency declaration latch plus per-frame resolved-mode publication |
| `WindowMetricsPublisher` | Shared, change-guarded path every shell drives to provide window-shape context without per-frame re-provides (see [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md)) |
| `DesktopExtensions` / `NoExtensions` / `CloseAction` / `DesktopEventLoopBuilder` | The per-OS desktop seam: its eight hooks — including `on_platform_view_commands` (the per-frame native-view batch) and `on_platform_views_suspended` (remove every hosted view) — the whole-set no-op, the close verdict, and the builder type a platform crate extends |
| `DesktopConfig` / `MenuSpec` / `MenuItemSpec` / `MenuRole` / `IconData` | Platform-independent desktop identity and native-menu vocabulary, re-exported by the facade (`IconData` as `DesktopIconData`) |
| `MacosExtensions` / `WindowsExtensions` / `LinuxExtensions` | The three per-OS implementations, each built from a borrowed `DesktopConfig` and installed by the facade under its own target `cfg` |
| `android_app!` / `ios_app!` / `web_app!` / `app!` | Facade macros binding a generated app's state/logic to the fixed JNI / C-ABI export set, the wasm-bindgen start shim, and the desktop entry point |

## Verification State

The desktop tier is unevenly proven, and the gap is tracked rather than assumed closed:

- **macOS** — runtime-verified on hardware: real windows, the app-named menu bar, ⌘Q by both menu
  and accelerator, Hide/Show All, and close-then-Dock-click reopen.
- **Windows** — runtime-verified on hardware: titlebar/taskbar icon, AppUserModelID identity,
  titlebar theming, menu-bar activation delivery, accelerators, and quit. Two Win32 menu rules the
  pass established are load-bearing and documented at their `menu.rs` sites: muda's predefined quit
  dead-ends under winit's pump, and an accelerator enters the `HACCEL` only if its submenu is
  attached before the item is appended.
- **Linux** — proven by cross-target compilation only, plus every native call site read against
  its vendored source. Owed: a non-headless `app_id`/icon pass. See
  [LIMITATIONS.md](LIMITATIONS.md) `desktop-shells-runtime-unverified` and
  `desktop-windows-titlebar-theme-revert`.

Multi-window is out of scope at this tier — every desktop shell manages exactly one window
(`desktop-single-window`).

## See Also

- [SHELLS_DEVELOPMENT.md](SHELLS_DEVELOPMENT.md) — device gates and this unit's version pins
- [CODE_STANDARDS.md](CODE_STANDARDS.md) — sanctioned-unsafe register, no-unwind-across-FFI rule
- [LIMITATIONS.md](LIMITATIONS.md) — the accepted-limitation entries referenced above
