//! iOS library browsing over PhotoKit (`library` feature only).
//!
//! # Access
//!
//! Every call reads `authorizationStatusForAccessLevel:` for read-write access
//! first; no prompt is shown from here (the app asks through its permission
//! flow). `Authorized` and `Limited` proceed, anything else answers
//! [`PhotoError::PermissionDenied`]. Under `Limited` every fetch returns only the
//! user's selected assets, which is the platform's own behaviour.
//!
//! # Threading
//!
//! Each call blocks, so each refuses the main thread with
//! [`PhotoError::UiThread`]. Browsing is read-only and never takes the pick
//! guard; [`manage_limited_selection`] presents system UI and does.
//!
//! # Thumbnails
//!
//! `PHImageManager.h` documents that a `highQualityFormat` request calls its
//! handler once when `synchronous` is `NO`, usually on the main thread, and that
//! a cancelled request "may not" call it at all. So the request is asynchronous
//! (the handler needs a free main thread, which the refused-main-thread rule
//! guarantees), the waiter has a bounded timeout ([`THUMBNAIL_TIMEOUT`]) and
//! cancels the request (`cancelImageRequest:`) when it gives up, so an iCloud
//! stall cannot park a grid worker forever. A timeout answers
//! [`PhotoError::Platform`]. The handler only hands the `UIImage` over; drawing
//! happens on the waiting thread, leaving the main queue free.
//!
//! `requestImageForAsset:...` is bound for macOS only (`NSImage`), so it is sent
//! by hand with `UIImage`. The header promises "as asked or better", so the
//! delivered size may exceed the box and only the drawing step can honour the
//! request. [`draw_plan`] sizes the output from it: `Fill` is exactly the box,
//! centre-cropped from the upright image; `Fit` keeps the aspect and never
//! exceeds the box or upscales. The image is drawn into an 8-bit sRGB RGBA
//! premultiplied bitmap of that size, upright for its `imageOrientation`, then
//! converted to straight alpha.
//!
//! # Limited selection
//!
//! `PHPhotoLibrary+PhotosUISupport.h` says the completion "will be called upon
//! the user finishing their selection", and that the call does nothing unless
//! the app is in limited mode, so [`manage_limited_selection`] answers `Ok(())`
//! at once outside it. The method has no presentation callback, and no header
//! promises the completion for every way the sheet can close, so a main-queue
//! watch ends the wait if the completion is skipped:
//!
//! - the completion runs: done;
//! - the sheet attaches to the presenter and later leaves it: done (a close
//!   without the completion);
//! - nothing attaches within [`PRESENT_GRACE`]: UIKit refused the presentation,
//!   so the call fails with [`PhotoError::NoPresenter`].
//!
//! The wait has no timeout of its own while the sheet is up: the user may keep
//! it open as long as they like.

use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchTime, MainThreadBound};
use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{ClassType, MainThreadMarker, msg_send, sel};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapInfo, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, CGInterpolationQuality, kCGColorSpaceSRGB,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSPredicate, NSSortDescriptor, NSString};
use objc2_photos::{
    PHAccessLevel, PHAsset, PHAssetCollection, PHAssetCollectionSubtype, PHAssetCollectionType,
    PHAssetMediaType, PHAuthorizationStatus, PHCachingImageManager, PHFetchOptions,
    PHImageCancelledKey, PHImageContentMode, PHImageErrorKey, PHImageRequestID,
    PHImageRequestOptions, PHImageRequestOptionsDeliveryMode, PHImageRequestOptionsResizeMode,
    PHImageResultIsDegradedKey, PHPhotoLibrary,
};
use objc2_ui_kit::{UIImage, UIImageOrientation, UIViewController};

use super::{PRESENT_GRACE, presenter};
use crate::library::{
    Album, AlbumId, AlbumKind, Asset, AssetPage, AssetQuery, Thumbnail, ThumbnailMode,
    ThumbnailRequest,
};
use crate::pick_cache::PickGuard;
use crate::pixels;
use crate::types::{AssetId, MediaFilter, MediaKind, PhotoError};

/// How long one thumbnail request may take before it is cancelled.
const THUMBNAIL_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the limited picker's watch looks at the presenter.
const WATCH_TICK: Duration = Duration::from_millis(250);

/// `CGSize` as `requestImageForAsset:` takes it. This crate has no direct
/// dependency on the crate that names the type.
#[repr(C)]
#[derive(Clone, Copy)]
struct TargetSize {
    width: f64,
    height: f64,
}

// SAFETY: `repr(C)` over two `f64`s is the layout of `CGSize` on 64-bit iOS,
// where `CGFloat` is `double`, and the encoding spells that struct.
unsafe impl Encode for TargetSize {
    const ENCODING: Encoding = Encoding::Struct("CGSize", &[Encoding::Double, Encoding::Double]);
}

// SAFETY: a pointer to the struct encoded above.
unsafe impl RefEncode for TargetSize {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}

fn refuse_main_thread() -> Result<(), PhotoError> {
    if MainThreadMarker::new().is_some() {
        return Err(PhotoError::UiThread);
    }
    Ok(())
}

/// The read-write authorization status, failing when Photos is not loaded (the
/// bindings panic on a missing class).
fn authorization() -> Result<PHAuthorizationStatus, PhotoError> {
    if AnyClass::get(c"PHPhotoLibrary").is_none() {
        return Err(PhotoError::Platform(
            "Photos is not loaded; link Photos.framework".to_owned(),
        ));
    }
    // SAFETY: a class method taking a declared `PHAccessLevel` value and
    // returning the status; it reads state and never prompts.
    Ok(unsafe { PHPhotoLibrary::authorizationStatusForAccessLevel(PHAccessLevel::ReadWrite) })
}

