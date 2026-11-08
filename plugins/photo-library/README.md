# frust-photo-library

Pick photos and videos in frust apps with `PhotoLibrary::pick(&PickOptions)`: the system photo
picker on Android and iOS (no permission and no `Info.plist` string), a native file dialog on Linux,
macOS and Windows.

Like every frust **platform plugin**, this crate is added to your app's own `Cargo.toml`
alongside `frust` — the `frust` facade does not re-export it.

More about Frust: <https://frust.dev> and <https://github.com/frust-rs/frust>.

## Platform support

| Platform | Backend |
|---|---|
| Android | system photo picker, picks copied into the app cache |
| iOS | `PHPickerViewController`, picks copied into the app cache |
| Linux, macOS, Windows | native file dialog (`frust-file-dialogs`), original files returned |
| web | none: `PhotoError::Unsupported` |

## Add the dependency

```toml
# app Cargo.toml — [dependencies]
frust-photo-library = "0.6"
```

On Android the plugin ships its own Gradle module; the frust TUI's **Add Plugin** dialog
(`photo-library`) adds it and links `PhotosUI` on iOS.

## Use

```rust
use frust::prelude::*;
use frust_photo_library::{MediaFilter, PhotoLibrary, PickOptions};

struct Gallery;

impl Component for Gallery {
    fn build(&self, cx: &mut Cx) -> impl View {
        let pick = use_task(cx, || {
            // `pick` blocks until the user chooses: it runs off the UI thread here.
            PhotoLibrary::pick(&PickOptions::new().filter(MediaFilter::Images).limit(1))
        });

        column![
            button("Choose photo").on_tap(move |_| pick.run()),
            // Decode a picked `PickedItem.path` with `frust::decode_image_async`.
        ]
    }
}
```

## Loading, cancel and copies

- The picker closing and the items loading are separate phases (an iCloud original can take as long as
  its download). `.on_progress(|p: PickProgress| ..)` reports `items`, `done` and `fraction`: its first call
  means the picker closed, calls are throttled to 20 per second, `fraction` never goes down and the last
  call is 1.0. Callbacks run off the UI thread.
- `.cancel(token)` with a `CancelToken` makes `pick` return `PhotoError::Cancelled` and removes that
  call's partial copies. A swipe-down or the picker's own Cancel returns an empty `Vec` instead. On Android
  a token cancelled while the system picker is still open returns at once, but the picker stays until the
  user closes it (`photo-library-android-cancel-cannot-dismiss-picker` in
  [LIMITATIONS.md](../../docs/LIMITATIONS.md)).
- One pick runs at a time: a second call returns `PhotoError::Busy`, and so does `clear_pick_cache`
  during a pick. On mobile `pick` refuses the UI thread (`PhotoError::UiThread`).
- On mobile each pick is a copy (`PickedItem::is_copy`) that grows the cache until
  `PhotoLibrary::clear_pick_cache()`; paths from earlier picks are invalid after a clear. Picked bytes are
  untranscoded (an iPhone HEIC stays HEIC, which `decode_image_async` cannot read).
- `PickedItem::file_name` is untrusted display text; never build a path from it.

## Tested platforms

| Platform | Result |
|---|---|
| iPhone 13 mini, iOS 27.2 | passed (pick, loading progress, cancel, swipe-down, share picked, clear cache) |
| Pixel 5, Android 14 | passed; Xiaomi 12 (Android 17) passed the picker and share legs on an earlier build |
| macOS | passed through the file dialog |
| Android 10/11 fallback picker, Linux, Windows | not device-tested (the API 29/30 leg is owed) |
