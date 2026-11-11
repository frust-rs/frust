//! `frust-photo-library`: pick photos and videos with the platform's own picker.
//!
//! - **Android, iOS:** the system photo picker (no photo-library permission needed to pick).
//! - **Linux, macOS, Windows:** a native file dialog through `frust-file-dialogs`, filtered to
//!   image and video extensions.
//! - **Any folder the user granted** (all of the above, not web): [`PhotoLibrary::scan_folder`]
//!   pages through its images and videos, newest first.
//!
//! # Charter: a platform plugin
//!
//! Like `frust-permissions`, this is a **platform plugin**: an app adds it to its own
//! `Cargo.toml` alongside `frust`; the `frust` facade does not re-export it. Its only
//! `frust-*` dependencies are `frust-paths` (the pick cache location), `frust-file-dialogs`
//! on desktop, Android and iOS, and `frust-plugin` on Android.
//!
//! # Threading
//!
//! [`PhotoLibrary::pick`] blocks until the user chooses or cancels. On the platform UI thread
//! of a mobile target it returns [`PhotoError::UiThread`]; pair it with `frust::spawn_blocking`.
//!
//! # Cancellation and copies
//!
//! An empty `Vec` means the user cancelled. On mobile each pick is **copied** into an app-private
//! cache (`PickedItem::is_copy` is `true`) so the path stays readable after the picker closes;
//! call [`PhotoLibrary::clear_pick_cache`] to reclaim the space. Desktop returns the user's
//! original files (`is_copy` is `false`) and never copies.
//!
//! ```no_run
//! use frust_photo_library::{
//!     CancelToken, MediaFilter, PhotoError, PhotoLibrary, PickOptions, PickProgress,
//! };
//!
//! let cancel = CancelToken::new();
//! let opts = PickOptions::new()
//!     .filter(MediaFilter::Videos)
//!     .on_progress(move |p: PickProgress| {
//!         let _ = (p.items, p.done, p.fraction);
//!     })
//!     .cancel(cancel.clone());
//! // UI: cancel.cancel();
//!
//! // Blocks: call inside `frust::spawn_blocking`.
//! match PhotoLibrary::pick(&opts) {
//!     Err(PhotoError::Cancelled) => {}
//!     Ok(items) => {
//!         for item in items {
//!             let _ = item.path; // decode with `frust::decode_image_async`
//!         }
//!     }
//!     Err(_) => {}
//! }
//! ```

// The ImageIO property reader serves `asset_metadata` (`library`) as well as the decoder.
#[cfg(all(
    any(feature = "decode", feature = "library"),
    any(target_os = "ios", target_os = "macos")
))]
mod apple_decode;
mod cancel;
#[cfg_attr(not(feature = "library"), allow(dead_code))]
mod changes;
mod decode;
#[cfg(any(
    target_os = "android",
    target_os = "ios",
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
))]
mod folder_scan;
mod library;
mod metadata;
mod originals;
mod types;

// Desktop uses only part of these; mobile targets keep the dead-code lint.
#[cfg_attr(not(any(target_os = "android", target_os = "ios")), allow(dead_code))]
mod media_types;
#[cfg_attr(not(any(target_os = "android", target_os = "ios")), allow(dead_code))]
mod pick_cache;
// The mobile browsing arms each call a different part of this; no target calls all of it, so
// the lint cannot stay on for any of them. The module's own tests cover every helper.
#[allow(dead_code)]
mod pixels;
#[cfg_attr(not(feature = "library"), allow(dead_code))]
mod thumb_cache;

pub use cancel::CancelToken;
pub use changes::{ChangeSubscription, ChangeToken, LibraryChanges};
pub use decode::{DecodeOptions, DecodedImage, ImageColorSpace};
#[cfg(any(
    target_os = "android",
    target_os = "ios",
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
))]
pub use folder_scan::{FolderMedia, FolderPage, FolderScan};
pub use library::{
    Album, AlbumId, AlbumKind, Asset, AssetPage, AssetQuery, MAX_PAGE_LIMIT, MAX_THUMBNAIL_EDGE,
    Thumbnail, ThumbnailMode, ThumbnailRequest,
};
pub use metadata::MetadataOptions;
pub use originals::{
    AssetVersion, ExportOptions, ExportedOriginal, ProgressFn, SaveRequest, SavedAsset,
};
pub use types::{
    AssetId, ExifData, MediaFilter, MediaKind, PhotoError, PickOptions, PickProgress, PickedItem,
};