fn ensure_readable() -> Result<(), PhotoError> {
    let status = authorization()?;
    if status == PHAuthorizationStatus::Authorized || status == PHAuthorizationStatus::Limited {
        Ok(())
    } else {
        Err(PhotoError::PermissionDenied)
    }
}

/// The `PHFetchOptions` predicate for `filter`. Numbers are `PHAssetMediaType`
/// values, so audio and unknown assets never appear.
fn media_predicate(filter: MediaFilter) -> &'static str {
    match filter {
        MediaFilter::Images => "mediaType == 1",
        MediaFilter::Videos => "mediaType == 2",
        MediaFilter::ImagesAndVideos => "mediaType == 1 OR mediaType == 2",
    }
}

/// Newest first, limited to `filter`.
fn fetch_options(filter: MediaFilter) -> Retained<PHFetchOptions> {
    let format = NSString::from_str(media_predicate(filter));
    let key = NSString::from_str("creationDate");
    let newest_first = NSSortDescriptor::sortDescriptorWithKey_ascending(Some(&key), false);
    let sort = NSArray::from_retained_slice(&[newest_first]);
    // SAFETY: `predicateWithFormat:argumentArray:` gets a format with no
    // substitutions and no arguments. `new` and the two setters are plain
    // messages to the fresh options object, which no other thread has seen.
    unsafe {
        let predicate = NSPredicate::predicateWithFormat_argumentArray(&format, None);
        let options = PHFetchOptions::new();
        options.setPredicate(Some(&predicate));
        options.setSortDescriptors(Some(&sort));
        options
    }
}

pub(crate) fn albums() -> Result<Vec<Album>, PhotoError> {
    refuse_main_thread()?;
    autoreleasepool(|_| {
        ensure_readable()?;
        let options = fetch_options(MediaFilter::ImagesAndVideos);
        let mut albums = Vec::new();
        let groups = [
            (
                PHAssetCollectionType::SmartAlbum,
                PHAssetCollectionSubtype::SmartAlbumUserLibrary,
                AlbumKind::AllMedia,
            ),
            (
                PHAssetCollectionType::SmartAlbum,
                PHAssetCollectionSubtype::SmartAlbumFavorites,
                AlbumKind::Favorites,
            ),
            (
                PHAssetCollectionType::Album,
                PHAssetCollectionSubtype::AlbumRegular,
                AlbumKind::User,
            ),
        ];
        for (kind, subtype, album_kind) in groups {
            push_albums(&mut albums, kind, subtype, album_kind, &options);
        }
        Ok(albums)
    })
}

/// Append every collection of this type and subtype, counted under `options`.
fn push_albums(
    albums: &mut Vec<Album>,
    kind: PHAssetCollectionType,
    subtype: PHAssetCollectionSubtype,
    album_kind: AlbumKind,
    options: &PHFetchOptions,
) {
    // SAFETY: the fetch takes declared type and subtype values and a null
    // options pointer, and `count` / `objectAtIndex:` stay below the count of
    // that immutable result. The fetch for the count takes a live collection and
    // options object. The property getters read live PhotoKit objects, which the
    // bindings mark `Send + Sync`.
    unsafe {
        let collections =
            PHAssetCollection::fetchAssetCollectionsWithType_subtype_options(kind, subtype, None);
        for index in 0..collections.count() {
            let collection = collections.objectAtIndex(index);
            let assets = PHAsset::fetchAssetsInAssetCollection_options(&collection, Some(options));
            albums.push(Album {
                id: AlbumId::from_raw(collection.localIdentifier().to_string()),
                title: collection
                    .localizedTitle()
                    .map(|title| title.to_string())
                    .unwrap_or_default(),
                kind: album_kind,
                count: Some(assets.count() as u64),
            });
        }
    }
}

pub(crate) fn assets(query: &AssetQuery) -> Result<AssetPage, PhotoError> {
    refuse_main_thread()?;
    autoreleasepool(|_| {
        ensure_readable()?;
        let options = fetch_options(query.filter);
        // SAFETY: the album lookup takes an `NSArray<NSString>` of identifiers
        // and null options, and `firstObject` reads the immutable result. The
        // asset fetches take a live collection and options object.
        let assets = unsafe {
            match &query.album {
                Some(album) => {
                    let ids = NSArray::from_retained_slice(&[NSString::from_str(album.as_str())]);
                    let collection =
                        PHAssetCollection::fetchAssetCollectionsWithLocalIdentifiers_options(
                            &ids, None,
                        )
                        .firstObject()
                        .ok_or(PhotoError::NotFound)?;
                    PHAsset::fetchAssetsInAssetCollection_options(&collection, Some(&options))
                }
                None => PHAsset::fetchAssetsWithOptions(Some(&options)),
            }
        };
        // SAFETY: `count` reads the immutable fetch result.
        let total = unsafe { assets.count() };
        let items = page_bounds(query.offset, query.effective_limit(), total)
            // SAFETY: the page is clamped below `total`, the result's count.
            .map(|index| asset_of(&*unsafe { assets.objectAtIndex(index) }))
            .collect();
        Ok(AssetPage {
            items,
            total: total as u64,
        })
    })
}

