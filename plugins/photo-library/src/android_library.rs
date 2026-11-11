//! Android library browsing, the `library` feature's half of the Rust-Kotlin
//! contract in [`super`]. Declared from `android.rs` so the crate root stays
//! platform-agnostic.
//!
//! `albums`, `assets` and `thumbnail` call `FrustPhotoLibraryHost`'s statics
//! on the caller's thread, inside one scoped JNI attachment, and read a result
//! object (`FrustPhotoLibraryHost$AlbumsResult` and siblings) field by field.
//! The result's `code` decides the outcome; parallel primitive arrays carry
//! the data. They never take the pick guard: browsing is read-only and runs
//! while a pick is up.
//!
//! `manage_limited_selection` presents system UI, so it takes the
//! [`pick_cache::PickGuard`] and uses the same request-id relay as a pick: the
//! host answers `PENDING`, then `nativeOnPickResult` (three empty arrays) when
//! the prompt closes, or `nativeOnPickError`.
//!
//! Every blocking call refuses the UI thread with [`PhotoError::UiThread`]
//! before any work.

use std::sync::atomic::Ordering;

use jni::objects::{JByteArray, JIntArray, JLongArray, JObject, JObjectArray, JValue};
use jni::sys::{jint, jlong};
use jni::{Env, jni_sig, jni_str};

use super::{
    Answer, LAUNCH_FAILED, NEXT_REQUEST_ID, NOT_INITIALIZED, PENDING, Slot, WAITS, await_answer,
    ensure_off_ui_thread, error_for, filter_code, lock, read_strings, run_jni, with_host,
};
use crate::library::{
    Album, AlbumId, AlbumKind, Asset, AssetPage, AssetQuery, Thumbnail, ThumbnailMode,
    ThumbnailRequest, favorite_from_code, modified_ms_from_seconds,
};
use crate::pick_cache;
use crate::pixels;
use crate::types::{AssetId, MediaKind, PhotoError};

/// A browse call succeeded.
const OK: jint = 7;
/// No photo permission of the SDK set is granted.
const PERMISSION_DENIED: jint = 8;
/// The asset no longer exists.
const NOT_FOUND: jint = 9;
/// A `MediaStore` query or decode failed.
const QUERY_FAILED: jint = 10;
/// `manageLimitedSelection` found access is not limited.
const NOT_LIMITED: jint = 11;

/// `AssetsResult.kinds`: a still image.
const KIND_IMAGE: jint = 0;
/// `AssetsResult.kinds`: a video.
const KIND_VIDEO: jint = 1;

/// The synthetic all-media album's id, pinned against the Kotlin host.
const ALL_MEDIA_ALBUM_ID: &str = "frust.all_media";

/// The crate root's `albums` arm.
pub(crate) fn albums() -> Result<Vec<Album>, PhotoError> {
    with_host(|env, class| {
        ensure_off_ui_thread(env)?;
        run_jni(env, "FrustPhotoLibraryHost.albums", |env| {
            let result = env
                .call_static_method(
                    class,
                    jni_str!("albums"),
                    jni_sig!("()Ldev/frust/photolibrary/FrustPhotoLibraryHost$AlbumsResult;"),
                    &[],
                )?
                .l()?;
            let code = int_field(env, &result, jni_str!("code"))?;
            if code != OK {
                return Ok(Err(browse_error(code)));
            }
            let ids = string_array_field(env, &result, jni_str!("ids"))?;
            let titles = string_array_field(env, &result, jni_str!("titles"))?;
            let counts = long_array_field(env, &result, jni_str!("counts"))?;
            Ok(albums_from(ids, titles, counts))
        })?
    })
}

/// Zip the parallel arrays of `AlbumsResult`.
fn albums_from(
    ids: Vec<Option<String>>,
    titles: Vec<Option<String>>,
    counts: Vec<jlong>,
) -> Result<Vec<Album>, PhotoError> {
    if titles.len() != ids.len() || counts.len() != ids.len() {
        return Err(mismatched_arrays());
    }
    let mut albums = Vec::with_capacity(ids.len());
    for ((id, title), count) in ids.into_iter().zip(titles).zip(counts) {
        // A null id breaks the same contract `assets_from` enforces.
        let Some(id) = id else {
            return Err(mismatched_arrays());
        };
        let kind = if id == ALL_MEDIA_ALBUM_ID {
            AlbumKind::AllMedia
        } else {
            AlbumKind::Folder
        };
        albums.push(Album {
            id: AlbumId::from_raw(id),
            title: title.unwrap_or_default(),
            kind,
            count: u64::try_from(count).ok(),
        });
    }
    Ok(albums)
}

