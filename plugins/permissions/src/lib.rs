//! `frust-permissions`: one runtime-permission API for photos, camera,
//! microphone and media location — Android and Apple behind
//! [`status`], [`request`] and [`open_settings`].
//!
//! # Charter: a platform plugin
//!
//! Like `frust-connectivity` and `frust-camera`, this is a **platform
//! plugin**: it depends on `frust-plugin` plus FFI crates only, and carries no
//! other `frust-*` framework dependency. An app adds it to its own
//! `Cargo.toml` alongside `frust`; the `frust` facade does not re-export it.
//!
//! # Opt-in features
//!
//! Every permission is behind a cargo feature (`photos`, `camera`,
//! `microphone`, `media-location`); none is on by default. An app that does
//! not use a capability then never links the code, and never has to declare
//! or justify that permission to a store. Asking about a permission whose
//! feature is off returns [`PermissionError::FeatureDisabled`] from
//! [`status`] and [`request`] without touching the platform. [`open_settings`]
//! is always available.
//!
//! # Threading
//!
//! - [`status`] never blocks and may be called from any thread.
//! - [`request`] blocks until the OS answers. Called on the platform UI thread
//!   it returns [`PermissionError::UiThread`] before any platform work; pair
//!   it with `frust::spawn_blocking`. A permission that is already decided
//!   returns immediately and is exempt from that guard.
//! - [`open_settings`] is fire-and-forget and may be called from any thread.
//!
//! ```
//! use frust_permissions::{status, Permission, PermissionError};
//!
//! match status(Permission::Camera) {
//!     Ok(s) if s.is_usable() => { /* capture */ }
//!     Ok(_) => { /* ask, or offer open_settings() */ }
//!     Err(PermissionError::FeatureDisabled { .. }) => { /* feature off */ }
//!     Err(_) => { /* unsupported platform or glue missing */ }
//! }
//! ```

pub mod codes;
mod types;

pub use types::{Permission, PermissionError, PermissionStatus, PhotoAccess};

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
use android as platform;

#[cfg(target_vendor = "apple")]
mod apple;
#[cfg(target_vendor = "apple")]
use apple as platform;

#[cfg(not(any(target_os = "android", target_vendor = "apple")))]
mod unsupported;
#[cfg(not(any(target_os = "android", target_vendor = "apple")))]
use unsupported as platform;

fn ensure_enabled(p: Permission) -> Result<(), PermissionError> {
    if codes::feature_enabled(p) {
        Ok(())
    } else {
        Err(PermissionError::FeatureDisabled {
            feature: codes::feature_of(p),
        })
    }
}

/// The OS's current answer for `p`. Never blocks; any thread.
pub fn status(p: Permission) -> Result<PermissionStatus, PermissionError> {
    ensure_enabled(p)?;
    platform::status(p)
}

/// Asks the OS for `p` and waits for the answer.
///
/// Returns [`PermissionError::UiThread`] on the platform UI thread unless the
/// permission is already decided. Run it with `frust::spawn_blocking`.
pub fn request(p: Permission) -> Result<PermissionStatus, PermissionError> {
    ensure_enabled(p)?;
    platform::request(p)
}

/// Opens the app's page in the system Settings. Fire-and-forget; any thread.
pub fn open_settings() -> Result<(), PermissionError> {
    platform::open_settings()
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
    fn disabled_features_name_themselves() {
        for p in ALL {
            let expected = PermissionError::FeatureDisabled {
                feature: codes::feature_of(p),
            };
            if codes::feature_enabled(p) {
                assert_ne!(status(p), Err(expected.clone()));
                assert_ne!(request(p), Err(expected));
            } else {
                assert_eq!(status(p), Err(expected.clone()));
                assert_eq!(request(p), Err(expected));
            }
        }
    }

    #[test]
    fn enabled_permissions_reach_the_platform_arm() {
        for p in ALL.into_iter().filter(|p| codes::feature_enabled(*p)) {
            assert_eq!(status(p), Err(PermissionError::Unsupported));
        }
        assert_eq!(open_settings(), Err(PermissionError::Unsupported));
    }

    #[test]
    fn usable_statuses() {
        assert!(PermissionStatus::Granted.is_usable());
        assert!(PermissionStatus::Limited.is_usable());
        for s in [
            PermissionStatus::NotDetermined,
            PermissionStatus::Denied,
            PermissionStatus::Blocked,
            PermissionStatus::Restricted,
        ] {
            assert!(!s.is_usable());
        }
    }

    #[test]
    fn display_carries_no_caller_data() {
        let secret = "SECRET-APP-DATA";
        let errors = [
            PermissionError::UiThread,
            PermissionError::PlatformNotInitialized,
            PermissionError::FeatureDisabled { feature: "photos" },
            PermissionError::MissingUsageDescription {
                key: "NSCameraUsageDescription",
            },
            PermissionError::NotDeclared {
                permission: "android.permission.CAMERA",
            },
            PermissionError::NoActivity,
            PermissionError::Unsupported,
            PermissionError::Platform(secret.to_string()),
        ];
        for e in errors {
            assert!(!e.to_string().contains(secret));
        }
    }
}
