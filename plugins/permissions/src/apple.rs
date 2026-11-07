//! The Apple arm: `AVCaptureDevice` for camera and microphone,
//! `PHPhotoLibrary` for photos (iOS only), and the system Settings shortcut.
//!
//! # Threading
//!
//! Status reads are plain class-method calls and are safe from any thread.
//! A prompt parks the caller until the user answers, and the alert needs the
//! main thread free to appear, so [`request`] refuses the main thread with
//! [`PermissionError::UiThread`] — but only on the path that would prompt: an
//! already-decided permission returns immediately from any thread. The
//! completion blocks run on an arbitrary queue; they only `try_send` into a
//! one-slot channel and never block or unwind.
//!
//! # Missing usage strings
//!
//! Prompting for a capability whose Info.plist usage string is absent aborts
//! the process. [`request`] therefore reads the key first
//! ([`crate::codes::usage_key`]) and returns
//! [`PermissionError::MissingUsageDescription`] instead. That is the expected
//! outcome of an unbundled macOS `cargo run`.
//!
//! # Photos
//!
//! `Photos.framework` is reached through `msg_send!` on the runtime class, so
//! the crate links nothing extra: an app that enables the `photos` feature
//! links the framework itself (the registry `photos` feature). When the class
//! is absent the call returns [`PermissionError::Platform`], never a panic.
//! macOS reports [`PermissionError::Unsupported`] for photos.
//!
//! # Media location
//!
//! iOS has no separate permission for the location stored in a photo:
//! `PHAsset.location` is governed by photo-library authorization alone (see
//! `PHAsset.h`). The location in a photo is readable exactly when Photos read
//! access is usable, so `MediaLocation` returns the `Photos(ReadWrite)`
//! authorization status (the same read as `status(Photos(ReadWrite))`). It
//! never prompts and never checks a usage key: ask for `Photos` first. macOS
//! reports `Unsupported`, like photos.

use std::sync::mpsc::sync_channel;
use std::time::Duration;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::runtime::Bool;
#[cfg(target_os = "ios")]
use objc2::{rc::Retained, runtime::AnyClass};
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio, AVMediaTypeVideo,
};
use objc2_foundation::{NSBundle, NSString};

use crate::codes;
use crate::{Permission, PermissionError, PermissionStatus, PhotoAccess};

/// How long a prompt may stay unanswered before the status is re-read.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(120);

/// `PHAccessLevelAddOnly` (`PHPhotoLibrary.h`).
#[cfg(target_os = "ios")]
const PH_ACCESS_LEVEL_ADD_ONLY: isize = 1;
/// `PHAccessLevelReadWrite` (`PHPhotoLibrary.h`).
#[cfg(target_os = "ios")]
const PH_ACCESS_LEVEL_READ_WRITE: isize = 2;

pub(crate) fn status(p: Permission) -> Result<PermissionStatus, PermissionError> {
    match p {
        Permission::Camera | Permission::Microphone => av_status(p),
        Permission::Photos(access) => photos_status(access),
        Permission::MediaLocation => media_location(),
    }
}

pub(crate) fn request(p: Permission) -> Result<PermissionStatus, PermissionError> {
    match p {
        Permission::Camera | Permission::Microphone => av_request(p),
        Permission::Photos(access) => photos_request(access),
        Permission::MediaLocation => media_location(),
    }
}

/// The `Photos(ReadWrite)` authorization status, never prompting: the location
/// stored in a photo is readable exactly when Photos read access is usable.
/// macOS has no such permission.
fn media_location() -> Result<PermissionStatus, PermissionError> {
    if cfg!(target_os = "ios") {
        photos_status(PhotoAccess::ReadWrite)
    } else {
        Err(PermissionError::Unsupported)
    }
}

/// Errors with the missing key when the main bundle lacks a non-empty usage
/// string for `p`; permissions without a key pass.
fn precheck_usage(p: Permission) -> Result<(), PermissionError> {
    let Some(key) = codes::usage_key(p) else {
        return Ok(());
    };
    let bundle = NSBundle::mainBundle();
    let value = bundle.objectForInfoDictionaryKey(&NSString::from_str(key));
    let present = value
        .and_then(|v| v.downcast::<NSString>().ok())
        .is_some_and(|s| !s.to_string().trim().is_empty());
    if present {
        Ok(())
    } else {
        Err(PermissionError::MissingUsageDescription { key })
    }
}

