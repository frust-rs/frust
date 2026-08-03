# Frust - SHELLS Architecture

## Overview

SHELLS is the seam between the platform-agnostic core+scene+render+text+theme+reactive stack and
each concrete host. `frust-shell-common` is the platform-free plumbing all three concrete shells
share: a type-erased app driver, the frame-gate pacing decision, the render-thread split
vocabulary, the platform-view differ, and a handful of signal-poll seams. `frust-shell-desktop`,
`frust-shell-android`, and `frust-shell-ios` are the real FFI/event-loop integration points — each
owns the event loop or frame callback for its host, drives rebuild → layout → paint → encode →
present every frame, and translates platform-native input/lifecycle/theme/insets/IME/deep-link/
back/platform-view signals into the framework's own vocabulary.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how SHELLS relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `frust-shell-common::app_tree` | Type-erased `AppTree` driver letting a native shell own and drive any app's rebuild/layout/paint/event/semantics cycle without generics |
| `frust-shell-common::frame_gate` | Shared run/skip frame decision and pacing used by both continuous-loop mobile shells |
| `frust-shell-common::render_split` | UI-thread/render-thread split vocabulary (scene handoff, lifecycle commands, completion barrier) all three shells' default frame path uses |
| `frust-shell-common::platform_view` | Differ turning per-paint platform-view frames into an idempotent create/update/dispose backlog for embedding native views |
| `frust-shell-common` (signal-poll seams) | Small process-global slot-plus-poll seams (surface mode, theme, fonts, system UI) each mobile shell drains once per frame |
| `frust-shell-desktop` | winit event loop with a UI-thread/render-thread surface split and pipeline-cache persistence |
| `frust-shell-android` | Sanctioned-unsafe JNI FFI boundary, app-binding macro, and Choreographer-synced frame pipeline |
| `frust-shell-ios` | Sanctioned-unsafe C-ABI FFI boundary, app-binding macro, and CADisplayLink-driven frame pipeline |

## Layer Dependencies

All three concrete shells depend on `frust-core` (`RenderRoot`, widget tree, input, insets,
semantics), `frust-scene` (renderer-agnostic `Scene`/`SceneBuilder`), `frust-text` (a `TextContext`
threaded through layout), and `frust-theme` (theme delivery, brightness/design-language state).
Desktop, Android, and iOS additionally depend on `frust-render` (`SurfaceRenderer`, encode/present,
pipeline cache) and `frust-reactive` (`ReactiveRuntime`, `deep_link`, `back`, `task`); desktop and
Android also depend on `frust-paths` for pipeline-cache and cache-dir persistence, and Android
depends on `frust-plugin` to install the JavaVM/Context handle plugins read.

`frust-shell-common` is a hard platform-free leaf: it carries no `jni`/`ndk`/`winit` dependency, no
unsafe code, and no reactive dependency in its shipped surface, so it can be shared unconditionally
by all three concrete shells. Platform-specific FFI lives only in `frust-shell-desktop` (`winit` +
`accesskit_winit`), `frust-shell-android` (`jni`/`ndk`/`accesskit_android`), and `frust-shell-ios`
(`objc2`/`accesskit_ios`).

Each mobile shell confines all unsafe/FFI code to one named module — `jni_glue` on Android,
`ffi_glue` on iOS — a sanctioned-unsafe-zones convention that keeps the JNI and C-ABI boundaries
auditable as a single surface per platform rather than scattered through the crate. `surface_mode`'s
writer set is pinned by a source-scan conformance test: exactly one call site per platform may
publish a resolved surface mode, a single-writer contract that keeps the desktop/Android/iOS
signal-poll seam race-free without a lock.

## Data Flow

- Desktop: winit events drive `AppTree`'s rebuild → layout → paint, then hand the scene across the
  render-split channel for the render thread to encode and present.
- Android: a Choreographer callback consults `frame_gate` for a run/skip decision, then drives
  rebuild → conditional layout → paint → render-thread present; touch input feeds the app between
  frames.
