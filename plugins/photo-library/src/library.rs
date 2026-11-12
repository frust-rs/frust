//! Library browsing: albums, paged asset queries and thumbnails.
//!
//! The types here are platform independent; the platform arms fill them. Everything is behind
//! the `library` cargo feature (a code gate): with it off each call returns
//! [`PhotoError::FeatureDisabled`](crate::PhotoError::FeatureDisabled).

use std::fmt;

use crate::types::{AssetId, MediaFilter, MediaKind, PhotoError};

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
    /// iOS. Filled only when the query set [`AssetQuery::with_details`]. For a MIME type on both
    /// phones, use [`Asset::mime_type`].
    pub content_type: Option<String>,
}

impl Asset {
    /// The asset's MIME type on both phones, from [`content_type`](Self::content_type).
    ///
    /// A `content_type` that already contains a `/` is a MIME type and is returned unchanged.
    /// An iOS uniform type identifier is mapped through a fixed table covering the common image,
    /// raw and video identifiers. An identifier the table does not know gives `None` rather than
    /// a guess, and so does a missing `content_type` (it is only filled with
    /// [`AssetQuery::with_details`]).
    pub fn mime_type(&self) -> Option<String> {
        let content_type = self.content_type.as_deref()?;
        if content_type.contains('/') {
            return Some(content_type.to_owned());
        }
        crate::media_types::mime_for_uti(content_type).map(str::to_owned)
    }
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

/// How many iOS cursor sessions keep their library snapshot; the least recently used is evicted
/// when another session starts.
#[cfg_attr(not(all(feature = "library", target_os = "ios")), allow(dead_code))]
pub(crate) const SNAPSHOT_SESSIONS: usize = 4;

/// A cursor used with a query whose album or filter differs from the one that made it.
const CURSOR_OTHER_QUERY: &str =
    "the cursor belongs to a query with another album or filter; restart paging from None";
/// A cursor made by another platform's paging scheme.
const CURSOR_OTHER_PLATFORM: &str =
    "the cursor was not made by this platform's library; restart paging from None";
/// An iOS cursor whose snapshot was evicted from the session store.
#[cfg_attr(not(all(feature = "library", target_os = "ios")), allow(dead_code))]
pub(crate) const CURSOR_EXPIRED: &str =
    "the cursor's library snapshot is no longer held; restart paging from None";

/// Where an [`AssetCursor`] resumes. Each platform makes and accepts only its own variant.
/// `Debug` prints the position; [`AssetCursor`]'s own `Debug` never does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(all(feature = "library", any(target_os = "android", target_os = "ios"))),
    allow(dead_code)
)]
pub(crate) enum CursorPosition {
    /// Android: the sort key and `_ID` of the last row returned.
    Keyset {
        /// The row's timestamp key, in milliseconds.
        key: i64,
        /// The row's `MediaStore` `_ID`.
        id: i64,
    },
    /// iOS: the index of the next asset in snapshot `session`.
    Snapshot {
        /// The snapshot's id in the session store.
        session: u64,
        /// The first index of the next page.
        index: u64,
    },
}

/// The part of a query a cursor is bound to: the parts that decide which assets match.
#[derive(Clone, PartialEq, Eq)]
struct CursorScope {
    album: Option<AlbumId>,
    filter: MediaFilter,
}

impl CursorScope {
    fn of(query: &AssetQuery) -> Self {
        Self {
            album: query.album.clone(),
            filter: query.filter,
        }
    }
}

/// Where [`PhotoLibrary::assets_after`](crate::PhotoLibrary::assets_after) resumes: the
/// `next` of an [`AssetCursorPage`].
///
/// Opaque and in-memory only: it cannot be persisted, and is meaningful only to the query
/// (album and filter) and the platform that made it. `Debug` prints no position.
#[derive(Clone, PartialEq, Eq)]
pub struct AssetCursor {
    scope: CursorScope,
    position: CursorPosition,
}

#[cfg_attr(
    not(all(feature = "library", any(target_os = "android", target_os = "ios"))),
    allow(dead_code)
)]
impl AssetCursor {
    /// A cursor resuming `query` at `position`.
    pub(crate) fn new(query: &AssetQuery, position: CursorPosition) -> Self {
        Self {
            scope: CursorScope::of(query),
            position,
        }
    }

