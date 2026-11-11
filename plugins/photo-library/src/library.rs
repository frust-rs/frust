//! Library browsing: albums, paged asset queries and thumbnails.
//!
//! The types here are platform independent; the platform arms fill them. Everything is behind
//! the `library` cargo feature (a code gate): with it off each call returns
//! [`PhotoError::FeatureDisabled`](crate::PhotoError::FeatureDisabled).

use std::fmt;

use crate::types::{AssetId, MediaFilter, MediaKind};

/// Largest page [`AssetQuery::limit`] allows.
pub const MAX_PAGE_LIMIT: u32 = 500;
/// Largest thumbnail edge, in pixels.
pub const MAX_THUMBNAIL_EDGE: u32 = 2048;

const DEFAULT_PAGE_LIMIT: u32 = 100;

/// An opaque platform album identifier. `Debug` never prints the value.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AlbumId(String);

impl AlbumId {
    /// Wraps a platform identifier.
    pub fn from_raw(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// The platform identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AlbumId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AlbumId(..)")
    }
}

/// What an [`Album`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AlbumKind {
    /// Every photo and video the app can see.
    AllMedia,
    /// The user's favourites.
    Favorites,
    /// A system-generated album (screenshots, selfies, ...).
    Smart,
    /// An album the user created.
    User,
    /// A folder-backed collection.
    Folder,
}

/// One album.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Album {
    /// Pass to [`AssetQuery::album`].
    pub id: AlbumId,
    /// The display title. Untrusted: for display only.
    pub title: String,
    /// What kind of album this is.
    pub kind: AlbumKind,
    /// The number of assets when the platform reports it cheaply.
    pub count: Option<u64>,
}

/// One photo or video in the library.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Asset {
    /// Pass to [`PhotoLibrary::thumbnail`](crate::PhotoLibrary::thumbnail).
    pub id: AssetId,
    /// Image or video.
    pub kind: MediaKind,
    /// Pixel width (`0` when unknown).
    pub width: u32,
    /// Pixel height (`0` when unknown).
    pub height: u32,
    /// Creation time, milliseconds since the Unix epoch, when known.
    pub created_unix_ms: Option<i64>,
    /// Video length in milliseconds; `None` for images.
    pub duration_ms: Option<u64>,
    /// Last modification time, milliseconds since the Unix epoch, when known.
    pub modified_unix_ms: Option<i64>,
    /// Whether the user marked the asset a favourite; `None` when the platform does not
    /// report it (Android below API 30).
    pub is_favorite: Option<bool>,
    /// The identifier shared by the shots of one burst, when the asset belongs to one. Android
    /// has no burst identifier, so it is always `None` there.
    pub burst_id: Option<String>,
    /// The original file name. Untrusted: for display only. Filled only when the query set
    /// [`AssetQuery::with_details`].
    pub file_name: Option<String>,
    /// The content type, platform-shaped: a MIME type on Android, a uniform type identifier on
    /// iOS. Filled only when the query set [`AssetQuery::with_details`].
    pub content_type: Option<String>,
}

/// Which assets to list, newest first. Pages with [`offset`](Self::offset) and
/// [`limit`](Self::limit).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AssetQuery {
    /// The album to list; `None` lists the whole library.
    pub album: Option<AlbumId>,
    /// Which kinds of media to include.
    pub filter: MediaFilter,
    /// Number of assets to skip.
    pub offset: u64,
    /// Page size, `1..=500` ([`effective_limit`](Self::effective_limit) clamps a hand-set value).
    pub limit: u32,
    /// Whether to fill [`Asset::file_name`] and [`Asset::content_type`]; off by default.
    pub details: bool,
}

impl Default for AssetQuery {
    fn default() -> Self {
        Self {
            album: None,
            filter: MediaFilter::ImagesAndVideos,
            offset: 0,
            limit: DEFAULT_PAGE_LIMIT,
            details: false,
        }
    }
}