/// The indices of one page within `total` assets.
fn page_bounds(offset: u64, limit: u32, total: usize) -> Range<usize> {
    let start = usize::try_from(offset).unwrap_or(usize::MAX).min(total);
    let end = start.saturating_add(limit as usize).min(total);
    start..end
}

/// Seconds to whole milliseconds; the `as` casts at the call sites saturate.
fn millis(seconds: f64) -> f64 {
    (seconds * 1000.0).round()
}

fn asset_of(asset: &PHAsset) -> Asset {
    // SAFETY: read-only property getters of a live `PHAsset`; the bindings mark
    // the class `Send + Sync`.
    unsafe {
        let video = asset.mediaType() == PHAssetMediaType::Video;
        Asset {
            id: AssetId::from_raw(asset.localIdentifier().to_string()),
            kind: if video {
                MediaKind::Video
            } else {
                MediaKind::Image
            },
            width: u32::try_from(asset.pixelWidth()).unwrap_or(u32::MAX),
            height: u32::try_from(asset.pixelHeight()).unwrap_or(u32::MAX),
            created_unix_ms: asset
                .creationDate()
                .map(|date| millis(date.timeIntervalSince1970()) as i64),
            duration_ms: video.then(|| millis(asset.duration()) as u64),
            // The following change fills these from PhotoKit.
            modified_unix_ms: None,
            is_favorite: None,
            burst_id: None,
            file_name: None,
            content_type: None,
        }
    }
}

pub(crate) fn thumbnail(id: &AssetId, request: &ThumbnailRequest) -> Result<Thumbnail, PhotoError> {
    refuse_main_thread()?;
    autoreleasepool(|_| {
        ensure_readable()?;
        let ids = NSArray::from_retained_slice(&[NSString::from_str(id.as_str())]);
        // SAFETY: the fetch takes an `NSArray<NSString>` of identifiers and null
        // options; `firstObject` reads the immutable result.
        let asset = unsafe {
            PHAsset::fetchAssetsWithLocalIdentifiers_options(&ids, None)
                .firstObject()
                .ok_or(PhotoError::NotFound)?
        };
        let image = request_image(&asset, request)?;
        rgba_of(&image, request)
    })
}

/// The process-wide image manager.
fn manager() -> &'static Retained<PHCachingImageManager> {
    static MANAGER: OnceLock<Retained<PHCachingImageManager>> = OnceLock::new();
    // SAFETY: `new` on a class `authorization` has confirmed is loaded; the
    // bindings mark the manager `Send + Sync`, so a shared instance is sound.
    MANAGER.get_or_init(|| unsafe { PHCachingImageManager::new() })
}

fn content_mode(mode: ThumbnailMode) -> PHImageContentMode {
    match mode {
        ThumbnailMode::Fill => PHImageContentMode::AspectFill,
        ThumbnailMode::Fit => PHImageContentMode::AspectFit,
    }
}

/// One final image from the manager, or the reason there is none.
fn request_image(
    asset: &PHAsset,
    request: &ThumbnailRequest,
) -> Result<Retained<UIImage>, PhotoError> {
    let manager = manager();
    // SAFETY: `new` and the setters are plain messages to the fresh options
    // object, with declared enum values.
    let options = unsafe {
        let options = PHImageRequestOptions::new();
        options.setDeliveryMode(PHImageRequestOptionsDeliveryMode::HighQualityFormat);
        options.setResizeMode(PHImageRequestOptionsResizeMode::Fast);
        options.setNetworkAccessAllowed(true);
        options.setSynchronous(false);
        options
    };
    let target = TargetSize {
        width: f64::from(request.width),
        height: f64::from(request.height),
    };
    let (tx, rx) = mpsc::channel();
    let handler = RcBlock::new(
        move |image: *mut UIImage, info: *mut NSDictionary<NSString, AnyObject>| {
            // Runs on a Photos queue: never unwind into its frame.
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                // SAFETY: Photos passes null or objects valid for this call;
                // `retain` keeps the image past it and `as_ref` does not outlive
                // the block's body.
                let (image, info) = unsafe { (Retained::retain(image), info.as_ref()) };
                delivered(image, info)
            }))
            .unwrap_or_else(|_| {
                Some(Err(PhotoError::Platform(
                    "panic while receiving a thumbnail".to_owned(),
                )))
            });
            if let Some(answer) = outcome {
                let _ = tx.send(answer);
            }
        },
    );
    // SAFETY: the selector and argument types match `PHImageManager.h` on iOS:
    // a live `PHAsset`, a `CGSize` by value (`TargetSize`), a
    // `PHImageContentMode`, an options object, and a `void (^)(UIImage *,
    // NSDictionary *)` block, which Photos copies; it returns the `int` request
    // id. The block may run on any queue and captures only a `Sender`.
    let id: PHImageRequestID = unsafe {
        msg_send![
            &**manager,
            requestImageForAsset: asset,
            targetSize: target,
            contentMode: content_mode(request.mode),
            options: &*options,
            resultHandler: &*handler
        ]
    };
    match rx.recv_timeout(THUMBNAIL_TIMEOUT) {
        Ok(answer) => answer,
        Err(RecvTimeoutError::Timeout) => {
            // SAFETY: `id` is the live request from this manager; a request that
            // has finished by now ignores the cancel.
            unsafe { manager.cancelImageRequest(id) };
            Err(PhotoError::Platform(
                "the thumbnail request timed out".to_owned(),
            ))
        }
        // The block is gone without answering (only a cancel skips it).
        Err(RecvTimeoutError::Disconnected) => Err(PhotoError::Platform(
            "no thumbnail was delivered".to_owned(),
        )),
    }
}

