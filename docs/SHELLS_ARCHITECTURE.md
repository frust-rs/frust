# Frust - SHELLS Architecture

## Overview

SHELLS is the seam between the platform-agnostic core+scene+render+text+theme+reactive stack and
each concrete host. `frust-shell-common` is the platform-free plumbing every shell shares: a
type-erased app driver, the frame-gate pacing decision, the render-thread split vocabulary, the
platform-view differ, and a handful of signal-poll seams. Above it sit two tiers:

- **Desktop** — `frust-shell-desktop` is the shared winit core (event loop, frame pipeline,
  input/IME/theme translation, accessibility) for *every* desktop host, plus the
  `DesktopExtensions` seam and the `DesktopConfig`/`MenuSpec` vocabulary. Three thin per-OS
  crates — `frust-shell-macos`, `frust-shell-windows`, `frust-shell-linux` — implement that seam
  with native integration the core neither knows nor should know about.
- **Mobile** — `frust-shell-android` and `frust-shell-ios` are FFI/frame-callback integration
  points that own their host's lifecycle end to end.

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
| `frust-shell-desktop` | The shared winit core: event loop, UI-thread/render-thread surface split, accessibility adapter, paced wake, pipeline-cache persistence — plus the `DesktopExtensions` seam and the `DesktopConfig`/`MenuSpec` vocabulary the per-OS crates read. Also the zero-config dev-preview entry point |
| `frust-shell-macos` | AppKit integration: the native menu bar (a standard application menu plus the app's own spec), hide-on-close and Dock-reopen lifecycle, and the reopen observer in its `appkit_glue` unsafe zone |
| `frust-shell-windows` | Win32 integration: taskbar identity, window/taskbar icons, an `HMENU` menu bar with keyboard accelerators, and titlebar brightness — every `unsafe` and every `windows-sys` call confined to `win32_glue` |
| `frust-shell-linux` | Wayland `app_id`/X11 `WM_CLASS` plus the X11 window icon, through winit's own cross-platform API. Deliberately thin: no GTK/X11 binding, no native menu (the menu is widget-drawn), no `unsafe`, one hook implemented |
| `frust-shell-android` | Sanctioned-unsafe JNI FFI boundary, app-binding macro, and Choreographer-synced frame pipeline; carries the theme precedence ladder and the conformance-pinned resolved-surface-mode publish site |
| `frust-shell-ios` | Sanctioned-unsafe C-ABI FFI boundary, app-binding macro, and CADisplayLink-driven frame pipeline, with optional present-sync gating against platform-view geometry |

## Layer Dependencies

Every concrete shell depends on `frust-core` (`RenderRoot`, widget tree, input, insets,
semantics), `frust-scene` (renderer-agnostic `Scene`/`SceneBuilder`), `frust-text` (a
`TextContext` threaded through layout), `frust-theme` (theme delivery, brightness/design-language
state), `frust-render` (`SurfaceRenderer`, encode/present, pipeline cache) and `frust-reactive`
(`ReactiveRuntime`, `deep_link`, `back`, `menu`, `task`). Desktop and Android also depend on
`frust-paths` for cache-dir persistence, and Android on `frust-plugin` to install the
JavaVM/Context handle plugins read.

`frust-shell-common` is a hard platform-free leaf: no `jni`/`ndk`/`winit` dependency, no unsafe
code, and no reactive dependency in its shipped surface, so every concrete shell can share it
unconditionally. Platform FFI lives only in the concrete shells — `winit` + `accesskit_winit` in
`frust-shell-desktop`, `jni`/`ndk`/`accesskit_android` in `frust-shell-android`,
`objc2`/`accesskit_ios` in `frust-shell-ios`, and the per-OS desktop bindings below.

**The three per-OS desktop crates depend only on `frust-shell-desktop`** plus `winit`, whatever
narrow seam they push into (`frust-reactive` for menu activations, `frust-theme` for the Windows
brightness hook) and their own native bindings — never on each other, never on
`frust-shell-common`, and nothing in the shared core depends on them. The core is compiled for
every desktop host; the native half is chosen above it.

### Target gating

The `frust` facade is the only place that names a per-OS shell crate. It gates each dependency and
the matching extension-selection arm under the identical `cfg(target_os = …)`, so a build resolves
only its own host's native bindings:

| Target | Shell composition |
|--------|-------------------|
| macOS / Windows / Linux | shared desktop core + that OS's extension crate |
| other desktop hosts (BSDs) | shared desktop core + the whole-set no-op extensions — window, input, theme and accessibility all work; only native identity/menu integration is absent |
| Android | `frust-shell-android` only; the desktop core is excluded (winit's `android-activity` edge does not build there and the app is JNI-driven) |
| iOS | `frust-shell-ios` only; the desktop core is excluded, keeping winit and `accesskit_winit` out of an iOS build entirely. The `app!` macro still emits a `__frust_main` **stub** on iOS, because the generated `main.rs` binary target Xcode builds calls it — it logs and exits non-zero rather than looking like an app that started and vanished |

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

`DesktopExtensions` is a six-hook trait, every method defaulted to a no-op, invoked at the six
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
| `on_close_requested() -> CloseAction` | on a close request | macOS hide-instead-of-quit |

Two contracts shape it. **Static dispatch, not `dyn`:** the builder hook hands out a
`DesktopEventLoopBuilder` that a platform crate extends through winit's own `EventLoopBuilderExt*`
traits (object safety would bite), and there is exactly one extension per binary anyway. **The
window reaches an extension exactly once:** only `on_window_created` receives it, as an `&Arc` the
extension clones and retains if a later hook needs it — which also keeps five of the six hooks
unit-testable with no live event loop.

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
exactly, so an app that configures nothing is unaffected by the seam's existence.

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
| Menu | always builds the standard application menu (About/Hide/Hide Others/Show All/Quit) and appends the app's spec after it, so even a menu-less app gets a conventional bar with a working ⌘Q | builds only the app's own spec, with `TranslateAcceleratorW` accelerators; a role this platform cannot perform is dropped rather than faked | none — the menu is widget-drawn |
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
skip pipeline compilation on adapters that advertise `PIPELINE_CACHE` — in wgpu 29 that is any
Vulkan adapter (Linux, and Windows when the selected backend is Vulkan; verified in wgpu-hal
29.0.4's unconditional Vulkan feature set), never Metal or DX12 — so macOS never writes a cache.
Loading is unconditional file I/O at startup on every desktop OS, and reads through to a legacy
macOS cache base when the current-location file is absent, migrating nothing — that read-through is inert by construction (the legacy base is `Some`
only on macOS, where a save never fires, so no legacy blob can exist); it survives purely as
compatibility for the `frust-paths` macOS-arm change. The whole path is best-effort — startup never
fails on cache I/O. See [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md) for `frust-paths` itself.

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

### Cross-cutting host signals

- **Event pass under root owner:** every shell routes input through `AppTree::event` inside a
  reactive `Owner.with` wrap, so handlers see root-level contexts (`Theme`, `WindowMetrics`,
  root-component `init`-installed values). `frust-reactive` is a leaf crate and cannot wrap
  itself. The pass is deliberately **not tracked** — entering a tracked scope resets dependencies
  and clears the dirty flag, which would unsubscribe the frame loop from pending wakes.
- **Window metrics** (logical size, scale, derived orientation, insets snapshot) are published
  from the points where the window's shape actually changes, guarded by the shared
  `WindowMetricsPublisher` against per-frame churn (see
  [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md)). Desktop seeds insets as the default, since winit
  0.30 offers no cross-platform safe-area accessor.
- **Theme and brightness** reach widgets through `RenderRoot::set_theme` and app code through a
  re-provide under the root owner; on desktop a third path fires the per-OS brightness hook, gated
  on the resolved brightness actually moving so an override swapping one dark theme for another
  fires nothing. Android additionally reads the app's resolved brightness back over JNI — after
  every appearance change and once per frame — so a theme forced from Rust reaches the status bar
  without waiting for a platform event.
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
- **Platform-view embedding:** paint-time view frames feed the `platform_view` differ, which
  exposes a command backlog each shell's FFI layer polls and applies to the native view hierarchy,
  frame-paired to keep geometry in sync.
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
| `WindowMetricsPublisher` | Shared, change-guarded path every shell drives to provide window-shape context without per-frame re-provides (see [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md)) |
| `DesktopExtensions` / `NoExtensions` / `CloseAction` / `DesktopEventLoopBuilder` | The per-OS desktop seam: its six hooks, the whole-set no-op, the close verdict, and the builder type a platform crate extends |
| `DesktopConfig` / `MenuSpec` / `MenuItemSpec` / `MenuRole` / `IconData` | Platform-independent desktop identity and native-menu vocabulary, re-exported by the facade (`IconData` as `DesktopIconData`) |
| `MacosExtensions` / `WindowsExtensions` / `LinuxExtensions` | The three per-OS implementations, each built from a borrowed `DesktopConfig` and installed by the facade under its own target `cfg` |
| `android_app!` / `ios_app!` / `app!` | Facade macros binding a generated app's state/logic to the fixed JNI / C-ABI export set, and to the desktop entry point |

## Verification State

The desktop tier is unevenly proven, and the gap is tracked rather than assumed closed:

- **macOS** — runtime-verified on hardware: real windows, the app-named menu bar, ⌘Q by both menu
  and accelerator, Hide/Show All, and close-then-Dock-click reopen.
- **Windows and Linux** — proven by cross-target compilation only, plus every native call site
  read against its vendored source. Owed: the Windows titlebar/taskbar/menu/accelerator pass and a
  non-headless Linux `app_id`/icon pass. See [LIMITATIONS.md](LIMITATIONS.md)
  `desktop-shells-runtime-unverified`, `desktop-windows-accelerator-compile-only` and
  `desktop-windows-titlebar-theme-revert`.

Multi-window is out of scope at this tier — every desktop shell manages exactly one window
(`desktop-single-window`).

## See Also

- [SHELLS_DEVELOPMENT.md](SHELLS_DEVELOPMENT.md) — device gates and this unit's version pins
- [CODE_STANDARDS.md](CODE_STANDARDS.md) — sanctioned-unsafe register, no-unwind-across-FFI rule
- [LIMITATIONS.md](LIMITATIONS.md) — the accepted-limitation entries referenced above