/// The folder types [`PhotoLibrary::scan_folder`] takes and returns, from `frust-file-dialogs`,
/// so an app need not depend on that crate to name them. Absent on web.
#[cfg(any(
    target_os = "android",
    target_os = "ios",
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
))]
pub mod folder {
    pub use frust_file_dialogs::{EntryLocation, FolderEntry, FolderGrant};
}

#[cfg(target_os = "ios")]
mod apple;
#[cfg(target_os = "ios")]
use apple as platform;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
use android as platform;

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
))]
mod desktop;
#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
))]
use desktop as platform;

#[cfg(not(any(
    target_os = "ios",
    target_os = "android",
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
)))]
mod unsupported;
#[cfg(not(any(
    target_os = "ios",
    target_os = "android",
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
)))]
use unsupported as platform;

/// Entry point for picking media.
pub struct PhotoLibrary;

impl PhotoLibrary {
    /// Shows the platform picker and waits. Blocks; see the crate-level threading notes.
    /// An empty `Vec` means the user dismissed the picker.
    ///
    /// # Loading and cancel
    ///
    /// After the picker closes, the chosen items may still be loading (an iCloud video or a
    /// fresh photo can take seconds to minutes). [`PickOptions::on_progress`] reports that
    /// phase: its first call is the loading signal, and calls are rate-limited. Pass a
    /// [`CancelToken`] through [`PickOptions::cancel`] to abort from another thread.
    /// Cancelling while items load returns `Err(`[`PhotoError::Cancelled`]`)` and the call's
    /// partial copies are removed. Cancelling while the picker is open returns the same error
    /// at once on iOS (which dismisses the picker) and on Android (where the system picker
    /// stays on screen until the user closes it, and its result is discarded). On desktop the
    /// call returns only after the file dialog closes.
    pub fn pick(options: &PickOptions) -> Result<Vec<PickedItem>, PhotoError> {
        platform::pick(options)
    }

    /// Deletes every file copied by earlier mobile picks. A missing cache is success.
    ///
    /// Returns [`PhotoError::Busy`] while a mobile pick is in flight (and a pick cannot start
    /// while the clear runs). Paths from earlier picks' [`PickedItem`]s become invalid after a
    /// clear; the caller owns that.
    pub fn clear_pick_cache() -> Result<(), PhotoError> {
        pick_cache::clear()
    }

    /// Lists the albums the app can see. Blocks; needs the `library` feature and photo access.
    ///
    /// Browsing is read-only and never takes the pick guard, so it runs while a pick is open.
    /// [`PhotoError::UiThread`] on a mobile UI thread, [`PhotoError::PermissionDenied`] when
    /// access is not usable.
    pub fn albums() -> Result<Vec<Album>, PhotoError> {
        #[cfg(feature = "library")]
        {
            platform::albums()
        }
        #[cfg(not(feature = "library"))]
        {
            Err(library_disabled())
        }
    }

    /// One page of assets, newest first. Blocks; see [`albums`](Self::albums) for the rules.
    pub fn assets(query: &AssetQuery) -> Result<AssetPage, PhotoError> {
        #[cfg(feature = "library")]
        {
            platform::assets(query)
        }
        #[cfg(not(feature = "library"))]
        {
            let _ = query;
            Err(library_disabled())
        }
    }

    /// The library's current [`ChangeToken`], to pass to [`changes_since`](Self::changes_since)
    /// later. Blocks; see [`albums`](Self::albums) for the rules. Desktop and web have no photo
    /// library and return [`PhotoError::Unsupported`].
    pub fn current_change_token() -> Result<ChangeToken, PhotoError> {
        #[cfg(feature = "library")]
        {
            platform::current_change_token()
        }
        #[cfg(not(feature = "library"))]
        {
            Err(library_disabled())
        }
    }

    /// What changed in the library since `token`. Blocks; see [`albums`](Self::albums) for the
    /// rules. A token from another device, platform or library, or one the platform no longer
    /// tracks, answers [`LibraryChanges::FullRescan`], not an error.
    ///
    /// Before it returns, a [`LibraryChanges::Delta`] evicts every size of each updated and
    /// deleted asset from the thumbnail cache, and a [`LibraryChanges::FullRescan`] empties it.
    /// A thumbnail still loading when either happens is returned to its caller but not cached.
    pub fn changes_since(token: &ChangeToken) -> Result<LibraryChanges, PhotoError> {
        #[cfg(feature = "library")]
        {
            changes::changes_since_with(token, platform::changes_since)
        }
        #[cfg(not(feature = "library"))]
        {
            let _ = token;
            Err(library_disabled())
        }
    }