impl AssetQuery {
    /// The whole library, images and videos, first page of 100.
    pub fn new() -> Self {
        Self::default()
    }

    /// Lists one album.
    pub fn album(mut self, album: AlbumId) -> Self {
        self.album = Some(album);
        self
    }

    /// Sets the media filter.
    pub fn filter(mut self, filter: MediaFilter) -> Self {
        self.filter = filter;
        self
    }

    /// Sets how many assets to skip.
    pub fn offset(mut self, offset: u64) -> Self {
        self.offset = offset;
        self
    }

    /// Sets the page size, clamped to `1..=500`.
    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = limit.clamp(1, MAX_PAGE_LIMIT);
        self
    }

    /// Also fills [`Asset::file_name`] and [`Asset::content_type`]. On Android these ride the
    /// same query at no extra round trip; on iOS they need a per-asset resource lookup, so a
    /// page can cost noticeably more per row.
    pub fn with_details(mut self) -> Self {
        self.details = true;
        self
    }

    /// [`limit`](Self::limit) clamped to `1..=500`, for a value set through the field.
    pub fn effective_limit(&self) -> u32 {
        self.limit.clamp(1, MAX_PAGE_LIMIT)
    }
}

/// `DATE_MODIFIED` seconds to milliseconds; a non-positive value is unknown.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn modified_ms_from_seconds(seconds: i64) -> Option<i64> {
    (seconds > 0).then(|| seconds.saturating_mul(1000))
}

/// The host's favourite code: `1` and `0` are answers, anything else (`-1`) is unknown.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn favorite_from_code(code: i32) -> Option<bool> {
    match code {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

/// One page of assets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetPage {
    /// The assets of this page, newest first.
    pub items: Vec<Asset>,
    /// The total number of assets matching the query.
    pub total: u64,
}

/// How a thumbnail fills the requested box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ThumbnailMode {
    /// Cover the box, cropping the overflow.
    Fill,
    /// Fit inside the box, keeping the whole image.
    Fit,
}

/// The size and mode of a thumbnail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThumbnailRequest {
    /// Box width in pixels, `1..=2048`.
    pub width: u32,
    /// Box height in pixels, `1..=2048`.
    pub height: u32,
    /// How the image fills the box.
    pub mode: ThumbnailMode,
}

impl ThumbnailRequest {
    /// A request with both edges clamped to `1..=2048`.
    pub fn new(width: u32, height: u32, mode: ThumbnailMode) -> Self {
        Self {
            width,
            height,
            mode,
        }
        .clamped()
    }

    /// This request with both edges clamped to `1..=2048`.
    pub fn clamped(self) -> Self {
        Self {
            width: self.width.clamp(1, MAX_THUMBNAIL_EDGE),
            height: self.height.clamp(1, MAX_THUMBNAIL_EDGE),
            mode: self.mode,
        }
    }
}

/// Decoded thumbnail pixels: straight-alpha RGBA8, `rgba.len() == width * height * 4`, ready
/// for `ImageSource::from_rgba8`.
#[derive(Clone, PartialEq, Eq)]
pub struct Thumbnail {
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Straight-alpha RGBA8 pixels.
    pub rgba: Vec<u8>,
}

impl Thumbnail {
    /// Wraps pixels; `rgba.len()` must equal `width * height * 4` (debug-asserted).
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        debug_assert_eq!(
            rgba.len() as u64,
            u64::from(width) * u64::from(height) * 4,
            "thumbnail buffer length must be width * height * 4"
        );
        Self {
            width,
            height,
            rgba,
        }
    }
}