    /// The position to resume `query` at, or [`PhotoError::InvalidRequest`] when the cursor
    /// was made for a query with another album or filter.
    pub(crate) fn position_for(&self, query: &AssetQuery) -> Result<CursorPosition, PhotoError> {
        if self.scope != CursorScope::of(query) {
            return Err(PhotoError::InvalidRequest {
                reason: CURSOR_OTHER_QUERY,
            });
        }
        Ok(self.position)
    }

    /// The Android keyset `(key, id)` to resume `query` after.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub(crate) fn keyset_for(&self, query: &AssetQuery) -> Result<(i64, i64), PhotoError> {
        match self.position_for(query)? {
            CursorPosition::Keyset { key, id } => Ok((key, id)),
            CursorPosition::Snapshot { .. } => Err(other_platform()),
        }
    }

    /// The iOS `(session, index)` to resume `query` at.
    #[cfg_attr(not(target_os = "ios"), allow(dead_code))]
    pub(crate) fn snapshot_for(&self, query: &AssetQuery) -> Result<(u64, u64), PhotoError> {
        match self.position_for(query)? {
            CursorPosition::Snapshot { session, index } => Ok((session, index)),
            CursorPosition::Keyset { .. } => Err(other_platform()),
        }
    }
}

fn other_platform() -> PhotoError {
    PhotoError::InvalidRequest {
        reason: CURSOR_OTHER_PLATFORM,
    }
}

impl fmt::Debug for AssetCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AssetCursor(..)")
    }
}

/// One page of [`PhotoLibrary::assets_after`](crate::PhotoLibrary::assets_after).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AssetCursorPage {
    /// The assets of this page, newest first.
    pub items: Vec<Asset>,
    /// Where the next page starts; `None` on the last page.
    pub next: Option<AssetCursor>,
}

/// An Android keyset page: `more` says whether the host saw a row past this page, and
/// `(last_key, last_id)` is the sort key and `_ID` of the page's last row.
#[cfg_attr(not(all(feature = "library", target_os = "android")), allow(dead_code))]
pub(crate) fn keyset_page(
    query: &AssetQuery,
    items: Vec<Asset>,
    more: bool,
    last_key: i64,
    last_id: i64,
) -> AssetCursorPage {
    let next = (more && !items.is_empty()).then(|| {
        AssetCursor::new(
            query,
            CursorPosition::Keyset {
                key: last_key,
                id: last_id,
            },
        )
    });
    AssetCursorPage { items, next }
}

/// The cursor after an iOS page that ended at index `end` of a `total`-asset snapshot, or
/// `None` when that page was the last.
#[cfg_attr(not(all(feature = "library", target_os = "ios")), allow(dead_code))]
pub(crate) fn snapshot_next(
    query: &AssetQuery,
    session: u64,
    end: usize,
    total: usize,
) -> Option<AssetCursor> {
    (end < total).then(|| {
        AssetCursor::new(
            query,
            CursorPosition::Snapshot {
                session,
                index: end as u64,
            },
        )
    })
}

/// A bounded store of iOS cursor-session snapshots, keyed by a session id that is never
/// reused. Inserting past the capacity evicts the least recently used session; a lookup marks
/// its session most recently used.
#[cfg_attr(not(all(feature = "library", target_os = "ios")), allow(dead_code))]
pub(crate) struct SnapshotStore<T> {
    capacity: usize,
    next_session: u64,
    /// Least recently used first.
    entries: Vec<(u64, T)>,
}

#[cfg_attr(not(all(feature = "library", target_os = "ios")), allow(dead_code))]
impl<T: Clone> SnapshotStore<T> {
    /// An empty store holding at most `capacity` sessions (at least one).
    pub(crate) const fn new(capacity: usize) -> Self {
        Self {
            capacity: if capacity == 0 { 1 } else { capacity },
            next_session: 1,
            entries: Vec::new(),
        }
    }

    /// Stores `snapshot` under a fresh session id, evicting the least recently used session
    /// when the store is full.
    pub(crate) fn insert(&mut self, snapshot: T) -> u64 {
        let session = self.next_session;
        self.next_session = self.next_session.wrapping_add(1);
        while self.entries.len() >= self.capacity {
            self.entries.remove(0);
        }
        self.entries.push((session, snapshot));
        session
    }

