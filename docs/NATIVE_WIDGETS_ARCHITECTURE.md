# Frust - NATIVE_WIDGETS Architecture

## Overview

NATIVE_WIDGETS (`plugins/native-widgets`) is a platform plugin rendering real OS controls —
Button, Label, Switch, Slider, ProgressBar, Image — plus arbitrary plugin-authored native view
hierarchies, all as Mode-B platform-view slots. Every control is driven entirely from Rust through
exactly one generic factory and one generic listener class per platform: a Kotlin pair on Android,
and a Rust-registered ObjC class on iOS with zero Swift involved. New controls extend the existing
generic dispatch rather than adding per-control platform glue.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how NATIVE_WIDGETS relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `runtime`/`registry` | Internal type-erased dispatch engine and retained-instance registry driving the create/update/dispose/event lifecycle; fully host-testable without JNI/ObjC |
| `controls/*` | The six v1 controls, each with shared host-tested props/decode logic plus per-platform Android/iOS implementations |
| `component.rs` | Public `NativeComponent` trait plus `Bridge<C>`, letting a third-party native view hierarchy run through the same internal runtime as the six controls |
| `api/*` | App-facing builders, events-as-signals wiring, and theme folding into native control params |
| `android/` (Kotlin) | One Kotlin factory + listener pair and frozen JNI exports, shipped as its own Gradle library module |
| `apple/` (objc2) | One Rust-registered ObjC factory + listener class, resolved at runtime via `NSClassFromString` — no Swift involved |

## Layer Dependencies

NATIVE_WIDGETS depends on `frust-plugin` (Android JavaVM/Context handle install), `jni` on Android
and the `objc2` family on iOS, plus `thiserror`/`log`. Its optional, default-on `frust-api` feature
additionally pulls in `frust`/`frust-core`/`frust-theme`/`kurbo` for the app-facing builder API; with
that feature off, the crate is a bare leaf plugin. It integrates with
`frust-shell-common`'s `platform_view`/Mode-B differ and platform-view-factory contract (see
[SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)) at the integration level, not as a Cargo
dependency.

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
who take the FFI dependency directly — app code stays on the six builders and never touches
`NativeComponent` at all.

## Data Flow

- Create/update/dispose calls route through the one platform factory into the internal runtime,
  which diffs props before any platform call and resolves disposal by view identity so late or
  duplicate calls are no-ops.
- Events: the platform listener fires on the main thread, routes by slot id into a typed payload
  delivered to app callbacks via signals, entirely bypassing the core render/event system described
  in [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md).
- The `NativeComponent` path reuses the same runtime/factory plumbing as the six built-in controls,
  so a plugin author's native view subtree mounts through one slot. It is display-only in this
  build, though: no production path attaches a listener to a component-built view, so overriding
  `NativeComponent::on_event` has no effect (a deferred Phase 4 gap) — only the six built-in
  controls actually deliver events end to end today.
- Mode-B translucency: a native-widgets slot needs the frust surface above/beside it to actually
  resolve translucent to be visible. The `frust-engine` render path never itself refuses a
  translucent surface (every backend/alpha-mode pair renders, see
  [RENDER_ARCHITECTURE.md](RENDER_ARCHITECTURE.md)); a surface can still resolve opaque when the
  platform's own compositor offers no translucent alpha mode, surfaced to app/plugin code as
  `frust::resolved_surface_mode() == ResolvedSurfaceMode::RefusedTranslucent`.
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
  [LIMITATIONS.md](LIMITATIONS.md)'s `native-typeface-first-publish-latch`).

## Key Types

| Type | Purpose |
|------|---------|
| `native_button` / `native_label` / `native_switch` / `native_slider` / `native_progress` / `native_image` | The six app-facing builders, each composing one platform_view slot |
| `NativeComponent` (+ `ComponentCtx`) | Public trait for a plugin author to drive a native view hierarchy from Rust |
| `native_component` / `register_component` | Generic define→register→mount path for a `NativeComponent`, reusing the controls' factory/runtime plumbing |
