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
excluded from the root Cargo graph pending a crates.io publication of its dependency. An eighth
crate, `plugins/database` (`frust-database`), is the tier's first plugin with no OS integration
at all: a synchronous embedded SQL API over a swappable engine seam — bundled SQLite via
`rusqlite` by default, an optional Turso (Rust-native SQLite rewrite) engine. A ninth crate,
`plugins/i18n` (`frust-i18n` plus its `frust-i18n-macros` companion — the tier's first
proc-macro crate), is a Fluent Project + ICU4X internationalization plugin: compile-time Fluent
bundle loading, locale-aware message resolution, system-locale detection, and (behind the
`formatting` feature) ICU4X number/date/currency formatting — reaching no OS capability beyond
a locale read.

A tenth through twelfth crate — `plugins/glyph`, `plugins/material`, `plugins/cupertino`
(`frust-glyph`/`frust-material`/`frust-cupertino`) — are the tier's **design-system plugins**: the
three widget catalogs (Glyph, Material 3, Cupertino) that used to live in-tree as feature-gated
`frust-widgets` modules now ship as ordinary sibling crates, each an app dependency beside `frust`
rather than a cargo feature on it. They reach no OS capability at all and share nothing with the
OS-capability plugins above beyond sitting in the same tier; see *Design-System Plugins* below.

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
| `plugins/database` | Synchronous embedded SQL database (`Database`/`Value`/`Engine`) over a swappable-engine seam — bundled SQLite via `rusqlite` (default) or an optional Turso engine (`engine-turso`); no OS integration |
| `plugins/i18n` | Fluent Project + ICU4X internationalization/localization: compile-time bundle loading (`locales!`, via the companion `frust-i18n-macros` proc-macro crate), locale-aware message resolution, system-locale detection, and (`formatting` feature) ICU4X number/date/currency formatting |
| `plugins/glyph` | The Glyph design-system plugin (`frust-glyph`): terminal-native, dark-first, monospace-led widget catalog — including its own switch-class control (`toggle`), since baseline `frust-widgets` deliberately ships no `Switch` — plus its bundled OFL monospace fonts |
| `plugins/material` | The Material 3 (+Expressive) design-system plugin (`frust-material`) |
| `plugins/cupertino` | The Cupertino (iOS-styled) design-system plugin (`frust-cupertino`), including its own "Liquid Glass" `GlassScale` recipe |

## Desktop Backend Status

Every plugin above targets Android/iOS first; desktop coverage is uneven by design, not omission —
this is the ground truth an app author needs before assuming a plugin "just works" in a desktop
preview or a `frust build macos|windows|linux`. See
[CLI_ARCHITECTURE.md](CLI_ARCHITECTURE.md)'s Data Flow for how a plugin's desktop-lane
`Contribution`s reach an assembled bundle.

