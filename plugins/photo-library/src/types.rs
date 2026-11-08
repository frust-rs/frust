//! Public value types: what to pick, what came back, and why a call failed.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use crate::cancel::CancelToken;

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

/// Loading progress of a pick, delivered to [`PickOptions::on_progress`].
///
/// The first call means the picker has closed and the chosen items are loading. `done` items
/// are finished; `fraction` is `0.0..=1.0` across all items. Callbacks may run on any thread:
/// never call them while holding a lock the callback might re-enter.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct PickProgress {
    /// Number of items chosen.
    pub items: usize,
    /// Number of items finished loading.
    pub done: usize,
    /// Overall progress, `0.0..=1.0`.
    pub fraction: f64,
}

/// Options for [`PhotoLibrary::pick`](crate::PhotoLibrary::pick).
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct PickOptions {
    /// Which kinds of media to offer.
    pub filter: MediaFilter,
    /// Maximum number of items; `0` means the platform maximum.
    pub limit: u32,
    #[allow(dead_code)] // Read by the platform arms.
    pub(crate) progress: Option<Arc<dyn Fn(PickProgress) + Send + Sync>>,
    #[allow(dead_code)] // Read by the platform arms.
    pub(crate) cancel: Option<CancelToken>,
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

    /// Reports loading progress after the picker closes. See [`PickProgress`].
    pub fn on_progress(mut self, f: impl Fn(PickProgress) + Send + Sync + 'static) -> Self {
        self.progress = Some(Arc::new(f));
        self
    }

    /// Lets the caller cancel the pick through `token`.
    pub fn cancel(mut self, token: CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }
}

impl fmt::Debug for PickOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PickOptions")
            .field("filter", &self.filter)
            .field("limit", &self.limit)
            .field("progress", &self.progress.is_some())
            .field("cancel", &self.cancel.is_some())
            .finish()
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
    /// The display name the provider reported, when it reports one.
    ///
    /// Untrusted and unsanitised: for display only, never a path component. The on-disk
    /// [`path`](Self::path) uses a sanitised name.
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
    /// The pick was cancelled through its [`CancelToken`].
    #[error("the pick was cancelled")]
    Cancelled,
    /// The asset or album no longer exists (a stale identifier).
    #[error("asset not found")]
    NotFound,
    /// Another pick is already in progress, or the pick cache is being cleared. On Android this
    /// can briefly follow [`Cancelled`](Self::Cancelled) while the host's copy thread stops.
    #[error("another photo pick is already in progress")]
    Busy,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_and_debug() {
        let o = PickOptions::new();
        assert_eq!(
            format!("{o:?}"),
            "PickOptions { filter: ImagesAndVideos, limit: 0, progress: false, cancel: false }"
        );
        let o = o
            .filter(MediaFilter::Videos)
            .limit(2)
            .on_progress(|_| {})
            .cancel(CancelToken::new());
        assert_eq!(o.filter, MediaFilter::Videos);
        assert_eq!(o.limit, 2);
        assert_eq!(
            format!("{:?}", o.clone()),
            "PickOptions { filter: Videos, limit: 2, progress: true, cancel: true }"
        );
    }
}
