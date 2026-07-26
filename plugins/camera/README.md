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

**Two** calls block until the platform answers, exactly like
`frust-secure-storage`'s gated calls:

| Call | Blocks until | Deadline (Android / Apple) |
|---|---|---|
| `Camera::request_permission` | the permission machinery resolves (a system dialog on Android; AVFoundation's `requestAccessForMediaType:completionHandler:` on iOS) | 120 s / 120 s |
| `CameraSession::take_picture` | the photo has been written to disk, or the capture failed | 15 s / 10 s |

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

**Never call either on the UI thread.** Every completion is relayed through the
platform's main thread, so a UI-thread caller would park on the very queue
carrying its own wake-up. Both backends refuse such a call immediately with
`CameraError::UiThread` instead of freezing until the deadline above — a
diagnosable error, not a silent hang.

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

## 4. Caveats

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
- **An image-stream callback has a hard deadline.** It runs on a
  plugin-owned thread with the platform's own buffer borrowed, and must copy
  or consume before returning: a late return stalls the Android stream
  outright (`STRATEGY_KEEP_ONLY_LATEST` holds one image in flight) and drops
  frames on iOS. Never write a signal from inside it — hand work off with
  `frust_reactive::use_task` (`docs/CODE_STANDARDS.md`'s heavy-work routing).

---

## 5. The TUI Add Plugin dialog automates all of this

Everything in §1 and §2 — the Cargo.toml dependency, the `:frust-camera`
Gradle include plus its app-module dependency, the `FrustCamera` Swift
package reference, and the `NSCameraUsageDescription` plist key — is applied
for you, idempotently, by the frust TUI's **Add Plugin** dialog (`frust tui` →
Add Plugin → `camera`, no optional features in v1). This README is the manual
contract that dialog encodes.