/// `info[key]` as a `BOOL`, `false` when absent.
fn info_flag(info: &NSDictionary<NSString, AnyObject>, key: &NSString) -> bool {
    info.objectForKey(key).is_some_and(|value| {
        // SAFETY: `PHImageManager.h` declares these values as `NSNumber`, whose
        // `boolValue` takes nothing and returns a `BOOL`.
        unsafe { msg_send![&*value, boolValue] }
    })
}

/// What one handler call means. `None` means wait for a later call.
fn delivered(
    image: Option<Retained<UIImage>>,
    info: Option<&NSDictionary<NSString, AnyObject>>,
) -> Option<Result<Retained<UIImage>, PhotoError>> {
    // SAFETY: reading Photos' constant `NSString` keys.
    let (degraded_key, cancelled_key, error_key) = unsafe {
        (
            PHImageResultIsDegradedKey,
            PHImageCancelledKey,
            PHImageErrorKey,
        )
    };
    if image.is_some() && info.is_some_and(|i| info_flag(i, degraded_key)) {
        // `highQualityFormat` should deliver no degraded frame; a final one
        // follows if it ever does.
        return None;
    }
    if let Some(image) = image {
        return Some(Ok(image));
    }
    if info.is_some_and(|i| info_flag(i, cancelled_key)) {
        return Some(Err(PhotoError::Platform(
            "the thumbnail request was cancelled".to_owned(),
        )));
    }
    let reason = info
        .and_then(|i| i.objectForKey(error_key))
        .and_then(|e| {
            e.downcast_ref::<NSError>()
                .map(|e| format!("photos error {} {}", e.domain(), e.code()))
        })
        .unwrap_or_else(|| "photos returned no image".to_owned());
    Some(Err(PhotoError::Platform(reason)))
}

fn rgba_of(image: &UIImage, request: &ThumbnailRequest) -> Result<Thumbnail, PhotoError> {
    // SAFETY: read-only getters of a `UIImage`; UIKit documents image objects as
    // immutable and safe from any thread, and the binding marks it `Send + Sync`.
    let (bitmap, orientation) = unsafe { (image.CGImage(), image.imageOrientation()) };
    let bitmap =
        bitmap.ok_or_else(|| PhotoError::Platform("the thumbnail has no bitmap".to_owned()))?;
    draw_upright(&bitmap, orientation, request)
}

/// The upright size of a raw `width` x `height` bitmap under `orientation`.
fn upright_size(width: usize, height: usize, orientation: UIImageOrientation) -> (usize, usize) {
    if quarter_turned(orientation) {
        (height, width)
    } else {
        (width, height)
    }
}