/// The crate root's `assets` arm.
pub(crate) fn assets(query: &AssetQuery) -> Result<AssetPage, PhotoError> {
    // `offset` is a row index; beyond `i64::MAX` it is simply past the end.
    let offset = jlong::try_from(query.offset).unwrap_or(jlong::MAX);
    let limit = jint::try_from(query.effective_limit()).unwrap_or(jint::MAX);
    let filter = filter_code(query.filter);
    let details = query.details;
    with_host(|env, class| {
        ensure_off_ui_thread(env)?;
        run_jni(env, "FrustPhotoLibraryHost.assets", |env| {
            let album = match &query.album {
                Some(album) => JObject::from(env.new_string(album.as_str())?),
                None => JObject::null(),
            };
            let result = env
                .call_static_method(
                    class,
                    jni_str!("assets"),
                    jni_sig!(
                        "(Ljava/lang/String;IJIZ)Ldev/frust/photolibrary/FrustPhotoLibraryHost$AssetsResult;"
                    ),
                    &[
                        JValue::Object(&album),
                        JValue::Int(filter),
                        JValue::Long(offset),
                        JValue::Int(limit),
                        JValue::Bool(details),
                    ],
                )?
                .l()?;
            let code = int_field(env, &result, jni_str!("code"))?;
            if code != OK {
                return Ok(Err(browse_error(code)));
            }
            let total = env
                .get_field(&result, jni_str!("total"), jni_sig!("J"))?
                .j()?;
            let columns = AssetColumns {
                ids: string_array_field(env, &result, jni_str!("ids"))?,
                kinds: int_array_field(env, &result, jni_str!("kinds"))?,
                widths: int_array_field(env, &result, jni_str!("widths"))?,
                heights: int_array_field(env, &result, jni_str!("heights"))?,
                dates: long_array_field(env, &result, jni_str!("dates"))?,
                durations: long_array_field(env, &result, jni_str!("durations"))?,
                modified: long_array_field(env, &result, jni_str!("modified"))?,
                favorites: int_array_field(env, &result, jni_str!("favorites"))?,
                names: string_array_field(env, &result, jni_str!("names"))?,
                types: string_array_field(env, &result, jni_str!("types"))?,
            };
            Ok(assets_from(columns, total, details))
        })?
    })
}

/// The parallel arrays of `AssetsResult`.
struct AssetColumns {
    ids: Vec<Option<String>>,
    kinds: Vec<jint>,
    widths: Vec<jint>,
    heights: Vec<jint>,
    dates: Vec<jlong>,
    durations: Vec<jlong>,
    /// `DATE_MODIFIED`, in seconds.
    modified: Vec<jlong>,
    /// `1`, `0`, or `-1` when the platform does not report it.
    favorites: Vec<jint>,
    /// Only meaningful when the query asked for details.
    names: Vec<Option<String>>,
    /// Only meaningful when the query asked for details.
    types: Vec<Option<String>>,
}

/// Zip [`AssetColumns`] into a page.
fn assets_from(
    columns: AssetColumns,
    total: jlong,
    details: bool,
) -> Result<AssetPage, PhotoError> {
    let n = columns.ids.len();
    if [
        columns.kinds.len(),
        columns.widths.len(),
        columns.heights.len(),
        columns.dates.len(),
        columns.durations.len(),
        columns.modified.len(),
        columns.favorites.len(),
    ]
    .iter()
    .any(|len| *len != n)
    {
        return Err(mismatched_arrays());
    }
    if details && (columns.names.len() != n || columns.types.len() != n) {
        return Err(mismatched_arrays());
    }
    let mut names = columns.names;
    let mut types = columns.types;
    let mut items = Vec::with_capacity(n);
    for (index, id) in columns.ids.into_iter().enumerate() {
        let Some(id) = id else {
            return Err(mismatched_arrays());
        };
        items.push(Asset {
            id: AssetId::from_raw(id),
            kind: kind_for_code(columns.kinds[index]),
            width: u32::try_from(columns.widths[index]).unwrap_or(0),
            height: u32::try_from(columns.heights[index]).unwrap_or(0),
            created_unix_ms: Some(columns.dates[index]).filter(|ms| *ms > 0),
            duration_ms: u64::try_from(columns.durations[index])
                .ok()
                .filter(|ms| *ms > 0),
            modified_unix_ms: modified_ms_from_seconds(columns.modified[index]),
            is_favorite: favorite_from_code(columns.favorites[index]),
            // Android has no burst identifier.
            burst_id: None,
            file_name: if details { names[index].take() } else { None },
            content_type: if details { types[index].take() } else { None },
        });
    }
    Ok(AssetPage {
        items,
        total: u64::try_from(total).unwrap_or(0),
    })
}

