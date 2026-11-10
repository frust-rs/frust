# frust-permissions

One runtime-permission API for frust apps: `status(Permission)`,
`request(Permission)` and `open_settings()` over Android runtime permissions
and Apple authorization. Covers photos (read/write or add-only), camera,
microphone and media location.

Like every frust **platform plugin**, this crate is added to your app's own
`Cargo.toml` alongside `frust` — the `frust` facade does not re-export it.

More about Frust: <https://frust.dev> and <https://github.com/frust-rs/frust>.

## Platform support

| Platform | Backend |
|---|---|
| Android | runtime permissions through the plugin's host module |
| iOS, macOS | `AVCaptureDevice` and `PHPhotoLibrary` authorization |
| Linux, Windows, web | none: every call returns `PermissionError::Unsupported` |

## Add the dependency

Every permission is an opt-in cargo feature (`photos`, `camera`, `microphone`,
`media-location`); none is on by default, so you only ship and declare what
you use.

```toml
# app Cargo.toml — [dependencies]
frust-permissions = { version = "0.6", features = ["camera"] }
```

## Use

```rust
use frust::{
    Component, Get, RwSignal, Set, View, button, column, spawn_blocking, spawn_local, text,
};
use frust_permissions::{Permission, open_settings, request, status};

#[derive(Default)]
struct CameraGate;

struct GateState {
    label: RwSignal<String>,
}

impl Component for CameraGate {
    type State = GateState;

    fn init(&self) -> GateState {
        GateState {
            label: RwSignal::new(format!("{:?}", status(Permission::Camera))),
        }
    }

    fn build(&self, state: &mut GateState) -> impl View<GateState> {
        column()
            .child(text(state.label.get()))
            .child(Self::allow_button())
            .child(Self::settings_button())
    }
}

impl CameraGate {
    fn allow_button() -> impl View<GateState> {
        button("Allow camera", |state: &mut GateState| {
            let label = state.label;
            spawn_local(async move {
                // `request` blocks until the OS answers: run it off the UI thread.
                let answer = spawn_blocking(|| request(Permission::Camera)).await;
                label.set(match answer {
                    Ok(Ok(status)) => format!("{status:?}"),
                    Ok(Err(err)) => format!("error: {err}"),
                    Err(err) => format!("error: {err}"),
                });
            });
        })
    }

    fn settings_button() -> impl View<GateState> {
        button("Settings", |_state: &mut GateState| {
            let _ = open_settings();
        })
    }
}

frust::app!(CameraGate);
```

Writes to signals happen in event handlers or task completions, never in
`build`. A status of `Blocked` means the OS will not prompt again: offer
`open_settings()`.

## Threading

`status` never blocks and runs on any thread. `request` blocks until the OS
answers and returns `PermissionError::UiThread` on the UI thread (use
`frust::spawn_blocking`); an already-decided permission returns immediately and
is exempt. `open_settings` is fire-and-forget.

## Permissions

- **Android:** the manifest permissions for each enabled feature
  (`CAMERA`, `RECORD_AUDIO`, `READ_MEDIA_*` / `READ_EXTERNAL_STORAGE`,
  `ACCESS_MEDIA_LOCATION`) must be declared by your app.
- **iOS / macOS:** the matching Info.plist usage strings
  (`NSCameraUsageDescription`, `NSMicrophoneUsageDescription`,
  `NSPhotoLibraryUsageDescription`, `NSPhotoLibraryAddUsageDescription`).
  A missing string is reported as `MissingUsageDescription`, not a crash.

## Limitations

- `MediaLocation` covers reading the location already stored inside a photo.
  It never reads the device position.
- On Linux, Windows and web every call returns `Unsupported`.
- `MediaLocation` is granted by Android only alongside full photo access: with
  partial or no access the status is `Denied`, never `Blocked`. On iOS it
  mirrors Photos read access (no prompt of its own, `NotDetermined` until
  Photos is asked); macOS `Unsupported`.
- Android keeps its own "asked" flag. Where another requester (such as
  `frust-camera`) did not write it, a permission showing a rationale reads
  `Denied`, and a denial the system answers within 500 ms without a dialog
  reads `Blocked` after one request. A person dismissing the dialog faster
  than 500 ms also reads `Blocked`.
- `Denied` versus `Blocked` on Android is authoritative only while an Activity
  is resumed; without one a blocked permission reads `Denied`.
- Android runs one request at a time; a request made while an earlier prompt
  is still showing (for example after the 120 s timeout) returns the current
  status without a second dialog.
- `open_settings` returns `Ok` when the open was dispatched, not when Settings
  opened; failures are logged at warn.
- macOS answers `Unsupported` for photos.
- The app manifest may omit `READ_MEDIA_VISUAL_USER_SELECTED`; without it
  Android runs the app in compatibility mode and a partial selection reads
  `Granted` for that session.

## Tested platforms

| Platform | Result |
|---|---|
| iPhone 13 mini, iOS 27 | passed (photos limited and add-only, camera, microphone, media location) |
| Pixel 5, Android 14 | passed; the API 30 (`READ_EXTERNAL_STORAGE`) path is not yet run |
| macOS 27, Apple M4 | passed unbundled: camera and microphone status, missing usage string reported, photos `Unsupported` |
| Linux, Windows, web | every call returns `Unsupported` (host tests) |

Accepted limitations are recorded in the repository's `docs/LIMITATIONS.md`
(`permissions-android-asked-flag-local`, `permissions-request-lost-on-process-death`,
`permissions-photos-macos-unsupported`, `permissions-read-external-storage-no-maxsdk`,
`permissions-media-location-android-only`, `permissions-android-status-needs-activity`).

## License

Licensed under either of MIT or Apache-2.0 (SPDX: `MIT OR Apache-2.0`), at your
option. See `LICENSE-MIT` and `LICENSE-APACHE` beside this README.