- iOS: a CADisplayLink tick consults the same `frame_gate`, then unconditionally rebuilds/lays
  out/paints/presents, with optional present-sync gating against platform-view geometry.
- **Event pass under root owner:** all three shells route every input event through `AppTree::event`
  inside a reactive `Owner.with` wrap, so the event handler has access to root-level contexts
  (`Theme`, `WindowMetrics`, root-component `init`-installed values). Desktop names this wrapper
  `event_under_owner`, Android and iOS name it `under_root_owner` (with a panic-safe degrade for
  missing runtime). **Architectural reason:** `frust-reactive` is a leaf crate and cannot call
  `with_owner` itself, so shells must do it. **Residual limit:** the event pass is deliberately
  **not tracked** — entering `TrackedScope::track` resets dependencies and clears the dirty flag,
  so tracking dispatch would unsubscribe the frame loop from pending wakes and silently drop
  subsequent signals.
- Platform-view embedding: paint-time view frames feed the `platform_view` differ, which exposes a
  command backlog each shell's FFI layer polls and applies to the native view hierarchy, frame-paired
  to keep geometry in sync.
- Cross-cutting host signals (theme, insets, IME, deep-link, back, system UI, window metrics, reduced
  motion) arrive via each shell's native input path, translate into the framework's vocabulary, and
  force a relayout/repaint. Window metrics (logical size, scale, derived orientation, insets snapshot)
  are published from the points where the window's shape actually changes — Android (`set_window`,
  `split_recreate_surface`, `resize_surface`, `set_insets`), iOS (`set_surface`, `resize`,
  `set_insets`), and desktop (`resumed` and `WindowEvent::Resized`). Desktop gains app-facing
  window-shape context it never carried before; insets are seeded as `WindowInsets::default()` since
  winit 0.30 offers no cross-platform safe-area accessor. The shared `WindowMetricsPublisher` guards
  re-publication against unconditional per-frame churn (see CORE_ARCHITECTURE.md).
- Reduced motion is the one host signal whose sensor is not the appearance sensor. Android watches
  `Settings.Global.ANIMATOR_DURATION_SCALE` (reduced at exactly `0`) via a main-`Looper`
  `ContentObserver` registered while resumed, plus an `onResume` re-read — `onConfigurationChanged`
  never fires for it. iOS watches `UIAccessibility.isReduceMotionEnabled` via
  `reduceMotionStatusDidChangeNotification`, plus an `appDidBecomeActive` re-read —
  `traitCollectionDidChange` never fires for it. Desktop has no source at all, a checked gap rather
  than an oversight — winit exposes nothing. Delivery also bypasses `set_app_theme` on purpose:
  routing through it would latch `theme_override_active` and pin brightness, so each mobile shell
  instead edits `self.theme.motion.reduce_motion` directly, calls `push_theme()`, and sets
  `appearance_dirty` so the frame gate cannot skip the carrying tick.
- **Android appearance bidirectionality:** Kotlin can now read the app's current resolved theme
  brightness via `nativeAppIsDark` JNI, replacing re-derivation from `Configuration.uiMode`. Called
  after every `nativeSetAppearance` (when `Configuration` changes) **and** polled once per frame to
  catch runtime `frust::set_app_theme`/`clear_app_theme` calls that have no platform event of their
  own. This closes the race where theme changes pushed from Rust reached the status bar only after
  the next platform event.
