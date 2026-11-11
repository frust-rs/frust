//! iOS: the EXIF of one library asset, for
//! [`PhotoLibrary::asset_metadata`](crate::PhotoLibrary::asset_metadata).
//!
//! # No header-only path
//!
//! Photos has no call that returns a library asset's EXIF on its own: `PHAsset` carries only
//! Photos' own fields, and ImageIO reads properties from an image's bytes. So the original's bytes
//! must be on the device. This module reads the asset's original photo resource (the `Photo`
//! resource, else `AlternatePhoto`, else `FullSizePhoto`, as an `AssetVersion::Original` export
//! chooses) with `requestDataForAssetResource:options:dataReceivedHandler:completionHandler:`
//! into memory, never to a file, and hands the bytes to the ImageIO property reader the decoder
//! uses ([`crate::apple_decode::exif_from_data`]). A video has no EXIF ImageIO reads, so it
//! answers an empty [`ExifData`] without a transfer.
//!
//! The chunks arrive "on an arbitrary serial queue", and the complete data is "the concatenation
//! of the `data` parameters from all calls to your `handler` block" (`PHAssetResourceManager.h`),
//! so each chunk is appended as it arrives, before its handler returns. The buffer grows
//! fallibly: a chunk the allocator refuses fails the read rather than aborting.
//!
//! # Network
//!
//! `networkAccessAllowed` follows [`MetadataOptions::allow_network`], off by default. With it off,
//! an original that is only in iCloud completes with `PHPhotosErrorNetworkAccessRequired` (3164),
//! which answers [`PhotoError::NetworkRequired`].
//!
//! # Cancel
//!
//! The crate root has already refused a token cancelled before the call. Once the request has an
//! id, the token's hook calls `cancelDataRequest:`. The wait has no timeout, because a download can
//! take minutes and the token is the way out; it wakes every [`POLL`] to notice a cancel, and the
//! token is checked once more after the wait, so a cancel that races the last chunk still wins.
//! No header promises the completion block runs after a cancel, so the wait does not depend on it.
//!
//! # Access
//!
//! The call blocks, so it refuses the main thread with [`PhotoError::UiThread`]. Read-write or
//! limited access is required ([`PhotoError::PermissionDenied`] otherwise), and an identifier that
//! fetches nothing is [`PhotoError::NotFound`].

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyClass;
use objc2_foundation::{NSArray, NSData, NSError, NSString};
use objc2_photos::{
    PHAccessLevel, PHAsset, PHAssetMediaType, PHAssetResource, PHAssetResourceManager,
    PHAssetResourceRequestOptions, PHAssetResourceType, PHAuthorizationStatus,
    PHInvalidAssetResourceDataRequestID, PHPhotoLibrary,
};

use super::lock;
use crate::{AssetId, ExifData, MetadataOptions, PhotoError};

/// How often a blocked read looks for a cancel.
const POLL: Duration = Duration::from_millis(100);

/// `PHPhotosErrorDomain`. The bindings' `PHError` feature is off, so the codes below are spelled
/// here from `PHError.h`.
const PHOTOS_ERROR_DOMAIN: &str = "PHPhotosErrorDomain";
/// `PHPhotosErrorNetworkAccessRequired` (`PHError.h`).
const NETWORK_ACCESS_REQUIRED: isize = 3164;
/// `PHPhotosErrorUserCancelled` (`PHError.h`).
const USER_CANCELLED: isize = 3072;

/// The bytes received so far, or the reason the read stopped growing them.
#[derive(Default)]
struct Received {
    bytes: Vec<u8>,
    out_of_memory: bool,
}

impl Received {
    /// Appends one chunk; after an allocation failure, chunks are dropped.
    fn append(&mut self, chunk: &[u8]) {
        if self.out_of_memory {
            return;
        }
        if self.bytes.try_reserve(chunk.len()).is_err() {
            self.out_of_memory = true;
            self.bytes = Vec::new();
            return;
        }
        self.bytes.extend_from_slice(chunk);
    }
}