fn quarter_turned(orientation: UIImageOrientation) -> bool {
    matches!(
        orientation,
        UIImageOrientation::Left
            | UIImageOrientation::LeftMirrored
            | UIImageOrientation::Right
            | UIImageOrientation::RightMirrored
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    Translate(f64, f64),
    Rotate(f64),
    Scale(f64, f64),
}

/// The context transforms that draw a bitmap upright into a `width` x `height`
/// (upright) context, in order. The raw bitmap is drawn at the origin.
fn orientation_steps(orientation: UIImageOrientation, width: f64, height: f64) -> Vec<Step> {
    use std::f64::consts::{FRAC_PI_2, PI};
    let mut steps = Vec::new();
    match orientation {
        UIImageOrientation::Down | UIImageOrientation::DownMirrored => {
            steps.push(Step::Translate(width, height));
            steps.push(Step::Rotate(PI));
        }
        UIImageOrientation::Left | UIImageOrientation::LeftMirrored => {
            steps.push(Step::Translate(width, 0.0));
            steps.push(Step::Rotate(FRAC_PI_2));
        }
        UIImageOrientation::Right | UIImageOrientation::RightMirrored => {
            steps.push(Step::Translate(0.0, height));
            steps.push(Step::Rotate(-FRAC_PI_2));
        }
        _ => {}
    }
    match orientation {
        UIImageOrientation::UpMirrored | UIImageOrientation::DownMirrored => {
            steps.push(Step::Translate(width, 0.0));
            steps.push(Step::Scale(-1.0, 1.0));
        }
        UIImageOrientation::LeftMirrored | UIImageOrientation::RightMirrored => {
            steps.push(Step::Translate(height, 0.0));
            steps.push(Step::Scale(-1.0, 1.0));
        }
        _ => {}
    }
    steps
}

/// The output bitmap of a thumbnail and how to draw into it.
#[derive(Clone, Debug, PartialEq)]
struct DrawPlan {
    width: u32,
    height: u32,
    /// The context transforms, in order; the raw bitmap is drawn at the origin.
    steps: Vec<Step>,
}

/// How a raw `raw_width` x `raw_height` bitmap under `orientation` becomes the
/// thumbnail `request` asks for, or `None` for an empty bitmap.
///
/// `Fill` outputs exactly the box, the upright image scaled to cover it (a small
/// source scales up) and centred, so the overflow is cropped equally on both
/// sides. `Fit` outputs the upright image scaled to fit, never above its own
/// size. The output comes from the clamped request, which bounds the allocation.
fn draw_plan(
    raw_width: usize,
    raw_height: usize,
    orientation: UIImageOrientation,
    request: &ThumbnailRequest,
) -> Option<DrawPlan> {
    let (upright_width, upright_height) = upright_size(raw_width, raw_height, orientation);
    if upright_width == 0 || upright_height == 0 {
        return None;
    }
    let request = request.clamped();
    let (source_w, source_h) = (upright_width as f64, upright_height as f64);
    let (box_w, box_h) = (f64::from(request.width), f64::from(request.height));
    let (width, height, drawn_w, drawn_h) = match request.mode {
        ThumbnailMode::Fill => {
            let scale = (box_w / source_w).max(box_h / source_h);
            // `max`: a rounding error must not leave the box uncovered.
            let (drawn_w, drawn_h) = ((source_w * scale).max(box_w), (source_h * scale).max(box_h));
            (request.width, request.height, drawn_w, drawn_h)
        }
        ThumbnailMode::Fit => {
            let scale = (box_w / source_w).min(box_h / source_h).min(1.0);
            let width = ((source_w * scale).round() as u32).clamp(1, request.width);
            let height = ((source_h * scale).round() as u32).clamp(1, request.height);
            (width, height, f64::from(width), f64::from(height))
        }
    };
    // The context applies the last step first: orient, then scale, then centre.
    let mut steps = vec![
        Step::Translate(
            (f64::from(width) - drawn_w) / 2.0,
            (f64::from(height) - drawn_h) / 2.0,
        ),
        Step::Scale(drawn_w / source_w, drawn_h / source_h),
    ];
    steps.extend(orientation_steps(orientation, source_w, source_h));
    Some(DrawPlan {
        width,
        height,
        steps,
    })
}

/// Draw `bitmap` upright into a fresh sRGB RGBA context sized by [`draw_plan`]
/// and return the pixels.
fn draw_upright(
    bitmap: &CGImage,
    orientation: UIImageOrientation,
    request: &ThumbnailRequest,
) -> Result<Thumbnail, PhotoError> {
    let (raw_width, raw_height) = (CGImage::width(Some(bitmap)), CGImage::height(Some(bitmap)));
    let DrawPlan {
        width: pixel_width,
        height: pixel_height,
        steps,
    } = draw_plan(raw_width, raw_height, orientation, request)
        .ok_or_else(|| PhotoError::Platform("the thumbnail is empty".to_owned()))?;
    // Both edges are at most `MAX_THUMBNAIL_EDGE`, so none of this can overflow.
    let (width, height) = (pixel_width as usize, pixel_height as usize);
    let stride = width * 4;
    let mut rgba = vec![0u8; stride * height];
    // SAFETY: reading CoreGraphics' constant color-space name.
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
        .ok_or_else(|| PhotoError::Platform("no sRGB color space".to_owned()))?;
    let info =
        CGBitmapInfo(CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0);
    // SAFETY: `rgba` holds exactly `stride * height` writable bytes and outlives
    // the context, which is dropped below before the buffer is read or moved.
    // The parameters describe an 8-bit RGBA layout CoreGraphics supports.
    let context = unsafe {
        CGBitmapContextCreate(
            rgba.as_mut_ptr().cast(),
            width,
            height,
            8,
            stride,
            Some(&space),
            info.0,
        )
    }
    .ok_or_else(|| PhotoError::Platform("cannot create a bitmap context".to_owned()))?;
    // Read before any transform: the clip of a fresh context is its bounds, in
    // the rect type this crate cannot name.
    let mut rect = CGContext::clip_bounding_box(Some(&context));
    rect.origin.x = 0.0;
    rect.origin.y = 0.0;
    rect.size.width = raw_width as f64;
    rect.size.height = raw_height as f64;
    for step in steps {
        match step {
            Step::Translate(x, y) => CGContext::translate_ctm(Some(&context), x, y),
            Step::Rotate(angle) => CGContext::rotate_ctm(Some(&context), angle),
            Step::Scale(x, y) => CGContext::scale_ctm(Some(&context), x, y),
        }
    }
    // Photos often delivers more pixels than the box, so this usually downscales.
    CGContext::set_interpolation_quality(Some(&context), CGInterpolationQuality::High);
    CGContext::draw_image(Some(&context), rect, Some(bitmap));
    drop(context);
    pixels::unpremultiply_rgba8_in_place(&mut rgba);
    Ok(Thumbnail::new(pixel_width, pixel_height, rgba))
}

pub(crate) fn manage_limited_selection() -> Result<(), PhotoError> {
    refuse_main_thread()?;
    autoreleasepool(|_| {
        if authorization()? != PHAuthorizationStatus::Limited {
            return Ok(());
        }
        let _in_flight = PickGuard::acquire()?;
        let (tx, rx) = mpsc::channel();
        let answer = tx.clone();
        DispatchQueue::main().exec_async(move || {
            // SAFETY: the block is submitted to the main queue, which always
            // runs on the process's main thread.
            let mtm = unsafe { MainThreadMarker::new_unchecked() };
            // A panic must not unwind out of a dispatch block.
            let outcome = catch_unwind(AssertUnwindSafe(|| present_limited_picker(mtm, tx)))
                .unwrap_or_else(|_| {
                    Err(PhotoError::Platform(
                        "panic while presenting the limited library picker".to_owned(),
                    ))
                });
            if let Err(e) = outcome {
                let _ = answer.send(Err(e));
            }
        });
        // Every sender gone without an answer reads as a closed sheet.
        rx.recv().unwrap_or(Ok(()))
    })
}

/// Present the picker from the top controller and start its watch (module doc).
fn present_limited_picker(
    mtm: MainThreadMarker,
    tx: Sender<Result<(), PhotoError>>,
) -> Result<(), PhotoError> {
    let selector = sel!(presentLimitedLibraryPickerFromViewController:completionHandler:);
    if PHPhotoLibrary::class().instance_method(selector).is_none() {
        return Err(PhotoError::Platform(
            "PhotosUI is not loaded; link PhotosUI.framework".to_owned(),
        ));
    }
    let presenter = presenter(mtm).ok_or(PhotoError::NoPresenter)?;
    let answered = Arc::new(AtomicBool::new(false));
    let completion = {
        let tx = tx.clone();
        let answered = Arc::clone(&answered);
        RcBlock::new(move |_added: *mut NSArray<NSString>| {
            answered.store(true, Ordering::SeqCst);
            let _ = tx.send(Ok(()));
        })
    };
    // SAFETY: `sharedPhotoLibrary` is a non-null class getter. The selector was
    // checked above; its signature is `PHPhotoLibrary+PhotosUISupport.h`'s (a
    // non-null view controller and a `void (^)(NSArray<NSString *> *)` block,
    // which the method copies; it returns `void`). This runs on the main thread,
    // and the block captures only a `Sender` and an `Arc`.
    let () = unsafe {
        let library = PHPhotoLibrary::sharedPhotoLibrary();
        msg_send![
            &*library,
            presentLimitedLibraryPickerFromViewController: &*presenter,
            completionHandler: &*completion
        ]
    };
    schedule_watch(
        mtm,
        Watch {
            presenter,
            tx,
            answered,
            attached: false,
            started: Instant::now(),
        },
    );
    Ok(())
}

/// What the main-queue watch knows about one limited picker.
struct Watch {
    presenter: Retained<UIViewController>,
    tx: Sender<Result<(), PhotoError>>,
    answered: Arc<AtomicBool>,
    /// The sheet has been seen on the presenter.
    attached: bool,
    started: Instant,
}

/// What one watch tick decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatchStep {
    /// Look again next tick.
    Keep,
    /// The sheet appeared and has gone: the wait ends.
    Closed,
    /// The sheet never appeared within the grace period.
    GaveUp,
}

