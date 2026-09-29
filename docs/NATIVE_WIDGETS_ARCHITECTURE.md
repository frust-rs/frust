# Frust - NATIVE_WIDGETS Architecture

## Overview

NATIVE_WIDGETS (`plugins/native-widgets`) is a platform plugin rendering real OS controls — eight
controls shared by all three platform arms (Button, Label, Switch, Slider, ProgressBar, Image,
Spinner, DatePicker) plus two Apple-only controls (Segmented, Stepper) that exist on iOS and macOS
only — every other target resolves their builder to a frust-drawn refusal banner at compile time,
never an empty slot — plus arbitrary plugin-authored native view hierarchies. Every control is
driven entirely from Rust through exactly one generic factory and one generic listener class per
platform, in three arms: a Kotlin factory + listener pair on
Android (Mode-B platform-view slots — the differ punches a transparent hole and composites the
native view into it); a Rust-registered ObjC class on iOS, zero Swift involved (also Mode-B); and
on macOS one Rust `DesktopViewFactory` plus one `define_class!` target-action class, registered by
a `view_type` string through the desktop shell's Mode-A host — there the frust surface stays
opaque and the native view is an AppKit sibling composited *above* it, no punch (no Mac Catalyst).
New controls extend the existing generic dispatch rather than adding per-control platform glue.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how NATIVE_WIDGETS relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `runtime`/`registry` | Internal type-erased dispatch engine and retained-instance registry driving the create/update/dispose/event lifecycle; its `NativeCtx`/`NativeView` pair is real on each platform arm and a recording stand-in on a host with none, so the shared dispatch/diff/lifecycle contract is exercised by `cargo test` on Linux/Windows/web (not on Android/iOS/macOS, each of which swaps in its own real types) |
| `controls/*` | The built-in controls — `SHARED_KINDS` (eight, all three platform arms) plus `APPLE_KINDS` (two, iOS/macOS only) — each with shared host-tested props/decode logic plus per-platform implementations |
| `component.rs` | Public `NativeComponent` trait plus `Bridge<C>`, letting a third-party native view hierarchy run through the same internal runtime as the built-in controls; `ComponentCtx` has the same real-arm/host-stand-in split as `runtime`/`registry` |
| `api/*` | App-facing builders, events-as-signals wiring, and theme folding into native control params |
| `android/` (Kotlin) | One Kotlin factory + listener pair and frozen JNI exports, shipped as its own Gradle library module |
| `apple/` (objc2) | One Rust-registered ObjC factory + listener class, resolved at runtime via `NSClassFromString` — no Swift involved |
| `appkit/` (objc2) | One Rust `DesktopViewFactory` (`ensure_registered` registers it once, lazily, under a `view_type` string) plus one `define_class!` `FrustNativeControlTarget` class carrying one action selector for all interactive controls — no Swift involved |
| `coretext.rs` | Theme ladder L3's shared CoreText half (embedded-bytes-to-`CTFontDescriptor` resolution, latch-on-first-publish caching): one module used by both `apple` (re-exported as `apple::fonts`) and `appkit`, so the two Apple arms cannot drift from each other's typeface behaviour |

## Layer Dependencies

NATIVE_WIDGETS depends on `frust-plugin` (Android JavaVM/Context handle install), `jni` on Android,
and the `objc2` family on iOS (`objc2-ui-kit`/`objc2-quartz-core`/`objc2-core-text`/
`objc2-core-foundation`) and macOS (`objc2-app-kit`/`objc2-quartz-core`/`objc2-core-graphics`/
`objc2-core-text`/`objc2-core-foundation`), plus `thiserror`/`log`. Its optional, default-on
`frust-api` feature additionally pulls in `frust`/`frust-core`/`frust-theme`/`kurbo` for the
app-facing builder API; with that feature off, the crate is a bare leaf plugin. On Android and iOS
it integrates with `frust-shell-common`'s `platform_view`/Mode-B differ and platform-view-factory
contract (see [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)); on macOS it instead registers a
`frust_plugin::desktop::DesktopViewFactory` that `crates/frust-shell-macos`' desktop Mode-A host
resolves by `view_type` string — a different integration seam, not a Cargo dependency either way.

This is a chartered leaf plugin: `frust-plugin` + FFI only with default features off, a boundary
pinned by a no-default-features conformance gate. The one-generic-factory/one-generic-listener
shape per platform is a permanent architectural constraint, not an interim simplification — a new
control extends the existing dispatch rather than adding platform glue. Once shipped, JNI export
symbol names and the Kotlin package/class names are frozen; iOS gates on `target_os = "ios"` rather
than `target_vendor = "apple"`; and the FFI boundary uses hand-rolled flat-JSON parsing instead of
`serde`, keeping the wire format independent of any Rust-side type change.

That same FFI boundary caps who can practically extend `NativeComponent`: an app crate cannot
implement `NativeComponent` itself, because doing so means naming raw `jni`/`objc2-ui-kit` types
that this plugin does not re-export. The trait's practical audience is therefore plugin authors,
who take the FFI dependency directly — app code stays on the app-facing builders and never
touches `NativeComponent` at all.

## Data Flow

- Create/update/dispose calls route through the one platform factory into the internal runtime,
  which diffs props before any platform call and resolves disposal by view identity so late or
  duplicate calls are no-ops.
