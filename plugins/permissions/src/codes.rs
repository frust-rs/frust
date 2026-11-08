//! The frozen contract shared with the Android host, plus the iOS raw-value
//! mappings. Pure functions, testable on any host.

use crate::{Permission, PermissionStatus, PhotoAccess};

/// Android host result code: never asked.
pub const NOT_DETERMINED: i32 = 0;
/// Android host result code: granted.
pub const GRANTED: i32 = 1;
/// Android host result code: partial access.
pub const LIMITED: i32 = 2;
/// Android host result code: denied, rationale available.
pub const DENIED: i32 = 3;
/// Android host result code: denied, only Settings can change it.
pub const BLOCKED: i32 = 4;
/// Android host result code: restricted by policy.
pub const RESTRICTED: i32 = 5;
/// Android host result code: a request is still outstanding.
pub const PENDING: i32 = 6;
/// Android host result code: no foreground Activity.
pub const NO_ACTIVITY: i32 = 7;
/// Android host result code: permission missing from the manifest.
pub const NOT_DECLARED: i32 = 8;

/// Kind code: photo library, read and write.
pub const PHOTOS_READ_WRITE: i32 = 0;
/// Kind code: photo library, add only.
pub const PHOTOS_ADD_ONLY: i32 = 1;
/// Kind code: camera.
pub const CAMERA: i32 = 2;
/// Kind code: microphone.
pub const MICROPHONE: i32 = 3;
/// Kind code: media location.
pub const MEDIA_LOCATION: i32 = 4;

/// The kind code the Android host receives for `p`.
pub fn kind_code(p: Permission) -> i32 {
    match p {
        Permission::Photos(PhotoAccess::ReadWrite) => PHOTOS_READ_WRITE,
        Permission::Photos(PhotoAccess::AddOnly) => PHOTOS_ADD_ONLY,
        Permission::Camera => CAMERA,
        Permission::Microphone => MICROPHONE,
        Permission::MediaLocation => MEDIA_LOCATION,
    }
}

/// Maps an Android host status code. `None` for the non-status codes
/// ([`PENDING`], [`NO_ACTIVITY`], [`NOT_DECLARED`]) and unknown values; an
/// unknown value is never treated as granted.
pub fn status_from_android_code(code: i32) -> Option<PermissionStatus> {
    match code {
        NOT_DETERMINED => Some(PermissionStatus::NotDetermined),
        GRANTED => Some(PermissionStatus::Granted),
        LIMITED => Some(PermissionStatus::Limited),
        DENIED => Some(PermissionStatus::Denied),
        BLOCKED => Some(PermissionStatus::Blocked),
        RESTRICTED => Some(PermissionStatus::Restricted),
        _ => None,
    }
}

/// Maps a `PHAuthorizationStatus` raw value (0 notDetermined, 1 restricted,
/// 2 denied, 3 authorized, 4 limited). Confirmed against the iPhoneOS 27.0 SDK
/// `Photos.framework` headers (`PHAuthorizationStatus` 0..=4; `PHAccessLevel`
/// AddOnly=1, ReadWrite=2). Unknown values map to `Denied`.
pub fn status_from_ph_authorization(raw: isize) -> PermissionStatus {
    match raw {
        0 => PermissionStatus::NotDetermined,
        1 => PermissionStatus::Restricted,
        2 => PermissionStatus::Blocked,
        3 => PermissionStatus::Granted,
        4 => PermissionStatus::Limited,
        _ => PermissionStatus::Denied,
    }
}

/// Maps an `AVAuthorizationStatus` raw value (0 notDetermined, 1 restricted,
/// 2 denied, 3 authorized). Unknown values map to `Denied`.
pub fn status_from_av_authorization(raw: isize) -> PermissionStatus {
    match raw {
        0 => PermissionStatus::NotDetermined,
        1 => PermissionStatus::Restricted,
        2 => PermissionStatus::Blocked,
        3 => PermissionStatus::Granted,
        _ => PermissionStatus::Denied,
    }
}

/// The Info.plist usage-description key an Apple app must carry for `p`;
/// `None` when the permission has no such key.
pub fn usage_key(p: Permission) -> Option<&'static str> {
    match p {
        Permission::Photos(PhotoAccess::ReadWrite) => Some("NSPhotoLibraryUsageDescription"),
        Permission::Photos(PhotoAccess::AddOnly) => Some("NSPhotoLibraryAddUsageDescription"),
        Permission::Camera => Some("NSCameraUsageDescription"),
        Permission::Microphone => Some("NSMicrophoneUsageDescription"),
        Permission::MediaLocation => None,
    }
}

/// The Android manifest permissions behind `p` on a device at `sdk_int`.
/// Photos add-only needs none on API 29+. The sets were checked against
/// developer.android.com and the Pixel 5 / Android 14 device gate.
pub fn android_permissions(p: Permission, sdk_int: i32) -> &'static [&'static str] {
    match p {
        Permission::Photos(PhotoAccess::ReadWrite) => {
            if sdk_int >= 34 {
                &[
                    "android.permission.READ_MEDIA_IMAGES",
                    "android.permission.READ_MEDIA_VIDEO",
                    "android.permission.READ_MEDIA_VISUAL_USER_SELECTED",
                ]
            } else if sdk_int == 33 {
                &[
                    "android.permission.READ_MEDIA_IMAGES",
                    "android.permission.READ_MEDIA_VIDEO",
                ]
            } else {
                &["android.permission.READ_EXTERNAL_STORAGE"]
            }
        }
        Permission::Photos(PhotoAccess::AddOnly) => &[],
        Permission::Camera => &["android.permission.CAMERA"],
        Permission::Microphone => &["android.permission.RECORD_AUDIO"],
        Permission::MediaLocation => &["android.permission.ACCESS_MEDIA_LOCATION"],
    }
}

