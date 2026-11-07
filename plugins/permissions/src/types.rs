//! Public value types: what can be asked for, and what the OS answered.

/// How much of the photo library an app wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PhotoAccess {
    /// Read and write the library.
    ReadWrite,
    /// Add items only; the app never reads the library back.
    AddOnly,
}

/// A permission the app can query or request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Permission {
    /// Photo library access (cargo feature `photos`).
    Photos(PhotoAccess),
    /// Camera capture (cargo feature `camera`).
    Camera,
    /// Microphone capture (cargo feature `microphone`).
    Microphone,
    /// Reading the location already stored inside a photo (cargo feature
    /// `media-location`). It never reads the device position.
    MediaLocation,
}

/// The OS's current answer for a [`Permission`].
///
/// | Variant | iOS / macOS | Android |
/// |---|---|---|
/// | `NotDetermined` | `notDetermined` | never asked |
/// | `Granted` | `authorized` | granted |
/// | `Limited` | `limited` | only `READ_MEDIA_VISUAL_USER_SELECTED` |
/// | `Denied` | (not produced) | denied, the system will still show a rationale |
/// | `Blocked` | `denied` (the OS never re-prompts) | denied after an ask, no rationale: only Settings can change it |
/// | `Restricted` | `restricted` | (not produced) |
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PermissionStatus {
    /// The user has not been asked yet.
    NotDetermined,
    /// Full access.
    Granted,
    /// Partial access (a user-selected subset of photos).
    Limited,
    /// Refused, but a further request may prompt again.
    Denied,
    /// Refused and the OS will not prompt again; use [`crate::open_settings`].
    Blocked,
    /// Forbidden by policy (parental controls, device management).
    Restricted,
}

impl PermissionStatus {
    /// Whether the capability can be used now: [`Granted`](Self::Granted) or
    /// [`Limited`](Self::Limited).
    pub fn is_usable(self) -> bool {
        matches!(self, Self::Granted | Self::Limited)
    }
}

/// Why a permission call failed. No variant's `Display` echoes app data.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PermissionError {
    /// `request` was called on the platform UI thread, which the prompt needs
    /// free. Run it with `frust::spawn_blocking`.
    #[error("request would block the UI thread; run it on a background thread")]
    UiThread,
    /// The platform glue was not initialised before the call.
    #[error("the permissions platform glue is not initialised")]
    PlatformNotInitialized,
    /// The cargo feature for this permission is off.
    #[error("the `{feature}` cargo feature is disabled")]
    FeatureDisabled {
        /// The cargo feature that enables the permission.
        feature: &'static str,
    },
    /// The app bundle lacks the Info.plist usage string the permission needs.
    #[error("the `{key}` usage description is missing from Info.plist")]
    MissingUsageDescription {
        /// The Info.plist key.
        key: &'static str,
    },
    /// The app manifest does not declare the permission.
    #[error("`{permission}` is not declared in the app manifest")]
    NotDeclared {
        /// The Android permission name.
        permission: &'static str,
    },
    /// No foreground Activity is available to show the prompt.
    #[error("no foreground activity is available to show the prompt")]
    NoActivity,
    /// This platform has no permission system behind the API.
    #[error("runtime permissions are not supported on this platform")]
    Unsupported,
    /// The platform reported an error; the detail is available through `Debug`.
    #[error("the platform reported an error")]
    Platform(String),
}