fn watch_step(was_attached: bool, attached: bool, elapsed: Duration) -> WatchStep {
    match (was_attached, attached) {
        (_, true) => WatchStep::Keep,
        (true, false) => WatchStep::Closed,
        (false, false) if elapsed >= PRESENT_GRACE => WatchStep::GaveUp,
        (false, false) => WatchStep::Keep,
    }
}

impl Watch {
    /// One tick on the main queue; `true` to look again.
    fn tick(&mut self) -> bool {
        if self.answered.load(Ordering::SeqCst) {
            return false;
        }
        let attached = self
            .presenter
            .presentedViewController()
            .is_some_and(|sheet| !sheet.isBeingDismissed());
        let step = watch_step(self.attached, attached, self.started.elapsed());
        self.attached |= attached;
        match step {
            WatchStep::Keep => true,
            WatchStep::Closed => {
                let _ = self.tx.send(Ok(()));
                false
            }
            WatchStep::GaveUp => {
                let _ = self.tx.send(Err(PhotoError::NoPresenter));
                false
            }
        }
    }
}

/// Run the watch's next tick on the main queue after [`WATCH_TICK`].
fn schedule_watch(mtm: MainThreadMarker, watch: Watch) {
    let bound = MainThreadBound::new(watch, mtm);
    let when = DispatchTime::try_from(WATCH_TICK).unwrap_or(DispatchTime::NOW);
    let _ = DispatchQueue::main().after(when, move || {
        // SAFETY: the block is submitted to the main queue, which always runs on
        // the process's main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut watch = bound.into_inner(mtm);
        match catch_unwind(AssertUnwindSafe(|| watch.tick())) {
            Ok(true) => schedule_watch(mtm, watch),
            Ok(false) => {}
            Err(_) => {
                log::error!("photo library: panic while watching the limited picker");
                let _ = watch.tx.send(Err(PhotoError::Platform(
                    "panic while watching the limited library picker".to_owned(),
                )));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRACE: Duration = PRESENT_GRACE;

    #[test]
    fn a_page_is_clamped_to_the_total() {
        assert_eq!(page_bounds(0, 100, 250), 0..100);
        assert_eq!(page_bounds(200, 100, 250), 200..250);
        assert_eq!(page_bounds(300, 100, 250), 250..250);
        assert_eq!(page_bounds(0, 100, 0), 0..0);
        assert_eq!(page_bounds(u64::MAX, u32::MAX, 5), 5..5);
    }

    #[test]
    fn the_predicate_follows_the_filter() {
        assert_eq!(media_predicate(MediaFilter::Images), "mediaType == 1");
        assert_eq!(media_predicate(MediaFilter::Videos), "mediaType == 2");
        assert_eq!(
            media_predicate(MediaFilter::ImagesAndVideos),
            "mediaType == 1 OR mediaType == 2"
        );
    }

    #[test]
    fn the_content_mode_follows_the_request() {
        assert_eq!(
            content_mode(ThumbnailMode::Fill),
            PHImageContentMode::AspectFill
        );
        assert_eq!(
            content_mode(ThumbnailMode::Fit),
            PHImageContentMode::AspectFit
        );
    }

    #[test]
    fn a_watch_keeps_going_until_the_sheet_settles() {
        let early = Duration::from_millis(1);
        assert_eq!(watch_step(false, false, early), WatchStep::Keep);
        assert_eq!(watch_step(false, true, early), WatchStep::Keep);
        assert_eq!(watch_step(true, true, GRACE), WatchStep::Keep);
    }

    #[test]
    fn a_sheet_that_leaves_ends_the_wait() {
        assert_eq!(watch_step(true, false, GRACE), WatchStep::Closed);
        assert_eq!(
            watch_step(true, false, Duration::from_millis(1)),
            WatchStep::Closed
        );
    }

    #[test]
    fn a_sheet_that_never_attaches_gives_up_after_the_grace() {
        assert_eq!(watch_step(false, false, GRACE), WatchStep::GaveUp);
        assert_eq!(
            watch_step(false, false, GRACE + Duration::from_secs(1)),
            WatchStep::GaveUp
        );
    }

    #[test]
    fn upright_size_swaps_for_quarter_turns() {
        for o in [
            UIImageOrientation::Up,
            UIImageOrientation::Down,
            UIImageOrientation::UpMirrored,
            UIImageOrientation::DownMirrored,
        ] {
            assert_eq!(upright_size(30, 20, o), (30, 20));
        }
        for o in [
            UIImageOrientation::Left,
            UIImageOrientation::Right,
            UIImageOrientation::LeftMirrored,
            UIImageOrientation::RightMirrored,
        ] {
            assert_eq!(upright_size(30, 20, o), (20, 30));
        }
    }

    /// Apply `steps` to a point of the raw bitmap, as the context would: each
    /// step prepends to the current transform, so the last step applies first.
    fn apply(steps: &[Step], x: f64, y: f64) -> (f64, f64) {
        steps.iter().rev().fold((x, y), |(x, y), step| match *step {
            Step::Translate(tx, ty) => (x + tx, y + ty),
            Step::Rotate(a) => (x * a.cos() - y * a.sin(), x * a.sin() + y * a.cos()),
            Step::Scale(sx, sy) => (x * sx, y * sy),
        })
    }

    fn near(a: (f64, f64), b: (f64, f64)) {
        assert!(
            (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9,
            "{a:?} != {b:?}"
        );
    }

    #[test]
    fn up_needs_no_transform() {
        assert!(orientation_steps(UIImageOrientation::Up, 30.0, 20.0).is_empty());
    }

    #[test]
    fn down_rotates_about_the_centre() {
        // A 30x20 upright target: the raw origin maps to the far corner.
        let steps = orientation_steps(UIImageOrientation::Down, 30.0, 20.0);
        near(apply(&steps, 0.0, 0.0), (30.0, 20.0));
        near(apply(&steps, 30.0, 20.0), (0.0, 0.0));
    }

    #[test]
    fn up_mirrored_flips_horizontally() {
        let steps = orientation_steps(UIImageOrientation::UpMirrored, 30.0, 20.0);
        near(apply(&steps, 0.0, 0.0), (30.0, 0.0));
        near(apply(&steps, 30.0, 20.0), (0.0, 20.0));
    }

    /// A quarter-turned 30x20 upright target is drawn from a 20x30 raw bitmap,
    /// so all four of its raw corners land on the target's corners.
    fn lands_on_the_target_corners(orientation: UIImageOrientation) {
        let steps = orientation_steps(orientation, 30.0, 20.0);
        let mut mapped: Vec<(i64, i64)> = [(0.0, 0.0), (20.0, 0.0), (0.0, 30.0), (20.0, 30.0)]
            .into_iter()
            .map(|(x, y)| {
                let (x, y) = apply(&steps, x, y);
                (x.round() as i64, y.round() as i64)
            })
            .collect();
        mapped.sort_unstable();
        assert_eq!(
            mapped,
            [(0, 0), (0, 20), (30, 0), (30, 20)],
            "{orientation:?}"
        );
    }

    #[test]
    fn quarter_turns_fill_the_target() {
        for orientation in [
            UIImageOrientation::Left,
            UIImageOrientation::Right,
            UIImageOrientation::LeftMirrored,
            UIImageOrientation::RightMirrored,
        ] {
            lands_on_the_target_corners(orientation);
        }
    }

    fn fill(width: u32, height: u32) -> ThumbnailRequest {
        ThumbnailRequest::new(width, height, ThumbnailMode::Fill)
    }

    fn fit(width: u32, height: u32) -> ThumbnailRequest {
        ThumbnailRequest::new(width, height, ThumbnailMode::Fit)
    }

    fn plan(
        (w, h): (usize, usize),
        orientation: UIImageOrientation,
        request: ThumbnailRequest,
    ) -> DrawPlan {
        draw_plan(w, h, orientation, &request).expect("a non-empty source")
    }

    fn size(plan: &DrawPlan) -> (u32, u32) {
        (plan.width, plan.height)
    }

    /// Where the raw bitmap's corners land: min and max x, then min and max y.
    fn drawn_bounds(plan: &DrawPlan, (w, h): (f64, f64)) -> (f64, f64, f64, f64) {
        let corners =
            [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].map(|(x, y)| apply(&plan.steps, x, y));
        let fold = |pick: fn(&(f64, f64)) -> f64, start: f64, f: fn(f64, f64) -> f64| {
            corners.iter().map(pick).fold(start, f)
        };
        (
            fold(|c| c.0, f64::MAX, f64::min),
            fold(|c| c.0, f64::MIN, f64::max),
            fold(|c| c.1, f64::MAX, f64::min),
            fold(|c| c.1, f64::MIN, f64::max),
        )
    }

    fn assert_bounds(actual: (f64, f64, f64, f64), expected: (f64, f64, f64, f64)) {
        near((actual.0, actual.1), (expected.0, expected.1));
        near((actual.2, actual.3), (expected.2, expected.3));
    }

    #[test]
    fn fill_crops_a_landscape_source_to_the_exact_box() {
        // 4032x3024 scales to 304x228; the 76 spare pixels go 38 to each side.
        let p = plan((4032, 3024), UIImageOrientation::Up, fill(228, 228));
        assert_eq!(size(&p), (228, 228));
        assert_bounds(
            drawn_bounds(&p, (4032.0, 3024.0)),
            (-38.0, 266.0, 0.0, 228.0),
        );
    }

    #[test]
    fn fill_crops_a_portrait_source_to_the_exact_box() {
        let p = plan((3024, 4032), UIImageOrientation::Up, fill(228, 228));
        assert_eq!(size(&p), (228, 228));
        assert_bounds(
            drawn_bounds(&p, (3024.0, 4032.0)),
            (0.0, 228.0, -38.0, 266.0),
        );
    }

    #[test]
    fn fill_scales_a_square_source_without_cropping() {
        let p = plan((1000, 1000), UIImageOrientation::Up, fill(228, 228));
        assert_eq!(size(&p), (228, 228));
        assert_bounds(drawn_bounds(&p, (1000.0, 1000.0)), (0.0, 228.0, 0.0, 228.0));
    }

    #[test]
    fn fill_honours_a_non_square_box() {
        // A 2:1 source in a 1:2 box scales by height and crops the width.
        let p = plan((200, 100), UIImageOrientation::Up, fill(50, 100));
        assert_eq!(size(&p), (50, 100));
        assert_bounds(drawn_bounds(&p, (200.0, 100.0)), (-75.0, 125.0, 0.0, 100.0));
    }

    #[test]
    fn fill_scales_a_small_source_up_to_the_box() {
        let p = plan((40, 20), UIImageOrientation::Up, fill(100, 100));
        assert_eq!(size(&p), (100, 100));
        assert_bounds(drawn_bounds(&p, (40.0, 20.0)), (-50.0, 150.0, 0.0, 100.0));
    }

    #[test]
    fn fit_keeps_a_landscape_source_inside_the_box() {
        let p = plan((4032, 3024), UIImageOrientation::Up, fit(228, 228));
        assert_eq!(size(&p), (228, 171));
        assert_bounds(drawn_bounds(&p, (4032.0, 3024.0)), (0.0, 228.0, 0.0, 171.0));
    }

    #[test]
    fn fit_keeps_a_portrait_source_inside_the_box() {
        let p = plan((3024, 4032), UIImageOrientation::Up, fit(228, 228));
        assert_eq!(size(&p), (171, 228));
        assert_bounds(drawn_bounds(&p, (3024.0, 4032.0)), (0.0, 171.0, 0.0, 228.0));
    }

    #[test]
    fn fit_never_upscales_a_small_source() {
        let p = plan((40, 20), UIImageOrientation::Up, fit(100, 100));
        assert_eq!(size(&p), (40, 20));
        assert_bounds(drawn_bounds(&p, (40.0, 20.0)), (0.0, 40.0, 0.0, 20.0));
    }

    #[test]
    fn fit_gives_an_extreme_panorama_at_least_one_row() {
        let p = plan((10_000, 1), UIImageOrientation::Up, fit(228, 228));
        assert_eq!(size(&p), (228, 1));
    }

    #[test]
    fn a_quarter_turn_is_measured_upright() {
        // The raw 4032x3024 bitmap is a 3024x4032 portrait once upright.
        for orientation in [
            UIImageOrientation::Left,
            UIImageOrientation::Right,
            UIImageOrientation::LeftMirrored,
            UIImageOrientation::RightMirrored,
        ] {
            let p = plan((4032, 3024), orientation, fit(228, 228));
            assert_eq!(size(&p), (171, 228), "{orientation:?}");
            assert_bounds(drawn_bounds(&p, (4032.0, 3024.0)), (0.0, 171.0, 0.0, 228.0));

            let p = plan((4032, 3024), orientation, fill(228, 228));
            assert_eq!(size(&p), (228, 228), "{orientation:?}");
            assert_bounds(
                drawn_bounds(&p, (4032.0, 3024.0)),
                (0.0, 228.0, -38.0, 266.0),
            );
        }
    }

    #[test]
    fn the_output_never_exceeds_the_largest_edge() {
        let raw = ThumbnailRequest {
            width: 99_999,
            height: 99_999,
            mode: ThumbnailMode::Fill,
        };
        let p = draw_plan(4032, 3024, UIImageOrientation::Up, &raw).expect("non-empty");
        assert_eq!(size(&p), (2048, 2048));
    }

    #[test]
    fn an_empty_bitmap_has_no_plan() {
        assert_eq!(draw_plan(0, 10, UIImageOrientation::Up, &fill(8, 8)), None);
        assert_eq!(draw_plan(10, 0, UIImageOrientation::Left, &fit(8, 8)), None);
    }

    #[test]
    fn left_sends_the_raw_origin_to_the_right_edge() {
        // `Left`: the raw bitmap is rotated 90 degrees counterclockwise, so its
        // origin lands at the target's bottom-right in the context's y-up space.
        let steps = orientation_steps(UIImageOrientation::Left, 30.0, 20.0);
        near(apply(&steps, 0.0, 0.0), (30.0, 0.0));
    }
}
