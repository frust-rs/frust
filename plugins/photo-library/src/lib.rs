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

mod cancel;
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
pub use originals::{
    AssetVersion, ExportOptions, ExportedOriginal, ProgressFn, SaveRequest, SavedAsset,
};
pub use types::{
    AssetId, MediaFilter, MediaKind, PhotoError, PickOptions, PickProgress, PickedItem,
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

    /// A thumbnail as straight-alpha RGBA8, from the in-memory cache when present. Blocks on a
    /// miss; [`PhotoError::NotFound`] for a stale `id`. The request is clamped to `1..=2048`.
    ///
    /// # The cache
    ///
    /// A hit returns before any platform call, so it skips the access check, and an edited or
    /// deleted asset can be served until [`clear_thumbnail_cache`](Self::clear_thumbnail_cache).
    /// The cache empties itself when [`manage_limited_selection`](Self::manage_limited_selection)
    /// succeeds and when a miss answers [`PhotoError::PermissionDenied`];
    /// [`set_thumbnail_cache_budget(0)`](Self::set_thumbnail_cache_budget) also empties it. A
    /// selection changed any other way (such as a `frust-permissions` request on Android while
    /// access is limited) needs an explicit clear.
    pub fn thumbnail(id: &AssetId, request: &ThumbnailRequest) -> Result<Thumbnail, PhotoError> {
        #[cfg(feature = "library")]
        {
            cached_thumbnail(id, request, platform::thumbnail)
        }
        #[cfg(not(feature = "library"))]
        {
            let _ = (id, request);
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

/// The cache-then-`load` path of [`PhotoLibrary::thumbnail`]; `load` is the platform call.
#[cfg(feature = "library")]
fn cached_thumbnail(
    id: &AssetId,
    request: &ThumbnailRequest,
    load: impl FnOnce(&AssetId, &ThumbnailRequest) -> Result<Thumbnail, PhotoError>,
) -> Result<Thumbnail, PhotoError> {
    let request = request.clamped();
    let key = thumb_cache::Key::new(id, &request);
    if let Some(hit) = thumb_cache::get(&key) {
        return Ok(hit);
    }
    let epoch = thumb_cache::epoch();
    let loaded = load(id, &request);
    match &loaded {
        Ok(thumb) => thumb_cache::insert(epoch, key, thumb.clone()),
        // Access is gone, so everything cached under it is stale.
        Err(PhotoError::PermissionDenied) => thumb_cache::clear(),
        Err(_) => {}
    }
    loaded
}

#[cfg(not(feature = "library"))]
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

    /// One test, because the cache under test is process-wide.
    #[cfg(feature = "library")]
    #[test]
    fn the_cache_path_serves_hits_and_clears_on_denial() {
        let id = AssetId::from_raw("cache-path");
        let request = ThumbnailRequest::new(2, 1, ThumbnailMode::Fill);
        let pixels = || Thumbnail::new(2, 1, vec![9; 8]);
        let hit_only = |_: &AssetId, _: &ThumbnailRequest| -> Result<Thumbnail, PhotoError> {
            panic!("a hit must not reach the platform")
        };

        PhotoLibrary::clear_thumbnail_cache();
        assert_eq!(
            cached_thumbnail(&id, &request, |_, _| Ok(pixels())),
            Ok(pixels())
        );
        assert_eq!(cached_thumbnail(&id, &request, hit_only), Ok(pixels()));

        // Another failure leaves the cache alone.
        let other = AssetId::from_raw("cache-path-other");
        assert_eq!(
            cached_thumbnail(&other, &request, |_, _| Err(PhotoError::NotFound)),
            Err(PhotoError::NotFound)
        );
        assert_eq!(cached_thumbnail(&id, &request, hit_only), Ok(pixels()));

        // A denied miss empties it: the earlier hit now reaches the platform.
        assert_eq!(
            cached_thumbnail(&other, &request, |_, _| Err(PhotoError::PermissionDenied)),
            Err(PhotoError::PermissionDenied)
        );
        assert_eq!(
            cached_thumbnail(&id, &request, |_, _| Err(PhotoError::NotFound)),
            Err(PhotoError::NotFound)
        );

        // So does the public clear.
        assert_eq!(
            cached_thumbnail(&id, &request, |_, _| Ok(pixels())),
            Ok(pixels())
        );
        PhotoLibrary::clear_thumbnail_cache();
        assert_eq!(
            cached_thumbnail(&id, &request, |_, _| Err(PhotoError::NotFound)),
            Err(PhotoError::NotFound)
        );
    }
}
