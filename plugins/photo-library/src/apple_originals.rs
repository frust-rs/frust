//! iOS: exporting an asset's original file and saving a file into the library.
//!
//! Both calls block, so both refuse the main thread with
//! [`PhotoError::UiThread`]. Neither takes the pick guard.
//!
//! # Export
//!
//! Export uses `requestDataForAssetResource:options:dataReceivedHandler:completionHandler:`
//! rather than `writeDataForAssetResource:toFile:options:completionHandler:`.
//! Only the former returns a request id for `cancelDataRequest:`, and its
//! documentation settles the ordering question: Photos calls both blocks "on an
//! arbitrary serial queue", and "the complete data for the asset is the
//! concatenation of the `data` parameters from all calls to your `handler`
//! block". Appending each chunk as it arrives is therefore the whole file.
//! `PHAssetResourceManager.h` adds that a chunk does not outlive its handler, so
//! each chunk is written before the handler returns.
//!
//! **Choosing the resource.** `AssetVersion::Original` takes the `Photo` or
//! `Video` resource, `AssetVersion::Current` the `FullSizePhoto` or
//! `FullSizeVideo` one, which exists only when the asset has edits, so an
//! unedited asset falls back to its original. A Live Photo exports its still
//! only; the paired video is never selected.
//!
//! **Network.** `networkAccessAllowed` follows
//! `ExportOptions::allow_network`. With it off, the documentation says a
//! resource that is not on the device completes with "an `NSError` object
//! indicating that the resource requires network access", which `PHError.h`
//! names `PHPhotosErrorNetworkAccessRequired` (3164); that code answers
//! [`PhotoError::NetworkRequired`].
//!
//! **Progress.** `progressHandler` runs only "while downloading", so a file
//! already on the device reports `0.0` and then the final `1.0`. A download's own
//! `1.0` is held back below completion: the final `1.0` is reserved for a
//! finished, successful export.
//!
//! **Cancel and cleanup.** The token's hook calls `cancelDataRequest:`. The wait
//! has no timeout, because an iCloud original can take minutes and the token is
//! the way out; it wakes periodically to notice a cancel or a failed write.
//! No header promises the completion block runs after a cancel, so the wait does
//! not depend on it. The destination is claimed with `create_new`, so an export
//! never replaces an existing file: a taken name gets a `-2`, `-3`, ... suffix.
//! The open file sits behind a lock that the waiter empties before deleting the
//! path, so a chunk arriving afterwards finds no file and is dropped.
//!
//! # Save
//!
//! The OS terminates an app that reaches a photo-library prompt without the
//! matching usage string, so the `Info.plist` key is checked first and a missing
//! one answers [`PhotoError::MissingUsageDescription`]. A save presents no UI of
//! its own; when access is undetermined, Photos may ask, and a refusal comes
//! back through the completion handler as [`PhotoError::PermissionDenied`].
//!
//! An asset alone needs add-only or read-write access. An album also needs
//! read-write access, because add-only access cannot read or change albums, so a
//! caller holding add-only access gets [`PhotoError::PermissionDenied`]. Under
//! limited access the fetch sees only what the user shared, so an existing album
//! may not be found and a new one of the same name is created.
//!
//! The change block and the completion handler both run on Photos' own serial
//! queue. `PHAssetCreationRequest` throws outside a change block, so every change
//! request is made inside it. Its `addResourceWithType:fileURL:options:` reports
//! invalid data through the completion handler, not by throwing.
//!
//! The change-request classes and `performChanges:completionHandler:` are absent
//! from the bindings this crate enables (`PHChangeRequest` and `dispatch2` are
//! off), so they are sent by hand.

use objc2::MainThreadMarker;
use objc2::runtime::AnyClass;
use objc2_photos::PHAuthorizationStatus;

use super::lock;
use crate::PhotoError;

/// `PHPhotosErrorDomain`. The bindings' `PHError` feature is off, so the codes
/// below are spelled here from `PHError.h`.
const PHOTOS_ERROR_DOMAIN: &str = "PHPhotosErrorDomain";

fn refuse_main_thread() -> Result<(), PhotoError> {
    if MainThreadMarker::new().is_some() {
        return Err(PhotoError::UiThread);
    }
    Ok(())
}