fn kind_for_code(code: jint) -> MediaKind {
    match code {
        KIND_VIDEO => MediaKind::Video,
        KIND_IMAGE => MediaKind::Image,
        // The host writes only the two codes above.
        _ => MediaKind::Image,
    }
}

fn mismatched_arrays() -> PhotoError {
    PhotoError::Platform("the media library host returned mismatched arrays".to_string())
}

/// The crate root's `thumbnail` arm; `lib.rs` serves cache hits before this.
pub(crate) fn thumbnail(id: &AssetId, request: &ThumbnailRequest) -> Result<Thumbnail, PhotoError> {
    let width = jint::try_from(request.width).unwrap_or(jint::MAX);
    let height = jint::try_from(request.height).unwrap_or(jint::MAX);
    let fill = matches!(request.mode, ThumbnailMode::Fill);
    with_host(|env, class| {
        ensure_off_ui_thread(env)?;
        run_jni(env, "FrustPhotoLibraryHost.thumbnail", |env| {
            let raw_id = env.new_string(id.as_str())?;
            let result = env
                .call_static_method(
                    class,
                    jni_str!("thumbnail"),
                    jni_sig!(
                        "(Ljava/lang/String;IIZ)Ldev/frust/photolibrary/FrustPhotoLibraryHost$ThumbnailResult;"
                    ),
                    &[
                        JValue::Object(&raw_id),
                        JValue::Int(width),
                        JValue::Int(height),
                        JValue::Bool(fill),
                    ],
                )?
                .l()?;
            let code = int_field(env, &result, jni_str!("code"))?;
            if code != OK {
                return Ok(Err(browse_error(code)));
            }
            let width = int_field(env, &result, jni_str!("width"))?;
            let height = int_field(env, &result, jni_str!("height"))?;
            let row_bytes = int_field(env, &result, jni_str!("rowBytes"))?;
            let pixels = env
                .get_field(&result, jni_str!("pixels"), jni_sig!("[B"))?
                .l()?;
            let pixels = env.cast_local::<JByteArray>(pixels)?;
            let pixels = env.convert_byte_array(&pixels)?;
            Ok(thumbnail_from(width, height, row_bytes, pixels))
        })?
    })
}

/// Tightens the host's premultiplied rows to straight-alpha RGBA8.
fn thumbnail_from(
    width: jint,
    height: jint,
    row_bytes: jint,
    pixels: Vec<u8>,
) -> Result<Thumbnail, PhotoError> {
    let invalid = || PhotoError::Platform("the media library host returned a bad bitmap".into());
    let w = u32::try_from(width).map_err(|_| invalid())?;
    let h = u32::try_from(height).map_err(|_| invalid())?;
    let stride = usize::try_from(row_bytes).map_err(|_| invalid())?;
    let row = (w as usize).checked_mul(4).ok_or_else(invalid)?;
    let expected = stride.checked_mul(h as usize).ok_or_else(invalid)?;
    if stride < row || pixels.len() != expected {
        return Err(invalid());
    }
    let mut rgba = if stride == row {
        pixels
    } else {
        pixels::repack_rows(&pixels, w as usize, h as usize, stride)
    };
    pixels::unpremultiply_rgba8_in_place(&mut rgba);
    Ok(Thumbnail::new(w, h, rgba))
}

/// The crate root's `manage_limited_selection` arm. Blocks while the system
/// UI is up; a no-op when access is not limited.
pub(crate) fn manage_limited_selection() -> Result<(), PhotoError> {
    with_host(|env, _class| ensure_off_ui_thread(env))?;
    let _one_at_a_time = pick_cache::PickGuard::acquire()?;

    let request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    lock(&WAITS).insert(request_id, Slot::default());

    let code = with_host(|env, class| {
        run_jni(env, "FrustPhotoLibraryHost.manageLimitedSelection", |env| {
            env.call_static_method(
                class,
                jni_str!("manageLimitedSelection"),
                jni_sig!("(J)I"),
                &[JValue::Long(request_id as jlong)],
            )?
            .i()
        })
    });
    let code = match code {
        Ok(code) => code,
        Err(err) => {
            lock(&WAITS).remove(&request_id);
            return Err(err);
        }
    };
    if code != PENDING {
        lock(&WAITS).remove(&request_id);
        return if code == NOT_LIMITED {
            Ok(())
        } else {
            Err(selection_error(code))
        };
    }

    match await_answer(request_id, None) {
        Answer::Picked(_) => Ok(()),
        Answer::Failed(code) => Err(selection_error(code)),
        Answer::Cancelled => Err(PhotoError::Cancelled),
    }
}