- Events: the platform listener fires on the main thread, routes by slot id into a typed payload
  delivered to app callbacks via signals, entirely bypassing the core render/event system described
  in [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md). Seven wire kinds cross that boundary — `CLICK`,
  `TOGGLED`, `VALUE_CHANGED`, `DRAG_START`, `DRAG_END` (1-5, all three arms), `SELECTION` (6,
  Apple-arm-only — the segmented control), `DATE` (7, all three arms — the date picker) — mirrored
  verbatim in `FrustNativeListener.kt`'s `KIND_*` constants and pinned against drift by
  `tests/kotlin_conformance.rs`.
- The `NativeComponent` path reuses the same runtime/factory plumbing as the built-in controls, so
  a plugin author's native view subtree mounts through one slot. A component attaches the
  platform's one listener class to any view it built — root or child — through
  `ComponentCtx::attach_listener`, naming a `ListenerKinds` family (click, toggled, value changed —
  the only three families a component can ask for today; no selection/date family yet). Each
  attach answers one `ListenerHandle`, kept in `NativeComponent::State` and released when the
  handle drops or the component is disposed. An event for an attached family routes by slot id to
  `NativeComponent::on_event`, mapped into the same `EventPayload` vocabulary the builders' own
  signals use; a family the slot never attached is dropped before the component sees it. An event
  carries only the slot id, kind and a primitive detail — never which child fired — so a composite
  with two interactive children in the same family cannot be told apart in `on_event`
  ([LIMITATIONS.md](LIMITATIONS.md)'s
  `native-widgets-component-same-kind-children-indistinguishable`). The plugin's own non-default
  `demo-components` feature proves the whole define → register → mount → attach path with
  `DemoCard`, one composite (a parent view, a title label, two buttons — only the primary button's
  clicks wired, for exactly this reason) published as one slot: real native coverage on all three
  platform arms (a Kotlin `LinearLayout` on Android, a `UIView` with explicit child frames on iOS,
  a layer-backed `NSView` with explicit child frames on macOS) plus a fourth, host-only arm on
  Linux/Windows/web that records the same create/update/dispose plan instead of building real
  views.
- Mode-B translucency (Android/iOS only): a native-widgets slot needs the frust surface
  above/beside it to actually resolve translucent to be visible. The `frust-engine` render path
  never itself refuses a translucent surface (every backend/alpha-mode pair renders, see
  [RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md)); a surface can still resolve opaque when the
  platform's own compositor offers no translucent alpha mode, surfaced to app/plugin code as
  `frust::resolved_surface_mode() == ResolvedSurfaceMode::RefusedTranslucent`. The macOS arm has no
  such concern: its desktop Mode-A host never punches the surface at all (see Overview).
- Theme ladder: `Theme` folds into control props every frame but is diff-gated, so an unchanged
  theme costs zero FFI calls (see [WIDGETS_ARCHITECTURE.md](WIDGETS_ARCHITECTURE.md) for `Theme`
  itself). Typefaces resolve through a **two-step ladder, independently per slot** (button/body):
  `NativeTypefaces` (a design system's attached `ThemeExtensions` payload — `frust_glyph::baseline()`
  is the one built-in that attaches it, publishing its bundled monospace faces) beats the
  platform's own system font. There is no design-language shortcut: `Theme::design_language` is
  never read here, so a Glyph-tagged theme with no `NativeTypefaces` attached gets the platform
  font like any other. Publishing is last-pair-wins and diff-gated the same as every other prop,
  but the *platform* halves still latch their first published pair — a mid-process face swap
  re-publishes host-side without re-registering on device until relaunch (see
  [LIMITATIONS.md](LIMITATIONS.md)'s `native-typeface-first-publish-latch`). On macOS the ladder is
  L1 `NSAppearance` (Dark/Light Aqua), re-pinned on the hosted view's whole subtree on *every*
  create and update — not baked once at construction like Android — because AppKit exposes no
  "construct against a themed context" step and a culled-then-recreated control must not diverge
  from its never-culled siblings; L2 is a packed-ARGB-to-`NSColor`/`CGColor` mapping applied
  per-control, with `NSSwitch` and `NSProgressIndicator` exposing no tintable property at all (both
  draw in the system accent colour regardless, logged and no-op'd rather than faked); L3 is the
  `coretext.rs` module shared with iOS.

## Key Types

| Type | Purpose |
|------|---------|
| `native_button` / `native_label` / `native_switch` / `native_slider` / `native_progress` / `native_image` / `native_spinner` / `native_date_picker` | The eight shared app-facing builders (all three platform arms), each composing one platform_view slot |
| `native_segmented` / `native_stepper` | The two Apple-only app-facing builders (iOS + macOS; a compile-time refusal banner everywhere else) |
| `CivilDate` | Plain year/month/day value type `native_date_picker` takes and its `DATE` event payload reports back |
| `NativeComponent` (+ `ComponentCtx`) | Public trait for a plugin author to drive a native view hierarchy from Rust |
| `ListenerKinds` / `ListenerHandle` | Which event families (click, toggled, value changed) a component attaches to a view it built, and the RAII handle releasing that attach |
| `native_component` / `register_component` | Generic define→register→mount path for a `NativeComponent`, reusing the controls' factory/runtime plumbing |
