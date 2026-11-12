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

`PhotoLibrary::scan_folder(&FolderGrant, &FolderScan)` pages through the images and videos of a folder the user
granted (`frust_photo_library::folder::FolderGrant`, from `frust-file-dialogs`), newest first, on Android (a tree URI),
iOS (a bookmarked folder) and desktop. It is not available on web.

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
- Every `Asset` carries `modified_unix_ms`, `is_favorite` and `burst_id`. `is_favorite` is `None` below
  Android API 30 and `burst_id` is always `None` on Android (`photo-library-android-asset-fields`).
  `AssetQuery::with_details()` also fills `file_name` (untrusted, display only) and `content_type`, which is
  platform-shaped: a MIME type on Android, a uniform type identifier on iOS
  (`photo-library-content-type-platform-shaped`). `Asset::mime_type()` gives a MIME type on both phones: it maps the
  iOS identifiers in a fixed table and returns `None` for an identifier it does not know rather than guessing. They are off by default because on iOS each row then costs
  one resource lookup (Android reads them in the same query). An asset has no cloud or locally-available
  flag; an iCloud-only original shows as `NetworkRequired` when you ask for it
  (`photo-library-no-local-availability`).
- `PhotoLibrary::thumbnail_shared(&AssetId, &ThumbnailRequest, &CancelToken)` returns an `Arc<Thumbnail>`:
  the cache never copies pixels and a hit shares the cached allocation, but building an `ImageSource` from the pixels copies them once; cancelling the token abandons
  a miss on iOS and Android (`PhotoError::Cancelled`, nothing cached). `thumbnail` shares the same cache.
  A token is single-use, so give each cell's load its own. Build one `ImageSource` per asset and keep it with
  the cell's state: each `ImageSource` made from pixels is a new engine image.