fn reject_on_main_thread() -> Result<(), PermissionError> {
    if MainThreadMarker::new().is_none() {
        Ok(())
    } else {
        Err(PermissionError::UiThread)
    }
}

// ---- camera and microphone -------------------------------------------------

fn media_type(p: Permission) -> Result<&'static objc2_av_foundation::AVMediaType, PermissionError> {
    // SAFETY: reading an `extern` AVFoundation constant static (unsafe in
    // edition 2024); linker-provided and non-null wherever AVFoundation is
    // linked.
    let ty = unsafe {
        if p == Permission::Microphone {
            AVMediaTypeAudio
        } else {
            AVMediaTypeVideo
        }
    };
    ty.ok_or_else(|| PermissionError::Platform("AVFoundation media type is unavailable".into()))
}

fn av_status(p: Permission) -> Result<PermissionStatus, PermissionError> {
    let ty = media_type(p)?;
    // SAFETY: `ty` is `AVMediaTypeVideo` or `AVMediaTypeAudio`, the only two
    // media types this class method accepts (any other throws).
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(ty) };
    Ok(av_to_status(status))
}

fn av_to_status(s: AVAuthorizationStatus) -> PermissionStatus {
    codes::status_from_av_authorization(s.0)
}

fn av_request(p: Permission) -> Result<PermissionStatus, PermissionError> {
    let current = av_status(p)?;
    if current != PermissionStatus::NotDetermined {
        return Ok(current);
    }
    precheck_usage(p)?;
    reject_on_main_thread()?;

    let ty = media_type(p)?;
    let (tx, rx) = sync_channel::<()>(1);
    let handler = RcBlock::new(move |_granted: Bool| {
        // A full channel means the receiver timed out already; dropping the
        // late answer is correct and `try_send` never blocks this queue.
        let _ = tx.try_send(());
    });
    // SAFETY: `ty` is `AVMediaTypeVideo` or `AVMediaTypeAudio`; `handler` is a
    // live block of the `void (^)(BOOL)` shape this method calls, and it
    // outlives the wait below, which ends only after the channel resolves or
    // the timeout expires.
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(ty, &handler) };

    // Either way the settled status is the source of truth: re-read it.
    let _ = rx.recv_timeout(PROMPT_TIMEOUT);
    av_status(p)
}

// ---- photos ----------------------------------------------------------------

#[cfg(target_os = "ios")]
fn photo_library() -> Result<&'static AnyClass, PermissionError> {
    AnyClass::get(c"PHPhotoLibrary").ok_or_else(|| {
        PermissionError::Platform(
            "Photos framework not linked - enable the registry photos feature".into(),
        )
    })
}

#[cfg(target_os = "ios")]
fn access_level(access: PhotoAccess) -> isize {
    match access {
        PhotoAccess::AddOnly => PH_ACCESS_LEVEL_ADD_ONLY,
        PhotoAccess::ReadWrite => PH_ACCESS_LEVEL_READ_WRITE,
    }
}

#[cfg(target_os = "ios")]
fn photos_status(access: PhotoAccess) -> Result<PermissionStatus, PermissionError> {
    let cls = photo_library()?;
    let level = access_level(access);
    // SAFETY: `+[PHPhotoLibrary authorizationStatusForAccessLevel:]` takes a
    // `PHAccessLevel` (NSInteger) and returns a `PHAuthorizationStatus`
    // (NSInteger); both are `isize` here. It is a class method with no side
    // effects and no prompt.
    let raw: isize = unsafe { objc2::msg_send![cls, authorizationStatusForAccessLevel: level] };
    Ok(codes::status_from_ph_authorization(raw))
}

#[cfg(target_os = "ios")]
fn photos_request(access: PhotoAccess) -> Result<PermissionStatus, PermissionError> {
    let current = photos_status(access)?;
    if current != PermissionStatus::NotDetermined {
        return Ok(current);
    }
    precheck_usage(Permission::Photos(access))?;
    reject_on_main_thread()?;

    let cls = photo_library()?;
    let level = access_level(access);
    let (tx, rx) = sync_channel::<()>(1);
    let handler = RcBlock::new(move |_status: isize| {
        // See `av_request`: never blocks, never unwinds.
        let _ = tx.try_send(());
    });
    // SAFETY: `+[PHPhotoLibrary requestAuthorizationForAccessLevel:handler:]`
    // takes a `PHAccessLevel` (NSInteger) and a `void (^)(PHAuthorizationStatus)`
    // block, where the status is an NSInteger (`isize`); `handler` has that
    // shape and outlives the wait below. The method returns void.
    unsafe {
        let _: () =
            objc2::msg_send![cls, requestAuthorizationForAccessLevel: level, handler: &*handler];
    }
    let _ = rx.recv_timeout(PROMPT_TIMEOUT);
    photos_status(access)
}

