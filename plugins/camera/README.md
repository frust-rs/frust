# frust-camera

A platform-independent camera API for frust apps — permission, live preview,
still capture, and a YUV/BGRA image stream — CameraX on Android
(`dev.frust.camera.FrustCameraHost`, driven over this plugin's own JNI
surface) and AVFoundation on iOS (driven straight from Rust via
`objc2-av-foundation`, no Swift camera glue). It is the first frust plugin
whose preview renders as a **platform view** rather than anything frust
paints itself — Flutter's `camera` plugin is the API model, but its
external-texture preview mechanism has no equivalent on our stack (see
`docs/ARCHITECTURE.md`'s Module Structure / Platform-view flow).

Like every frust **platform plugin**, this crate is added to your app's own
`Cargo.toml` alongside `frust` (the pubspec model) — the `frust` facade does
not re-export it.

> **Templates stay clean.** A generated frust project ships **no** camera
> code, permissions, or plist keys. You add exactly the lines below by hand —
> or let the frust TUI's **Add Plugin** dialog apply them for you (it
> automates every step in this document). Nothing here is scaffolded, so an
> app that never uses the camera never carries an unused
> `NSCameraUsageDescription` string or `CAMERA` permission. On Android the
> plugin's platform code ships as its own Gradle library module rather than
> as files copied into your app, so there is nothing to keep in sync by hand;
> on iOS it ships as its own local Swift package, added as a second package
> reference beside the embedding's `FrustEmbedding` — no pbxproj source-list
> surgery either.

---

## 1. Add the dependency (always)

The **only** required Cargo step is the dependency line:

```toml
# app Cargo.toml — [dependencies]
frust-camera = { path = "<frust>/plugins/camera" }  # crates.io later
```

`<frust>` is the path to your frust checkout — derive it from the `frust = {
path = "…" }` line the scaffold already wrote.

```rust
use frust_camera::{Camera, Lens, Resolution};

let session = Camera::open(Lens::Back, Resolution::Auto)?;
```

Opening a session is the platform-independent half. The other half — showing
its live preview — needs the platform additions in §2, since the preview is
composited by the OS itself, not painted by frust.

---

## 2. The preview is a platform view — extra setup, always needed for preview

