# Frust - PLUGINS Architecture

## Overview

PLUGINS is the leaf plugin tier sitting beside the `frust` facade. `frust-plugin` is a shared
Android platform-handle substrate (JavaVM/Context) that plugins needing JNI use to reach the OS. On
top of it sit six independent OS-capability plugins — `shared-preferences`, `secure-storage`,
`camera`, `clipboard`, `haptics`, and `iap` — each exposing one platform-independent public API
behind a per-platform backend. `iap` is in-app purchases and subscriptions over OpenIAP 3.0.1: Play
Billing via the `openiap-google` Kotlin host on Android, StoreKit 2 via the `FrustIap` Swift glue on
iOS, both sides speaking a JSON-string wire protocol, with purchase outcomes delivered on a
registered event listener rather than as a call's return value. A seventh crate,
`clean-signals-frust`, is a facade-tier plugin gluing the external `clean_signals`
clean-architecture core into Frust's `Component`/reactive model; it is a standalone workspace
excluded from the root Cargo graph pending a crates.io publication of its dependency.

See [ARCHITECTURE.md](ARCHITECTURE.md) for how PLUGINS relates to the other units.

## Module Structure

| Module | Responsibility |
|--------|-----------------|
| `crates/frust-plugin` | Android platform-handle substrate (JavaVM/Context) plus a scoped JNI-attach helper; inert on Apple |
| `plugins/shared-preferences` | Synchronous KV store routed to NSUserDefaults, Android SharedPreferences, or a JSON file backend |
| `plugins/secure-storage` | Synchronous, named-store secure string storage with an optional biometric gate, routed to Keychain, Android Keystore, or keyring-core |
| `plugins/camera` | Camera capability (permission, still capture, image stream, barcode/QR decode, torch) over CameraX (Android) or AVFoundation (Apple); preview surfaces as a native platform-view slot |
| `plugins/clipboard` | Synchronous plain-text clipboard over Android `ClipboardManager` (10+ returns nothing to an unfocused reader), iOS `UIPasteboard` (14+ shows a one-time paste banner), or desktop `arboard` (X11 clipboard content dies with the owning process unless a clipboard manager adopts it) |
| `plugins/haptics` | Fire-and-forget haptic effects over Android `Vibrator`/`VibrationEffect` or iOS `UI*FeedbackGenerator`; desktop is unavailable by design (no first-class API to route to) |
| `plugins/iap` | In-app purchases and subscriptions (products, purchases, restore, deep-link to subscription management) over Play Billing (`openiap-google`) or StoreKit 2 (`FrustIap` Swift glue), both speaking OpenIAP 3.0.1; desktop is a v1 deferral, not a capability gap |
| `plugins/clean-signals-frust` | Facade-tier glue crate binding the `clean_signals` clean-architecture core into Frust's `Component`/reactive model |

## Layer Dependencies

Every OS-capability plugin (`shared-preferences`, `secure-storage`, `camera`, `clipboard`,
`haptics`, `iap`) depends on `frust-plugin` and, where it needs a data directory, `frust-paths`,
plus its own target-gated FFI crates: `jni`/`ndk-context` on Android; `objc2` and the matching
`objc2-*` crates (foundation, security, local-authentication, av-foundation, ui-kit, etc.) on
Apple; `keyring-core` plus a secret-service/Credential-Manager backend for `secure-storage`'s
desktop/Linux/Windows path; `arboard` for `clipboard`'s desktop text backend (macOS shares this
arm rather than its Apple one, since `UIPasteboard` is UIKit-only). `iap` narrows this to `jni` on
Android and `objc2`/`objc2-foundation`/`block2` on iOS, with **no** desktop dependency at all — its
desktop arm is a dependency-free, always-erroring stub rather than a real backend. Its iOS package
also makes it the second plugin (after `camera`) to ship its own Swift package, and the first whose
package declares an **external** SwiftPM dependency (`OpenIAP`, exact-pinned) rather than only the
local `FrustEmbedding` one every other plugin package depends on.
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
permission/capture, every `iap` store round trip except `request_purchase`/
`set_purchase_listener`) — see [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md).

## Data Flow

- Android requires an explicit platform-handle init (JavaVM/Context, via `frust_plugin::android`)
  before any plugin can reach the OS, done through scoped, per-call JNI attaches; Apple needs no
  init step since `objc2` reaches the ObjC runtime globally. `camera` and `iap` additionally each
  ship their own Android `ContentProvider`-based init provider — installing the process's
  `Context` before `Application.onCreate` runs and caching the resumed `Activity` via
  `ActivityLifecycleCallbacks` (billing/capture flows both need one to launch a platform sheet
  from) — the same bootstrap pattern, now with two independent users.