- `PhotoLibrary::asset_metadata(&AssetId, &MetadataOptions)` reads one asset's `ExifData` with only the
  `library` feature (it links `ImageIO` on iOS, which the TUI's `library` option adds). An asset without EXIF,
  a video included, gives an empty `ExifData`. On iOS Photos has no header-only read, so the call reads the
  whole local original into one in-memory buffer that ImageIO reads in place (`photo-library-ios-metadata-reads-original`); a read with no data for 30 s times out, an original over 256 MiB is `InvalidRequest`, and an iCloud-only original
  is `NetworkRequired` unless `MetadataOptions::allow_network(true)`. On Android GPS fields need
  `ACCESS_MEDIA_LOCATION`. `MetadataOptions::cancel(token)` stops the read.
- iOS shows its own prompt while access is Limited; to silence it add the boolean key
  `PHPhotoLibraryPreventAutomaticLimitedAccessAlert` to `Info.plist` by hand
  (`photo-library-limited-alert-plist-manual`). Android pages cost a count and a sort per page
  (`photo-library-android-paging-cost`).

## Follow library changes and page by cursor

With `library`, an app can keep its own copy of the library up to date instead of re-listing it:

1. First run: store the bytes of `PhotoLibrary::current_change_token()` (`ChangeToken::as_bytes`).
2. Later, restore it with `ChangeToken::from_bytes` and call `PhotoLibrary::changes_since(&token)`.
3. A `LibraryChanges::Delta { inserted, updated, deleted, next }` lists asset ids to apply; a
   `LibraryChanges::FullRescan { next }` means re-read the library. Either way store `next`.

```rust
use frust_photo_library::{ChangeToken, LibraryChanges, PhotoError, PhotoLibrary};

// Blocking: call off the UI thread. `saved` holds the bytes stored last time, if any.
fn sync(saved: Option<Vec<u8>>) -> Result<Vec<u8>, PhotoError> {
    let Some(bytes) = saved else {
        return Ok(PhotoLibrary::current_change_token()?.as_bytes().to_vec());
    };
    let next = match PhotoLibrary::changes_since(&ChangeToken::from_bytes(bytes))? {
        LibraryChanges::Delta { inserted, updated, deleted, next } => {
            // Apply the three id lists to the app's copy.
            let _ = (inserted, updated, deleted);
            next
        }
        LibraryChanges::FullRescan { next } => {
            // Re-list with `assets` or `assets_after`.
            next
        }
        _ => PhotoLibrary::current_change_token()?,
    };
    Ok(next.as_bytes().to_vec())
}
```

- `PhotoLibrary::on_library_changed(|| ..)` returns a `ChangeSubscription`; dropping it unregisters the
  callback. The callback only wakes the app: it carries no data and may be coalesced or spurious. It
  runs inline on a platform notification thread (the Photos change queue on iOS, a binder thread on
  Android), so it must only signal (set a flag, send on a channel, wake a task) and return; calling
  `changes_since` or anything blocking from inside it delays other subscribers and the platform's
  delivery. Answer the signal by calling `changes_since` off the UI thread.
- Call `changes_since` once right after subscribing: a change that lands while subscribing may not
  produce a wake.
- A `Delta` evicts every id it names (inserted, updated and deleted) from the thumbnail cache, and a `FullRescan` clears it.
  An id in a delta may already be gone by the time you use it (`PhotoError::NotFound`).
- iOS keeps a change history from iOS 16; iOS 15 always answers `FullRescan`. Android needs API 30; on
  API 29 every call is a `FullRescan`. A token from another device or platform, an expired token (iOS),
  a changed `MediaStore` volume version (Android) or bytes that do not decode are also a `FullRescan`,
  never an error. On Android a visible row missing from the token's id set counts as `inserted`. The
  Android token is about 100 KB per 100,000 assets (about 200 KB when ids are very sparse), so store it
  in a file or database (`photo-library-change-feed-full-rescan`, `photo-library-android-change-token-size`).
  On Android the token also holds the list of the user's visible media ids plus the volume version, so
  keep it in app-private storage and preferably exclude it from cloud backup.
- `PhotoLibrary::assets_after(&AssetQuery, Option<&AssetCursor>)` returns an `AssetCursorPage` (`items`
  and `next`, `None` on the last page), newest first and stable while the library changes between pages.
  Start with `None`; the `AssetQuery` offset is ignored and no total is computed. `limit` and `details`
  may change between pages, the album and filter may not, and a cursor is in memory only (restart from
  `None` after a relaunch). Android pages by keyset with no count and no seek; iOS pins one snapshot per
  session, so changes appear only in a new session, four sessions are kept and an evicted session's
  cursor answers `InvalidRequest` (`photo-library-ios-cursor-pins-snapshot`). `assets` keeps offset paging.
- `PhotoLibrary::cloud_identifiers(&[AssetId])` maps a whole batch to iCloud identifiers in one call
  (`None` where an asset has none): iOS 15+ only, `Unsupported` elsewhere, and expensive and blocking, so
  map every id at once and off the UI thread. A cloud identifier names the asset on every device of the
  same iCloud Photos library.
- An `AssetId` stays the same across launches on one device on a best-effort basis, so an app may store
  it. It is not portable to another device or platform, and a restore from backup or a reinstall may
  change it; use a cloud identifier to name an asset across devices.
- The core paths ran on a Pixel 5 and an iPhone 13 mini; iOS updated and deleted deltas, `cloud_identifiers`,
  iOS 15, API 29 and large Android libraries have not run on a device (`photo-library-change-feed-not-device-tested`).

### Apply a delta with `assets_by_id`

`PhotoLibrary::assets_by_id(&[AssetId], details)` returns the `Asset` rows for given ids, so a delta
needs no re-listing. The answer has one entry per input id, in input order: `None` where the id does
not exist, was deleted, is outside a limited selection or is not a library id, and a duplicate id gives
a duplicate entry. `details: true` fills `file_name` and `content_type` as `AssetQuery::with_details`
does. An empty slice returns an empty `Vec` without a platform call; otherwise the call blocks, refuses
the UI thread and needs the same access as `assets`. Android takes a whole delta in one call (it
queries at most 500 ids at a time on its own); iOS, desktop and web answer `Unsupported` for now.

```rust
use frust_photo_library::{Asset, AssetId, PhotoError, PhotoLibrary};
use std::collections::HashMap;

// Blocking: call off the UI thread, with the id lists of a `LibraryChanges::Delta`.
fn apply(
    catalogue: &mut HashMap<AssetId, Asset>,
    inserted: Vec<AssetId>,
    updated: Vec<AssetId>,
    deleted: Vec<AssetId>,
) -> Result<(), PhotoError> {
    for id in &deleted {
        catalogue.remove(id);
    }
    let ids: Vec<AssetId> = inserted.into_iter().chain(updated).collect();
    for (id, asset) in ids.iter().zip(PhotoLibrary::assets_by_id(&ids, false)?) {
        match asset {
            Some(asset) => {
                catalogue.insert(id.clone(), asset);
            }
            // Gone again since the delta, or outside a limited selection.
            None => {
                catalogue.remove(id);
            }
        }
    }
    Ok(())
}
```

## Export originals and save

`PhotoLibrary::export_original(&AssetId, &ExportOptions)` (the `library` feature) writes an asset's
untouched bytes to a file: no transcode, no metadata strip. `ExportOptions` takes `version`
(`AssetVersion::Original` or `Current`, the edited rendition), `allow_network` (when `false`, an
iCloud-only original fails with `PhotoError::NetworkRequired`), `dest_dir`, `on_progress` (a fraction,
throttled, ending at 1.0 only on success) and a `CancelToken` (a cancelled export leaves no partial
file). The default destination is `<cache_dir>/frust-photo-library/originals`: `clear_pick_cache` leaves it
alone and the plugin has no cleanup API (`photo-library-originals-no-cleanup` in
[LIMITATIONS.md](../../docs/LIMITATIONS.md)). Export and save block, refuse the mobile UI thread and
take no pick guard, so they run while a pick does.

`PhotoLibrary::save(&SaveRequest)` (the `save` cargo and registry feature) files a local image or video
into the library and returns the new asset's id when the platform reports one. A bad file name, album name
or media kind is `PhotoError::InvalidRequest` before any platform call. The registry feature repeats
`permissions`' `photos-add` usage string and `Photos` link, so the two compose; request access with
`permissions` (`photos-add`).

- **iOS:** export needs read-write access (Limited counts) and streams to a hidden `.<name>.part` file that
  takes its final name only once complete, never replacing an existing file (a taken name gets `-2`,
  `-3`). A Live Photo exports its still only (`photo-library-live-photo-still-only`). A bare save needs
  add-only or read-write access; a save into an album needs read-write access and
  `NSPhotoLibraryUsageDescription` (the `library` feature writes it), and add-only access with an album
  returns `PermissionDenied` (`photo-library-addonly-no-album`). iOS ends the app when its Photos access
  changes.
- **Android:** export reads `content://media/<external|external_primary>/<images|video>/media/<digits>`
  ids on its own executor (`photo-library-android-serial-exports`); `AssetVersion` and `allow_network`
  are ignored (`photo-library-android-no-rendition-split`). Save inserts through `MediaStore` into
  `Pictures/<album or app label>/` (`Movies/...` for video) with no permission; a stuck save has no
  cancel or deadline (`photo-library-android-save-no-deadline`).
- **Location:** a stored GPS location is kept by Android browse + export with Media location granted
  (byte-identical to the camera file) and by the iOS picker with default options. It is zeroed (latitude,
  longitude, GPS date) by the Android Photo Picker even when granted, and by export without Media location
  (`photo-library-android-location-redacted`). Browse + export is the route that keeps location.

## Decode a photo natively

With the `decode` cargo feature (`frust-photo-library = { version = "0.6", features = ["decode"] }`, or the
TUI's Add Plugin `decode` option, which also links `ImageIO` on iOS) `PhotoLibrary::decode_file(&Path,
&DecodeOptions)` decodes an image at full resolution, HEIC included, into a `DecodedImage`: straight-alpha
RGBA8 (ready for `ImageSource::from_rgba8`), its size and the source size, an `ImageColorSpace` and optional
`ExifData`. `decode_asset(&AssetId, &DecodeOptions, &ExportOptions)` (also needs `library`) exports an asset,
decodes it and deletes the exported file, even when the decode fails, is cancelled or panics.

- `DecodeOptions::new(max_dimension)` has no uncapped form: larger images are downscaled to fit, and zero is
  `PhotoError::InvalidRequest`. `.srgb()` converts to sRGB (Display P3 is kept for wide-gamut images otherwise);
  `.cancel(token)` is checked between stages, but a decode already running cannot be interrupted
  (`photo-library-decode-not-interruptible` in [LIMITATIONS.md](../../docs/LIMITATIONS.md)).
- iOS and macOS use ImageIO and Android uses `ImageDecoder`; Linux and Windows return `Unsupported`, and so does
  an Android device with no decoder for the format, notably HEIF (`photo-library-decode-unsupported-desktop`,
  `photo-library-android-heif-device-optional`). A file truncated inside its pixel data decodes with the missing
  part blank on iOS and macOS (`photo-library-apple-decode-truncated-pixels`). Without the feature both calls
  return `PhotoError::FeatureDisabled`.
- Calls block. A shared precheck runs first on every platform (a cancelled token is `PhotoError::Cancelled`, a
  zero cap `PhotoError::InvalidRequest`); the UI-thread refusal follows, so iOS and Android return
  `PhotoError::UiThread` on their UI thread, while macOS decodes on the calling thread and does not refuse the main
  one. A missing file is `PhotoError::NotFound` everywhere (a plain path or a content URI on Android).
- `max_dimension` bounds the returned image and its buffer, not the platform's own decode work, which reads the
  whole file (on Android the platform bitmap is held beside the buffer). Every buffer is also held under a byte
  ceiling of `i32::MAX` bytes (just under 2 GiB): a capped image that would need more is
  `PhotoError::InvalidRequest` before anything is allocated, and an allocation the allocator refuses is an
  error. The system can still end the app if memory runs out while a very large buffer is filled. A full-size
  decode is large (a 48 MP image is about 192 MB of RGBA8), so for an untrusted file cap `max_dimension` near
  the size you display.
- `ExifData::capture_time` is EXIF `DateTimeOriginal` only, never the last-modified TIFF `DateTime`; an ISO,
  exposure, aperture or focal length recorded as zero or negative is `None`.

```rust
use frust::{
    Component, Get, RwSignal, Set, View, button, column, spawn_blocking, spawn_local, text,
};
use frust_photo_library::{DecodeOptions, PhotoLibrary};

#[derive(Default)]
struct Viewer;

struct ViewerState {
    status: RwSignal<String>,
}

impl Component for Viewer {
    type State = ViewerState;

    fn init(&self) -> ViewerState {
        ViewerState {
            status: RwSignal::new(String::from("Nothing decoded")),
        }
    }

    fn build(&self, state: &mut ViewerState) -> impl View<ViewerState> {
        column()
            .child(text(state.status.get()))
            .child(Self::decode_button())
    }
}

impl Viewer {
    fn decode_button() -> impl View<ViewerState> {
        button("Decode", |state: &mut ViewerState| {
            let status = state.status;
            spawn_local(async move {
                // `path` is a `PickedItem.path` from `PhotoLibrary::pick`.
                let path = std::path::PathBuf::from("picked.heic");
                let options = DecodeOptions::new(2048).srgb();
                // `decode_file` blocks: run it off the UI thread.
                let result = spawn_blocking(move || PhotoLibrary::decode_file(&path, &options)).await;
                status.set(match result {
                    Ok(Ok(image)) => format!("{}x{} decoded", image.width, image.height),
                    Ok(Err(err)) => format!("error: {err}"),
                    Err(err) => format!("error: {err}"),
                });
            });
        })
    }
}

frust::app!(Viewer);
```

## Read metadata and show a shared thumbnail

```rust
use frust::{
    Component, Get, Image, ImageSource, RwSignal, Set, View, button, column, either, spawn_blocking,
    spawn_local, text,
};
use frust_photo_library::{
    AssetQuery, CancelToken, MetadataOptions, PhotoError, PhotoLibrary, ThumbnailMode, ThumbnailRequest,
};

#[derive(Default)]
struct Newest;

struct NewestState {
    info: RwSignal<String>,
    image: RwSignal<Option<ImageSource>>,
    token: Option<CancelToken>,
}

impl Component for Newest {
    type State = NewestState;

    fn init(&self) -> NewestState {
        NewestState {
            info: RwSignal::new(String::from("Nothing loaded")),
            image: RwSignal::new(None),
            token: None,
        }
    }

    fn build(&self, state: &mut NewestState) -> impl View<NewestState> {
        let image = state.image.get();
        column()
            .child(text(state.info.get()))
            .child(either(
                image.is_some(),
                move || Image(image.expect("checked above")),
                || text("No thumbnail"),
            ))
            .child(Self::load_button())
            .child(Self::cancel_button())
    }
}

impl Newest {
    /// The newest asset's name and camera, then its 256 px thumbnail. Blocks: call it off the UI thread.
    fn load(token: &CancelToken) -> Result<(String, ImageSource), PhotoError> {
        let page = PhotoLibrary::assets(&AssetQuery::new().limit(1).with_details())?;
        let asset = page.items.first().ok_or(PhotoError::NotFound)?;
        let exif = PhotoLibrary::asset_metadata(&asset.id, &MetadataOptions::new())?;
        let request = ThumbnailRequest::new(256, 256, ThumbnailMode::Fill);
        let thumb = PhotoLibrary::thumbnail_shared(&asset.id, &request, token)?;
        let label = format!(
            "{} ({})",
            asset.file_name.as_deref().unwrap_or("unnamed"),
            exif.camera_make.as_deref().unwrap_or("camera unknown"),
        );
        // One ImageSource per asset: made here, kept in state, never rebuilt in `build`.
        Ok((label, ImageSource::from_rgba8(thumb.rgba.clone(), thumb.width, thumb.height)))
    }

    fn load_button() -> impl View<NewestState> {
        button("Load newest", |state: &mut NewestState| {
            let token = CancelToken::new();
            state.token = Some(token.clone());
            let (info, image) = (state.info, state.image);
            spawn_local(async move {
                match spawn_blocking(move || Self::load(&token)).await {
                    Ok(Ok((label, source))) => {
                        info.set(label);
                        image.set(Some(source));
                    }
                    Ok(Err(err)) => info.set(format!("error: {err}")),
                    Err(err) => info.set(format!("error: {err}")),
                }
            });
        })
    }

    fn cancel_button() -> impl View<NewestState> {
        button("Cancel", |state: &mut NewestState| {
            if let Some(token) = &state.token {
                token.cancel();
            }
        })
    }
}

frust::app!(Newest);
```

## Add the dependency

```toml
# app Cargo.toml — [dependencies]
frust-photo-library = "0.6"
```

On Android the plugin ships its own Gradle module; the frust TUI's **Add Plugin** dialog
(`photo-library`) adds it and links `PhotosUI` on iOS.

## Use

```rust
use frust::{
    Component, Get, RwSignal, Set, View, button, column, spawn_blocking, spawn_local, text,
};
use frust_photo_library::{MediaFilter, PhotoLibrary, PickOptions};

#[derive(Default)]
struct Gallery;

struct GalleryState {
    picked: RwSignal<String>,
}

impl Component for Gallery {
    type State = GalleryState;

    fn init(&self) -> GalleryState {
        GalleryState {
            picked: RwSignal::new(String::from("Nothing picked")),
        }
    }

    fn build(&self, state: &mut GalleryState) -> impl View<GalleryState> {
        column()
            .child(text(state.picked.get()))
            .child(Self::choose_button())
    }
}

impl Gallery {
    fn choose_button() -> impl View<GalleryState> {
        button("Choose photo", |state: &mut GalleryState| {
            let picked = state.picked;
            spawn_local(async move {
                let options = PickOptions::new().filter(MediaFilter::Images).limit(1);
                // `pick` blocks until the user chooses: run it off the UI thread.
                let result = spawn_blocking(move || PhotoLibrary::pick(&options)).await;
                picked.set(match result {
                    // Decode a picked `PickedItem.path` with `frust::decode_image_async`.
                    Ok(Ok(items)) => format!("{} picked", items.len()),
                    Ok(Err(err)) => format!("error: {err}"),
                    Err(err) => format!("error: {err}"),
                });
            });
        })
    }
}

frust::app!(Gallery);
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
  untranscoded (an iPhone HEIC stays HEIC, which `decode_image_async` cannot read; the `decode` feature's
  `decode_file` can).
- `PickedItem::file_name` is untrusted display text; never build a path from it.

## Tested platforms

| Platform | Result |
|---|---|
| iPhone SE 2nd gen, iOS 26.7.1 | passed (export, save, album save, location through the picker) |
| iPhone 13 mini, iOS 27.2 | passed (pick, loading progress, cancel, swipe-down, share picked, clear cache; browsing: albums, paging, thumbnails, Limited and Manage selection) |
| Pixel 5, Android 14 | passed, browsing, export and save included; Xiaomi 12 (Android 17) passed the picker and share legs on an earlier build |
| macOS | passed through the file dialog; `decode` has a host test that encoded and decoded a HEIC through ImageIO |
| `with_details`, the new `Asset` fields, `asset_metadata` and `thumbnail_shared` (including a thumbnail cancel) | not device-tested; covered by host tests and the Android contract test only |
| `decode` on iOS and Android devices, Android 10/11 fallback picker, Linux, Windows, DNG export on a phone | not device-tested (the API 29/30 leg is owed; so are API 29 paging and a second API 31+ device for browsing) |