Unlike `frust-shared-preferences`/`frust-secure-storage`, this plugin has no
"storage-only, stop here" shortcut: any app that shows a live preview needs
both platform additions below (permission and still capture work without
them, but you'll be looking at a black rectangle).

```rust
let view_type = session.preview_view_type(); // target-gated — see the doc below
frust::platform_view(view_type)
    .params(session.params_json())
    // size it off `session.preview_aspect_ratio()` once non-zero
```

`CameraSession::preview_view_type()` is **target-gated by design** (platform-
views' W5 finding, restated on that method's own doc): Android returns the
fully-qualified `"dev.frust.camera.CameraPreviewFactory"`, iOS the bare
`"CameraPreviewFactory"` — never hardcode either string yourself, always call
the method.

### 2a. Android setup — include this plugin's Gradle module

The TUI Add Plugin dialog (`frust tui` → Add Plugin → camera) makes both
edits below for you, idempotently. By hand:

1. `android/settings.gradle.kts` — include the module by path, and redirect
   its build directory so two apps can share one frust checkout:

   ```kotlin
   include(":frust-camera")
   project(":frust-camera").projectDir =
       file("<frust checkout>/plugins/camera/platform/android")

   gradle.lifecycle.beforeProject {
       if (path == ":frust-camera") {
           layout.buildDirectory.set(rootDir.resolve("build/frust-camera"))
       }
   }
   ```

   The path is derived the same way `gradle.properties`' `frust.embedding.dir`
   is — one machine-specific line, replaced by a Maven coordinate once the
   module publishes.

2. `android/app/build.gradle.kts` — depend on it:

   ```kotlin
   dependencies {
       implementation(project(":frust-camera"))
   }
   ```

The plugin's own `AndroidManifest.xml` (inside the module) already declares
`android.permission.CAMERA` — the manifest merger folds it into your app, so
there is nothing to add to your own manifest. `dev.frust.camera.FrustCameraHost`
and `dev.frust.camera.CameraPreviewFactory` are both hard contracts (JNI
symbol names and a classloader-resolved `viewType` respectively) — never move
or rename either class in this module.

### 2b. iOS setup — add the plugin's local Swift package

The **first** frust plugin to ship as a Swift package (`plugins/camera/platform/ios`,
product `FrustCamera`), added to your project as a second local package
reference beside the embedding's `FrustEmbedding` — the same mechanism
`Contribution::SwiftPackageRef` automates. The TUI Add Plugin dialog does
this for you; by hand, in Xcode: **File → Add Package Dependencies… → Add
Local…**, select `<frust checkout>/plugins/camera/platform/ios`, and add the
`FrustCamera` product to the `Runner` target (alongside the existing
`FrustEmbedding` package — SwiftPM dedupes the shared dependency rather than
vendoring it twice).

Add to `Info.plist` (Apple requires this string in the **app's own**
Info.plist — no plugin module can supply it):

```xml
<key>NSCameraUsageDescription</key>
<string>Take photos and record video.</string>
```

No other iOS wiring is needed: `plugins/camera/src/apple.rs` drives
AVFoundation directly from Rust (permission, session config, capture,
stream) — the Swift package contributes only the preview-layer factory,
no capture/permission code of its own.

---

## 3. Blocking calls — pair with `spawn_blocking`, never the UI thread

**Three** calls in the table below can block the calling thread — but only
the first two refuse to run on the platform's UI thread at all;
`CameraSession::close` carries no such guard and genuinely blocks it (see
below the table). Like `frust-secure-storage`'s gated calls, pair each with
`frust_reactive::spawn_blocking`:

| Call | Blocks until | Deadline (Android / Apple) |
|---|---|---|
| `Camera::request_permission` | the permission machinery resolves (a system dialog on Android; AVFoundation's `requestAccessForMediaType:completionHandler:` on iOS) | 120 s / 120 s |
| `CameraSession::take_picture` | the photo has been written to disk, or the capture failed | 15 s / 10 s |
| `CameraSession::close` (dropping a `CameraSession` without calling it first runs the identical hop) | the capture device is released — Apple parks on the session's serial queue, which may still be working through `open()`'s in-flight `startRunning()` or a just-issued stream-start attach; Android's JNI call only waits for the teardown to be *scheduled*, not finished | doesn't block (Android) / none — no timeout, by design (Apple) |

Pair each with `frust-reactive`'s `spawn_blocking` (an app-tier concern — the
plugin itself stays framework-free per the platform-plugin charter):

```rust
let status = frust_reactive::spawn_blocking(Camera::request_permission).await??;

// `take_picture` takes `&self`, so hold the session behind a shared handle
// (e.g. `Arc<CameraSession>`) and move a clone of that into the closure:
let session = Arc::clone(&session);
let path = photo_path.clone();
frust_reactive::spawn_blocking(move || session.take_picture(&path)).await??;
```

**Never call `request_permission` or `take_picture` on the UI thread.** On
Android every completion is relayed through the main `Looper`, so a UI-thread
caller parks on the very queue carrying its own wake-up; on Apple the
completion arrives on an arbitrary queue, but the permission alert still
needs a free main thread to be shown. Either way both backends refuse such a
call immediately with `CameraError::UiThread` instead of freezing until the
deadline above — a diagnosable error, not a silent hang.

The guard covers every path that can block. Paths that answer without waiting
are exempt: on Apple, `request_permission` with an already-decided
authorization status returns immediately and is callable from any thread.

**`CameraSession::close` carries no such guard.** Unlike the two calls above,
calling it (or dropping a `CameraSession` without calling it — the same
queue hop runs either way) on the UI thread is never refused with
`CameraError::UiThread`; it genuinely blocks the calling thread, by design —
the release is exactly what a caller reopening right after (a lens switch)
depends on. Pair it with `frust_reactive::spawn_blocking` anyway, especially
whenever a stream start may still be mid-flight on the session queue.

**The torch is not in the table above.** `set_torch`/`torch_available` (§5)
never wait on a platform answer, and never hop onto a queue synchronously
either — Android hands CameraX a fire-and-forget `enableTorch`, and Apple
fires the request onto the session's own serial queue **asynchronously**,
returning before the queue body ever runs, answering `torch_available` from a
cached value the queue keeps current instead of asking the device live. Both
are callable from **any** thread, including the UI thread, unconditionally,
and neither needs `spawn_blocking`.

**Neither are the stream start/stop calls.** `start_image_stream`/
`start_barcode_stream` and `stop_image_stream`/`stop_barcode_stream` are
callable from **any** thread too, sharing `set_torch`'s treatment for the
same reason: Android answers a start synchronously (its CameraX bind is
itself fire-and-forget), and Apple hands the attach to the session's own
serial queue **asynchronously**. That is what keeps a start issued moments
after `open()` — the scan-sheet shape, where one tap opens the session and
starts a stream — from parking behind `open()`'s own in-flight
`startRunning()`, which would otherwise freeze the UI thread for however
long that takes (hundreds of milliseconds on a real device).

So `Ok(())` from a start means the stream was **accepted**, not that frames
are flowing yet. The one failure iOS can only discover once its queue runs
the attach (the session refusing the video data output) is reported through
`CameraSession::take_stream_error()` — except when the session is closed
before the queue ever gets to the attach: that's a cancellation, not a
failure, and reports nothing here at all.

```rust
session.start_barcode_stream(BarcodeStreamOptions::default(), on_detect)?;

// later — e.g. from a rebuild, if no detection ever arrives
if let Some(err) = session.take_stream_error() {
    // the stream never attached; its claim is already released, so this
    // can just start again
}
```

Reading takes the value (a `CameraError` is not `Clone`), so keep what it
returns in your own state rather than polling for it twice. The failed start
releases its own stream claim as it reports, so a retry is never refused
with `CameraError::StreamBusy`. `None` is the normal answer — and always the
answer on Android, whose start failures are all synchronous. It is not a
health check either: an accepted, attached stream that has yet to deliver a
frame reports nothing here.

A stop retires the running callback on the calling thread, so no frame
reaches your callback after `stop_*` returns, even though the platform-side
detach runs afterwards.

`take_picture` correlates each attempt with its own completion, so `Ok(())`
means *that* call's photo was written to the path you passed.

### Close-deadline contract (image stream)

`CameraSession::start_image_stream`'s frame callback runs on a plugin-owned
thread (never the UI thread — see [`ImageFrameCallback`]'s own doc) and hands
you an [`ImageFrame`] whose [`ImagePlane`]s are **borrowed, zero-copy views**
onto the platform's own in-flight buffer, valid only until the callback
**returns**. Copy or fully consume the data before returning — Android's
`ImageProxy.close()` runs only after the JNI call back into Kotlin completes,
and with `STRATEGY_KEEP_ONLY_LATEST` a deferred `close()` stalls every
subsequent frame (only one may be in flight). Never store an `ImageFrame`/
`ImagePlane` past the callback's return — see the type's own rustdoc for the
full contract.

[`ImageFrameCallback`]: https://docs.rs/frust-camera (or your local `cargo doc -p frust-camera`)
[`ImageFrame`]: https://docs.rs/frust-camera
[`ImagePlane`]: https://docs.rs/frust-camera

---

## 4. Barcode stream

`frust_camera::barcode` decodes QR codes from any luma buffer or
[`ImageFrame`] standalone (no camera involved — see its own module doc).
`CameraSession::start_barcode_stream` composes that decoder directly with
the live camera feed:

```rust
use frust_camera::barcode::Barcode;
use frust_camera::{BarcodeStreamOptions, DetectionPolicy};

session.start_barcode_stream(
    BarcodeStreamOptions {
        formats: Vec::new(), // empty = all supported (v1: QR only)
        detection: DetectionPolicy::NoDuplicates,
    },
    move |found: &[Barcode]| {
        // never called with an empty slice
        for barcode in found {
            println!("{:?}", barcode.raw_value);
        }
    },
)?;

// later
session.stop_barcode_stream();
```

Both calls are non-blocking and *accepted, not confirmed*, exactly like the
raw stream — see §3 for that contract and `take_stream_error()`.

### Same thread + close-deadline LAW as raw frames

`on_detect` fires on the same plugin-owned thread `start_image_stream`'s
callback runs on (never the UI thread) — the decode happens *inside* that
callback, so the same rule applies: **never write a signal from inside
it.** Hand detections off the same way `start_image_stream` consumers
already have to — write into a plain `Arc<Mutex<..>>`/atomic cell from
`on_detect`, and read it from the UI thread on the next rebuild (a timer or
another signal write wakes one), or route the value through
`frust_reactive::use_task`/`spawn_blocking` if it needs async follow-up work:

```rust
let last_detection: Arc<Mutex<Option<Barcode>>> = Arc::new(Mutex::new(None));
let cell = Arc::clone(&last_detection);
session.start_barcode_stream(BarcodeStreamOptions::default(), move |found| {
    *cell.lock().unwrap() = found.first().cloned();
})?;
// `Component::build` reads `last_detection.lock().unwrap()` on its own next
// rebuild — never inside `on_detect` itself.
```

### Occupancy: one session, one image stream

`start_image_stream` and `start_barcode_stream` share the session's single
underlying image stream — at most one can be active at a time:

| Already running | New call | Result |
|---|---|---|
| (none) | `start_image_stream` | starts |
| (none) | `start_barcode_stream` | starts |
| `start_image_stream` | `start_image_stream` (again) | **allowed** — restarts at the newly requested format (existing behavior, unchanged) |
| `start_image_stream` | `start_barcode_stream` | `CameraError::StreamBusy` — call `stop_image_stream()` first |
| `start_barcode_stream` | `start_image_stream` | `CameraError::StreamBusy` — call `stop_barcode_stream()` first |
| `start_barcode_stream` | `start_barcode_stream` (again) | `CameraError::StreamBusy` — **no** silent re-bind (keeps the running `DetectionPolicy`'s state unambiguous); call `stop_barcode_stream()` first, then start again |

`stop_image_stream`/`stop_barcode_stream` each release only their own claim
— stopping the wrong kind is a documented no-op, and `close()` releases
whichever is held.

### Detection policies

`BarcodeStreamOptions::detection` decides when `on_detect` actually fires,
relative to the underlying decode rate (default: `Throttled` at 250ms,
matching the `mobile_scanner` package's own default):

- **`NoDuplicates`** — emit each distinct value once; a value re-arms (may
  emit again) only after it has been absent for 30 consecutive processed
  frames (`mobile_scanner` parity is "until it leaves view" — this crate
  approximates that with a frame count).
- **`Throttled { interval }`** — decode, and therefore ever emit, at most
  once per `interval`; frames inside the window skip the decoder entirely
  (the cheap path). Emits are **not** deduplicated.
- **`Unrestricted`** — every non-empty decode emits, no throttling or
  deduplication.

`formats: &[]` means "all supported" (empty = all, the same convention
`decode_luma`/`decode_frame` use) — the v1 engine decodes QR only.

---

## 5. Torch

`CameraSession::set_torch` drives the flash unit in continuous ("torch")
mode; `torch_available` reports whether the active lens has one to drive:

```rust
if session.torch_available() {
    session.set_torch(true)?;   // on
    // …
    session.set_torch(false)?;  // off
}
```

Both are **non-blocking and callable from any thread, unconditionally**,
including the UI thread (§3) — no `spawn_blocking` wrapper, unlike
`request_permission`/`take_picture`. On Apple `set_torch` fires onto the
session's own queue **asynchronously** and returns before that queue body
ever runs, so no queue depth or device state can delay the calling thread —
not even briefly, and not even right after `open()` while `startRunning()`
is still coming up on the same queue.

Torch is **session-level** state, not stream state: it survives
`start_image_stream`/`start_barcode_stream` and their stops (a stream never
touches it), and it goes out with `close()`, which releases the camera device
itself.

**Availability is a real answer, not a formality** — check it before offering
a torch control:

| Situation | `torch_available()` | `set_torch(..)` |
|---|---|---|
| Back lens with a flash unit | `true` | `Ok(())` |
| Front lens (most devices), or any device with no flash unit — **Android** | `false` | `CameraError::Platform` |
| Front lens (most devices), or any device with no flash unit — **Apple** | `false` | `Ok(())` — accepted onto the session queue, never applied; `torch_available()` stays `false` |
| iOS, device cooling off (torch temporarily withdrawn) | `false` | `Ok(())` — accepted onto the session queue, never applied; `torch_available()` stays `false` |
| Android, before CameraX finishes binding the camera | `false` | `CameraError::Platform` — **retryable**, try again once the preview is live |
| Apple, before `startRunning` completes (cache still holds the pre-start seed) | may read `false` | `Ok(())` — accepted; **re-check availability** after the pipeline is live (a later rebuild), same guidance as the Android bind row |
| After `close()` | `false` | `CameraError::SessionClosed` |
| Desktop/wasm (no camera backend) | `false` | `CameraError::PlatformNotInitialized` |

`set_torch` never panics. On Android it never silently no-ops either: a lens
with no torch is reported as a synchronous error, so a control wired to it
can surface *why* nothing happened. **On Apple `set_torch` always returns
`Ok(())` once past `SessionClosed`** — the request is accepted onto the
session queue, not confirmed by AVFoundation, so a refusal (no controllable
torch, the device mid cool-off, a configuration-lock conflict) surfaces only
through `torch_available()`, never through this call's `Result`; check it
before offering the control, and again afterward if the UI needs to know
whether the LED actually changed. On-device behavior (real illumination,
front-lens refusal, the overheating path) is verified by the camera device
gate, not by any host-side test — `docs/PLUGINS_DEVELOPMENT.md`'s *Camera
manual test*.

---

## 6. Caveats

- **`frust create --overwrite` destroys these additions.** `--overwrite`
  re-renders the generated project wholesale, silently dropping the Gradle
  include, the Swift package reference and the plist key. All additions are
  **idempotent** — just re-run the Add Plugin dialog (or re-apply this
  document) to restore them.
- **Physical device recommended for preview/capture.** The Android emulator's
  virtual camera and the iOS Simulator's camera are both limited stand-ins;
  a physical device is the only way to verify real preview/capture/stream
  behavior end to end (`docs/DEVELOPMENT.md`'s device-gate conventions).
- **Permission timing.** `NeedsUi`/`Pending` (see `PermissionStatus`) reflect
  that Android's permission dialog needs a live `Activity` — request
  permission once a `platform_view` preview slot exists (and hence an
  Activity is cached), not necessarily before `Camera::open`.
- **`ImageFormat::Bgra` is Apple-only.** CameraX's one packed-32-bit
  `ImageAnalysis` output is `RGBA_8888` (byte order `R,G,B,A`), so
  `start_image_stream` reports `CameraError::Platform` for `Bgra` on Android
  rather than handing over mislabelled bytes — Flutter's `camera` plugin
  draws the same line. Ask for `ImageFormat::Yuv420` in cross-platform code.
- **A stream start is accepted, not confirmed, on iOS.** Its attach runs on
  the session queue after the call returns (§3, the same shape `set_torch`
  has), so `Ok(())` is an acceptance and a rejected attach surfaces through
  `take_stream_error()` instead. Android reports every start failure
  synchronously, so this is a one-platform asymmetry a cross-platform app
  handles by checking both.
- **An image-stream callback has a hard deadline.** It runs on a
  plugin-owned thread with the platform's own buffer borrowed, and must copy
  or consume before returning: a late return stalls the Android stream
  outright (`STRATEGY_KEEP_ONLY_LATEST` holds one image in flight) and drops
  frames on iOS. Never write a signal from inside it — hand work off with
  `frust_reactive::use_task` (`docs/CODE_STANDARDS.md`'s heavy-work routing).
- **No torch on most front lenses, and none at all on a device without a
  flash unit.** `torch_available()` (§5) is the check — treat it as a UI
  gate, not a formality, and expect `false` from `Lens::Front` on nearly
  every device. On Android it also reads `false` until CameraX has finished
  binding the camera, so a torch control shown before the preview goes live
  should re-check rather than latch its first answer.
- **`set_torch` is accepted, not confirmed.** Android hands CameraX the
  request without waiting on its `ListenableFuture` (that is what keeps the
  call non-blocking), so `Ok(())` means "the camera control took it", not
  "the LED is lit" — the same shape a torch toggle in any CameraX app has.
  iOS fires the request onto the session's own queue **asynchronously** and
  returns before that body runs — its effect is only ever observable via
  `torch_available()`/the LED, never confirmed by the call's `Result` (see
  §5). On BOTH platforms, re-check availability rather than latching its
  first answer: Android's answer flips once CameraX binds; Apple's cached
  answer is refreshed at three points only (configure, after `startRunning`,
  after each `set_torch`), so between refreshes it can lag reality in either
  direction.
- **v1 is on/off only.** No torch *level* (`setTorchModeOnWithLevel:` on iOS
  has no CameraX equivalent), no `Auto` mode, and no zoom/focus control —
  those stay Future Enhancements rather than a half-symmetric API.

---

## 7. The TUI Add Plugin dialog automates all of this

Everything in §1 and §2 — the Cargo.toml dependency, the `:frust-camera`
Gradle include plus its app-module dependency, the `FrustCamera` Swift
package reference, and the `NSCameraUsageDescription` plist key — is applied
for you, idempotently, by the frust TUI's **Add Plugin** dialog (`frust tui` →
Add Plugin → `camera`, no optional features in v1). This README is the manual
contract that dialog encodes.