    /// Calls `callback` when the library may have changed, until the returned
    /// [`ChangeSubscription`] is dropped. Needs the `library` feature and photo access; desktop
    /// and web return [`PhotoError::Unsupported`].
    ///
    /// The callback only wakes the app: it carries no data, may run on any thread, and may be
    /// coalesced or spurious. Answer it by calling [`changes_since`](Self::changes_since) off
    /// the UI thread, and keep the callback itself short. The platform observer starts with
    /// the first subscription and stops when the last one drops.
    pub fn on_library_changed(
        callback: impl Fn() + Send + Sync + 'static,
    ) -> Result<ChangeSubscription, PhotoError> {
        #[cfg(feature = "library")]
        {
            changes::FEED.subscribe(
                std::sync::Arc::new(callback),
                platform::start_change_observer,
                platform::stop_change_observer,
            )
        }
        #[cfg(not(feature = "library"))]
        {
            let _ = callback;
            Err(library_disabled())
        }
    }

    /// A thumbnail as straight-alpha RGBA8, from the in-memory cache when present. Blocks on a
    /// miss; [`PhotoError::NotFound`] for a stale `id`. The request is clamped to `1..=2048`.
    ///
    /// # The cache
    ///
    /// A hit returns before any platform call, so it skips the access check, and an edited or
    /// deleted asset can be served until [`clear_thumbnail_cache`](Self::clear_thumbnail_cache)
    /// or until [`changes_since`](Self::changes_since) reports it.
    /// The cache empties itself when [`manage_limited_selection`](Self::manage_limited_selection)
    /// succeeds and when a miss answers [`PhotoError::PermissionDenied`];
    /// [`set_thumbnail_cache_budget(0)`](Self::set_thumbnail_cache_budget) also empties it. A
    /// selection changed any other way (such as a `frust-permissions` request on Android while
    /// access is limited) needs an explicit clear.
    ///
    /// The returned value is a copy of the cached pixels; [`thumbnail_shared`](Self::thumbnail_shared)
    /// shares them instead and can be cancelled.
    pub fn thumbnail(id: &AssetId, request: &ThumbnailRequest) -> Result<Thumbnail, PhotoError> {
        #[cfg(feature = "library")]
        {
            // A fresh token is never cancelled, so this is the uncancellable path.
            cached_thumbnail(id, request, &CancelToken::new(), platform::thumbnail)
                .map(std::sync::Arc::unwrap_or_clone)
        }
        #[cfg(not(feature = "library"))]
        {
            let _ = (id, request);
            Err(library_disabled())
        }
    }

    /// [`thumbnail`](Self::thumbnail) for a grid cell: the pixels are shared with the cache, and
    /// `token` abandons a miss whose cell has left the screen. Blocks on a miss; needs the
    /// `library` feature. The cache rules are those of `thumbnail`, and both calls share one
    /// cache.
    ///
    /// A hit returns a clone of the cached [`Arc`](std::sync::Arc), so serving it copies no
    /// pixel. A miss loads from the platform and caches the result.
    ///
    /// # Cancel
    ///
    /// A token cancelled before the call returns [`PhotoError::Cancelled`] before the cache or
    /// the platform is consulted. A cancel during a miss returns [`PhotoError::Cancelled`]
    /// and caches nothing, even when the pixels arrived first.
    ///
    /// - **iOS:** the cancel calls `cancelImageRequest:` on the in-flight Photos request and
    ///   wakes the waiting thread at once. The 30 second request timeout stays as a backstop.
    /// - **Android:** the load runs with a `CancellationSignal`, which the cancel fires; the
    ///   decode stops where `MediaStore` honours the signal. A cancel that lands just before
    ///   the host starts the load does not stop it, but the call still returns `Cancelled`.
    /// - **Desktop and web:** there is no library, so the call returns what `thumbnail` returns
    ///   there.
    ///
    /// A [`CancelToken`] is single-use: once cancelled it stays cancelled, so give each call
    /// (each cell's load) its own token.
    ///
    /// # Showing it
    ///
    /// The cache itself never copies pixels: a hit shares the cached allocation. Building an
    /// `ImageSource` from the pixels copies them once, though, so compared with
    /// [`thumbnail`](Self::thumbnail) an app saves the cache-side copy only, not that one.
    ///
    /// Keep one image per asset: build an `ImageSource` from the pixels once and hold it with
    /// the cell's state, rather than making a new one in every build. Each `ImageSource` made
    /// from pixels is a new engine image, so one per build copies and uploads the same pixels
    /// again and again even though the cache shares them.
    pub fn thumbnail_shared(
        id: &AssetId,
        request: &ThumbnailRequest,
        token: &CancelToken,
    ) -> Result<std::sync::Arc<Thumbnail>, PhotoError> {
        #[cfg(feature = "library")]
        {
            cached_thumbnail(id, request, token, platform::thumbnail)
        }
        #[cfg(not(feature = "library"))]
        {
            let _ = (id, request, token);
            Err(library_disabled())
        }
    }