- **IME content-type:** each focused widget publishes `ImeState::content_type` (Normal/Password/
  NoSuggestions/Terminal), and the shells destructure the state field-by-field into platform
  payloads per their capabilities and security posture:
  - **Android:** maps to `EditorInfo.inputType` (`TYPE_CLASS_TEXT`; Password, NoSuggestions, and
    Terminal all add `TYPE_TEXT_FLAG_NO_SUGGESTIONS`, Password additionally adds
    `TYPE_TEXT_VARIATION_PASSWORD`) and `EditorInfo.imeOptions` (`IME_FLAG_NO_PERSONALIZED_LEARNING`
    for all three), blocking the suggestion strip and personalized learning on each — each branch
    was previously missing one of its two required flags (fix F5), and `applyImeContentType` is now
    the first Kotlin change in three batches to have cleared `compileDebugKotlin`. Terminal
    additionally adds `IME_FLAG_NO_EXTRACT_UI` (no fullscreen extract-UI takeover for a raw
    byte-entry field); Android's `InputType`/`EditorInfo` vocabulary has no separate
    smart-punctuation bit, so `TYPE_TEXT_FLAG_NO_SUGGESTIONS` is already Terminal's whole available
    lever there. Handles content-type changes on a **steady-focused** field (field is active/focused
    but changes from Normal→Password mid-interaction) via `restartInput`, because Android never
    re-queries `EditorInfo` for a bound `InputConnection` — the hint must be reestablished by forcing
    a new connection.
  - **iOS:** JSON-serializes the content type alongside editing state for Swift to apply. Password
    and NoSuggestions also suppress the `UITextInputTraits` smart-quotes/dashes/insert-delete traits
    and autocapitalization (`autocapitalizationType = .none`) — closing the same silent-rewrite/
    auto-capitalize defect class the suggestion-strip leak closes, just for punctuation/case rather
    than disclosure; Terminal gets the full non-secret suppression matrix — `isSecureTextEntry =
    false` (not masked), autocorrect/spell-check/smart-punctuation all off, and
    `autocapitalizationType = .none`. Every one of the eight traits `applyImeContentType` manages is
    assigned explicitly in every arm, so no value from a prior classification survives a switch on
    the shared `forgeView` responder.
    `syncImeFocus` matches two of Android's three properties: the **per-frame poll** (driven from
    the `CADisplayLink` tick via `renderFrame`, the `doFrame` analogue) and the **divergence-guarded
    reconcile**, plus re-seeds the `UITextInput` mirror on content-type change and the resign/become
    cycle — a shared responder means a field switch has no focus edge, so both traits and mirror text
    must reconcile, not just apply once on focus. It has **no analogue of Android's `onKeyDown` call
    site**: iOS has no key event for the soft keyboard's Return, which arrives as `insertText("\n")`
    through `UITextInput` instead. The per-frame `becomeFirstResponder()` retry is now **bounded**
    (`imeFocusSatisfied`, fix F3) rather than fighting an intentional UIKit-originated resign every
    frame forever, since nothing on the Swift side can observe *why* first responder was resigned. A
    user touch on the Frust surface re-arms the bound *while the surface is not already first
    responder* (fixes F3b/F3c) — a self-dismissed field can be recovered by re-tapping it, while a
    touch inside a still-focused field cannot leave the bound dangling — but a touch landing inside a Mode B hosted slot never
    reaches this path (`FrustView.hitTest` returns `nil` there), so a sibling native control holding
    first responder is unaffected by the per-frame tick. **Swift side remains compile- and
    device-unverified on the build host** (Linux; even a Mac needs `xcodebuild`, and
    `aarch64-apple-ios` is not installed here) — see `docs/LIMITATIONS.md`
    `ime-ios-content-type-unverified`, which also records a residual gap in the touch-re-arm design.
  - **Desktop:** forwards to winit's `Window::set_ime_purpose(ImePurpose)`, which is **documented
    unsupported on all platforms except Wayland, and a cosmetic hint even there** (no secure-text
    entry). `ImePurpose` distinguishes only `Normal`/`Password`/`Terminal`; NoSuggestions has no
    corresponding category and maps to Normal, while Terminal maps exactly to `ImePurpose::Terminal`
    — winit's own purpose for raw byte entry, the first mapping this precise besides Password.
- Surface-mode resolution: each mobile shell resolves the host's declared translucency mode against
  actual surface capabilities at configure time and republishes the resolved verdict every frame.
- A set of additive, off-by-default kill-switch env vars (`FRUST_NO_RENDER_THREAD`,
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
| `WindowMetricsPublisher` | Shared, change-guarded path all three shells drive to provide window-shape context without per-frame re-provides (see CORE_ARCHITECTURE.md) |
| `android_app!` / `ios_app!` | Facade macros binding a generated app's state/logic to the fixed JNI / C-ABI export set |