/// The cargo feature that enables `p`.
pub fn feature_of(p: Permission) -> &'static str {
    match p {
        Permission::Photos(_) => "photos",
        Permission::Camera => "camera",
        Permission::Microphone => "microphone",
        Permission::MediaLocation => "media-location",
    }
}

/// Whether the cargo feature for `p` is on in this build.
pub fn feature_enabled(p: Permission) -> bool {
    match p {
        Permission::Photos(_) => cfg!(feature = "photos"),
        Permission::Camera => cfg!(feature = "camera"),
        Permission::Microphone => cfg!(feature = "microphone"),
        Permission::MediaLocation => cfg!(feature = "media-location"),
    }
}

/// Constants re-exported for the Kotlin contract test.
#[doc(hidden)]
pub mod __contract {
    pub use super::{
        BLOCKED, CAMERA, DENIED, GRANTED, LIMITED, MEDIA_LOCATION, MICROPHONE, NO_ACTIVITY,
        NOT_DECLARED, NOT_DETERMINED, PENDING, PHOTOS_ADD_ONLY, PHOTOS_READ_WRITE, RESTRICTED,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Permission; 5] = [
        Permission::Photos(PhotoAccess::ReadWrite),
        Permission::Photos(PhotoAccess::AddOnly),
        Permission::Camera,
        Permission::Microphone,
        Permission::MediaLocation,
    ];

    #[test]
    fn android_codes_round_trip() {
        let expected = [
            (NOT_DETERMINED, PermissionStatus::NotDetermined),
            (GRANTED, PermissionStatus::Granted),
            (LIMITED, PermissionStatus::Limited),
            (DENIED, PermissionStatus::Denied),
            (BLOCKED, PermissionStatus::Blocked),
            (RESTRICTED, PermissionStatus::Restricted),
        ];
        for (code, status) in expected {
            assert_eq!(status_from_android_code(code), Some(status));
        }
        for code in [PENDING, NO_ACTIVITY, NOT_DECLARED, -1, 99] {
            assert_eq!(status_from_android_code(code), None);
        }
        assert_eq!(
            [
                NOT_DETERMINED,
                GRANTED,
                LIMITED,
                DENIED,
                BLOCKED,
                RESTRICTED,
                PENDING,
                NO_ACTIVITY,
                NOT_DECLARED
            ],
            [0, 1, 2, 3, 4, 5, 6, 7, 8]
        );
    }

    #[test]
    fn kind_codes_are_frozen() {
        let codes: Vec<i32> = ALL.iter().map(|p| kind_code(*p)).collect();
        assert_eq!(codes, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn ph_authorization_table() {
        use PermissionStatus::*;
        let got: Vec<_> = (0..=4).map(status_from_ph_authorization).collect();
        assert_eq!(got, [NotDetermined, Restricted, Blocked, Granted, Limited]);
        assert_eq!(status_from_ph_authorization(5), Denied);
        assert_eq!(status_from_ph_authorization(-1), Denied);
    }

    #[test]
    fn av_authorization_table() {
        use PermissionStatus::*;
        let got: Vec<_> = (0..=3).map(status_from_av_authorization).collect();
        assert_eq!(got, [NotDetermined, Restricted, Blocked, Granted]);
        assert_eq!(status_from_av_authorization(4), Denied);
        assert_eq!(status_from_av_authorization(-1), Denied);
    }

    #[test]
    fn usage_keys() {
        assert_eq!(
            usage_key(Permission::Camera),
            Some("NSCameraUsageDescription")
        );
        assert_eq!(
            usage_key(Permission::Microphone),
            Some("NSMicrophoneUsageDescription")
        );
        assert_eq!(
            usage_key(Permission::Photos(PhotoAccess::ReadWrite)),
            Some("NSPhotoLibraryUsageDescription")
        );
        assert_eq!(
            usage_key(Permission::Photos(PhotoAccess::AddOnly)),
            Some("NSPhotoLibraryAddUsageDescription")
        );
        assert_eq!(usage_key(Permission::MediaLocation), None);
    }

    #[test]
    fn android_permission_sets() {
        let rw = Permission::Photos(PhotoAccess::ReadWrite);
        assert_eq!(
            android_permissions(rw, 29),
            ["android.permission.READ_EXTERNAL_STORAGE"]
        );
        assert_eq!(
            android_permissions(rw, 32),
            ["android.permission.READ_EXTERNAL_STORAGE"]
        );
        assert_eq!(android_permissions(rw, 33).len(), 2);
        assert_eq!(android_permissions(rw, 34).len(), 3);
        assert!(
            android_permissions(rw, 34)
                .contains(&"android.permission.READ_MEDIA_VISUAL_USER_SELECTED")
        );
        assert!(android_permissions(Permission::Photos(PhotoAccess::AddOnly), 29).is_empty());
        assert_eq!(
            android_permissions(Permission::Camera, 33),
            ["android.permission.CAMERA"]
        );
        assert_eq!(
            android_permissions(Permission::Microphone, 33),
            ["android.permission.RECORD_AUDIO"]
        );
        for sdk in [29, 33, 34] {
            assert_eq!(
                android_permissions(Permission::MediaLocation, sdk),
                ["android.permission.ACCESS_MEDIA_LOCATION"]
            );
        }
    }

    #[test]
    fn feature_names() {
        let names: Vec<_> = ALL.iter().map(|p| feature_of(*p)).collect();
        assert_eq!(
            names,
            ["photos", "photos", "camera", "microphone", "media-location"]
        );
    }
}