    /// Lets the user change which photos the app may see when access is limited. Blocks until
    /// the system UI closes. Unlike browsing it presents UI, so it takes the pick guard
    /// ([`PhotoError::Busy`] while a pick runs). Success empties the thumbnail cache, which may
    /// hold assets the new selection left out.
    pub fn manage_limited_selection() -> Result<(), PhotoError> {
        #[cfg(feature = "library")]
        {
            let result = platform::manage_limited_selection();
            if result.is_ok() {
                thumb_cache::clear();
            }
            result
        }
        #[cfg(not(feature = "library"))]
        {
            Err(library_disabled())
        }
    }

    /// Copies an asset's original file (or its edited rendition) into `options.dest_dir`.
    /// Blocks; needs the `library` feature and photo access, and takes neither the pick guard
    /// nor the pick in-flight slot.
    ///
    /// The default destination is `<cache_dir>/frust-photo-library/originals/`. The app owns it:
    /// [`clear_pick_cache`](Self::clear_pick_cache) does not touch it. Progress and cancellation
    /// work as for [`pick`](Self::pick): a cancelled export returns
    /// `Err(`[`PhotoError::Cancelled`]`)` and leaves no partial file. A cloud-only original with
    /// [`ExportOptions::allow_network`] off returns [`PhotoError::NetworkRequired`]. Android has
    /// no rendition split or network distinction, so [`AssetVersion::Current`] and
    /// `allow_network` are ignored there.
    pub fn export_original(
        id: &AssetId,
        options: &ExportOptions,
    ) -> Result<ExportedOriginal, PhotoError> {
        #[cfg(feature = "library")]
        {
            if originals::is_cancelled(options) {
                return Err(PhotoError::Cancelled);
            }
            platform::export_original(id, options)
        }
        #[cfg(not(feature = "library"))]
        {
            let _ = (id, options);
            Err(library_disabled())
        }
    }

    /// Reads the EXIF of one library asset without the app exporting or decoding it. Blocks;
    /// needs the `library` feature and photo access.
    ///
    /// A token cancelled before the call returns [`PhotoError::Cancelled`] before any platform
    /// work. iOS and Android then return [`PhotoError::UiThread`] on their UI thread. A stale
    /// `id` is [`PhotoError::NotFound`]. An asset with no EXIF, a video included, returns an
    /// empty [`ExifData`] (every field `None`), not an error. Desktop and web have no photo
    /// library and return [`PhotoError::Unsupported`]. The fields follow the rules
    /// [`ExifData`] documents, as for [`decode_file`](Self::decode_file).
    ///
    /// - **iOS:** Photos offers no way to read an asset's metadata from its header alone, so the
    ///   original's bytes must be on the device: the call reads the whole original resource into
    ///   memory (never to a file) and hands it to the same ImageIO property reader
    ///   [`decode_file`](Self::decode_file) uses, which reads that buffer in place. An original
    ///   that is only in iCloud is downloaded when [`MetadataOptions::allow_network`] is set, and
    ///   returns [`PhotoError::NetworkRequired`] otherwise (the default). A video is not read at
    ///   all.
    ///   - *Deadline:* with or without a token, a read that receives neither data nor its
    ///     completion for 30 seconds is stopped and returns a [`PhotoError::Platform`] timeout.
    ///     Each piece of data restarts that clock. Only delivered data counts: if Photos
    ///     downloads an iCloud original for longer than that before it delivers the first piece,
    ///     the read times out (not yet checked on a device). A cancel during the read stops the
    ///     transfer sooner and returns [`PhotoError::Cancelled`].
    ///   - *Size and memory:* an original over 256 MiB is refused with
    ///     [`PhotoError::InvalidRequest`] once that much has arrived. The buffer holds the
    ///     original once, with up to the same again in spare capacity; while it grows, the old
    ///     and the new block are briefly both live, about three times the bytes received.
    ///     An allocation the system refuses returns [`PhotoError::Platform`] rather than
    ///     aborting, but iOS can still end the app under memory pressure while the buffer is
    ///     large, so read originals one at a time.
    /// - **Android:** the asset's content URI is opened as
    ///   [`export_original`](Self::export_original) opens it, unredacted only when the app holds
    ///   `ACCESS_MEDIA_LOCATION`, and read with `android.media.ExifInterface`. Without that
    ///   permission the GPS fields are `None`, including when the platform returns a redacted 0, 0
    ///   location. `allow_network` has no effect: `MediaStore` makes
    ///   no network distinction. The token is checked before the read and after it.
    pub fn asset_metadata(id: &AssetId, options: &MetadataOptions) -> Result<ExifData, PhotoError> {
        #[cfg(feature = "library")]
        {
            metadata::asset_metadata_with(id, options, platform::asset_metadata)
        }
        #[cfg(not(feature = "library"))]
        {
            let _ = (id, options);
            Err(library_disabled())
        }
    }