pub(crate) fn asset_metadata(
    id: &AssetId,
    options: &MetadataOptions,
) -> Result<ExifData, PhotoError> {
    if MainThreadMarker::new().is_some() {
        return Err(PhotoError::UiThread);
    }
    autoreleasepool(|_| {
        // The bindings panic on a missing class, so the call starts by confirming Photos is
        // linked.
        AnyClass::get(c"PHPhotoLibrary").ok_or_else(|| {
            PhotoError::Platform("Photos is not loaded; link Photos.framework".into())
        })?;
        // SAFETY: a class method taking a declared `PHAccessLevel` value and returning the
        // status; it reads state and never prompts.
        let status =
            unsafe { PHPhotoLibrary::authorizationStatusForAccessLevel(PHAccessLevel::ReadWrite) };
        if !is_granted(status) {
            return Err(PhotoError::PermissionDenied);
        }
        let ids = NSArray::from_retained_slice(&[NSString::from_str(id.as_str())]);
        // SAFETY: the fetch takes an `NSArray<NSString>` of identifiers and null options;
        // `firstObject` reads the immutable result.
        let asset = unsafe {
            PHAsset::fetchAssetsWithLocalIdentifiers_options(&ids, None)
                .firstObject()
                .ok_or(PhotoError::NotFound)?
        };
        // SAFETY: read-only getter of a live `PHAsset`.
        if unsafe { asset.mediaType() } != PHAssetMediaType::Image {
            return Ok(ExifData::default());
        }
        let resource = photo_resource(&asset)?;
        let bytes = read_resource(&resource, options)?;
        crate::apple_decode::exif_from_data(&bytes)
    })
}

/// Whether a status lets the app read the library.
fn is_granted(status: PHAuthorizationStatus) -> bool {
    status == PHAuthorizationStatus::Authorized || status == PHAuthorizationStatus::Limited
}

/// The asset's original photo resource.
fn photo_resource(asset: &PHAsset) -> Result<Retained<PHAssetResource>, PhotoError> {
    // SAFETY: the class method takes a live `PHAsset`, and `type` is a read-only getter of each
    // live resource.
    let (resources, types) = unsafe {
        let resources = PHAssetResource::assetResourcesForAsset(asset);
        let types: Vec<PHAssetResourceType> = resources.iter().map(|r| r.r#type()).collect();
        (resources, types)
    };
    choose_photo(&types)
        .map(|index| resources.objectAtIndex(index))
        .ok_or_else(|| PhotoError::Platform("the asset has no photo resource".to_owned()))
}

/// The index of the original photo resource. A Live Photo's `PairedVideo` is never chosen.
fn choose_photo(types: &[PHAssetResourceType]) -> Option<usize> {
    use PHAssetResourceType as T;
    [T::Photo, T::AlternatePhoto, T::FullSizePhoto]
        .iter()
        .find_map(|wanted| types.iter().position(|t| t == wanted))
}

/// The typed error for a failed transfer.
fn read_failure(domain: &str, code: isize) -> PhotoError {
    match (domain == PHOTOS_ERROR_DOMAIN, code) {
        (true, NETWORK_ACCESS_REQUIRED) => PhotoError::NetworkRequired,
        (true, USER_CANCELLED) => PhotoError::Cancelled,
        _ => PhotoError::Platform(format!("reading the asset failed: {domain} {code}")),
    }
}

/// Reads all of `resource` into memory (module doc).
fn read_resource(
    resource: &PHAssetResource,
    options: &MetadataOptions,
) -> Result<Vec<u8>, PhotoError> {
    let received = Arc::new(Mutex::new(Received::default()));

    // SAFETY: `new` and the setter are plain messages to the fresh options object.
    let request_options = unsafe {
        let request_options = PHAssetResourceRequestOptions::new();
        request_options.setNetworkAccessAllowed(options.allow_network);
        request_options
    };

    let on_chunk = {
        let received = Arc::clone(&received);
        RcBlock::new(move |data: NonNull<NSData>| {
            // Runs on a Photos queue: never unwind into its frame.
            if catch_unwind(AssertUnwindSafe(|| {
                // SAFETY: Photos passes a non-null `NSData` valid for this call and does not
                // mutate it while the handler runs; the slice is copied before the handler
                // returns.
                let chunk = unsafe { data.as_ref().as_bytes_unchecked() };
                lock(&received).append(chunk);
            }))
            .is_err()
            {
                log::error!("photo metadata: panic while receiving a chunk");
            }
        })
    };

    let (tx, rx) = mpsc::channel();
    let on_done = RcBlock::new(move |error: *mut NSError| {
        // Runs on a Photos queue: never unwind into its frame.
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: Photos passes null or an `NSError` valid for this call; the reference
            // does not outlive this closure.
            match unsafe { error.as_ref() } {
                None => Ok(()),
                Some(error) => Err(read_failure(&error.domain().to_string(), error.code())),
            }
        }))
        .unwrap_or_else(|_| {
            Err(PhotoError::Platform(
                "panic while finishing the read".to_owned(),
            ))
        });
        let _ = tx.send(outcome);
    });

    // SAFETY: `defaultManager` is a plain class-method getter, and the request takes a live
    // resource, a live options object and two valid blocks, which Photos copies. Each block runs
    // on Photos' serial queue and captures only `Send` state (an `Arc<Mutex<_>>`, a `Sender`).
    let (manager, request) = unsafe {
        let manager = PHAssetResourceManager::defaultManager();
        let request = manager
            .requestDataForAssetResource_options_dataReceivedHandler_completionHandler(
                resource,
                Some(&request_options),
                &on_chunk,
                &on_done,
            );
        (manager, request)
    };
    if request == PHInvalidAssetResourceDataRequestID {
        return Err(PhotoError::Platform(
            "Photos did not start the read".to_owned(),
        ));
    }

    // Installed once the id exists; a token already cancelled runs it here.
    if let Some(token) = &options.cancel {
        let manager = manager.clone();
        // SAFETY: `request` is the id this manager returned, and a request that has already
        // finished ignores the cancel. The manager is `Send + Sync` in the bindings.
        token.set_hook(move || unsafe { manager.cancelDataRequest(request) });
    }
    let outcome = wait_for_completion(&rx, &|| options.is_cancelled());
    if let Some(token) = &options.cancel {
        token.clear_hook();
    }
    // Checked after the hook is cleared, so a cancel that raced the last chunk still wins.
    let outcome = if options.is_cancelled() {
        Err(PhotoError::Cancelled)
    } else {
        outcome
    };
    if outcome.is_err() {
        // SAFETY: as for the hook above.
        unsafe { manager.cancelDataRequest(request) };
    }
    outcome?;
    finish(std::mem::take(&mut *lock(&received)))
}