- Each plugin exposes one platform-independent public API (`SharedPreferences`/`SecureStorage`/
  `Camera`/`Clipboard`/`Haptics`/`Iap`) that dispatches to a per-platform backend implementation; a
  shared conformance suite validates all backends uniformly, including a test-only file/desktop
  harness (`haptics` has no desktop backend to conform, being unavailable there by design; `iap`'s
  suite runs only against a `cfg(test)` in-memory fake store, since neither mobile backend can run
  host-side at all).
- Camera preview mounts as a native-sibling compositing slot via `frust::platform_view` — the core
  render pipeline paints nothing for it (see [SHELLS_ARCHITECTURE.md](SHELLS_ARCHITECTURE.md)) —
  while the underlying camera session's lifetime is independent of any single preview slot.
- The camera plugin's barcode/QR decoder runs Rust-side, synchronously inside the image-stream
  callback on the plugin-owned thread (never the UI thread) — the same lossy-latest backpressure
  that governs a raw image stream self-regulates decode cost. `rqrr` is the private v1 engine
  behind an internal, swappable decode-engine seam. Torch control is session-level state,
  independent of any stream, and dies with session close.
- Blocking or gated calls (secure-storage's biometric gate, camera's permission/capture, every
  `iap` store call except `request_purchase`) fail fast with a typed UI-thread error rather than
  parking when invoked on the platform UI thread; callers re-issue the call via
  `frust_reactive::spawn_blocking`.
- `iap` crosses the Rust↔Kotlin/Swift FFI boundary as JSON strings on both platforms — OpenIAP's
  own serializers on the host side, `serde`/`serde_json` on the Rust side — rather than typed
  per-field calls. A purchase is two-phase: `Iap::request_purchase` only acknowledges that the
  store accepted the request; the actual outcome (`IapEvent::PurchaseUpdated`/`PurchaseError`)
  arrives later on the listener registered via `Iap::set_purchase_listener`, delivered in reported
  order on the plugin's own event-delivery thread — one process-global consumer every store-SDK
  callback is handed to, since both hosts report on the platform main thread (Play Billing's
  `PurchasesUpdatedListener`; OpenIAP's main-actor listener delivery) — never the UI thread and
  never the thread that called `request_purchase`.
- Plugin distribution: `frust-drive`'s plugin registry applies each plugin's OS-side integration
  (Gradle module, Swift package, Info.plist key, manifest permission, Cargo dependency) onto a
  scaffolded app, driven by the `frust` TUI's Add Plugin dialog or manually per plugin README (see
  [CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md) and [TUI_ARCHITECTURE.md](TUI_ARCHITECTURE.md)).
  `haptics` is the registry's first entry to add a manifest permission
  (`android.permission.VIBRATE`) directly rather than folding it inside a plugin's own Gradle
  module, since its Android backend is plain JNI with no Kotlin helper class to carry it.
- `clean-signals-frust` wires a `clean_signals` controller into a Frust `Component`'s reactive
  `Owner`, surfacing failures and async state as `View`s; `Component` builds are coarse-grained,
  fully re-running on any tracked-signal write.

## Key Types

| Type | Purpose |
|------|---------|
| `PlatformHandleError` | Error reported when platform handles aren't yet installed or a JNI attach fails |
| `SharedPreferences` / `PrefsError` | The KV-store handle and its typed error enum |
| `SecureStorage` / `SecureStorageError` / `AuthPolicy` / `Accessibility` | The secure-store handle, its error enum, and the biometric-gate/accessibility policy types |
| `Camera` / `CameraSession` / `CameraError` / `ImageFrame` / `ImagePlane` | Camera entry point, an open session handle, its error enum (including `StreamBusy`, raised when the raw image stream and the barcode stream contend for the session's single stream claim), and the per-frame image-stream payload types |
| `Barcode` / `BarcodeFormat` / `DetectionPolicy` | A decoded barcode/QR result; its symbology enum (`#[non_exhaustive]`, QR-only in v1); the emit-timing policy (`NoDuplicates`/`Throttled`/`Unrestricted`) for `CameraSession::start_barcode_stream` |
| `Clipboard` / `ClipboardError` | Synchronous plain-text clipboard entry point and its error enum |
| `Haptics` / `HapticEffect` / `HapticsError` | Haptic-feedback entry point, its closed effect vocabulary, and its error enum |
| `Iap` / `IapError` / `IapEvent` / `IapErrorCode` / `ListenerHandle` | In-app-purchase entry point (stateless, one store connection per process); its error enum; the two purchase-outcome events (`PurchaseUpdated`/`PurchaseError`) delivered on the registered listener; the store-reported failure code carried inside a `PurchaseError`; the drop-to-unregister handle returned by `set_purchase_listener` |
| `use_controller` / `provide_controller` / `expect_controller` / `use_failure_listener` / `async_view` / `use_interval` | `clean-signals-frust`'s public hooks bridging a `clean_signals` controller into a Frust `Component`'s reactive `Owner` |
