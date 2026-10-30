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
  `pollster::block_on` bring-up, a render thread and `frust-paths` cache I/O, none of which exist on
  `wasm32-unknown-unknown`. It carries its own copy of that core's winit-*generic* halves (input
  mapping, the reactive-owner event wrap, the `WindowMetrics` publish, brightness-follow) under a
  must-not-diverge contract (`crates/frust-shell-web/src/app_handler.rs`'s header).

Every concrete shell drives rebuild → layout → paint → encode → present and translates
platform-native input/lifecycle/theme/insets/IME/deep-link/back/platform-view signals into the
framework's own vocabulary.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how SHELLS relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `frust-shell-common::app_tree` | Type-erased `AppTree` driver letting a native shell own and drive any app's rebuild/layout/paint/event/semantics cycle without generics |
| `frust-shell-common::frame_gate` | Shared run/skip frame decision and pacing used by both continuous-loop mobile shells; `LayoutKnobs::resolve(no_skip, audit, no_retained)` takes one `KnobSource { compile, runtime }` per knob (`FRUST_NO_LAYOUT_SKIP`, `FRUST_LAYOUT_AUDIT`, `FRUST_NO_RETAINED_LAYERS`) |
| `frust-shell-common::render_split` | UI-thread/render-thread split vocabulary (scene handoff, lifecycle commands, completion barrier) every shell's default frame path uses. `FrameMeta.paint_serial` travels with the `SceneFrame` and every encode site calls `SurfaceRenderer::note_scene_serial` first; the helper `publish_resident_pages(frame, pages)` is what each shell's one-line bridge calls to publish the ledger after a committed scene |
| `frust-shell-common::platform_view` | Differ turning per-paint platform-view frames into an idempotent create/update/dispose backlog for embedding native views |
| `frust-shell-common::resample` | `PointerResampler` — one lane per `PointerId`, emitting a frame-boundary-interpolated `Move` per contact while passing `Down`/`Up`/`Cancel` through losslessly; shared by both mobile shells |
| `frust-shell-common` (signal-poll seams) | Small process-global slot-plus-poll seams (surface mode, theme override, fonts, system UI) drained once per frame — surface mode and system UI on mobile only; theme override, fonts and the default locale on every shell, desktop included: a per-frame `LocaleDefaultWatcher`, polled beside `ThemeOverrideWatcher`, reports a change of `frust_text::default_locale_generation`, and the shell re-pushes the active theme so a layout + paint pass re-faces untagged text. `locale_default` also holds `BackgroundFlush` (atomic, one trigger per background transition, re-armed on foreground) and the `FontCacheFlush` trait `TextContext` implements, which flush the persisted font-oracle cache (`flush_font_cache`): iOS pause/resume (`IosAppHandle`), Android `native_on_pause`/`native_on_resume`, desktop `suspended`/`resumed` and window focus loss/gain (winit rarely suspends), web `pagehide`/`pageshow`/`visibilitychange` via `ShellUserEvent::PageHidden`/`PageShown` (a no-op: no cache file). Also the process-global retained-ledger slot (`publish_retained_ledger`/`latest_retained_ledger`: `try_lock`, never blocks the UI thread, one-frame-stale by design) and the repaint-owed flag (`request_retained_repaint`/`take_retained_repaint`; Android/iOS frame gates OR it in, desktop/web redraw instead) |
| `frust-shell-common::devtools` (feature `devtools`) | Shell-side `DevtoolsBackend` implementation plus the per-frame UI-thread hop and pump each shell drives; see [DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md) |
| `frust-shell-common::gpu` (feature `gpu`) | Process-wide install-once slot (`install_gpu_handle`/`gpu_handle`) a shell publishes its live GPU device handle into, read back through `frust::gpu::with_context` (see [RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md)'s GPU Seam). `frust-shell-desktop` publishes at both device-creation sites; Android and iOS forward the feature but install no handle yet, so `with_context` answers `None` there |
| `frust-shell-common::overlay` | The platform-neutral overlay core every overlay host shares: `OverlayTable<L, S>` (per overlay id a layer plus its surface; see Native-view hosting), the `OverlaySwitch` parser for `FRUST_OVERLAYS` (first non-blank source wins: runtime then compile-time `FRUST_OVERLAYS`, then the `FRUST_OVERLAY_SPIKE` alias the same way, so a runtime `FRUST_OVERLAY_SPIKE=0` switches off a baked-in alias), the `FrameRequest` latch, `assemble_overlay_cuts` and the `SkewWindow` present-skew report. Generic over layer and surface, no GPU or platform type, no `unsafe` |
| `frust-shell-desktop` | The shared winit core: event loop, UI-thread/render-thread surface split, accessibility adapter, paced wake, pipeline-cache and window-state persistence — plus the `DesktopExtensions` seam and the `DesktopConfig`/`MenuSpec` vocabulary the per-OS crates read. Also the zero-config dev-preview entry point |
| `frust-shell-desktop::platform_view` | The OS-neutral desktop platform-view host: owns the shared differ, ingesting each paint's frames right after `RenderRoot::paint` and draining the batch to the per-OS hook right after the frame is submitted, then acknowledging it; plus the two lifecycle resets (replay on surface re-create, drop-everything on suspend) |
| `frust-shell-macos` | AppKit integration: the native menu bar (a standard application menu plus the app's own spec), hide-on-close and Dock-reopen lifecycle, the reopen observer in its `appkit_glue` unsafe zone, and the platform-view host below |
| `frust-shell-macos::platform_view` | The AppKit (Mode A) native-view host: resolves a slot's `view_type` to a plugin factory, parents the returned `NSView` above winit's content view, and places/shows/disposes it from every later command. Its slot bookkeeping, geometry mapping and retain accounting sit behind a `NativeViewOps` seam and compile on every host; only the AppKit implementation is `cfg(target_os = "macos")` |
| `frust-shell-windows` | Win32 integration: taskbar identity, window/taskbar icons, an `HMENU` menu bar with keyboard accelerators, and titlebar brightness — every `unsafe` and every `windows-sys` call confined to `win32_glue` |
| `frust-shell-linux` | Wayland `app_id`/X11 `WM_CLASS` plus the X11 window icon, through winit's own cross-platform API. Deliberately thin: no GTK/X11 binding, no native menu (the menu is widget-drawn), no `unsafe`, one hook implemented |
| `frust-shell-android` | Sanctioned-unsafe JNI FFI boundary, app-binding macro, and Choreographer-synced frame pipeline; carries the theme precedence ladder and the conformance-pinned resolved-surface-mode publish site; hosts the host-testable `overlay_state` (`OverlayLedger`, `OverlayMode`, `pair_cuts`) and segmented render tail (see Native-view hosting) and calls `frust_render::native_import::install` first in `render_scene` |
| Render-thread scheduling (`schedule_render_thread`, Android and iOS) | Each mobile `render_loop` starts by putting its thread in `frust_reactive::thread_policy`'s `Frame` class (Android: nice `min(inherited, -4)`, so the UI thread's inherited -10 is kept; iOS: `QOS_CLASS_USER_INTERACTIVE`); Android logs `thread_policy::runtime_policy_summary()` there. The background runtime keeps tokio's blocking-pool size (no cap, no thread-start hook) and lowers only blocking-pool threads, once each on their first job, inside the `spawn_blocking` facade (`Background`: nice reference + 10 / `QOS_CLASS_UTILITY`; idle keep-alive `BLOCKING_KEEP_ALIVE`, 10 s); its async workers keep what they inherit. `FRUST_NO_THREAD_TUNING` restores the fixed render-thread priority (Android's -4 written over the inherited value) and tokio's defaults; desktop and web schedule nothing |
| `frust-shell-android` pipeline-cache persistence (`jni_glue::CachePersistence`) | The split render thread persists the Vulkan pipeline-cache blob once per surface after the warm-up is done (checked after each frame), again on `SurfaceDestroyed` after the ack and on a stray `Pause`, only when it differs from the bytes the file holds — a slot a writer updates only after its write succeeded. Writers run detached (`frust pipeline-cache writer`) with a generation ticket taken on the reading thread, so writes serialize and an older blob never lands over a newer one; a persisted blob also seeds the process's next install, and a writer thread the OS refuses is retried on the next frame. Log lines: `pipeline cache persisting N B <after warm-up\|on surface destroy\|on pause> (<n> pipelines)`, then `pipeline cache written N B <moment> to <path>`, or `… (unchanged, not rewritten; …)`. The inline `FRUST_NO_RENDER_THREAD` path persists once at install (`… at install`); iOS has no blob (Metal lacks `PIPELINE_CACHE`) |
| `frust-shell-android::adpf` | The opt-in performance-hint (ADPF) session, off unless `FRUST_ADPF` is set to a non-`"0"` value (otherwise the render thread logs `ADPF off (default; set FRUST_ADPF=1 to opt in)`): one session per process over the UI and render threads, owned by the render thread's `RenderHint` (`RenderHint::open(ui_tid)` in `render_loop`) and parked in a process-wide slot; opened on the first rendered frame of a continuous loop while a surface is installed and the activity resumed, closed on pause (`suspend`, from the UI thread, until `resume`), on surface destroy and when the render thread exits; target = the display period from recent Choreographer deltas, actual = the frame's UI spans plus its encode span, one report per frame (API 35+ through `AWorkDuration`, 33-34 the basic call). Its NDK entry points are `dlsym`-resolved from `libandroid.so` in the Android-only `platform` submodule, so a missing symbol (below API 33) leaves it off; the session logic is host-tested in `tests/adpf.rs`. Measured and kept opt-in (`android-adpf-opt-in-measured` in LIMITATIONS.md) |
| `frust-shell-android` frame sync (`ffi_support::FrameSyncLatch`) | One JNI crossing per `doFrame`, `nativeFrameSync(handle) -> Long`, answers which of Kotlin's polls have anything to report: bit 0 IME surface changed (or an IME-side input arrived), 1 clipboard write pending, 2 paste request pending, 3 system-UI generation advanced, 4 app brightness flipped; Kotlin then makes only the `nativeImeState`/`nativeTakeClipboardWrite`/`nativeTakePasteRequest`/`nativeSystemUiState`/`nativeAppIsDark` calls whose bit is set, while the platform-view command poll stays per frame by design (its peek acknowledges the last batch). The latch holds a drained clipboard value until the matching `nativeTake*` call and records the IME and brightness baselines Kotlin last read, including reads outside a sync (`native_ime_state`, `native_app_is_dark`); it is one latch per process, reset when a different handle syncs. `FRUST_NO_FRAME_SYNC` (any non-`"0"` value) answers `FRAME_SYNC_ALL_BITS` (`0x1F`) without syncing, so every poll runs. A `perf-trace` build arms Kotlin's poll timers through the additive `nativePerfTrace` export and logs one `frust-perf jni ticks= sum_us= sync_us= polls_us= timer_overhead_ns=` line per 120 frames; `tests/frame_sync.rs` covers the latch |
| `frust-shell-ios` | Sanctioned-unsafe C-ABI FFI boundary, app-binding macro, and CADisplayLink-driven frame pipeline, with optional present-sync gating against platform-view geometry |
| `frust-shell-web::app_handler` | Winit-generic host-signal translation ported from `frust-shell-desktop` (input mapping, the reactive-owner event wrap, theme delivery, window-metrics publish) plus, gated to `wasm32`, the `browser_loop` submodule owning the canvas-bound event loop and frame turn (`spawn_app`/`run_app`) |
| `frust-shell-web::pacing` | Pure paced-wake decision logic composing the browser's two wake mechanisms (`requestAnimationFrame` redraws, `ControlFlow::WaitUntil` deadlines) |
| `frust-shell-web::render` | The single-thread inline frame executor (`WebFrameExecutor`) over the wgpu surface: swapchain reconciliation, encode/acquire/submit, and the cold-page adapter-retry bring-up |
| `frust-shell-web::input` | `TouchTracker` — maps every winit touch `id` to a per-sequence slot and emits `InputEvent::PointerContact` for each contact, the mobile shells' multi-contact model; the one input mapping with no `frust-shell-desktop` twin |
| `frust-shell-web::ime` | The hidden-`<input>` overlay bridging real browser composition into `ImeEvent::Compose`/`Commit`, and the canvas re-dispatch that keeps plain typing on winit's existing key path — see Cross-cutting host signals below |
| `frust-shell-web::logging` | Routes the `log` facade to the browser console (`console_log`/`console_error_panic_hook`), idempotent against the facade's own install, plus a `?log=` query-param level knob |

The Android (`platform/android/frust-embedding`), iOS (`platform/ios/FrustEmbedding`) and web
(`platform/web`) embeddings ship inside their shell crates, so a project reaches them wherever cargo
resolves those crates. `frust-drive`'s `packages::locate` finds the directory and
`platform_wiring::sync` records it per host: Android's `frust.embedding.dir` key in the gitignored
`local.properties`, the iOS `ios/FrustEmbedding` symlink, and for web a build-time lookup by
`web_build` (see [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md)).

## Layer Dependencies

Every concrete shell depends on `frust-core` (`RenderRoot`, widget tree, input, insets,
semantics), `frust-scene` (renderer-agnostic `Scene`/`SceneBuilder`), `frust-text` (a
`TextContext` threaded through layout), `frust-theme` (theme delivery, brightness/design-language
state), `frust-render` (`SurfaceRenderer`, encode/present, pipeline cache) and `frust-reactive`
(`ReactiveRuntime`, `deep_link`, `back`, `menu`, `task`). Desktop and Android also depend on
`frust-paths` for cache-dir persistence, and Android on `frust-plugin` to install the
JavaVM/Context handle plugins read — the same `nativeInitPlatform` call first installs the app's
files/cache directories into `frust-paths` (`install_android_dirs`), so `data_dir()`/`cache_dir()`
resolve there.

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
the shared core depends on them.

`frust-shell-macos` additionally depends on `frust-plugin`, to resolve a slot's `view_type` through
the desktop view-factory registry: the **second shell → substrate edge** after `frust-shell-android`'s,
for the same reason — a shell reads the leaf substrate to host a plugin's native view without
depending on any plugin (see [PLUGINS_ARCHITECTURE.md](PLUGINS_ARCHITECTURE.md)). That edge and the
`frust-shell-common` one are unconditional, so the OS-neutral core is compiled and unit-tested on
every build host.

**`frust-shell-web` depends only on `frust-shell-common`**, never on `frust-shell-desktop`, for the
Overview's reasons. Its `winit` edge rides the desktop tier's workspace pin — a second `winit`
identity would be a resolution hazard.

**Hot patching is an opt-in feature chain, never default.** `frust`'s `hotpatch` forwards to
`hotpatch` on `frust-core`, `frust-shell-common`, `frust-shell-desktop`, `frust-shell-android` and
`frust-shell-ios`; each shell's own `hotpatch` implies `devtools` and forwards `frust-core/hotpatch`
and `frust-shell-common/hotpatch`. The shared leaf's feature carries the in-app apply (chunk
decode, the unix loopback patch-file hand-off, a safe entry into `frust-hotpatch`) over the
devtools transport ([DEVTOOLS_ARCHITECTURE.md](DEVTOOLS_ARCHITECTURE.md)); absent it
`frust-hotpatch` is not in the graph. Each shell registers one `frust_core::set_patch_listener`
callback, which runs on the applying thread and must only signal: desktop sends
`ShellUserEvent::HotPatched` through its winit proxy, Android and iOS raise their frame gate's
signals-dirty input (`request_patch_frame`). The seam, layout witness and `__frust_hotpatch_anchor`
are CORE's ([CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md)).

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

