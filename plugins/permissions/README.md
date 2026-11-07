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
use frust::prelude::*;
use frust_permissions::{open_settings, request, status, Permission, PermissionStatus};

struct CameraGate;

impl Component for CameraGate {
    fn build(&self, cx: &mut Cx) -> impl View {
        let state = use_signal(cx, || status(Permission::Camera).ok());
        // `request` blocks until the OS answers: run it off the UI thread.
        let ask = use_task(cx, move || request(Permission::Camera));

        column![
            text(format!("{:?}", state.get())),
            button("Allow camera").on_tap(move |_| ask.run()),
            button("Settings").on_tap(|_| {
                let _ = open_settings();
            }),
        ]
    }
}
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
- Platform arms are filled in incrementally; until then they return
  `Unsupported`.

## License

Licensed under either of MIT or Apache-2.0 (SPDX: `MIT OR Apache-2.0`), at your
option. See `LICENSE-MIT` and `LICENSE-APACHE` beside this README.