impl fmt::Debug for Thumbnail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Thumbnail")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.rgba.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_defaults_and_builders() {
        let q = AssetQuery::new();
        assert_eq!(q.album, None);
        assert_eq!(q.filter, MediaFilter::ImagesAndVideos);
        assert_eq!((q.offset, q.limit), (0, 100));
        let q = q
            .album(AlbumId::from_raw("x"))
            .filter(MediaFilter::Videos)
            .offset(7);
        assert_eq!(q.album.as_ref().map(AlbumId::as_str), Some("x"));
        assert_eq!((q.filter, q.offset), (MediaFilter::Videos, 7));
    }

    #[test]
    fn details_are_off_until_asked_for() {
        assert!(!AssetQuery::new().details);
        assert!(!AssetQuery::default().details);
        let q = AssetQuery::new().limit(5).with_details();
        assert!(q.details);
        assert_eq!(q.limit, 5);
    }

    #[test]
    fn modified_seconds_become_milliseconds() {
        assert_eq!(
            modified_ms_from_seconds(1_700_000_000),
            Some(1_700_000_000_000)
        );
        assert_eq!(modified_ms_from_seconds(0), None);
        assert_eq!(modified_ms_from_seconds(-3), None);
        assert_eq!(modified_ms_from_seconds(i64::MAX), Some(i64::MAX));
    }

    #[test]
    fn favourite_code_maps_and_unknown_is_none() {
        assert_eq!(favorite_from_code(1), Some(true));
        assert_eq!(favorite_from_code(0), Some(false));
        assert_eq!(favorite_from_code(-1), None);
        assert_eq!(favorite_from_code(7), None);
    }

    #[test]
    fn query_limit_is_clamped() {
        assert_eq!(AssetQuery::new().limit(0).limit, 1);
        assert_eq!(AssetQuery::new().limit(9999).limit, 500);
        assert_eq!(AssetQuery::new().limit(42).limit, 42);
        let mut q = AssetQuery::new();
        q.limit = 0;
        assert_eq!(q.effective_limit(), 1);
        q.limit = u32::MAX;
        assert_eq!(q.effective_limit(), 500);
    }

    #[test]
    fn thumbnail_request_is_clamped() {
        let r = ThumbnailRequest::new(0, 99_999, ThumbnailMode::Fit);
        assert_eq!((r.width, r.height), (1, 2048));
        let raw = ThumbnailRequest {
            width: 5000,
            height: 0,
            mode: ThumbnailMode::Fill,
        };
        assert_eq!((raw.clamped().width, raw.clamped().height), (2048, 1));
    }

    #[test]
    fn thumbnail_length_matches_dimensions() {
        let t = Thumbnail::new(3, 2, vec![0; 24]);
        assert_eq!(t.rgba.len(), (t.width * t.height * 4) as usize);
        assert_eq!(
            format!("{t:?}"),
            "Thumbnail { width: 3, height: 2, bytes: 24 }"
        );
    }

    #[test]
    #[should_panic(expected = "width * height * 4")]
    #[cfg(debug_assertions)]
    fn thumbnail_rejects_a_wrong_length_in_debug() {
        let _ = Thumbnail::new(2, 2, vec![0; 3]);
    }

    #[test]
    fn ids_do_not_leak_in_debug() {
        assert_eq!(format!("{:?}", AlbumId::from_raw("secret")), "AlbumId(..)");
    }

    #[cfg(not(feature = "library"))]
    #[test]
    fn calls_are_refused_without_the_feature() {
        use crate::{PhotoError, PhotoLibrary};
        let off = PhotoError::FeatureDisabled { feature: "library" };
        assert_eq!(PhotoLibrary::albums().unwrap_err(), off);
        assert_eq!(PhotoLibrary::assets(&AssetQuery::new()).unwrap_err(), off);
        let req = ThumbnailRequest::new(8, 8, ThumbnailMode::Fill);
        assert_eq!(
            PhotoLibrary::thumbnail(&AssetId::from_raw("a"), &req).unwrap_err(),
            off
        );
        assert_eq!(PhotoLibrary::manage_limited_selection().unwrap_err(), off);
        assert_eq!(
            PhotoLibrary::set_thumbnail_cache_budget(1).unwrap_err(),
            off
        );
    }
}