    /// Decodes an image file at its full resolution, downscaled so neither side exceeds
    /// [`DecodeOptions::max_dimension`], into straight-alpha RGBA8. Blocks; needs the `decode`
    /// feature.
    ///
    /// # Order of checks and errors
    ///
    /// A shared precheck runs first on every platform: a token cancelled before the call returns
    /// [`PhotoError::Cancelled`] and a zero `max_dimension` returns
    /// [`PhotoError::InvalidRequest`]. The UI-thread refusal follows: iOS and Android return
    /// [`PhotoError::UiThread`] on their UI thread, while macOS decodes on the calling thread
    /// and does not refuse the main one. A missing file is [`PhotoError::NotFound`] on every
    /// platform that decodes (a plain path or a content URI on Android, a path on iOS and
    /// macOS). A platform with no decoder returns [`PhotoError::Unsupported`]. A capped image
    /// over the byte ceiling (see [`DecodeOptions`]) returns [`PhotoError::InvalidRequest`]
    /// before its buffer is allocated.
    ///
    /// iOS and macOS decode through ImageIO (HEIC, JPEG, PNG and every other format it reads),
    /// apply the EXIF orientation, and return Display P3 pixels for a wide-gamut image unless
    /// [`DecodeOptions::srgb`] is set. A file with no decodable image, or one cut short inside
    /// its headers, is [`PhotoError::Platform`]. A file whose pixel data stops early decodes
    /// with the missing part blank (ImageIO's own leniency). The token is checked between
    /// stages, but the platform decode itself is not interruptible.
    pub fn decode_file(
        path: &std::path::Path,
        options: &DecodeOptions,
    ) -> Result<DecodedImage, PhotoError> {
        #[cfg(feature = "decode")]
        {
            decode::decode_file_with(path, options, platform::decode_file)
        }
        #[cfg(not(feature = "decode"))]
        {
            let _ = (path, options);
            Err(decode::decode_disabled())
        }
    }

    /// Exports an asset's file (see [`export_original`](Self::export_original)), decodes it as
    /// [`decode_file`](Self::decode_file) does, and removes the exported file afterwards on
    /// success, failure, cancel and a panic in the decode. Blocks; needs the `decode` and
    /// `library` features. The same shared precheck runs first, before the export.
    pub fn decode_asset(
        id: &AssetId,
        options: &DecodeOptions,
        export: &ExportOptions,
    ) -> Result<DecodedImage, PhotoError> {
        #[cfg(all(feature = "decode", feature = "library"))]
        {
            decode::decode_asset_with(
                id,
                options,
                export,
                |id, export| {
                    if originals::is_cancelled(export) {
                        return Err(PhotoError::Cancelled);
                    }
                    platform::export_original(id, export)
                },
                platform::decode_file,
            )
        }
        #[cfg(not(all(feature = "decode", feature = "library")))]
        {
            let _ = (id, options, export);
            Err(if cfg!(feature = "decode") {
                library_disabled()
            } else {
                decode::decode_disabled()
            })
        }
    }

