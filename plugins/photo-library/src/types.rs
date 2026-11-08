//! Public value types: what to pick, what came back, and why a call failed.

use std::fmt;
use std::path::PathBuf;

/// Whether a picked item is a still image or a video.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MediaKind {
    /// A still image.
    Image,
    /// A video.
    Video,
}

/// Which kinds of media the picker offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum MediaFilter {
    /// Still images only.
    Images,
    /// Videos only.
    Videos,
    /// Both (the default).
    #[default]
    ImagesAndVideos,
}

/// Options for [`PhotoLibrary::pick`](crate::PhotoLibrary::pick).
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct PickOptions {
    /// Which kinds of media to offer.
    pub filter: MediaFilter,
    /// Maximum number of items; `0` means the platform maximum.
    pub limit: u32,
}

impl PickOptions {
    /// Both kinds of media, platform-maximum selection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the media filter.
    pub fn filter(mut self, filter: MediaFilter) -> Self {
        self.filter = filter;
        self
    }

    /// Sets the selection limit (`0` = platform maximum).
    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = limit;
        self
    }
}

/// An opaque platform asset identifier. `Debug` never prints the value.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AssetId(String);

impl AssetId {
    /// Wraps a platform identifier.
    pub fn from_raw(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// The platform identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AssetId(..)")
    }
}

/// One picked item, readable at [`path`](Self::path).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PickedItem {
    /// A file the app can read.
    pub path: PathBuf,
    /// Image or video.
    pub kind: MediaKind,
    /// The original file name when the platform reports one.
    pub file_name: Option<String>,
    /// The MIME type when known.
    pub mime: Option<String>,
    /// The platform asset identifier when the picker exposes one.
    pub asset_id: Option<AssetId>,
    /// `true` when [`path`](Self::path) is a copy in the app's pick cache (mobile);
    /// `false` when it is the user's original file (desktop).
    pub is_copy: bool,
}

/// Why a photo-library call failed. Display never echoes paths or identifiers.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PhotoError {
    /// Called on the platform UI thread; run it with `frust::spawn_blocking`.
    #[error("blocking photo-library call on the UI thread")]
    UiThread,
    /// The platform glue has not been initialised yet.
    #[error("platform glue not initialised")]
    PlatformNotInitialized,
    /// No foreground screen to present the picker from.
    #[error("no screen to present the picker from")]
    NoPresenter,
    /// The user or the OS denied access.
    #[error("photo access denied")]
    PermissionDenied,
    /// The capability's cargo feature is off.
    #[error("cargo feature `{feature}` is disabled")]
    FeatureDisabled {
        /// The feature that would enable the call.
        feature: &'static str,
    },
    /// This platform has no picker.
    #[error("photo picking is not supported on this platform")]
    Unsupported,
    /// A filesystem error while preparing the pick cache.
    #[error("filesystem error: {0:?}")]
    Io(std::io::ErrorKind),
    /// Any other platform failure.
    #[error("platform error: {0}")]
    Platform(String),
}

impl From<std::io::Error> for PhotoError {
    fn from(e: std::io::Error) -> Self {
        PhotoError::Io(e.kind())
    }
}