/// The bindings panic on a missing class, so a call starts by confirming Photos
/// is linked.
fn require_class(name: &'static std::ffi::CStr) -> Result<&'static AnyClass, PhotoError> {
    AnyClass::get(name)
        .ok_or_else(|| PhotoError::Platform("Photos is not loaded; link Photos.framework".into()))
}

/// Whether a status lets the app use the library at that level.
fn is_granted(status: PHAuthorizationStatus) -> bool {
    status == PHAuthorizationStatus::Authorized || status == PHAuthorizationStatus::Limited
}

#[cfg(feature = "library")]
pub(crate) use export::export_original;
#[cfg(feature = "save")]
pub(crate) use saving::save;

#[cfg(feature = "library")]
mod export {
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, Write};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::path::{Path, PathBuf};
    use std::ptr::NonNull;
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2_foundation::{NSArray, NSData, NSError, NSString};
    use objc2_photos::{
        PHAccessLevel, PHAsset, PHAssetMediaType, PHAssetResource, PHAssetResourceManager,
        PHAssetResourceRequestOptions, PHAssetResourceType, PHInvalidAssetResourceDataRequestID,
        PHPhotoLibrary,
    };

    use super::{PHOTOS_ERROR_DOMAIN, is_granted, lock, refuse_main_thread, require_class};
    use crate::media_types::lookup;
    use crate::originals::{default_dest_dir, is_cancelled, report_fraction, throttled};
    use crate::pick_cache::sanitize_file_name;
    use crate::{AssetId, AssetVersion, ExportOptions, ExportedOriginal, MediaKind, PhotoError};

    /// How often a blocked export looks for a cancel or a failed write.
    const POLL: Duration = Duration::from_millis(100);

    /// `PHPhotosErrorNetworkAccessRequired` (`PHError.h`).
    const NETWORK_ACCESS_REQUIRED: isize = 3164;
    /// `PHPhotosErrorUserCancelled` (`PHError.h`).
    const USER_CANCELLED: isize = 3072;

    /// The highest fraction reported before the export has finished.
    const MAX_IN_FLIGHT: f64 = 0.99;

    /// How many suffixed names to try before giving up on a directory.
    const MAX_NAME_ATTEMPTS: u32 = 10_000;

    /// What the waiting thread shares with the data handler.
    struct Transfer {
        /// The destination; emptied by a failed write or by the waiter.
        file: Mutex<Option<File>>,
        /// The first write failure.
        write_error: Mutex<Option<io::ErrorKind>>,
    }

    impl Transfer {
        /// Appends one chunk. After a failure, or once the waiter has taken the
        /// file, chunks are dropped.
        fn write_chunk(&self, bytes: &[u8]) {
            let mut slot = lock(&self.file);
            let Some(file) = slot.as_mut() else { return };
            if let Err(e) = file.write_all(bytes) {
                *slot = None;
                *lock(&self.write_error) = Some(e.kind());
            }
        }

        fn write_error(&self) -> Option<io::ErrorKind> {
            *lock(&self.write_error)
        }
    }

    pub(crate) fn export_original(
        id: &AssetId,
        options: &ExportOptions,
    ) -> Result<ExportedOriginal, PhotoError> {
        refuse_main_thread()?;
        autoreleasepool(|_| {
            require_class(c"PHPhotoLibrary")?;
            // SAFETY: a class method taking a declared `PHAccessLevel` value and
            // returning the status; it reads state and never prompts.
            let status = unsafe {
                PHPhotoLibrary::authorizationStatusForAccessLevel(PHAccessLevel::ReadWrite)
            };
            if !is_granted(status) {
                return Err(PhotoError::PermissionDenied);
            }
            let ids = NSArray::from_retained_slice(&[NSString::from_str(id.as_str())]);
            // SAFETY: the fetch takes an `NSArray<NSString>` of identifiers and
            // null options; `firstObject` reads the immutable result.
            let asset = unsafe {
                PHAsset::fetchAssetsWithLocalIdentifiers_options(&ids, None)
                    .firstObject()
                    .ok_or(PhotoError::NotFound)?
            };
            // SAFETY: read-only getter of a live `PHAsset`.
            let video = unsafe { asset.mediaType() } == PHAssetMediaType::Video;
            let resource = resource_for(&asset, video, options.version)?;
            // SAFETY: read-only getter of a live `PHAssetResource`.
            let raw_name = unsafe { resource.originalFilename() }.to_string();
            // `contentType` needs a binding feature this crate leaves off.
            // SAFETY: read-only getter of a live `PHAssetResource`.
            #[allow(deprecated)]
            let uti = unsafe { resource.uniformTypeIdentifier() }.to_string();
            let extension = extension_for_uti(&uti);

            let dir = match &options.dest_dir {
                Some(dir) => dir.clone(),
                None => default_dest_dir()?,
            };
            fs::create_dir_all(&dir)?;
            let name = sanitize_file_name(&raw_name, 0, extension.unwrap_or(""));
            let (path, file) = reserve_file(&dir, &name)?;

            let byte_len = match transfer(&resource, file, options) {
                Ok(byte_len) => byte_len,
                Err(e) => {
                    let _ = fs::remove_file(&path);
                    return Err(e);
                }
            };
            Ok(ExportedOriginal {
                path,
                file_name: (!raw_name.is_empty()).then_some(raw_name),
                mime: extension.and_then(lookup).map(|(_, mime)| mime.to_owned()),
                kind: if video {
                    MediaKind::Video
                } else {
                    MediaKind::Image
                },
                byte_len,
            })
        })
    }

    /// The resource to export, picked from the asset's resources.
    fn resource_for(
        asset: &PHAsset,
        video: bool,
        version: AssetVersion,
    ) -> Result<Retained<PHAssetResource>, PhotoError> {
        // SAFETY: the class method takes a live `PHAsset`, and `type` is a
        // read-only getter of each live resource.
        let (resources, types) = unsafe {
            let resources = PHAssetResource::assetResourcesForAsset(asset);
            let types: Vec<PHAssetResourceType> = resources.iter().map(|r| r.r#type()).collect();
            (resources, types)
        };
        choose_resource(&types, video, version)
            .map(|index| resources.objectAtIndex(index))
            .ok_or_else(|| PhotoError::Platform("the asset has no exportable file".to_owned()))
    }

    /// The index of the resource to export. A Live Photo's `PairedVideo` is
    /// never chosen, so it exports its still only.
    fn choose_resource(
        types: &[PHAssetResourceType],
        video: bool,
        version: AssetVersion,
    ) -> Option<usize> {
        use PHAssetResourceType as T;
        let preference: &[T] = match (video, version) {
            (false, AssetVersion::Original) => &[T::Photo, T::AlternatePhoto, T::FullSizePhoto],
            (false, AssetVersion::Current) => &[T::FullSizePhoto, T::Photo, T::AlternatePhoto],
            (true, AssetVersion::Original) => &[T::Video, T::FullSizeVideo],
            (true, AssetVersion::Current) => &[T::FullSizeVideo, T::Video],
        };
        preference
            .iter()
            .find_map(|wanted| types.iter().position(|t| t == wanted))
    }

    /// A file extension in the shared media table for a type identifier.
    fn extension_for_uti(uti: &str) -> Option<&'static str> {
        Some(match uti {
            "public.jpeg" => "jpg",
            "public.png" => "png",
            "public.heic" => "heic",
            "public.heif" => "heif",
            "com.compuserve.gif" => "gif",
            "org.webmproject.webp" => "webp",
            "public.avif" => "avif",
            "com.microsoft.bmp" => "bmp",
            "public.tiff" => "tiff",
            "com.adobe.raw-image" => "dng",
            "com.canon.cr2-raw-image" => "cr2",
            "com.canon.cr3-raw-image" => "cr3",
            "com.nikon.raw-image" => "nef",
            "com.sony.arw-raw-image" => "arw",
            "com.fuji.raw-image" => "raf",
            "com.olympus.raw-image" => "orf",
            "com.panasonic.rw2-raw-image" => "rw2",
            "public.mpeg-4" => "mp4",
            "com.apple.quicktime-movie" => "mov",
            "com.apple.m4v-video" => "m4v",
            "public.3gpp" => "3gp",
            _ => return None,
        })
    }

    /// Claims a new file in `dir`, never replacing one that exists.
    fn reserve_file(dir: &Path, name: &str) -> Result<(PathBuf, File), PhotoError> {
        for attempt in 1..=MAX_NAME_ATTEMPTS {
            let path = dir.join(candidate_name(name, attempt));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok((path, file)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(PhotoError::Io(io::ErrorKind::AlreadyExists))
    }

    /// `name` on the first attempt, then `stem-<attempt>.ext`.
    fn candidate_name(name: &str, attempt: u32) -> String {
        if attempt <= 1 {
            return name.to_owned();
        }
        let path = Path::new(name);
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
        match path.extension().and_then(|e| e.to_str()) {
            Some(ext) => format!("{stem}-{attempt}.{ext}"),
            None => format!("{stem}-{attempt}"),
        }
    }

    /// A fraction safe to report while the export is still running.
    fn in_flight_fraction(fraction: f64) -> f64 {
        if fraction.is_nan() {
            0.0
        } else {
            fraction.min(MAX_IN_FLIGHT)
        }
    }

    fn export_failure(domain: &str, code: isize) -> PhotoError {
        match (domain == PHOTOS_ERROR_DOMAIN, code) {
            (true, NETWORK_ACCESS_REQUIRED) => PhotoError::NetworkRequired,
            (true, USER_CANCELLED) => PhotoError::Cancelled,
            _ => PhotoError::Platform(format!("asset export failed: {domain} {code}")),
        }
    }

    /// Streams `resource` into `file`, returning the bytes written. The caller
    /// deletes the file on an error.
    fn transfer(
        resource: &PHAssetResource,
        file: File,
        options: &ExportOptions,
    ) -> Result<u64, PhotoError> {
        let shared = Arc::new(Transfer {
            file: Mutex::new(Some(file)),
            write_error: Mutex::new(None),
        });
        let throttle = options.progress.clone().map(|f| Arc::new(throttled(f)));
        if let Some(throttle) = &throttle {
            report_fraction(throttle, 0.0);
        }

        let on_progress = throttle.clone().map(|throttle| {
            RcBlock::new(move |fraction: f64| {
                // Runs on a Photos queue: never unwind into its frame.
                if catch_unwind(AssertUnwindSafe(|| {
                    report_fraction(&throttle, in_flight_fraction(fraction));
                }))
                .is_err()
                {
                    log::error!("photo export: panic while reporting progress");
                }
            })
        });
        // SAFETY: `new` and the setters are plain messages to the fresh options
        // object. The progress block is a valid block that the setter copies.
        let request_options = unsafe {
            let request_options = PHAssetResourceRequestOptions::new();
            request_options.setNetworkAccessAllowed(options.allow_network);
            if let Some(block) = &on_progress {
                request_options.setProgressHandler(RcBlock::as_ptr(block));
            }
            request_options
        };

        let on_chunk = {
            let shared = Arc::clone(&shared);
            RcBlock::new(move |data: NonNull<NSData>| {
                // Runs on a Photos queue: never unwind into its frame.
                if catch_unwind(AssertUnwindSafe(|| {
                    // SAFETY: Photos passes a non-null `NSData` valid for this
                    // call and does not mutate it while the handler runs; the
                    // slice is written out before the handler returns.
                    let bytes = unsafe { data.as_ref().as_bytes_unchecked() };
                    shared.write_chunk(bytes);
                }))
                .is_err()
                {
                    log::error!("photo export: panic while writing a chunk");
                }
            })
        };

        let (tx, rx) = mpsc::channel();
        let on_done = RcBlock::new(move |error: *mut NSError| {
            // Runs on a Photos queue: never unwind into its frame.
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                // SAFETY: Photos passes null or an `NSError` valid for this
                // call; the reference does not outlive this closure.
                match unsafe { error.as_ref() } {
                    None => Ok(()),
                    Some(error) => Err(export_failure(&error.domain().to_string(), error.code())),
                }
            }))
            .unwrap_or_else(|_| {
                Err(PhotoError::Platform(
                    "panic while finishing the export".to_owned(),
                ))
            });
            let _ = tx.send(outcome);
        });

        // SAFETY: `defaultManager` is a plain class-method getter, and the
        // request takes a live resource, a live options object and two valid
        // blocks, which Photos copies. Each block runs on Photos' serial queue
        // and captures only `Send` state (an `Arc<Transfer>`, a `Sender`).
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
                "Photos did not start the export".to_owned(),
            ));
        }

        // Installed once the id exists; a token already cancelled runs it here.
        if let Some(token) = &options.cancel {
            let manager = manager.clone();
            // SAFETY: `request` is the id this manager returned, and a request
            // that has already finished ignores the cancel. The manager is
            // `Send + Sync` in the bindings.
            token.set_hook(move || unsafe { manager.cancelDataRequest(request) });
        }
        let outcome = wait_for_completion(&rx, &|| is_cancelled(options), &|| shared.write_error());
        if let Some(token) = &options.cancel {
            token.clear_hook();
        }
        // Checked after the hook is cleared, so a cancel that raced the last
        // chunk still wins.
        let outcome = if is_cancelled(options) {
            Err(PhotoError::Cancelled)
        } else {
            outcome
        };
        if outcome.is_err() {
            // Covers a write failure, which no token cancelled.
            // SAFETY: as for the hook above.
            unsafe { manager.cancelDataRequest(request) };
        }

        let file = lock(&shared.file).take();
        outcome?;
        let byte_len = match file {
            Some(file) => file.metadata()?.len(),
            None => {
                return Err(PhotoError::Platform(
                    "the export finished without a file".to_owned(),
                ));
            }
        };
        if let Some(throttle) = &throttle {
            report_fraction(throttle, 1.0);
        }
        Ok(byte_len)
    }

    /// Waits for the completion handler, waking every [`POLL`] for a cancel or
    /// a failed write. A failed write wins over the handler's own answer: the
    /// answer describes a transfer whose bytes did not all reach the file.
    fn wait_for_completion(
        rx: &Receiver<Result<(), PhotoError>>,
        cancelled: &dyn Fn() -> bool,
        write_error: &dyn Fn() -> Option<io::ErrorKind>,
    ) -> Result<(), PhotoError> {
        loop {
            match rx.recv_timeout(POLL) {
                Ok(outcome) => {
                    return write_error().map_or(outcome, |kind| Err(PhotoError::Io(kind)));
                }
                Err(RecvTimeoutError::Timeout) => {
                    if cancelled() {
                        return Err(PhotoError::Cancelled);
                    }
                    if let Some(kind) = write_error() {
                        return Err(PhotoError::Io(kind));
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(PhotoError::Platform(
                        "Photos dropped the export without answering".to_owned(),
                    ));
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::time::{SystemTime, UNIX_EPOCH};

        use PHAssetResourceType as T;

        fn scratch(tag: &str) -> PathBuf {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!("frust-apple-export-{tag}-{nanos}"));
            fs::create_dir_all(&dir).unwrap();
            dir
        }

        #[test]
        fn an_unedited_photo_exports_its_original_for_either_version() {
            let types = [T::Photo];
            assert_eq!(
                choose_resource(&types, false, AssetVersion::Original),
                Some(0)
            );
            assert_eq!(
                choose_resource(&types, false, AssetVersion::Current),
                Some(0)
            );
        }

        #[test]
        fn an_edited_photo_picks_the_rendition_by_version() {
            let types = [T::AdjustmentData, T::Photo, T::FullSizePhoto];
            assert_eq!(
                choose_resource(&types, false, AssetVersion::Original),
                Some(1)
            );
            assert_eq!(
                choose_resource(&types, false, AssetVersion::Current),
                Some(2)
            );
        }

        #[test]
        fn an_edited_video_picks_the_rendition_by_version() {
            let types = [T::FullSizeVideo, T::Video];
            assert_eq!(
                choose_resource(&types, true, AssetVersion::Original),
                Some(1)
            );
            assert_eq!(
                choose_resource(&types, true, AssetVersion::Current),
                Some(0)
            );
        }

        #[test]
        fn a_live_photo_exports_only_its_still() {
            let types = [T::PairedVideo, T::Photo];
            assert_eq!(
                choose_resource(&types, false, AssetVersion::Original),
                Some(1)
            );
        }

        #[test]
        fn no_matching_resource_is_none() {
            assert_eq!(
                choose_resource(&[T::Audio], false, AssetVersion::Original),
                None
            );
            assert_eq!(
                choose_resource(&[T::Photo], true, AssetVersion::Original),
                None
            );
        }

        #[test]
        fn every_mapped_type_identifier_is_in_the_media_table() {
            for uti in [
                "public.jpeg",
                "public.png",
                "public.heic",
                "public.heif",
                "com.compuserve.gif",
                "org.webmproject.webp",
                "public.avif",
                "com.microsoft.bmp",
                "public.tiff",
                "com.adobe.raw-image",
                "com.canon.cr2-raw-image",
                "com.canon.cr3-raw-image",
                "com.nikon.raw-image",
                "com.sony.arw-raw-image",
                "com.fuji.raw-image",
                "com.olympus.raw-image",
                "com.panasonic.rw2-raw-image",
                "public.mpeg-4",
                "com.apple.quicktime-movie",
                "com.apple.m4v-video",
                "public.3gpp",
            ] {
                let ext = extension_for_uti(uti).unwrap();
                assert!(lookup(ext).is_some(), "{uti}");
            }
            assert_eq!(extension_for_uti("public.data"), None);
        }

        #[test]
        fn candidates_keep_the_extension() {
            assert_eq!(candidate_name("IMG_1.HEIC", 1), "IMG_1.HEIC");
            assert_eq!(candidate_name("IMG_1.HEIC", 2), "IMG_1-2.HEIC");
            assert_eq!(candidate_name("a.tar.gz", 3), "a.tar-3.gz");
            assert_eq!(candidate_name("plain", 2), "plain-2");
        }

        #[test]
        fn a_taken_name_gets_a_suffix_and_keeps_the_old_file() {
            let dir = scratch("reserve");
            let (first, mut file) = reserve_file(&dir, "a.jpg").unwrap();
            file.write_all(b"old").unwrap();
            let (second, _) = reserve_file(&dir, "a.jpg").unwrap();
            assert_eq!(first.file_name().unwrap(), "a.jpg");
            assert_eq!(second.file_name().unwrap(), "a-2.jpg");
            assert_eq!(fs::read(&first).unwrap(), b"old");
            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn a_running_export_never_reports_completion() {
            assert_eq!(in_flight_fraction(0.25), 0.25);
            assert_eq!(in_flight_fraction(1.0), MAX_IN_FLIGHT);
            assert_eq!(in_flight_fraction(7.0), MAX_IN_FLIGHT);
            assert_eq!(in_flight_fraction(f64::NAN), 0.0);
        }

        #[test]
        fn error_codes_map_to_typed_errors() {
            let domain = PHOTOS_ERROR_DOMAIN;
            assert_eq!(
                export_failure(domain, NETWORK_ACCESS_REQUIRED),
                PhotoError::NetworkRequired
            );
            assert_eq!(
                export_failure(domain, USER_CANCELLED),
                PhotoError::Cancelled
            );
            assert!(matches!(
                export_failure(domain, 3169),
                PhotoError::Platform(_)
            ));
            assert!(matches!(
                export_failure("NSCocoaErrorDomain", NETWORK_ACCESS_REQUIRED),
                PhotoError::Platform(_)
            ));
        }

        #[test]
        fn chunks_append_in_order_and_stop_after_the_file_is_taken() {
            let dir = scratch("chunks");
            let path = dir.join("out.bin");
            let transfer = Transfer {
                file: Mutex::new(Some(File::create(&path).unwrap())),
                write_error: Mutex::new(None),
            };
            transfer.write_chunk(b"ab");
            transfer.write_chunk(b"cd");
            let taken = lock(&transfer.file).take();
            transfer.write_chunk(b"ef");
            drop(taken);
            assert_eq!(fs::read(&path).unwrap(), b"abcd");
            assert_eq!(transfer.write_error(), None);
            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn the_completion_answer_is_returned() {
            let (tx, rx) = mpsc::channel();
            tx.send(Err(PhotoError::NetworkRequired)).unwrap();
            assert_eq!(
                wait_for_completion(&rx, &|| false, &|| None),
                Err(PhotoError::NetworkRequired)
            );
        }

        #[test]
        fn a_cancel_ends_the_wait_without_a_completion() {
            let (_tx, rx) = mpsc::channel();
            assert_eq!(
                wait_for_completion(&rx, &|| true, &|| None),
                Err(PhotoError::Cancelled)
            );
        }

        #[test]
        fn a_failed_write_ends_the_wait_and_beats_the_answer() {
            let (_tx, rx) = mpsc::channel();
            let kind = io::ErrorKind::Other;
            assert_eq!(
                wait_for_completion(&rx, &|| false, &|| Some(kind)),
                Err(PhotoError::Io(kind))
            );
            let (tx, rx) = mpsc::channel();
            tx.send(Ok(())).unwrap();
            assert_eq!(
                wait_for_completion(&rx, &|| false, &|| Some(kind)),
                Err(PhotoError::Io(kind))
            );
        }

        #[test]
        fn a_dropped_completion_is_a_platform_error() {
            let (tx, rx) = mpsc::channel::<Result<(), PhotoError>>();
            drop(tx);
            assert!(matches!(
                wait_for_completion(&rx, &|| false, &|| None),
                Err(PhotoError::Platform(_))
            ));
        }
    }
}

#[cfg(feature = "save")]
mod saving {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    use block2::RcBlock;
    use objc2::msg_send;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2_foundation::{NSArray, NSBundle, NSError, NSString, NSURL};
    use objc2_photos::{
        PHAccessLevel, PHAssetCollection, PHAssetCollectionSubtype, PHAssetCollectionType,
        PHAssetResourceCreationOptions, PHAssetResourceType, PHAuthorizationStatus,
        PHObjectPlaceholder, PHPhotoLibrary,
    };

    use super::{PHOTOS_ERROR_DOMAIN, is_granted, lock, refuse_main_thread, require_class};
    use crate::{AssetId, MediaKind, PhotoError, SaveRequest, SavedAsset};

    /// Required to add to the library.
    const ADD_USAGE_KEY: &str = "NSPhotoLibraryAddUsageDescription";
    /// Required for the read-write access an album needs.
    const READ_WRITE_USAGE_KEY: &str = "NSPhotoLibraryUsageDescription";

    /// `PHPhotosErrorAccessRestricted` (`PHError.h`).
    const ACCESS_RESTRICTED: isize = 3310;
    /// `PHPhotosErrorAccessUserDenied` (`PHError.h`).
    const ACCESS_USER_DENIED: isize = 3311;

    pub(crate) fn save(request: &SaveRequest, kind: MediaKind) -> Result<SavedAsset, PhotoError> {
        refuse_main_thread()?;
        autoreleasepool(|_| {
            let wants_album = request.album.is_some();
            let key = usage_key(wants_album);
            if !has_usage_description(key) {
                return Err(PhotoError::MissingUsageDescription { key });
            }
            require_class(c"PHPhotoLibrary")?;
            let creation_class = require_class(c"PHAssetCreationRequest")?;
            let album_class = require_class(c"PHAssetCollectionChangeRequest")?;
            // SAFETY: class methods taking declared `PHAccessLevel` values and
            // returning the status; they read state and never prompt.
            let (read_write, add_only) = unsafe {
                (
                    PHPhotoLibrary::authorizationStatusForAccessLevel(PHAccessLevel::ReadWrite),
                    PHPhotoLibrary::authorizationStatusForAccessLevel(PHAccessLevel::AddOnly),
                )
            };
            check_access(read_write, add_only, wants_album)?;

            let url = NSURL::from_file_path(&request.source)
                .ok_or_else(|| PhotoError::Platform("the source path is not a file URL".into()))?;
            let name = request
                .file_name
                .clone()
                .or_else(|| file_name_of(&request.source));
            // SAFETY: `new` and the setter are plain messages to the fresh
            // options object, and the setter copies the string.
            let options = unsafe {
                let options = PHAssetResourceCreationOptions::new();
                if let Some(name) = &name {
                    options.setOriginalFilename(Some(&NSString::from_str(name)));
                }
                options
            };

            let created = Arc::new(Mutex::new(None));
            let change = AssetChange {
                creation_class,
                album_class,
                resource_type: resource_type(kind),
                url,
                options,
                album: request.album.clone(),
                created: Arc::clone(&created),
            };
            let on_change = RcBlock::new(move || {
                // Runs on a Photos queue: never unwind into its frame.
                if catch_unwind(AssertUnwindSafe(|| change.apply())).is_err() {
                    log::error!("photo save: panic while building the change");
                }
            });

            let (tx, rx) = mpsc::channel();
            let on_done = RcBlock::new(move |success: Bool, error: *mut NSError| {
                // Runs on a Photos queue: never unwind into its frame.
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    if success.as_bool() {
                        return Ok(());
                    }
                    // SAFETY: Photos passes null or an `NSError` valid for this
                    // call; the reference does not outlive this closure.
                    Err(match unsafe { error.as_ref() } {
                        Some(error) => save_failure(&error.domain().to_string(), error.code()),
                        None => PhotoError::Platform("the save failed".to_owned()),
                    })
                }))
                .unwrap_or_else(|_| {
                    Err(PhotoError::Platform(
                        "panic while finishing the save".to_owned(),
                    ))
                });
                let _ = tx.send(outcome);
            });

            // SAFETY: `sharedPhotoLibrary` is a class-method getter on a class
            // checked above. The selector and argument types match
            // `PHPhotoLibrary.h`: a `void (^)(void)` change block and a
            // `void (^)(BOOL, NSError *)` completion block, returning void.
            // Photos copies both blocks and runs them on its serial queue.
            unsafe {
                let library = PHPhotoLibrary::sharedPhotoLibrary();
                let _: () = msg_send![
                    &*library,
                    performChanges: &*on_change,
                    completionHandler: &*on_done
                ];
            }

            // No timeout: Photos may be waiting for the user to answer its own
            // prompt.
            match rx.recv() {
                Ok(Ok(())) => Ok(SavedAsset {
                    id: lock(&created).take().map(AssetId::from_raw),
                }),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(PhotoError::Platform(
                    "Photos dropped the save without answering".to_owned(),
                )),
            }
        })
    }

    fn usage_key(wants_album: bool) -> &'static str {
        if wants_album {
            READ_WRITE_USAGE_KEY
        } else {
            ADD_USAGE_KEY
        }
    }

    fn has_usage_description(key: &str) -> bool {
        NSBundle::mainBundle()
            .objectForInfoDictionaryKey(&NSString::from_str(key))
            .is_some()
    }

    /// Whether a save may go ahead. Undetermined access proceeds, because
    /// Photos then asks the user itself and reports a refusal through the
    /// completion handler. An album with only add-only access is refused: it
    /// needs read-write access, which the app requests through its permission
    /// flow.
    fn check_access(
        read_write: PHAuthorizationStatus,
        add_only: PHAuthorizationStatus,
        wants_album: bool,
    ) -> Result<(), PhotoError> {
        let undetermined = PHAuthorizationStatus::NotDetermined;
        let allowed = if wants_album {
            is_granted(read_write)
                || (read_write == undetermined && add_only != PHAuthorizationStatus::Authorized)
        } else {
            is_granted(read_write) || is_granted(add_only) || add_only == undetermined
        };
        if allowed {
            Ok(())
        } else {
            Err(PhotoError::PermissionDenied)
        }
    }

    fn resource_type(kind: MediaKind) -> PHAssetResourceType {
        match kind {
            MediaKind::Image => PHAssetResourceType::Photo,
            MediaKind::Video => PHAssetResourceType::Video,
        }
    }

    fn file_name_of(path: &std::path::Path) -> Option<String> {
        path.file_name()?.to_str().map(str::to_owned)
    }

    fn save_failure(domain: &str, code: isize) -> PhotoError {
        match (domain == PHOTOS_ERROR_DOMAIN, code) {
            (true, ACCESS_USER_DENIED | ACCESS_RESTRICTED) => PhotoError::PermissionDenied,
            _ => PhotoError::Platform(format!("saving to the library failed: {domain} {code}")),
        }
    }

    /// Everything the change block needs, built before it runs.
    struct AssetChange {
        creation_class: &'static AnyClass,
        album_class: &'static AnyClass,
        resource_type: PHAssetResourceType,
        url: Retained<NSURL>,
        options: Retained<PHAssetResourceCreationOptions>,
        album: Option<String>,
        created: Arc<Mutex<Option<String>>>,
    }

    impl AssetChange {
        /// Runs inside the change block, the only place these requests are
        /// legal.
        fn apply(&self) {
            // SAFETY: called from `performChanges:`'s change block, where
            // `creationRequestForAsset` is valid. The selectors and argument
            // types match `PHAssetCreationRequest.h` and `PHAssetChangeRequest.h`:
            // an `NSInteger`-backed resource type, a live file URL and a live
            // options object, then a nullable placeholder.
            let placeholder: Option<Retained<PHObjectPlaceholder>> = unsafe {
                let request: Retained<AnyObject> =
                    msg_send![self.creation_class, creationRequestForAsset];
                let _: () = msg_send![
                    &*request,
                    addResourceWithType: self.resource_type,
                    fileURL: &*self.url,
                    options: &*self.options
                ];
                msg_send![&*request, placeholderForCreatedAsset]
            };
            let Some(placeholder) = placeholder else {
                log::warn!("photo save: Photos gave no placeholder for the new asset");
                return;
            };
            // SAFETY: a read-only getter of the placeholder Photos just made.
            let id = unsafe { placeholder.localIdentifier() }.to_string();
            *lock(&self.created) = Some(id);
            if let Some(title) = &self.album {
                self.add_to_album(title, placeholder);
            }
        }

        /// Adds the new asset to the album called `title`, creating the album
        /// when none has that name.
        fn add_to_album(&self, title: &str, placeholder: Retained<PHObjectPlaceholder>) {
            let existing = find_album(title);
            // SAFETY: called from the change block. The selectors and argument
            // types match `PHAssetCollectionChangeRequest.h`: a live collection,
            // or an `NSString` title.
            let request: Option<Retained<AnyObject>> = unsafe {
                match &existing {
                    Some(album) => {
                        msg_send![self.album_class, changeRequestForAssetCollection: &**album]
                    }
                    None => {
                        let title = NSString::from_str(title);
                        msg_send![
                            self.album_class,
                            creationRequestForAssetCollectionWithTitle: &*title
                        ]
                    }
                }
            };
            let Some(request) = request else {
                log::warn!("photo save: the album cannot be changed; the asset was saved");
                return;
            };
            let assets = NSArray::from_retained_slice(&[placeholder]);
            // SAFETY: `addAssets:` takes an `id<NSFastEnumeration>`, which
            // `NSArray` conforms to, holding the placeholder from this change
            // block, which is what `PHAssetCollectionChangeRequest.h` accepts.
            let _: () = unsafe { msg_send![&*request, addAssets: &*assets] };
        }
    }

    /// The user album titled `title`, if the current access can see one.
    fn find_album(title: &str) -> Option<Retained<PHAssetCollection>> {
        // SAFETY: the fetch takes declared type and subtype values and null
        // options; `count` and `objectAtIndex:` stay below that immutable
        // result's count; `localizedTitle` is a read-only getter.
        unsafe {
            let albums = PHAssetCollection::fetchAssetCollectionsWithType_subtype_options(
                PHAssetCollectionType::Album,
                PHAssetCollectionSubtype::AlbumRegular,
                None,
            );
            (0..albums.count())
                .map(|index| albums.objectAtIndex(index))
                .find(|album| {
                    album
                        .localizedTitle()
                        .is_some_and(|name| name.to_string() == title)
                })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use PHAuthorizationStatus as S;

        #[test]
        fn the_album_path_needs_the_read_write_usage_string() {
            assert_eq!(usage_key(false), ADD_USAGE_KEY);
            assert_eq!(usage_key(true), READ_WRITE_USAGE_KEY);
        }

        #[test]
        fn a_plain_save_needs_any_granted_or_undetermined_access() {
            assert!(check_access(S::NotDetermined, S::NotDetermined, false).is_ok());
            assert!(check_access(S::Denied, S::NotDetermined, false).is_ok());
            assert!(check_access(S::Denied, S::Authorized, false).is_ok());
            assert!(check_access(S::Limited, S::Denied, false).is_ok());
            assert_eq!(
                check_access(S::Denied, S::Denied, false),
                Err(PhotoError::PermissionDenied)
            );
            assert_eq!(
                check_access(S::NotDetermined, S::Restricted, false),
                Err(PhotoError::PermissionDenied)
            );
        }

        #[test]
        fn an_album_needs_read_write_access() {
            assert!(check_access(S::Authorized, S::NotDetermined, true).is_ok());
            assert!(check_access(S::Limited, S::Authorized, true).is_ok());
            assert!(check_access(S::NotDetermined, S::NotDetermined, true).is_ok());
            assert_eq!(
                check_access(S::NotDetermined, S::Authorized, true),
                Err(PhotoError::PermissionDenied)
            );
            assert_eq!(
                check_access(S::Denied, S::Authorized, true),
                Err(PhotoError::PermissionDenied)
            );
        }

        #[test]
        fn access_errors_map_to_permission_denied() {
            for code in [ACCESS_USER_DENIED, ACCESS_RESTRICTED] {
                assert_eq!(
                    save_failure(PHOTOS_ERROR_DOMAIN, code),
                    PhotoError::PermissionDenied
                );
            }
            assert!(matches!(
                save_failure(PHOTOS_ERROR_DOMAIN, 3305),
                PhotoError::Platform(_)
            ));
            assert!(matches!(
                save_failure("NSCocoaErrorDomain", ACCESS_USER_DENIED),
                PhotoError::Platform(_)
            ));
        }

        #[test]
        fn resource_type_follows_the_kind() {
            assert_eq!(resource_type(MediaKind::Image), PHAssetResourceType::Photo);
            assert_eq!(resource_type(MediaKind::Video), PHAssetResourceType::Video);
        }
    }
}
