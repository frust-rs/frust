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

## Browse the library

With the `library` cargo feature (`frust-photo-library = { version = "0.6", features = ["library"] }`, or the TUI's
Add Plugin `library` option) the crate also lists the user's photos: `PhotoLibrary::albums()`,
`PhotoLibrary::assets(&AssetQuery)` (newest first, pages of at most 500) and
`PhotoLibrary::thumbnail(&AssetId, &ThumbnailRequest)`, which returns straight-alpha RGBA8 ready for
`ImageSource::from_rgba8`. The feature adds the Android media read permissions,
`NSPhotoLibraryUsageDescription` and the `Photos` framework; request access with `frust-permissions`
(the `photos` feature), browsing never prompts. Without the feature every call returns
`PhotoError::FeatureDisabled`; desktop returns `Unsupported`.

- Calls block and refuse the mobile UI thread (`PhotoError::UiThread`); run them in `spawn_blocking`.
  Browsing takes no pick guard. `manage_limited_selection()` shows the system UI that changes the
  selection while access is Limited, and returns `Busy` while a pick runs.
- `Fill` returns exactly the requested box, centre-cropped; `Fit` stays inside it. iOS `Fit` never
  upscales, Android `Fit` can upscale a small source. An iOS thumbnail waits at most 30 s, then returns
  `PhotoError::Platform`.
- Thumbnails are cached in memory (48 MiB; `set_thumbnail_cache_budget`, `clear_thumbnail_cache`). A hit
  skips the access check. The cache clears when `manage_limited_selection` succeeds and when access is
  denied; after any other selection change, or an edited or deleted asset, call `clear_thumbnail_cache`
  (`photo-library-thumbnail-cache-staleness` in [LIMITATIONS.md](../../docs/LIMITATIONS.md)).
- With `library` on iOS, picks carry `PickedItem::asset_id`, and a HEIC pick can be shown through
  `thumbnail`.
- iOS shows its own prompt while access is Limited; to silence it add the boolean key
  `PHPhotoLibraryPreventAutomaticLimitedAccessAlert` to `Info.plist` by hand
  (`photo-library-limited-alert-plist-manual`). Android pages cost a count and a sort per page
  (`photo-library-android-paging-cost`).

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
| iPhone 13 mini, iOS 27.2 | passed (pick, loading progress, cancel, swipe-down, share picked, clear cache; browsing: albums, paging, thumbnails, Limited and Manage selection) |
| Pixel 5, Android 14 | passed, browsing included; Xiaomi 12 (Android 17) passed the picker and share legs on an earlier build |
| macOS | passed through the file dialog |
| Android 10/11 fallback picker, Linux, Windows | not device-tested (the API 29/30 leg is owed; so are API 29 paging and a second API 31+ device for browsing) |