    /// Saves a file into the photo library, optionally into a named album. Blocks; needs the
    /// `save` feature. The request is validated first: the source must exist and its kind must
    /// be given or inferable from the extension, and a bad file name, album or kind returns
    /// [`PhotoError::InvalidRequest`]. On iOS an album save needs read-write Photos access and
    /// `NSPhotoLibraryUsageDescription` (the `library` feature provides the key), while a plain
    /// save needs only the add-only key the `save` feature provides; add-only access with an
    /// album returns [`PhotoError::PermissionDenied`], and a missing key returns
    /// [`PhotoError::MissingUsageDescription`].
    pub fn save(request: &SaveRequest) -> Result<SavedAsset, PhotoError> {
        #[cfg(feature = "save")]
        {
            let kind = originals::validate_save(request)?;
            platform::save(request, kind)
        }
        #[cfg(not(feature = "save"))]
        {
            let _ = request;
            Err(PhotoError::FeatureDisabled { feature: "save" })
        }
    }

    /// Empties the in-memory thumbnail cache, so the next [`thumbnail`](Self::thumbnail) of every
    /// asset goes back to the platform. Non-blocking, and safe on a UI thread. A thumbnail
    /// still loading when this runs is returned to its caller but not cached. Does nothing
    /// without the `library` feature.
    pub fn clear_thumbnail_cache() {
        #[cfg(feature = "library")]
        thumb_cache::clear();
    }

    /// Sets the thumbnail cache budget in bytes (default 48 MiB), evicting least-recently-used
    /// entries at once to fit. Non-blocking.
    pub fn set_thumbnail_cache_budget(bytes: usize) -> Result<(), PhotoError> {
        #[cfg(feature = "library")]
        {
            thumb_cache::set_budget(bytes);
            Ok(())
        }
        #[cfg(not(feature = "library"))]
        {
            let _ = bytes;
            Err(library_disabled())
        }
    }
}

/// The cache-then-`load` path of [`PhotoLibrary::thumbnail`] and
/// [`PhotoLibrary::thumbnail_shared`]; `load` is the platform call.
#[cfg(feature = "library")]
fn cached_thumbnail(
    id: &AssetId,
    request: &ThumbnailRequest,
    token: &CancelToken,
    load: impl FnOnce(&AssetId, &ThumbnailRequest, &CancelToken) -> Result<Thumbnail, PhotoError>,
) -> Result<std::sync::Arc<Thumbnail>, PhotoError> {
    if token.is_cancelled() {
        return Err(PhotoError::Cancelled);
    }
    let request = request.clamped();
    let key = thumb_cache::Key::new(id, &request);
    if let Some(hit) = thumb_cache::get(&key) {
        return Ok(hit);
    }
    let epoch = thumb_cache::epoch();
    let loaded = load(id, &request, token);
    if matches!(loaded, Err(PhotoError::PermissionDenied)) {
        // Access is gone, so everything cached under it is stale.
        thumb_cache::clear();
    }
    // A cancel that raced the load wins, and its pixels are not cached.
    if token.is_cancelled() {
        return Err(PhotoError::Cancelled);
    }
    let shared = std::sync::Arc::new(loaded?);
    thumb_cache::insert(epoch, key, std::sync::Arc::clone(&shared));
    Ok(shared)
}