/// The bytes of a transfer that completed without an error.
fn finish(received: Received) -> Result<Vec<u8>, PhotoError> {
    if received.out_of_memory {
        return Err(PhotoError::Platform(
            "not enough memory for the asset's data".to_owned(),
        ));
    }
    Ok(received.bytes)
}

/// Waits for the completion handler, waking every [`POLL`] for a cancel.
fn wait_for_completion(
    rx: &Receiver<Result<(), PhotoError>>,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), PhotoError> {
    loop {
        match rx.recv_timeout(POLL) {
            Ok(outcome) => return outcome,
            Err(RecvTimeoutError::Timeout) => {
                if cancelled() {
                    return Err(PhotoError::Cancelled);
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(PhotoError::Platform(
                    "Photos dropped the read without answering".to_owned(),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PHAssetResourceType as T;

    #[test]
    fn the_original_photo_is_chosen_and_never_the_paired_video() {
        assert_eq!(choose_photo(&[T::Photo]), Some(0));
        assert_eq!(
            choose_photo(&[T::AdjustmentData, T::FullSizePhoto, T::Photo]),
            Some(2)
        );
        assert_eq!(choose_photo(&[T::PairedVideo, T::Photo]), Some(1));
        assert_eq!(choose_photo(&[T::AlternatePhoto]), Some(0));
        assert_eq!(choose_photo(&[T::Video, T::PairedVideo]), None);
    }

    #[test]
    fn error_codes_map_to_typed_errors() {
        let domain = PHOTOS_ERROR_DOMAIN;
        assert_eq!(
            read_failure(domain, NETWORK_ACCESS_REQUIRED),
            PhotoError::NetworkRequired
        );
        assert_eq!(read_failure(domain, USER_CANCELLED), PhotoError::Cancelled);
        assert!(matches!(
            read_failure(domain, 3169),
            PhotoError::Platform(_)
        ));
        assert!(matches!(
            read_failure("NSCocoaErrorDomain", NETWORK_ACCESS_REQUIRED),
            PhotoError::Platform(_)
        ));
    }

    #[test]
    fn chunks_append_in_order_and_a_refused_allocation_fails_the_read() {
        let mut received = Received::default();
        received.append(b"ab");
        received.append(b"cd");
        assert_eq!(finish(received), Ok(b"abcd".to_vec()));

        let mut received = Received {
            out_of_memory: true,
            ..Received::default()
        };
        received.append(b"ef");
        assert!(received.bytes.is_empty());
        assert!(matches!(finish(received), Err(PhotoError::Platform(_))));
    }

    #[test]
    fn the_wait_returns_the_answer_or_stops_on_a_cancel() {
        let (tx, rx) = mpsc::channel();
        tx.send(Err(PhotoError::NetworkRequired)).unwrap();
        assert_eq!(
            wait_for_completion(&rx, &|| false),
            Err(PhotoError::NetworkRequired)
        );
        let (_tx, rx) = mpsc::channel();
        assert_eq!(
            wait_for_completion(&rx, &|| true),
            Err(PhotoError::Cancelled)
        );
        let (tx, rx) = mpsc::channel::<Result<(), PhotoError>>();
        drop(tx);
        assert!(matches!(
            wait_for_completion(&rx, &|| false),
            Err(PhotoError::Platform(_))
        ));
    }
}