| Plugin | Desktop status | Detail |
|--------|-----------------|--------|
| `shared-preferences` | generic-desktop, macOS-native on macOS | NSUserDefaults (macOS, sharing the Apple arm with iOS) / a JSON file backend (Linux, Windows) |
| `secure-storage` | generic-desktop, macOS-native on macOS | Keychain (macOS, sharing the Apple arm with iOS) / `keyring-core` + secret-service or Credential Manager (Linux, Windows) |
| `camera` | macOS-native only | AVFoundation (macOS, sharing the Apple arm with iOS); no backend at all on Linux/Windows |
| `clipboard` | generic-desktop | `arboard` on macOS, Linux, and Windows alike — macOS shares this arm rather than its own Apple `UIPasteboard` one (UIKit-only) |
| `haptics` | unavailable-by-design | no first-class OS API to route to, on macOS, Linux, or Windows |
| `iap` | deferred (v1) | dependency-free, always-erroring stub on macOS, Linux, and Windows — mobile-first scope, not a capability gap (`iap-desktop-unavailable-v1` in [LIMITATIONS.md](LIMITATIONS.md)) |
| `clean-signals-frust` | platform-free | facade-tier glue with no OS integration to split by platform at all |
| `database` | platform-free | file IO via `rusqlite`/`turso`; no OS integration, so no platform split |
| `i18n` | platform-free | reaches the OS only for a `sys_locale` read; no backend split |
| `glyph` / `material` / `cupertino` | platform-free | pure widget/token crates over `frust::authoring`; no OS integration of any kind, desktop included |
| `native-widgets` (NATIVE_WIDGETS unit) | unavailable | no desktop backend of any kind — Android/iOS only (see [NATIVE_WIDGETS_ARCHITECTURE.md](NATIVE_WIDGETS_ARCHITECTURE.md)) |

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
to bind a `clean_signals` controller into a `Component`'s reactive `Owner`. `database` depends on
neither `frust-plugin` nor any target-gated FFI crate — its own dependencies are `frust-paths` (data
directory), `log`, and its two swappable SQLite engines, `rusqlite` (default, C via `cc`) and the
optional pure-Rust `turso`; both reach storage through plain file IO, so the crate needs no
platform-handle substrate at all. `i18n` is the second plugin (after `native-widgets`) built on the single-crate
platform-plugin-plus-facade-glue shape (`docs/PLUGINS_CODE_STANDARDS.md`'s Plugin Conventions): a
default-on `frust-api` feature gates its sole `frust` facade dependency, so `cargo check -p
frust-i18n --no-default-features` mechanically re-verifies the platform-plugin charter line — no
`frust` crate anywhere in the tree — the way every split plugin's two-crate boundary enforces
structurally instead. Its platform track is `frust-plugin` plus `jni`, Android-target-gated rather
than unconditional — a documented deviation from every Android-reaching plugin's norm above, since
only its Android detection backend needs the platform handle — plus `objc2`/`objc2-foundation` on
Apple and `sys-locale` on desktop (macOS routes through the `sys-locale` arm, not the Apple one).
Its message engine depends on `fluent-bundle`/`fluent-langneg`/`unic-langid`; the `formatting`
feature layers ICU4X's `icu_decimal`/`icu_datetime`/`icu_plurals`/`icu_experimental` plus their
`icu_locale_core`/`tinystr`/`icu_provider` support crates underneath. Like `database`, it needs no
`frust-paths` — it persists nothing itself.

This is an architectural charter, not just current practice: a plugin depends on `frust-plugin`
(plus `frust-paths` where needed) and FFI crates only if it reaches the OS through one, never
another `frust-*` framework crate; and the facade never depends on or re-exports a plugin — the
dependency always runs from an app's own manifest into the plugin, never through the facade.
`database` is the first plugin to need neither `frust-plugin` nor an FFI crate at all — a pure-Rust
plugin whose "platform" is the filesystem — which the charter accommodates rather than exempts: it
still depends on nothing but `frust-paths`, `log`, and its engines, never another framework crate. Each
plugin's backends are cfg-gated modules (`apple`/`android`/`file`/`desktop`/`unsupported`) behind
one platform-independent public API, with FFI dependencies target-gated rather than unconditional.
The three design-system plugins are a fourth, distinct shape the charter above accommodates rather
than covers: no `frust-plugin`, no FFI crate, and — unlike every OS-capability plugin — a direct
`frust` facade dependency, since a widget catalog's whole job is building against `frust::authoring`
(see *Design-System Plugins* below).

A few cross-plugin conventions hold as boundary facts rather than mere style: pre-init detection is
a plain atomic flag checked first, so a `NotInitialized` error holds even under `panic=abort`; a JNI
attach is always scoped per call, never held permanently; and a store shared with the OS namespaces
its keys so plugin keys cannot collide with another library's. Plugin Kotlin/Swift host code may
ship warn-level store/host-failure logs in release builds — release-lean governs Rust's own `log`
output only, and payload-content hygiene (truncation/redaction) is a separate concern: `iap`'s
`Purchase`/`PurchaseInput`/`ActiveSubscription` redact their bearer token in `Debug`, and
host-payload excerpts embedded in error strings are capped at `PAYLOAD_EXCERPT_BYTES`.
`frust-reactive`'s `spawn_blocking` is the documented pairing for every plugin's blocking or gated
call (biometric prompts, camera permission/capture, every `iap` store round trip except
`request_purchase`/`set_purchase_listener`) — see [CORE_ARCHITECTURE.md](CORE_ARCHITECTURE.md).

## Design-System Plugins

`frust-glyph`/`frust-material`/`frust-cupertino` are the three built-in widget catalogs, extracted
from `frust-widgets` into sibling plugin crates (see [WIDGETS_ARCHITECTURE.md](WIDGETS_ARCHITECTURE.md)'s
External Design-System Contract for the toolkit they build against). Their charter:

- **Production deps: `frust` (`default-features = false`) plus `kurbo`/`peniko` only, never
  another `frust-*` crate.** This is what makes each one a real proof of the external
  design-system contract rather than a special-cased in-tree exception — an app depends on any of
  the three exactly the way it would depend on a third-party catalog.
- **`frust-widgets` (`test-support` feature) and `frust-core` (test-only) are dev-dependencies,
  never production ones.** The sanctioned test-fixture/`RenderRoot` route: a catalog's own tests
  need a real `RenderRoot` to paint against and `frust-widgets`' GPU-free container fixtures, both
  of which the facade deliberately does not re-export to production code.
- **`install()` is the one-line entry point, called from `app!`'s `setup = { .. }` block —
  before shell construction, the one point a shell reads the default-theme slot.** Each `install()`
  calls `frust::set_default_theme(baseline())`; `frust_glyph::install()` additionally calls
  `frust::register_app_fonts` for its bundled OFL monospace faces (Space Mono, IBM Plex Mono,
  `plugins/glyph/fonts/`) — registered unconditionally, not behind a feature, since the crate has
  no feature to gate them with. A call after shell construction takes effect only on a later
  theme reseed, which may never happen.
- **`frust_glyph::baseline()` attaches the `NativeTypefaces` theme extension** — the bundled faces
  reach `frust-native-widgets`' native controls through this attach, not through any
  `DesignLanguage`-keyed special case (see [NATIVE_WIDGETS_ARCHITECTURE.md](NATIVE_WIDGETS_ARCHITECTURE.md)'s
  theme ladder). Material's and Cupertino's `baseline()` attach no font extension.

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
- `database` dispatches `Database::execute`/`query`/`transaction` through a crate-private
  `EngineConn` seam to whichever engine is compiled in; both engines enforce the same interop
  discipline (WAL journal mode, no `mvcc`/cipher/hexkey pragma) so a file either engine writes
  stays readable by the other. Its `turso` backend is the tier's first to own a process-wide OS
  thread running a dedicated tokio runtime: seam calls send their future to that thread over a
  channel and park, rather than `block_on`-ing on the caller's own thread, so there is no
  panic-under-abort path. `DatabaseError::AsyncContext` is returned for the provable subset of
  wrong-context callers (inside a runtime's `block_on` body, not inside a spawned task); like
  every plugin's UI-thread discipline, `database`'s is docs-only (no typed guard) and
  `frust_reactive::spawn_blocking` is the sanctioned call path, never rejected by the guard.
  `Database::transaction` rolls back on every exit but a successful `COMMIT` — closure `Err`, a
  failing `COMMIT`, or a panicking closure, the last via a `Drop`-guard rollback; same-thread
  reentrancy is refused as a typed `Reentrant` error rather than deadlocking, while cross-thread
  contention still queues on the connection mutex.
- `i18n`'s `locales!` proc macro (`frust-i18n-macros`) expands per invoking module into
  `locale_set()`/`engine()`/typed `keys::` functions built over the `Resolve` seam every
  resolver — `Engine::with_chain`, the reactive `I18n` handle — implements; every `.ftl` file is
  parsed at macro-expansion time, so a malformed message is a compile error naming file/line
  rather than a runtime miss.
- `i18n`'s detection (`system_locales`) re-queries the OS on every call — no cache, no
  platform-change event plumbing — so an app that wants to react to a live system-locale change
  polls it itself (e.g. on resume) and re-negotiates through `I18n::set_locale`.
- `i18n`'s reactive `I18n` handle holds the active locale in a tracked `RwSignal`; `set_locale`
  re-negotiates and writes it, waking the shell through the normal signal-write → `FrameWaker`
  path like any other app-state change — a coarse, whole-subscribed-tree rebuild, the same shape
  `clean-signals-frust`'s `Component` builds take above.
- `i18n`'s `formatting` feature registers ICU-backed `NUMBER`/`DATETIME` Fluent functions per
  locale via `LocaleSet::with_locale_function` (one formatter factory call per registered
  locale, keyed by it), reachable both directly (`fmt::decimal`/`currency`/`date`/`time`) and
  from inside an `.ftl` message.

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
| `Iap` / `IapError` / `IapEvent` / `IapErrorCode` / `ListenerHandle` | In-app-purchase entry point (stateless, one store connection per process); its error enum (`Store` = a store refusal, `Platform` = a glue/boundary defect — Android tells them apart via a `synthetic` marker field the host JSON can never legitimately carry, iOS via the throw's type); the two purchase-outcome events (`PurchaseUpdated`/`PurchaseError`) delivered on the registered listener; the store-reported failure code carried inside a `PurchaseError`; the drop-to-unregister handle returned by `set_purchase_listener` |
| `use_controller` / `provide_controller` / `expect_controller` / `use_failure_listener` / `async_view` / `use_interval` | `clean-signals-frust`'s public hooks bridging a `clean_signals` controller into a Frust `Component`'s reactive `Owner` |
| `Database` / `Value` / `Engine` / `DatabaseError` | The embedded-SQL entry point (one serialized connection per handle, `Send + Sync`); the five-SQLite-storage-class param/result value; the compiled-engine selector (`Sqlite`/`Turso`, `#[non_exhaustive]`); the typed error enum (`Storage`, `Sql`, `AsyncContext`, `EngineUnavailable`, `Reentrant`) |
| `I18n` / `Locale` / `I18nError` | The reactive locale handle (`frust-api`) pairing an `frust_i18n::Engine` with a tracked active-locale signal, exposing `locale()` (message locale for bundle resolution) and `format_locale()` (composed message-language + requested-region for ICU formatting); the BCP-47 locale newtype; the crate's one public error enum |
| `frust_i18n::Engine` / `LocaleSet` / `Resolve` | The immutable, `Send + Sync` Fluent bundle core and its builder; the message-resolution trait both `locales!`-generated typed-key functions and the reactive `I18n` handle implement |
| `locales!` | `frust-i18n-macros`' compile-time proc macro loading a locale directory into `locale_set()`/`engine()`/typed `keys` |
| `fmt` (`CivilDate` / `CivilTime` / `DateLength`) | ICU4X-backed decimal/percent/currency/date/time formatting entry points and their date/time value types (`formatting` feature) |