/// The typed error for a browse result's non-[`OK`] code.
fn browse_error(code: jint) -> PhotoError {
    match code {
        NOT_INITIALIZED => PhotoError::PlatformNotInitialized,
        PERMISSION_DENIED => PhotoError::PermissionDenied,
        NOT_FOUND => PhotoError::NotFound,
        QUERY_FAILED => PhotoError::Platform("the media library query failed".to_string()),
        other => error_for(other),
    }
}

/// The typed error of a selection prompt, in its own words.
fn selection_error(code: jint) -> PhotoError {
    match code {
        LAUNCH_FAILED => {
            PhotoError::Platform("the photo selection prompt could not be launched".to_string())
        }
        other => error_for(other),
    }
}

fn int_field(
    env: &mut Env<'_>,
    object: &JObject<'_>,
    name: &'static jni::strings::JNIStr,
) -> Result<jint, jni::errors::Error> {
    env.get_field(object, name, jni_sig!("I"))?.i()
}

fn string_array_field(
    env: &mut Env<'_>,
    object: &JObject<'_>,
    name: &'static jni::strings::JNIStr,
) -> Result<Vec<Option<String>>, jni::errors::Error> {
    let array = env
        .get_field(object, name, jni_sig!("[Ljava/lang/String;"))?
        .l()?;
    let array = env.cast_local::<JObjectArray>(array)?;
    read_strings(env, &array)
}

fn int_array_field(
    env: &mut Env<'_>,
    object: &JObject<'_>,
    name: &'static jni::strings::JNIStr,
) -> Result<Vec<jint>, jni::errors::Error> {
    let array = env.get_field(object, name, jni_sig!("[I"))?.l()?;
    let array = env.cast_local::<JIntArray>(array)?;
    let mut values = vec![0; array.len(env)?];
    array.get_region(env, 0, &mut values)?;
    Ok(values)
}

fn long_array_field(
    env: &mut Env<'_>,
    object: &JObject<'_>,
    name: &'static jni::strings::JNIStr,
) -> Result<Vec<jlong>, jni::errors::Error> {
    let array = env.get_field(object, name, jni_sig!("[J"))?.l()?;
    let array = env.cast_local::<JLongArray>(array)?;
    let mut values = vec![0; array.len(env)?];
    array.get_region(env, 0, &mut values)?;
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(n: usize) -> AssetColumns {
        AssetColumns {
            ids: (0..n).map(|i| Some(format!("content://m/{i}"))).collect(),
            kinds: vec![KIND_IMAGE; n],
            widths: vec![4; n],
            heights: vec![3; n],
            dates: vec![1_000; n],
            durations: vec![0; n],
            modified: vec![1_700_000_000; n],
            favorites: vec![1; n],
            names: vec![Some("a.jpg".into()); n],
            types: vec![Some("image/jpeg".into()); n],
        }
    }

    #[test]
    fn new_columns_decode_into_the_asset() {
        let page = assets_from(columns(1), 1, true).unwrap();
        let asset = &page.items[0];
        assert_eq!(asset.modified_unix_ms, Some(1_700_000_000_000));
        assert_eq!(asset.is_favorite, Some(true));
        assert_eq!(asset.burst_id, None);
        assert_eq!(asset.file_name.as_deref(), Some("a.jpg"));
        assert_eq!(asset.content_type.as_deref(), Some("image/jpeg"));
    }

    #[test]
    fn a_missing_favourite_is_none_not_false() {
        let mut c = columns(2);
        c.favorites = vec![-1, 0];
        let page = assets_from(c, 2, false).unwrap();
        assert_eq!(page.items[0].is_favorite, None);
        assert_eq!(page.items[1].is_favorite, Some(false));
    }

    #[test]
    fn name_and_type_stay_none_without_details() {
        let page = assets_from(columns(1), 1, false).unwrap();
        assert_eq!(page.items[0].file_name, None);
        assert_eq!(page.items[0].content_type, None);
    }

    #[test]
    fn short_detail_arrays_are_a_protocol_error_only_with_details() {
        let mut c = columns(2);
        c.names.clear();
        assert!(assets_from(c, 2, true).is_err());
        let mut c = columns(2);
        c.names.clear();
        c.types.clear();
        assert!(assets_from(c, 2, false).is_ok());
    }
}