#[cfg(not(target_os = "ios"))]
fn photos_status(_access: PhotoAccess) -> Result<PermissionStatus, PermissionError> {
    Err(PermissionError::Unsupported)
}

#[cfg(not(target_os = "ios"))]
fn photos_request(_access: PhotoAccess) -> Result<PermissionStatus, PermissionError> {
    Err(PermissionError::Unsupported)
}

// ---- settings --------------------------------------------------------------

// `Ok(())` means the open was dispatched to the main queue, not that Settings
// opened; a failed open is logged as a warning (URL kind only).

#[cfg(target_os = "ios")]
pub(crate) fn open_settings() -> Result<(), PermissionError> {
    use dispatch2::DispatchQueue;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSDictionary, NSURL};
    use objc2_ui_kit::{UIApplication, UIApplicationOpenSettingsURLString};

    DispatchQueue::main().exec_async(|| {
        // SAFETY: this closure runs on `dispatch_get_main_queue()`, which
        // always executes on the process's main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        // SAFETY: reading an `extern` UIKit constant static; linker-provided
        // and non-null wherever UIKit is linked.
        let key: &NSString = unsafe { UIApplicationOpenSettingsURLString };
        let Some(url) = NSURL::URLWithString(key) else {
            log::warn!("open_settings: the app-settings URL could not be built");
            return;
        };
        let app = UIApplication::sharedApplication(mtm);
        let options: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::new();
        // SAFETY: `options` is the empty, correctly typed options dictionary
        // the binding's doc requires; `url` is a live `NSURL`; the completion
        // handler is a block of the `void (^)(BOOL)` shape, called once on the
        // main queue; it only logs.
        let done = RcBlock::new(|opened: Bool| {
            if !opened.as_bool() {
                log::warn!("open_settings: the app-settings URL did not open");
            }
        });
        unsafe { app.openURL_options_completionHandler(&url, &options, Some(&done)) };
    });
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn open_settings() -> Result<(), PermissionError> {
    use dispatch2::DispatchQueue;
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSURL;

    // The Privacy & Security pane. Apple documents both forms in the
    // EndpointSecurity header `ESClient.h`: the `.extension` id for macOS 13
    // and later, the older id for earlier releases.
    const PANES: [&str; 2] = [
        "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension",
        "x-apple.systempreferences:com.apple.preference.security",
    ];
    DispatchQueue::main().exec_async(|| {
        let workspace = NSWorkspace::sharedWorkspace();
        for pane in PANES {
            let Some(url) = NSURL::URLWithString(&NSString::from_str(pane)) else {
                continue;
            };
            if workspace.openURL(&url) {
                return;
            }
        }
        log::warn!("open_settings: no Privacy & Security pane URL opened");
    });
    Ok(())
}

#[cfg(not(any(target_os = "ios", target_os = "macos")))]
pub(crate) fn open_settings() -> Result<(), PermissionError> {
    Err(PermissionError::Unsupported)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn unbundled_camera_request_never_aborts() {
        // An unbundled test binary has no usage string, so the precheck
        // answers; an already-decided status is also acceptable.
        match request(Permission::Camera) {
            Err(PermissionError::MissingUsageDescription { key }) => {
                assert_eq!(key, "NSCameraUsageDescription");
            }
            Ok(_) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn photos_and_media_location_are_unsupported_on_macos() {
        let rw = Permission::Photos(PhotoAccess::ReadWrite);
        assert_eq!(status(rw), Err(PermissionError::Unsupported));
        assert_eq!(request(rw), Err(PermissionError::Unsupported));
        assert_eq!(
            status(Permission::MediaLocation),
            Err(PermissionError::Unsupported)
        );
    }

    #[test]
    fn camera_status_is_readable() {
        assert!(status(Permission::Camera).is_ok());
        assert!(status(Permission::Microphone).is_ok());
    }
}