Hot patching is available on the desktop shells (macOS, Linux, Windows x64 MSVC), the Android
shell (arm64) and the iOS shell on the simulator; a physical iOS device is refused when the
builder parses the target triple, and the web shell at compile time (`frust-hotpatch` is a
`compile_error!` on `wasm32`) — see `no-hot-reload-restart-is-a-rebuild` in
[LIMITATIONS.md](LIMITATIONS.md).

### Sanctioned-unsafe zones

Each concrete shell confines all unsafe/FFI code to one named module — `jni_glue` on Android,
`ffi_glue` on iOS, `appkit_glue` on macOS, `win32_glue` on Windows — so each platform boundary is
auditable as a single surface. `frust-shell-common` and `frust-shell-linux` hold no `unsafe` at
all, hot-patch code included (the patch loader's `unsafe` lives in `frust-hotpatch`);
`frust-shell-desktop` holds exactly one macOS-only block, `render.rs`'s `build_overlay_surface`;
macOS has a second zone, `frust-shell-macos`'s `cfg(macos)` `title_buttons` AppKit section. The
register of what each zone is permitted to do, and the `# Safety`/no-unwind rules that govern
it, live in [CODE_STANDARDS.md](CODE_STANDARDS.md) § Language Idioms.
Android's zone also holds the one `OverlaySurface::new` call in `prepare_overlays`, iOS's the one in
`take_frame_overlays` plus the overlay exports. Android's one other `unsafe` module is
`adpf::platform` (the `dlsym` binding and `gettid`); render-thread scheduling goes through
`frust-reactive::thread_policy` (`unsafe` in its `sys` module), with a bare `libc` call only for `FRUST_NO_THREAD_TUNING`.

`surface_mode`'s writer set is pinned by a source-scan conformance test: exactly one call site per
platform may publish a resolved surface mode, a single-writer contract that keeps the signal-poll
seam race-free without a lock.

## The Desktop Extension Seam

`DesktopExtensions` is a nine-hook trait, every method defaulted to a no-op, invoked where a
native integration has something to say. `NoExtensions` implements none and is what the zero-config
preview installs, so that path pays nothing for the seam.

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
| `window_maximized(&Window) -> Option<bool>` | wherever the core reads maximized: each resize-driven publish and the persistence snapshot | a macOS undecorated window reads it from its frame, so winit's `is_maximized` does not cancel AppKit's edge resize |

Two contracts shape it. **Static dispatch, not `dyn`:** the builder hook hands out a
`DesktopEventLoopBuilder` that a platform crate extends through winit's own `EventLoopBuilderExt*`
traits (object safety would bite); there is one extension per binary anyway. **Only three hooks
receive the window:** `on_window_created` (an `&Arc` the extension retains if a later hook needs it),
`on_platform_view_commands` (fires in the frame path and must parent into that exact window), and
`window_maximized` (a borrowed `&Window`). The other six stay unit-testable with no live loop.

`CloseAction::Exit` (the default) exits the loop, running the real shutdown path: `run_app`
returns, the frame executor drops, the render thread joins after a final present and its
best-effort cache persist. `CloseAction::KeepRunning` leaves the eventual exit to the extension.

### Configuration vocabulary

`DesktopConfig` (app name, reverse-DNS id, window icon, menu spec, last-window-close policy,
initial/min/max window size, opt-in window-state persistence, `decorations` (`Full`/`None`/`NoTitleBar`,
creation-time only), macOS `title_buttons`) and the `MenuSpec`/`MenuItemSpec`/`MenuRole` tree are
plain data defined in `frust-shell-desktop` — the lowest crate both the core and the per-OS shells
(which build a native menu from the spec) share. They cannot live in the facade, which depends on
the shell crates and never the reverse; the facade **re-exports** them instead. `IconData` is re-exported *renamed* as `DesktopIconData`, because the facade
already carries `frust-widgets`' `IconData` (an in-UI vector icon).

The core consumes the title, geometry and `decorations` fields (`window_state` persists size,
position and maximized state; precedence, file and clamp: [SHELLS_DEVELOPMENT.md](SHELLS_DEVELOPMENT.md)
§ Desktop window geometry; `decorations` is applied in `window_attributes` before the extension's
pre-create hook); the rest, `title_buttons` included, is carried for the per-OS shells, and `DesktopConfig::default()`
reproduces the zero-config preview window exactly. An icon is a decoded, tightly-packed RGBA8 buffer
whose dimensions are checked at construction — this tier carries no image decoder (decoding is the
tooling tier's job, see [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md)).

### Menu activations

Menus are built with `muda` — an `NSMenu` tree on macOS, an `HMENU` on Windows — and activations
travel a **push bridge**, not a poll. The shared core is dirty-driven (`ControlFlow::Wait`) and
calls `pump` only from a redraw, so draining muda's process-global channel alone would leave an
activation sitting until an unrelated event produced a frame (Windows accelerators are the sharp
case: winit never sees an event to wake on). Each per-OS crate installs muda's handler, filters
the activation against the ids its own spec declared, queues it in a bounded (oldest-dropped)
queue, and requests a redraw through a `MenuWaker` over the retained window.

`pump` hands **exactly one** activation per frame to `frust_reactive::push_menu_event` and re-wakes
while more remain: the reactive menu source is a single-slot signal, so a batch would coalesce. The
queue also keeps the push on the UI thread.
App code reads the other end through the facade's `menu_events()`, whose `MenuEvent` carries the
app's item id plus a monotonic sequence so choosing the same item twice reads as two activations;
that read side compiles on **every** target. See [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md) for
the reactive source. macOS and Windows keep separate copies of this strategy — each owns its own
platform's menu vocabulary — and must not diverge in behavior.

### Per-OS desktop integration

| Concern | macOS | Windows | Linux |
|---------|-------|---------|-------|
| App identity | resolved app name titles the application menu (configured name → bundle display name → executable stem → default) | `app_id` becomes the process AppUserModelID, claimed before the first window exists so the taskbar honors it | `app_id` becomes the Wayland `app_id` and X11 `WM_CLASS`, via winit's `with_name` |
| Icon | not attached (macOS wants an application icon, which the bundle carries) | both `ICON_SMALL` (titlebar/alt-tab) and `ICON_BIG` (taskbar) | X11 window icon; Wayland has no window-icon protocol and sources it from the `.desktop` entry instead |
| Menu | always builds the standard application menu (About/Hide/Hide Others/Show All/Quit) and appends the app's spec after it, so even a menu-less app gets a conventional bar with a working ⌘Q | builds only the app's own spec, with `TranslateAcceleratorW` accelerators; a role this platform cannot perform is dropped rather than faked, and the quit role is realized by the shell itself (an owned item mapped to `WM_CLOSE` — muda's predefined quit is inert under winit) | none — the menu is widget-drawn |
| Theme | winit handles it | titlebar brightness follows the app's resolved theme through winit's `Window::set_theme`, re-issued on every change the core signals; a resolution arriving before the window exists is held pending | winit handles it |
| Close / quit | `quit_on_last_window_closed = false` hides the window and keeps the loop running; a Dock re-activation brings it back | default exit-on-close | default exit-on-close |
| Activation policy | winit's, deliberately: it sets `NSApplicationActivationPolicy` while launching, so a competing call here could only disagree with it | n/a | n/a |
| Decorations | `None`: borderless + resizable; `NoTitleBar`: transparent title bar + full-size content view, traffic lights kept and placed from `TitleButtons` via `NSWindow` (skipped in fullscreen) | frame removed by winit's `WM_NCCALCSIZE`; the shell adds the undecorated DWM shadow | X11 Motif hints / Wayland client-side mode; nothing shell-side |
| Window commands | the `frust-core` queue is drained after each dispatch and per frame: `BeginMove`/`BeginResize` become `drag_window`/`drag_resize_window` (macOS `BeginResize` is a no-op), `RequestClose` takes `on_close_requested`; `WindowState` is republished on seed/`Resized`/`Focused`/`Occluded`/after commands | same | same |

Two macOS shapes are load-bearing. **Reopen** is observed through
`NSApplicationDidBecomeActiveNotification` on a notification observer registered beside winit's
delegate, because winit owns `NSApplication`'s delegate and `applicationShouldHandleReopen:` is
therefore unreachable. **Quit** is a frust-owned item answered synchronously on the main thread (so it works while the
window is hidden and no frames are produced) that ends in AppKit's `terminate:`, which skips the
whole handler `Drop` chain. Both carry accepted limitations
(`desktop-macos-reopen-gap-already-active`, `desktop-macos-quit-skips-executor-drop`,
and `desktop-macos-app-menu-title-process-name` for the unbundled menu title) in
[LIMITATIONS.md](LIMITATIONS.md).

Every per-OS crate compiles **inert off its own target**: native bindings sit in a
`cfg(target_os)` dependency table and each native entry point pairs its real body with a
do-nothing stand-in, so the workspace builds on any host while only the platform-free halves (menu planning, the
titlebar latch, the lifecycle state machine) stay host-testable. Failure is always degradation: a
refused menu install, unparseable accelerator or unattachable icon logs and the app runs without it.

## Data Flow

### Desktop frame path

winit events drive `AppTree`'s rebuild → layout → paint, then hand the scene across the
render-split channel for the render thread to encode and present (or run the tail inline, under
the `FRUST_NO_RENDER_THREAD` kill switch). The loop is dirty-driven, not continuous: a frame runs
when something asks for one, and a paced request resolves its next wake from that paint's outcome.

The desktop shell persists a pipeline cache through `frust-paths`, so later launches skip pipeline
compilation on adapters that advertise `PIPELINE_CACHE` — in wgpu 30.0.1 any Vulkan adapter
(Linux, and Windows on the Vulkan backend), never Metal or DX12, so macOS never writes a cache.
Loading is unconditional file I/O at startup on every desktop OS and reads through to a legacy
macOS cache base when the current file is absent (inert: no save ever fires there). The whole
path is best-effort: startup never fails on cache I/O. See [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md).
It persists right after each surface install.

### Mobile frame path

Both mobile shells gather an identical `FrameInputs` OR-list each tick — input-for-input
comparable across platforms — and consult the shared `frame_gate` for a run/skip decision before
rebuild → layout → paint → present. Android drives it from a Choreographer callback, iOS from a
CADisplayLink tick. Every shell gates layout on the drained change flags or a force (first layout,
surface size/scale change, iOS surface reinstall); the frame gate alone decides whether a frame runs.

`focus_or_ime_changed` is an *edge* (from a focus/IME generation counter, cached per handle,
committed only on a frame that actually runs), not a level: a steady focus session neither forces
a frame every tick nor blocks pacing, so a paced (`CosmeticLoop`) frame like a blinking caret
throttles to the theme's `cosmetic_loop_rate` even while focused, while a focus/IME transition
forces exactly one frame. `deferred_callbacks_pending` is the other run-forcing input, a peek at
CORE's pending-result-flush flag so a Housekeeping flush owed with nothing else dirty still runs.

**Per-request paced interval:** a paced request may name its own cadence, reaching the shell as
`PaintOutcome::paced_interval` — core's MIN-fold across every paced request in the pass. Each
mobile shell latches it for the *next* tick's pre-paint decision; desktop reads it straight into its
next wake and needs no latch. Both resolve it through `max(cap, requested)`: the theme's
`cosmetic_loop_rate` is a **ceiling, not a target**, so a tighter request clamps up to it and only
a slower one widens the cadence. The mobile pacing anchor re-anchors to `now` when the interval
changes, so a flip cannot double-fire, and a tick carrying the focus/IME edge tightens pacing back
to the bare theme cap (see [LIMITATIONS.md](LIMITATIONS.md)).

### iOS render-thread split

`frust-shell-ios`'s default (split) executor overlaps startup: the render thread is spawned and
starts GPU bring-up (adapter/device/renderer-ready) *before* the UI thread joins the background
font-preinit thread, so the two longest serial stretches of cold start run concurrently. Both share
one startup-span recorder (`SharedStartupSpans`) behind a `StartupRecorder` that is `Owned` for the
inline (`FRUST_NO_RENDER_THREAD`) executor and `Shared` for the split one; every `Shared` method
locks only for its own record/take, never across the blocking GPU tail (drawable acquire plus
submit), a UIKit watchdog hazard. Once the startup line is emitted on the first present, the render
thread drops to a lock-free `Owned` handle. The threads race onto one line, so spans do not print
in causal order — read each span's own delta, not its position.

Frame pacing is unchanged: the surface's maximum frame latency stays constant (two frames in
flight, `crates/frust-gpu/src/surface.rs`), and pre-acquiring a drawable ahead of its frame was
measured and rejected — a drawable parked across a `Pause` is a UIKit watchdog hazard. Instead a
`perf-trace`-only `FRUST_PACE_TRACE` dial emits one `frust-perf ios-pace` line per rendered frame
(wake/idle/acquire/submit/loop time plus `p2p_us`, the submit-to-submit cadence, `NA` for a frame
that presented nothing; under armed present-sync the UI thread commits the drawable later than the
submit, which the render thread never observes). `FramePasses::total` sums the UI and render threads'
concurrent spans, so it is a cost, not an interval to read a dropped vsync out of.

### Web frame path

The single-thread inline executor (`WebFrameExecutor`, `crates/frust-shell-web/src/render.rs`) —
mirroring the desktop core's `FrameExecutor::Inline` encode→acquire→submit tail — is this tier's
**first-class** path, not a fallback: `wasm32-unknown-unknown` has no OS threads and wgpu's handle
types are not `Send`/`Sync` there. Redraws ride `requestAnimationFrame`; a paced wake rides winit's
`ControlFlow::WaitUntil`, serviced by the browser's Prioritized Task Scheduling API (falling back
to `setTimeout`) rather than a heavily-clamped bare-`setTimeout` chain, every deadline computed on
`web_time::Instant` (`crates/frust-shell-web/src/pacing.rs`), the type winit itself uses there. The
swapchain is reconciled against the canvas's real size every frame, because winit reports
`inner_size` as 0x0 until its `ResizeObserver` has fired. Surface bring-up is `async` out to the
caller (no `pollster::block_on`, which would block the page's one JS thread) and retries a cold
page's `requestAdapter()`, which can spuriously answer `null` (a GPU-process warm-up race). There
is no pipeline-cache I/O: a page has no filesystem, so the renderer starts on an empty cache.

The tier's gate vehicle is the standalone [examples/web-gallery](../examples/web-gallery) workspace
(`frust::web_app!`: input, theme-follow, resize/DPR, repaint); its README holds the evidence.

### Native-view hosting

Paint-time view frames feed the `platform_view` differ, an idempotent create/update/dispose backlog
(plus overlay commands, below). The two mobile shells **poll** it across their FFI boundary and
apply it frame-paired against present, converting each rect to physical px. Desktop has neither
boundary nor poller: ingest after paint, then **push** the batch to
`DesktopExtensions::on_platform_view_commands` after the frame is submitted, acknowledged as the
hook returns, rects in **logical points** (plus the scale factor). A hosted view can lead its
surroundings by **at most one frame** (`desktop-platform-view-frame-lead` in
[LIMITATIONS.md](LIMITATIONS.md)); pairing would need a presented-frame id the desktop executor
does not publish. A suspend removes every hosted view; every surface bring-up replays `Create` +
`Update` per live slot.

Mode A is an opaque frust surface with hosted views above it (frust content cannot paint over a
view); Mode B is a translucent surface with the views below, hole-punched.

Android never re-parents the `FrustSurfaceView`'s root `FrameLayout` (`surfaceDestroyed` would fire
inside a frame): plugins add siblings in `android.R.id.content` and pad the root (native-widgets chrome).

| Platform | Mode | Overlays (frust paint above a Mode A view) |
|----------|------|--------------------------------------------|
| macOS | A: plugin `NSView` above winit's content view, nothing forwarded into it, the frust slot behind it covered | shipped behind `FRUST_OVERLAYS` (default off; verified in both paint orders) |
| iOS | A or B per the host's `translucentSurface`; a refused translucent surface degrades to A | shipped behind `FRUST_OVERLAYS` (default off), Mode A only; device-verified |
| Android | A by default, B on host opt-in; a refused translucent surface degrades to A | shipped behind `FRUST_OVERLAYS` (default off; Pixel 5 passed) |
| Windows, Linux, web | none | none |

**One overlay table.** Android, iOS and macOS drive the same `OverlayTable<L, S>`: the render side
takes the live surfaces out for the length of a frame and lands them afterwards, so a withdraw or
layer replacement that meets a surface in flight never blocks. The hand-back is deferred: a layer
comes back only after its surface dropped (the land that drops it), and no UI thread waits for
that. The table gives ordering and leak-freedom, not memory safety (wgpu-hal retains a
`CAMetalLayer` itself; an Android window dies at `surfaceDestroyed` regardless).

**Android overlay host.** `ingest_scene` (the UI thread's call in `app/frame.rs`) cuts the frame's
command list at the visible slots; each cut is an overlay, announced by two JSON ops additive to
the frozen create/update/updateParams/dispose set: `overlay` {overlay, aboveSlot, rect `[x,y,w,h]`
physical px, rangeStart, rangeEnd} and `disposeOverlay` {overlay}. Kotlin `FrustViewHost` stacks
one input-transparent translucent `SurfaceView` above the slot's view (hidden with the slot) —
Mode A only; Mode B acknowledges and ignores the ops. Each view reports through
`nativeOverlaySurfaceChanged`/`nativeOverlaySurfaceDestroyed`, and `nativeOverlayMode` reads the
knob; `media` selects `setZOrderMediaOverlay` (an A/B arm). With the knob **off** (the default) the
frame path is byte-identical. Rust side: the shared table plus host-testable `overlay_state` (wire
helpers stay in `ffi_support`); its `OverlayLedger` pairs cut ranges with the drawn frame id (16
kept). The overlay lock is never held across the main-surface acquire or a present, and
`surfaceDestroyed` never waits: an idle overlay's surface and window are released before it
returns, one out for a frame at that frame's land, so that frame may present into a destroyed
window (the present fails; the table's `ANativeWindow` reference keeps the object valid —
`android-overlay-withdraw-after-return`). The render tail presents main, then overlays; a `frust-overlay skew` line logs
every 120 frames. On-top overlays sit above every native view, so frust content interleaved between two slots covers
the later one (`android-overlays-cover-later-slots`).
**Apple overlay hosts.** The differ, the `overlay`/`disposeOverlay` ops (byte-identical to
Android's) and the `FRUST_OVERLAYS` resolution are shared; Android's `media` value means plain
on. Ranges travel in the frame (`PaintedScene.overlay_cuts`); overlays present bottom to top after
main. Each overlay view sits directly above its own slot's view, so interleaved content stays under a later native control.
- **iOS:** one `FrustOverlayView` per overlay (own non-opaque `CAMetalLayer`, user interaction
  off, `presentsWithTransaction` mirroring the main layer), Mode A only (Mode B logs and ignores
  the ops), on the render-thread split; the layer is a sublayer of the view, sized on the main
  thread, and the render thread leaves `drawableSize` alone. Four additive C exports
  (`frust_overlay_mode`, `frust_overlay_attach`, `frust_overlay_detach`,
  `frust_overlay_take_released`, emitted by `ios_app!`, declared in `frust_ffi.h`) hand layers
  across: detach never waits, and `FrustViewHost` keeps a detached view hidden until it drains
  `frust_overlay_take_released`. With present-sync armed the UI thread presents the parked
  frame, main then overlays, inside one `CATransaction` (present before poll); otherwise the
  render thread presents them back to back. The handoff waits up to 50 ms for the UI thread, then
  drops the parked frame before any acquire or resize; surface change/create/destroy, self-heal,
  pause and detach clear it first, and a withdrawn overlay's parked present is dropped before its
  surface. An attached or resized overlay layer requests a frame.
- **macOS:** `FrustOverlaySurfaceView` (non-opaque `CAMetalLayer`, `hitTest` nil) is added above
  its slot's view at the command's rect. The seam is `extensions::overlays_enabled()` (macOS only)
  plus the `#[doc(hidden)]` `publish_overlay_layer(id, layer, w, h) -> bool` and
  `withdraw_overlay_layer(id)` (never waits). A withdrawn view is hidden at once and removed on
  `ShellUserEvent::OverlaysReleased` or the next drain; `on_platform_view_commands` may therefore
  be called with an empty batch while releases are pending or after a scale change, and
  `NativeViewOps::sync_overlay_scale` re-applies the backing scale so overlays stay sharp across
  2x/1x moves. The main layer is a sublayer of winit's view, so hosted views and overlays sit above it.

**Texture wake.** A push into a registered texture sink owes the shell a frame
(`frust::texture::request_frame`; see [RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md)). The mobile
frame gates OR `texture_bus::take_frame_owed` beside `take_retained_repaint`; desktop installs
`set_frame_request_hook` around its loop, posting `ShellUserEvent::TextureFrameOwed`. The web shell
does not poll it yet, and the hook assumes one shell per process.

Apple limits (full-window overlay per cut, creation-order stacking, a transparent overlay above a
hidden slot) are in [LIMITATIONS.md](LIMITATIONS.md) § `apple-overlay-limits`.

### Cross-cutting host signals

- **Event pass under root owner:** every shell routes input through `AppTree::event` inside a
  reactive `Owner.with` wrap, so handlers see root-level contexts (`Theme`, `WindowMetrics`,
  root-component `init`-installed values). `frust-reactive` is a leaf crate and cannot wrap
  itself. The pass is deliberately **not tracked** — entering a tracked scope resets dependencies
  and clears the dirty flag, which would unsubscribe the frame loop from pending wakes. Desktop
  forwards the secondary mouse button as `PointerButton::Secondary` through a delivery-latched
  gate — an Up dispatches iff its Down did — guaranteeing every Down/Up pair reaches `AppTree`
  paired even under pointer capture.
- **Touch contacts and resampling:** each mobile shell maps its platform's per-finger id onto a
  gesture-local `PointerId` slot (a gesture's first contact is always slot `0`) and delivers every
  contact as `InputEvent::PointerContact`, direct or `PointerResampler`-buffered (a
  `RawPointerSample` carries the same `pointer_id`; one resampler lane per contact). Every shell
  uses the same **lowest-currently-free-slot** rule — taken on `Down`, freed on `Up`/`Cancel`,
  never renumbering a live slot — implemented independently by Android's `TouchSlotMap`
  (`frust-shell-android::ffi_support`), iOS `FrustView`'s `freeSlots` and the web `TouchTracker`'s
  `free_slots`. Entries: `nativeOnTouch` (Android), `frust_dispatch_touch` (iOS); web has no resampler.
- **Desktop scale gestures:** ctrl+wheel (Linux/Windows) or ⌘+wheel (macOS) maps to
  `InputEvent::Scale` at the cursor, `scale_delta = exp(WHEEL_SCALE_RATE_PER_LINE × lines)` (0.1,
  ~10% per notch); a plain wheel stays `Scroll`. Winit's `PinchGesture` (macOS only at this pin)
  maps to the same stream with `scale_delta = 1.0 + delta`; see `desktop-pinch-linux-windows-unavailable`
  in [LIMITATIONS.md](LIMITATIONS.md).
- **Desktop file drop:** winit's `HoveredFile`/`DroppedFile`/`HoveredFileCancelled` events map onto
  `InputEvent::FileDrop`'s three `FileDropPhase`s. Files arrive one event each with no "that's
  everything" signal, so only a hover's first arrival dispatches (`Hover`, empty `paths`) and a
  `FileDropAccumulator` batches one drop's files into a single `Drop`. `position` is the last known
  cursor position (a `FileHoverLatch` re-dispatches `Hover` on each `CursorMoved`);
  `HoveredFileCancelled`, `CursorLeft` and any real mouse button each unlatch and dispatch one `Cancel`.
- **App lifecycle:** each shell pushes `AppLifecycleState` via `frust_reactive::push_app_lifecycle`
  (state machine: [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md) § App Lifecycle):

  | Shell | Push site | Mapping |
  |---|---|---|
  | Android | `lifecycle.rs` `LifecycleFlags::target`, driven by `jni_glue` `native_on_start/stop/resume/pause/window_focus_changed` | `native_on_destroy`→Detached; !started→Paused; started+resumed+focused→Resumed; else Inactive |
  | iOS | `ffi_glue` pause/resume plus `frust_did_enter_background`/`frust_will_enter_foreground` | willResignActive→Inactive; didEnterBackground→Paused; willEnterForeground→Inactive; didBecomeActive→Resumed; never Detached |
  | Desktop | `app_handler.rs` `sync_app_lifecycle` | any window focused→Resumed; all occluded/minimized or `suspended`→Hidden; else Inactive; never Paused/Detached |
  | Web | `frust-shell-web` `app_handler.rs` | document hidden→Hidden; window focused→Resumed; else Inactive |

  **Cancelable exit** (`request_app_exit`): desktop `handle_close_requested` asks only when
  `resolve_close_action` yields `CloseAction::Exit`; macOS app-menu Quit is the frust item
  `frust.app.quit` (`QUIT_ID`); Windows menu Quit posts `WM_CLOSE`, becoming `CloseRequested`. Not
  asked: macOS Dock Quit, logout/shutdown, force quit, Android, iOS, web.
- **Window metrics** (logical size, scale, derived orientation, insets snapshot) are published
  from the points where the window's shape changes, guarded by `WindowMetricsPublisher` against
  per-frame churn (see [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md)). Desktop seeds insets as the
  default, since winit 0.30 offers no cross-platform safe-area accessor. On iOS 26+,
  `FrustViewController` also pushes four **physical** window-control corner sizes through
  `frust_set_corner_insets` (beside `frust_set_insets`); the Rust handle merges both calls into one
  `WindowInsets` (zero elsewhere and below iOS 26). An app's `FrustSceneDelegate` subclass owns
  `preferredWindowingControlStyle(for:)` and `windowScene.sizeRestrictions`; it stores the surface as `frustViewController` and roots the
  window at `makeWindowRootViewController(hosting:)` (default: that controller; a plugin may re-root).
- **Android edge-to-edge:** `FrustActivity`, not the manifest theme, owns edge-to-edge — `onCreate`
  calls androidx `enableEdgeToEdge` with transparent `SystemBarStyle`s, then on API < 35 adds
  `FLAG_DRAWS_SYSTEM_BAR_BACKGROUNDS` and (API 29+) clears the bar contrast scrim, so transparent
  bars and real insets arrive regardless of the manifest theme; API 35+ enforces it platform-side.
  A legacy theme (missing `windowDrawsSystemBarBackgrounds`) keeps working but logs one warning
  naming the migration recipe ([SHELLS_DEVELOPMENT.md](SHELLS_DEVELOPMENT.md)).
- **Theme and brightness** reach widgets through `RenderRoot::set_theme` and app code through a
  re-provide under the root owner; on desktop a third path fires the per-OS brightness hook, gated
  on the resolved brightness actually moving so an override swapping one dark theme for another
  fires nothing. Android also reads the resolved brightness back over JNI after every appearance
  change and on any frame whose `nativeFrameSync` word reports a flip, so a Rust-forced theme reaches
  the status bar.
- **Accessibility:** `frust-core` reports semantics `bounds` in logical pixels, accesskit wants physical:
  all three shells push one assembly, `frust-shell-common`'s `build_tree_update` / `tree_update_from_parts`,
  whose root carries `Affine::scale(sanitize_scale(scale))` (none at 1.0) — window scale factor on desktop,
  display scale on iOS, the frame's density on Android (`accesskit_android`, `accesskit_ios` do the delivery).
- **Reduced motion** is the one host signal whose sensor is not the appearance sensor: Android
  watches the animator duration scale (`ContentObserver` plus an `onResume` re-read), iOS the UIKit
  notification plus an activation re-read, desktop has no source (winit exposes nothing). Delivery
  bypasses the theme-override path, which would latch an override and pin brightness; each shell
  edits the motion field directly and marks appearance dirty so the gate cannot skip the tick.
- **IME content type:** each focused widget publishes a content type (Normal/Password/
  NoSuggestions/Terminal) and each shell destructures the editing state into its platform's
  payload — Android into `EditorInfo` input-type and IME flags (restarting the connection when the
  type changes on a steady-focused field), iOS into the `UITextInputTraits` matrix over its shared
  responder (every managed trait assigned in every arm), desktop into winit's `ImePurpose`, a
  cosmetic hint on Wayland only and never a secure-text-entry boundary. Both mobile shells
  reconcile the platform's IME mirror on a per-frame poll behind a divergence guard. The iOS Swift
  half remains compile- and device-unverified on the Linux build host — see
  [LIMITATIONS.md](LIMITATIONS.md) `ime-ios-content-type-unverified`.
- **Cursor:** desktop reads `RenderRoot::cursor()` after every event dispatch, beside
  `sync_ime` (`ShellHandler::sync_cursor`), and applies it via winit's `Window::set_cursor` only
  when it differs from the shape last pushed. `winit_cursor_for` (`frust-shell-desktop`) is the one
  platform touch point mapping the framework's `CursorIcon` onto winit's own vocabulary; a mobile
  shell never reads the resolved value. Web carries the identical request/resolve contract and a
  real implementation (winit's web backend writes the canvas's CSS `cursor` property), unlike its
  semantics/devtools seams below. See [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md)'s Hover
  and Cursor section.
- **Clipboard:** the framework side is two one-shot slots plus one inbound verb —
  `AppTree::take_clipboard_write`/`take_paste_request`, drained after a dispatch, and
  `InputEvent::EditCommand`, dispatched back. [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md) owns
  those slots and the rule an asynchronous read inherits: the answer is a later focus-routed
  dispatch carrying no identity, so it is bound to the session (`focus_epoch`) recorded when the
  paste request is drained and dropped when that session has gone. Desktop and Android read
  asynchronously; iOS reads synchronously.
  **Desktop** serialises both operations on one lazily spawned worker thread owning the process's
  single `arboard::Clipboard`, because on X11/Wayland a copy is served *live* by the process that
  claimed the selection and a read can block for as long as that owner takes to answer — so the read
  cannot run on the event loop. A finished read returns through the event-loop proxy as a fresh
  top-level dispatch; an outstanding read absorbs a later request rather than queuing a second,
  re-pointed at the most recent asker.
  **Web** cannot go through winit, because a browser grants clipboard access only to the focused
  editable: the hidden IME overlay (see Web IME bridge) carries the `copy`/`cut`/`paste`
  listeners. An app- or toolbar-driven access uses `navigator.clipboard`, undefined outside a
  secure context (warns once; the keyboard path is unaffected).
  **Android** owns the clipboard on the JVM side, so the channel is a JNI trio: the Kotlin view
  drains the write/paste slots on frames `nativeFrameSync` flags and pushes verbs back through two
  host routes (the IME's context-menu action and hardware `Ctrl+C`/`X`/`V`/`A`), both gated on a
  focused editable. See [LIMITATIONS.md](LIMITATIONS.md) for the focus and toast quirks this route
  works around.
  **iOS** locks the `Native` toolbar policy at start-up (a later `set_selection_toolbar_policy` is
  refused) because `UIPasteboard.general.string`'s paste-notice exemption applies only to a
  system-initiated read, which only UIKit's own edit menu provides; the framework's view is the
  app's single responder for that menu and for hardware `Cmd+C`/`X`/`V`/`A`, and its read runs
  synchronously inside `paste(_:)`. Both mobile clipboard paths are device-unverified.
- **Web IME bridge:** winit's web backend never emits `WindowEvent::Ime` (upstream issue 4424 is
  open), so `frust-shell-web::ime` bypasses it with one hidden `<input id="frust-ime-overlay">`
  under `document.body`, opened when the focused widget publishes an active `ImeState` and removed
  when it stops. Its attributes come from the field's `ImeContentType` (a secret field gets
  `type="password"`); a hint that moves rebuilds the element, since a browser reads `type` only
  when it first classifies one. Composition is an explicit three-state session, because an *empty*
  `compositionend` is not reliably a cancel: the preedit is held one signal longer and resolved as
  a commit when an `input` follows, as a clear otherwise. winit's key listeners stay on the canvas,
  so the overlay re-dispatches each plain keystroke there — except an `Unidentified` `keydown` (a
  mobile soft keyboard), whose edit is taken from the `input` behind it. A soft keyboard's
  Backspace is the known gap. The same element is the page's clipboard surface. Device-unverified —
  see [LIMITATIONS.md](LIMITATIONS.md) `web-ime-residual-gaps`.
- **Web host signals with no browser counterpart:** accessibility (AccessKit ships no web
  adapter) and the in-app devtools UI-thread hop (a `wasm32` build has no sockets, so
  `frust-shell-web` forwards no `devtools` feature) are each a documented no-op
  (`push_semantics`/`pump_devtools` in `crates/frust-shell-web/src/app_handler.rs`). See
  [LIMITATIONS.md](LIMITATIONS.md) `web-a11y-devtools`.
- **Surface-mode resolution:** each mobile shell resolves the host's declared translucency mode
  against actual surface capabilities at configure time and republishes the verdict every frame.
- **Devtools UI-thread hop** (feature-gated): a backend call needing live app state queues onto a
  process-wide request each shell drains once per frame — desktop wakes via its winit proxy, the
  mobile shells pump at the top of every tick (their display loop runs continuously while resumed).
  An injected event dispatches through the shell's real input path. `FRUST_DEVTOOLS=0` is the one
  security-relevant kill switch — it skips starting the service even in a `devtools` build. Detail
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
| `DesktopExtensions` / `NoExtensions` / `CloseAction` / `DesktopEventLoopBuilder` | The per-OS desktop seam: its nine hooks — including `on_platform_view_commands` (the per-frame native-view batch) and `on_platform_views_suspended` (remove every hosted view) — the whole-set no-op, the close verdict, and the builder type a platform crate extends |
| `DesktopConfig` / `MenuSpec` / `MenuItemSpec` / `MenuRole` / `IconData` | Platform-independent desktop identity and native-menu vocabulary, re-exported by the facade (`IconData` as `DesktopIconData`) |
| `MacosExtensions` / `WindowsExtensions` / `LinuxExtensions` | The three per-OS implementations, each built from a borrowed `DesktopConfig` and installed by the facade under its own target `cfg` |
| `android_app!` / `ios_app!` / `web_app!` / `app!` | Facade macros binding a generated app's state/logic to the fixed JNI / C-ABI export set, the wasm-bindgen start shim, and the desktop entry point |

## Verification State

The desktop tier is unevenly proven, and the gap is tracked rather than assumed closed:

| OS | Proven |
|---|---|
| **macOS** | Runtime-verified on hardware: real windows, the app-named menu bar, ⌘Q by both menu and accelerator, Hide/Show All, and close-then-Dock-click reopen |
| **Windows** | Runtime-verified on hardware: titlebar/taskbar icon, AppUserModelID identity, titlebar theming, menu-bar activation delivery, accelerators, and quit. An accelerator enters the `HACCEL` only if its submenu is attached before the item is appended (documented at its `menu.rs` site) |
| **Linux** | Proven by cross-target compilation only, plus every native call site read against its vendored source. Owed: a non-headless `app_id`/icon pass. See [LIMITATIONS.md](LIMITATIONS.md) `desktop-shells-runtime-unverified` and `desktop-windows-titlebar-theme-revert` |
| **Decorations / window commands** | Linux (X11, Wayland), Windows and macOS gates all passed (SHELLS_DEVELOPMENT.md § Decorations and window commands); open: `desktop-windows-menu-bar-undecorated-unverified` |

Multi-window is out of scope: every desktop shell manages exactly one window (`desktop-single-window`).

## See Also

- [SHELLS_DEVELOPMENT.md](SHELLS_DEVELOPMENT.md) — device gates and this unit's version pins
- [CODE_STANDARDS.md](CODE_STANDARDS.md) — sanctioned-unsafe register, no-unwind-across-FFI rule
- [LIMITATIONS.md](LIMITATIONS.md) — the accepted-limitation entries referenced above