    /// The snapshot of `session`, marked most recently used, or `None` once it was evicted.
    pub(crate) fn get(&mut self, session: u64) -> Option<T> {
        let at = self.entries.iter().position(|(id, _)| *id == session)?;
        let entry = self.entries.remove(at);
        let snapshot = entry.1.clone();
        self.entries.push(entry);
        Some(snapshot)
    }

    /// How many sessions are held.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
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

    fn asset(id: &str) -> Asset {
        Asset {
            id: AssetId::from_raw(id),
            kind: MediaKind::Image,
            width: 1,
            height: 1,
            created_unix_ms: None,
            duration_ms: None,
            modified_unix_ms: None,
            is_favorite: None,
            burst_id: None,
            file_name: None,
            content_type: None,
        }
    }

    #[test]
    fn a_cursor_round_trips_its_position() {
        let query = AssetQuery::new().album(AlbumId::from_raw("a"));
        let keyset = CursorPosition::Keyset { key: -5, id: 42 };
        let cursor = AssetCursor::new(&query, keyset);
        assert_eq!(cursor.position_for(&query), Ok(keyset));
        assert_eq!(cursor.keyset_for(&query), Ok((-5, 42)));
        let snapshot = CursorPosition::Snapshot {
            session: 3,
            index: 200,
        };
        let cursor = AssetCursor::new(&query, snapshot);
        assert_eq!(cursor.snapshot_for(&query), Ok((3, 200)));
        assert_eq!(cursor.clone(), cursor);
    }

    #[test]
    fn a_cursor_ignores_offset_limit_and_details() {
        let query = AssetQuery::new().filter(MediaFilter::Videos);
        let cursor = AssetCursor::new(&query, CursorPosition::Keyset { key: 1, id: 2 });
        let paged = query.clone().offset(900).limit(7).with_details();
        assert_eq!(cursor.keyset_for(&paged), Ok((1, 2)));
    }

    #[test]
    fn a_cursor_from_another_query_is_an_invalid_request() {
        let query = AssetQuery::new().album(AlbumId::from_raw("a"));
        let cursor = AssetCursor::new(&query, CursorPosition::Keyset { key: 1, id: 2 });
        let other_query = PhotoError::InvalidRequest {
            reason: CURSOR_OTHER_QUERY,
        };
        for other in [
            AssetQuery::new(),
            AssetQuery::new().album(AlbumId::from_raw("b")),
            query.clone().filter(MediaFilter::Images),
        ] {
            assert_eq!(cursor.keyset_for(&other), Err(other_query.clone()));
            assert_eq!(cursor.snapshot_for(&other), Err(other_query.clone()));
        }
    }

    #[test]
    fn a_cursor_from_another_platform_is_an_invalid_request() {
        let query = AssetQuery::new();
        let android = AssetCursor::new(&query, CursorPosition::Keyset { key: 1, id: 2 });
        let ios = AssetCursor::new(
            &query,
            CursorPosition::Snapshot {
                session: 1,
                index: 0,
            },
        );
        assert_eq!(android.snapshot_for(&query), Err(other_platform()));
        assert_eq!(ios.keyset_for(&query), Err(other_platform()));
    }

    #[test]
    fn a_cursor_prints_no_position() {
        let query = AssetQuery::new().album(AlbumId::from_raw("secret"));
        let cursor = AssetCursor::new(&query, CursorPosition::Keyset { key: 77, id: 99 });
        assert_eq!(format!("{cursor:?}"), "AssetCursor(..)");
        let page = AssetCursorPage {
            items: Vec::new(),
            next: Some(cursor),
        };
        let printed = format!("{page:?}");
        assert!(!printed.contains("77") && !printed.contains("secret"));
    }

