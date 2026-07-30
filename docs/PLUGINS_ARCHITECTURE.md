# Frust - PLUGINS Architecture

## Overview

PLUGINS is the leaf plugin tier sitting beside the `frust` facade. `frust-plugin` is a shared
Android platform-handle substrate (JavaVM/Context) that plugins needing JNI use to reach the OS. On
top of it sit three independent OS-capability plugins — `shared-preferences`, `secure-storage`, and
`camera` — each exposing one platform-independent public API behind a per-platform backend. A
fourth crate, `clean-signals-frust`, is a facade-tier plugin gluing the external `clean_signals`
clean-architecture core into Frust's `Component`/reactive model; it is a standalone workspace
excluded from the root Cargo graph pending a crates.io publication of its dependency.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how PLUGINS relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `crates/frust-plugin` | Android platform-handle substrate (JavaVM/Context) plus a scoped JNI-attach helper; inert on Apple |
| `plugins/shared-preferences` | Synchronous KV store routed to NSUserDefaults, Android SharedPreferences, or a JSON file backend |
| `plugins/secure-storage` | Synchronous, named-store secure string storage with an optional biometric gate, routed to Keychain, Android Keystore, or keyring-core |
| `plugins/camera` | Camera capability (permission, still capture, image stream) over CameraX (Android) or AVFoundation (Apple); preview surfaces as a native platform-view slot |
| `plugins/clean-signals-frust` | Facade-tier glue crate binding the `clean_signals` clean-architecture core into Frust's `Component`/reactive model |

## Layer Dependencies

Every OS-capability plugin (`shared-preferences`, `secure-storage`, `camera`) depends on
`frust-plugin` and, where it needs a data directory, `frust-paths`, plus its own target-gated FFI
crates: `jni`/`ndk-context` on Android; `objc2` and the matching `objc2-*` crates (foundation,
security, local-authentication, av-foundation, etc.) on Apple; `keyring-core` plus a
secret-service/Credential-Manager backend for `secure-storage`'s desktop/Linux/Windows path.
`clean-signals-frust` instead depends on the `frust` facade crate — its sole framework dependency —
to bind a `clean_signals` controller into a `Component`'s reactive `Owner`.

This is an architectural charter, not just current practice: platform plugins depend on
`frust-plugin` (plus `frust-paths` where needed) and FFI crates only, never another `frust-*`
framework crate; and the facade never depends on or re-exports a plugin — the dependency always
runs from an app's own manifest into the plugin, never through the facade. Each plugin's backends
are cfg-gated modules (`apple`/`android`/`file`/`desktop`/`unsupported`) behind one
platform-independent public API, with FFI dependencies target-gated rather than unconditional.

A few cross-plugin conventions hold as boundary facts rather than mere style: pre-init detection is
a plain atomic flag checked first, so a `NotInitialized` error holds even under `panic=abort`; a JNI
attach is always scoped per call, never held permanently; and a store shared with the OS namespaces
its keys so plugin keys cannot collide with another library's. `frust-reactive`'s `spawn_blocking`
is the documented pairing for every plugin's blocking or gated call (biometric prompts, camera
permission/capture) — see [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md).

## Data Flow

- Android requires an explicit platform-handle init (JavaVM/Context, via `frust_plugin::android`)
  before any plugin can reach the OS, done through scoped, per-call JNI attaches; Apple needs no
  init step since `objc2` reaches the ObjC runtime globally.
- Each plugin exposes one platform-independent public API (`SharedPreferences`/`SecureStorage`/
  `Camera`) that dispatches to a per-platform backend implementation; a shared conformance suite
  validates all backends uniformly, including a test-only file/desktop harness.
- Camera preview mounts as a native-sibling compositing slot via `frust::platform_view` — the core
  render pipeline paints nothing for it (see [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)) —
  while the underlying camera session's lifetime is independent of any single preview slot.
- Blocking or gated calls (secure-storage's biometric gate, camera's permission/capture) fail fast
  with a typed UI-thread error rather than parking when invoked on the platform UI thread; callers
  re-issue the call via `frust_reactive::spawn_blocking`.
- Plugin distribution: `frust-drive`'s plugin registry applies each plugin's OS-side integration
  (Gradle module, Swift package, Info.plist key, Cargo dependency) onto a scaffolded app, driven by
  the `frust` TUI's Add Plugin dialog or manually per plugin README (see
  [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md) and [TUI_ARCHITECTURE.md](TUI_ARCHITECTURE.md)).
- `clean-signals-frust` wires a `clean_signals` controller into a Frust `Component`'s reactive
  `Owner`, surfacing failures and async state as `View`s; `Component` builds are coarse-grained,
  fully re-running on any tracked-signal write.

## Key Types

| Type | Purpose |
|------|---------|
| `PlatformHandleError` | Error reported when platform handles aren't yet installed or a JNI attach fails |
| `SharedPreferences` / `PrefsError` | The KV-store handle and its typed error enum |
| `SecureStorage` / `SecureStorageError` / `AuthPolicy` / `Accessibility` | The secure-store handle, its error enum, and the biometric-gate/accessibility policy types |
| `Camera` / `CameraSession` / `CameraError` / `ImageFrame` / `ImagePlane` | Camera entry point, an open session handle, its error enum, and the per-frame image-stream payload types |
| `use_controller` / `provide_controller` / `expect_controller` / `use_failure_listener` / `async_view` / `use_interval` | `clean-signals-frust`'s public hooks bridging a `clean_signals` controller into a Frust `Component`'s reactive `Owner` |