// `decode_asset` names it with `library` on and `decode` off, too.
#[cfg(not(all(feature = "library", feature = "decode")))]
fn library_disabled() -> PhotoError {
    PhotoError::FeatureDisabled { feature: "library" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(feature = "save"))]
    #[test]
    fn save_is_feature_gated() {
        assert_eq!(
            PhotoLibrary::save(&SaveRequest::new("a.jpg")),
            Err(PhotoError::FeatureDisabled { feature: "save" })
        );
    }

    #[cfg(not(feature = "library"))]
    #[test]
    fn export_original_is_feature_gated() {
        assert_eq!(
            PhotoLibrary::export_original(&AssetId::from_raw("a"), &ExportOptions::new()),
            Err(PhotoError::FeatureDisabled { feature: "library" })
        );
    }

    #[cfg(feature = "library")]
    #[test]
    fn export_original_honours_a_cancelled_token_before_the_platform() {
        let token = CancelToken::new();
        token.cancel();
        assert_eq!(
            PhotoLibrary::export_original(
                &AssetId::from_raw("a"),
                &ExportOptions::new().cancel(token)
            ),
            Err(PhotoError::Cancelled)
        );
    }

    #[cfg(feature = "save")]
    #[test]
    fn save_validates_before_the_platform() {
        assert_eq!(
            PhotoLibrary::save(&SaveRequest::new("/nonexistent/frust/a.jpg")),
            Err(PhotoError::NotFound)
        );
    }

    #[cfg(not(feature = "library"))]
    #[test]
    fn clearing_the_thumbnail_cache_is_a_no_op_without_the_feature() {
        PhotoLibrary::clear_thumbnail_cache();
    }

    /// Serialises the tests that drive the process-wide cache.
    #[cfg(feature = "library")]
    static CACHE_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(feature = "library")]
    fn cache_lock() -> std::sync::MutexGuard<'static, ()> {
        CACHE_TESTS.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// A fake platform for the cache path, counting its calls.
    #[cfg(feature = "library")]
    #[derive(Default)]
    struct FakeBackend {
        calls: std::sync::atomic::AtomicUsize,
    }

    #[cfg(feature = "library")]
    impl FakeBackend {
        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }

        /// A loader answering `answer`, after running `during` (a cancel, say).
        fn load<'a>(
            &'a self,
            answer: Result<Thumbnail, PhotoError>,
            during: impl FnOnce(&CancelToken) + 'a,
        ) -> impl FnOnce(&AssetId, &ThumbnailRequest, &CancelToken) -> Result<Thumbnail, PhotoError> + 'a
        {
            move |_, _, token| {
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                during(token);
                answer
            }
        }
    }

    #[cfg(feature = "library")]
    fn pixels() -> Thumbnail {
        Thumbnail::new(2, 1, vec![9; 8])
    }

    #[cfg(feature = "library")]
    fn hit_only(
        _: &AssetId,
        _: &ThumbnailRequest,
        _: &CancelToken,
    ) -> Result<Thumbnail, PhotoError> {
        panic!("a hit must not reach the platform")
    }

    /// The uncancellable path, as `PhotoLibrary::thumbnail` takes it.
    #[cfg(feature = "library")]
    fn copied(
        id: &AssetId,
        request: &ThumbnailRequest,
        load: impl FnOnce(&AssetId, &ThumbnailRequest, &CancelToken) -> Result<Thumbnail, PhotoError>,
    ) -> Result<Thumbnail, PhotoError> {
        cached_thumbnail(id, request, &CancelToken::new(), load)
            .map(std::sync::Arc::unwrap_or_clone)
    }

    #[cfg(feature = "library")]
    #[test]
    fn the_cache_path_serves_hits_and_clears_on_denial() {
        let _serial = cache_lock();
        let id = AssetId::from_raw("cache-path");
        let request = ThumbnailRequest::new(2, 1, ThumbnailMode::Fill);

        PhotoLibrary::clear_thumbnail_cache();
        assert_eq!(copied(&id, &request, |_, _, _| Ok(pixels())), Ok(pixels()));
        assert_eq!(copied(&id, &request, hit_only), Ok(pixels()));

        // Another failure leaves the cache alone.
        let other = AssetId::from_raw("cache-path-other");
        assert_eq!(
            copied(&other, &request, |_, _, _| Err(PhotoError::NotFound)),
            Err(PhotoError::NotFound)
        );
        assert_eq!(copied(&id, &request, hit_only), Ok(pixels()));

        // A denied miss empties it: the earlier hit now reaches the platform.
        assert_eq!(
            copied(&other, &request, |_, _, _| Err(
                PhotoError::PermissionDenied
            )),
            Err(PhotoError::PermissionDenied)
        );
        assert_eq!(
            copied(&id, &request, |_, _, _| Err(PhotoError::NotFound)),
            Err(PhotoError::NotFound)
        );

        // So does the public clear.
        assert_eq!(copied(&id, &request, |_, _, _| Ok(pixels())), Ok(pixels()));
        PhotoLibrary::clear_thumbnail_cache();
        assert_eq!(
            copied(&id, &request, |_, _, _| Err(PhotoError::NotFound)),
            Err(PhotoError::NotFound)
        );
    }

    #[cfg(feature = "library")]
    #[test]
    fn a_shared_hit_is_the_cached_allocation() {
        let _serial = cache_lock();
        PhotoLibrary::clear_thumbnail_cache();
        let id = AssetId::from_raw("shared-hit");
        let request = ThumbnailRequest::new(2, 1, ThumbnailMode::Fit);
        let token = CancelToken::new();
        let backend = FakeBackend::default();

        let miss = cached_thumbnail(&id, &request, &token, backend.load(Ok(pixels()), |_| {}))
            .expect("the miss loads");
        let hit = cached_thumbnail(&id, &request, &token, hit_only).expect("the hit is served");
        let again = cached_thumbnail(&id, &request, &token, hit_only).expect("the hit is served");
        assert!(
            std::sync::Arc::ptr_eq(&miss, &hit),
            "the miss returns the cached Arc"
        );
        assert!(
            std::sync::Arc::ptr_eq(&hit, &again),
            "a hit copies no pixel"
        );
        assert_eq!(backend.calls(), 1);

        // The copying path serves the same entry, as equal pixels.
        assert_eq!(copied(&id, &request, hit_only), Ok(pixels()));
        assert_eq!(*hit, pixels());
    }

    #[cfg(feature = "library")]
    #[test]
    fn a_cancelled_miss_is_not_stored() {
        let _serial = cache_lock();
        PhotoLibrary::clear_thumbnail_cache();
        let id = AssetId::from_raw("cancelled-miss");
        let request = ThumbnailRequest::new(2, 1, ThumbnailMode::Fill);
        let backend = FakeBackend::default();

        // The pixels arrive, but the cell was cancelled while they loaded.
        let token = CancelToken::new();
        assert_eq!(
            cached_thumbnail(
                &id,
                &request,
                &token,
                backend.load(Ok(pixels()), CancelToken::cancel)
            ),
            Err(PhotoError::Cancelled)
        );
        // A platform answer of `Cancelled` is not stored either.
        let token = CancelToken::new();
        assert_eq!(
            cached_thumbnail(
                &id,
                &request,
                &token,
                backend.load(Err(PhotoError::Cancelled), |_| {})
            ),
            Err(PhotoError::Cancelled)
        );
        assert_eq!(backend.calls(), 2);

        // Nothing was cached: a fresh token reaches the backend again.
        let fresh = CancelToken::new();
        assert_eq!(
            cached_thumbnail(&id, &request, &fresh, backend.load(Ok(pixels()), |_| {})).as_deref(),
            Ok(&pixels())
        );
        assert_eq!(backend.calls(), 3);
    }

    #[cfg(feature = "library")]
    #[test]
    fn a_pre_cancelled_token_never_reaches_the_backend() {
        let _serial = cache_lock();
        PhotoLibrary::clear_thumbnail_cache();
        let id = AssetId::from_raw("pre-cancelled");
        let request = ThumbnailRequest::new(2, 1, ThumbnailMode::Fill);
        let token = CancelToken::new();
        token.cancel();
        assert_eq!(
            cached_thumbnail(&id, &request, &token, hit_only),
            Err(PhotoError::Cancelled)
        );
        assert_eq!(
            PhotoLibrary::thumbnail_shared(&id, &request, &token),
            Err(PhotoError::Cancelled)
        );

        // Not even a cached entry is served to it.
        let backend = FakeBackend::default();
        let live = CancelToken::new();
        assert!(cached_thumbnail(&id, &request, &live, backend.load(Ok(pixels()), |_| {})).is_ok());
        assert_eq!(
            cached_thumbnail(&id, &request, &token, hit_only),
            Err(PhotoError::Cancelled)
        );
        assert_eq!(backend.calls(), 1);
    }

    #[cfg(not(feature = "library"))]
    #[test]
    fn the_change_feed_is_feature_gated() {
        let disabled = PhotoError::FeatureDisabled { feature: "library" };
        assert_eq!(PhotoLibrary::current_change_token(), Err(disabled.clone()));
        assert_eq!(
            PhotoLibrary::changes_since(&ChangeToken::from_bytes(*b"t")),
            Err(disabled.clone())
        );
        assert_eq!(
            PhotoLibrary::on_library_changed(|| {}).map(|_| ()),
            Err(disabled)
        );
    }

    // Desktop and web have no library, so their arms refuse the whole feed.
    #[cfg(all(
        feature = "library",
        not(any(target_os = "android", target_os = "ios"))
    ))]
    #[test]
    fn the_change_feed_is_unsupported_without_a_library() {
        assert_eq!(
            PhotoLibrary::current_change_token(),
            Err(PhotoError::Unsupported)
        );
        assert_eq!(
            PhotoLibrary::changes_since(&ChangeToken::from_bytes(*b"t")),
            Err(PhotoError::Unsupported)
        );
        assert_eq!(
            PhotoLibrary::on_library_changed(|| {}).map(|_| ()),
            Err(PhotoError::Unsupported)
        );
    }

    #[cfg(not(feature = "library"))]
    #[test]
    fn thumbnail_shared_is_feature_gated() {
        let request = ThumbnailRequest::new(8, 8, ThumbnailMode::Fill);
        assert_eq!(
            PhotoLibrary::thumbnail_shared(&AssetId::from_raw("a"), &request, &CancelToken::new()),
            Err(PhotoError::FeatureDisabled { feature: "library" })
        );
    }
}