    #[test]
    fn a_keyset_page_has_a_next_only_when_more_rows_follow() {
        let query = AssetQuery::new();
        let page = keyset_page(&query, vec![asset("a"), asset("b")], true, 10, 4);
        let next = page.next.expect("more rows follow");
        assert_eq!(next.keyset_for(&query), Ok((10, 4)));

        // The last page, full or short, ends the session.
        let last = keyset_page(&query, vec![asset("a"), asset("b")], false, 10, 4);
        assert_eq!(last.next, None);
        assert_eq!(last.items.len(), 2);
        let empty = keyset_page(&query, Vec::new(), false, 0, 0);
        assert_eq!(empty.next, None);
        // An empty page has no last row to resume after.
        assert_eq!(keyset_page(&query, Vec::new(), true, 0, 0).next, None);
    }

    #[test]
    fn a_snapshot_page_has_a_next_until_the_snapshot_ends() {
        let query = AssetQuery::new();
        let next = snapshot_next(&query, 7, 100, 250).expect("a page follows");
        assert_eq!(next.snapshot_for(&query), Ok((7, 100)));
        assert_eq!(snapshot_next(&query, 7, 250, 250), None);
        assert_eq!(snapshot_next(&query, 7, 0, 0), None);
    }

    #[test]
    fn the_snapshot_store_is_bounded_and_evicts_the_least_recently_used() {
        let mut store = SnapshotStore::new(SNAPSHOT_SESSIONS);
        let sessions: Vec<u64> = (0..SNAPSHOT_SESSIONS).map(|n| store.insert(n)).collect();
        assert_eq!(store.len(), SNAPSHOT_SESSIONS);
        for (n, session) in sessions.iter().enumerate() {
            assert_eq!(store.get(*session), Some(n));
        }

        // Touch the oldest session, so the second-oldest is now the least recently used.
        assert_eq!(store.get(sessions[0]), Some(0));
        let fifth = store.insert(99);
        assert_eq!(store.len(), SNAPSHOT_SESSIONS, "the bound holds");
        assert_eq!(store.get(sessions[1]), None, "the LRU session was evicted");
        assert_eq!(store.get(sessions[0]), Some(0));
        assert_eq!(store.get(fifth), Some(99));

        // Session ids are never reused, so an evicted cursor never lands on a newer snapshot.
        let sixth = store.insert(100);
        assert!(!sessions.contains(&sixth) && sixth != fifth);
        assert_eq!(store.get(sessions[1]), None);
    }

    #[test]
    fn a_zero_capacity_store_still_holds_one_session() {
        let mut store = SnapshotStore::new(0);
        let a = store.insert("a");
        assert_eq!(store.get(a), Some("a"));
        let b = store.insert("b");
        assert_eq!(
            (store.get(a), store.get(b), store.len()),
            (None, Some("b"), 1)
        );
    }

    #[cfg(not(feature = "library"))]
    #[test]
    fn cursor_paging_is_refused_without_the_feature() {
        use crate::PhotoLibrary;
        let off = PhotoError::FeatureDisabled { feature: "library" };
        assert_eq!(
            PhotoLibrary::assets_after(&AssetQuery::new(), None).unwrap_err(),
            off
        );
        let cursor = AssetCursor::new(&AssetQuery::new(), CursorPosition::Keyset { key: 1, id: 1 });
        assert_eq!(
            PhotoLibrary::assets_after(&AssetQuery::new(), Some(&cursor)).unwrap_err(),
            off
        );
    }

    #[cfg(all(
        feature = "library",
        not(any(target_os = "android", target_os = "ios"))
    ))]
    #[test]
    fn cursor_paging_is_unsupported_without_a_library() {
        assert_eq!(
            crate::PhotoLibrary::assets_after(&AssetQuery::new(), None),
            Err(PhotoError::Unsupported)
        );
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

    #[test]
    fn mime_type_passes_a_mime_through_unchanged() {
        let mut a = asset("a");
        a.content_type = Some("image/x-unlisted-thing".to_owned());
        assert_eq!(a.mime_type().as_deref(), Some("image/x-unlisted-thing"));
    }

    #[test]
    fn mime_type_maps_a_uti_and_rejects_an_unknown_one() {
        let mut a = asset("a");
        a.content_type = Some("public.heic".to_owned());
        assert_eq!(a.mime_type().as_deref(), Some("image/heic"));
        a.content_type = Some("com.example.unknown".to_owned());
        assert_eq!(a.mime_type(), None);
    }

    #[test]
    fn mime_type_is_none_without_a_content_type() {
        assert_eq!(asset("a").mime_type(), None);
    }
}
