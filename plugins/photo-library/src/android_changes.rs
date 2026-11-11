//! Android change feed: `current_change_token`, `changes_since` and the
//! `ContentObserver` wake behind `on_library_changed`. Declared from
//! `android.rs` under the `library` feature; the host calls, the result object
//! and the shared constants are pinned by `tests/kotlin_contract.rs`. The
//! Kotlin side is `MediaStoreChanges`, reached through `FrustPhotoLibraryHost`'s
//! statics.
//!
//! | Method | Signature (Java) | Returns |
//! |---|---|---|
//! | `changeSnapshot` | `(boolean withGenerations) -> ChangesResult` | the volume version, the generation (`NO_GENERATION` below API 30), and every visible image and video id with its kind, plus each row's `GENERATION_ADDED` and `GENERATION_MODIFIED` when `withGenerations` is set; or a failure code |
//! | `startChangeObserver` | `() -> int` | `OK` once the observer is registered (also when it already was), `NOT_INITIALIZED`, `PERMISSION_DENIED` or `QUERY_FAILED` |
//! | `stopChangeObserver` | `() -> void` | nothing; a stopped observer is ignored |
//!
//! Kotlin → Rust: `nativeOnLibraryChanged()`, this module's own export, wakes
//! every subscriber through `changes::notify_all`.
//!
//! # The token
//!
//! A token is this module's own versioned encoding of a [`Snapshot`]: the
//! `MediaStore` volume version, the generation, and the sorted image and video
//! id sets, each id delta-encoded as a LEB128 varint after the first. Each id
//! costs one byte while the gap to the previous id of its kind is at most 128
//! and two bytes while it is at most 16 384, so **100 000 ids take about
//! 100 KB**: 100 000 bytes plus a header of under 100 bytes while every gap is
//! at most 128 (100 055 bytes for 100 000 consecutive ids, measured by the host
//! test), and at most about 200 KB while every gap stays within 16 384. The
//! host's `ChangesResult` for the same library crosses JNI as about 2.8 MB of
//! arrays (an id, a kind and two generations per row), once per call and never
//! kept. Bytes that do not decode (an Apple token, an older or
//! newer format, an empty or truncated token) are not an error: `changes_since`
//! answers a full rescan with a fresh token.
//!
//! # What a delta is
//!
//! Below API 30 `MediaStore` keeps no generations, so `changes_since` always
//! answers a full rescan. From API 30, a volume version other than the token's
//! (the database was rebuilt) or a generation below it is a full rescan too.
//! Otherwise, against the token's generation `g`:
//! - `inserted`: each row with `GENERATION_ADDED` above `g`, and each row
//!   absent from the token's id set of its kind. A row can become visible
//!   without being added (a pending item published, an item restored from the
//!   trash, a grant that widens a partial selection), and such a row is new to
//!   the app;
//! - `updated`: each other row with `GENERATION_MODIFIED` above `g`;
//! - `deleted`: each id in the token's set that is absent from the current one.
//!   `MediaStore.DeletedFiles` is not used: the id set also catches rows that
//!   stopped being visible without being deleted.
//!
//! The host reads the version and the generation before it queries the rows,
//! so a row written during the query is reported again next time, never
//! missed. The query is the browser's own (`MediaStore.Files` on the
//! `external` volume, images and videos only), so every id matches `assets`.
//!
//! # The wake
//!
//! `startChangeObserver` registers one `ContentObserver` on the image and
//! video external collections, with descendants; every `onChange` overload
//! calls `nativeOnLibraryChanged`, which carries no data: generations stay the
//! source of truth. `stopChangeObserver` unregisters it and may run from
//! inside that callback (a subscriber dropping its own subscription): the host
//! holds no lock while it calls the native, and the registry calls the stop
//! hook outside the lock `notify_all` takes.
//!
//! The pure part (the token, the generation filter, the id-set diff) uses
//! `std` only and is checked on the host: `tests/kotlin_contract.rs` includes
//! this file, and only the JNI arm needs Android.

/// The generation the host reports below API 30, where `MediaStore` keeps none;
/// the twin of `MediaStoreChanges.NO_GENERATION`.
const NO_GENERATION: i64 = -1;

/// The token's leading bytes: "frust photo library, Android".
const TOKEN_MAGIC: [u8; 4] = *b"FPLA";

/// The token encoding's version; a token of any other is a full rescan.
const TOKEN_FORMAT: u8 = 1;

/// Token flag: a generation follows the version.
const FLAG_GENERATION: u8 = 1;

/// The typed collection an id belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum IdKind {
    Image,
    Video,
}

/// One visible row: its `_ID`, kind, and generations (0 when not read).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Row {
    id: u64,
    kind: IdKind,
    added: u64,
    modified: u64,
}

/// What a token records: the volume version, the generation (`None` below API
/// 30), and the sorted, de-duplicated id set of each kind.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    version: String,
    generation: Option<u64>,
    images: Vec<u64>,
    videos: Vec<u64>,
}

/// One id of a delta, with the collection it is reported under.
type Entry = (IdKind, u64);

/// The answer to a token, before ids become asset ids.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Changes {
    FullRescan,
    Delta {
        inserted: Vec<Entry>,
        updated: Vec<Entry>,
        deleted: Vec<Entry>,
    },
}

/// The rows of a `ChangesResult`'s parallel arrays. `added` and `modified` are
/// either both empty (generations not read) or as long as `ids`. `None` for
/// mismatched lengths or a negative id; a negative generation reads as 0.
fn rows_from_columns(
    ids: &[i64],
    kinds: &[IdKind],
    added: &[i64],
    modified: &[i64],
) -> Option<Vec<Row>> {
    if kinds.len() != ids.len() || added.len() != modified.len() {
        return None;
    }
    let generations = !added.is_empty();
    if generations && added.len() != ids.len() {
        return None;
    }
    let generation = |values: &[i64], index: usize| {
        if generations {
            u64::try_from(values[index]).unwrap_or(0)
        } else {
            0
        }
    };
    ids.iter()
        .zip(kinds)
        .enumerate()
        .map(|(index, (id, kind))| {
            Some(Row {
                id: u64::try_from(*id).ok()?,
                kind: *kind,
                added: generation(added, index),
                modified: generation(modified, index),
            })
        })
        .collect()
}

impl Snapshot {
    /// The snapshot `rows` describe; a negative `generation` (the host's
    /// `NO_GENERATION`) is none.
    fn from_rows(version: String, generation: i64, rows: &[Row]) -> Self {
        let ids = |kind: IdKind| {
            let mut ids: Vec<u64> = rows
                .iter()
                .filter(|row| row.kind == kind)
                .map(|row| row.id)
                .collect();
            ids.sort_unstable();
            ids.dedup();
            ids
        };
        let generation = if generation == NO_GENERATION {
            None
        } else {
            u64::try_from(generation).ok()
        };
        Self {
            version,
            generation,
            images: ids(IdKind::Image),
            videos: ids(IdKind::Video),
        }
    }

    fn ids(&self, kind: IdKind) -> &[u64] {
        match kind {
            IdKind::Image => &self.images,
            IdKind::Video => &self.videos,
        }
    }

    /// The token bytes (module doc, "The token").
    fn encode(&self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(32 + self.version.len() + self.images.len() + self.videos.len());
        out.extend_from_slice(&TOKEN_MAGIC);
        out.push(TOKEN_FORMAT);
        out.push(if self.generation.is_some() {
            FLAG_GENERATION
        } else {
            0
        });
        put_varint(&mut out, self.version.len() as u64);
        out.extend_from_slice(self.version.as_bytes());
        if let Some(generation) = self.generation {
            put_varint(&mut out, generation);
        }
        put_ids(&mut out, &self.images);
        put_ids(&mut out, &self.videos);
        out
    }

    /// The snapshot `bytes` encode, or `None` for anything [`encode`](Self::encode)
    /// did not write: another magic or format, unknown flags, a truncated
    /// field, unsorted ids, or trailing bytes.
    fn decode(bytes: &[u8]) -> Option<Self> {
        let mut reader = Reader { bytes };
        if reader.take(TOKEN_MAGIC.len())? != TOKEN_MAGIC || reader.byte()? != TOKEN_FORMAT {
            return None;
        }
        let flags = reader.byte()?;
        if flags & !FLAG_GENERATION != 0 {
            return None;
        }
        let len = usize::try_from(reader.varint()?).ok()?;
        let version = String::from_utf8(reader.take(len)?.to_vec()).ok()?;
        let generation = if flags & FLAG_GENERATION != 0 {
            Some(reader.varint()?)
        } else {
            None
        };
        let images = reader.ids()?;
        let videos = reader.ids()?;
        if !reader.bytes.is_empty() {
            return None;
        }
        Some(Self {
            version,
            generation,
            images,
            videos,
        })
    }
}

/// `stored` against `current` and the `rows` it was built from (module doc,
/// "What a delta is"). Each list is sorted by id.
fn compare(stored: &Snapshot, current: &Snapshot, rows: &[Row]) -> Changes {
    let (Some(since), Some(now)) = (stored.generation, current.generation) else {
        return Changes::FullRescan;
    };
    if stored.version != current.version || now < since {
        return Changes::FullRescan;
    }
    let mut inserted = Vec::new();
    let mut updated = Vec::new();
    for row in rows {
        let known = stored.ids(row.kind).binary_search(&row.id).is_ok();
        if row.added > since || !known {
            inserted.push((row.kind, row.id));
        } else if row.modified > since {
            updated.push((row.kind, row.id));
        }
    }
    let mut deleted = Vec::new();
    for kind in [IdKind::Image, IdKind::Video] {
        let now = current.ids(kind);
        deleted.extend(
            stored
                .ids(kind)
                .iter()
                .filter(|id| now.binary_search(id).is_err())
                .map(|id| (kind, *id)),
        );
    }
    for list in [&mut inserted, &mut updated, &mut deleted] {
        list.sort_unstable_by_key(|(kind, id)| (*id, *kind));
        list.dedup();
    }
    Changes::Delta {
        inserted,
        updated,
        deleted,
    }
}

/// The asset id of `id` under its collection's content URI, as
/// `ContentUris.withAppendedId` builds it for the browser.
fn asset_uri(base: &str, id: u64) -> String {
    format!("{base}/{id}")
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let low = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(low);
            return;
        }
        out.push(low | 0x80);
    }
}

/// A count, then the first id and each later id's gap to its predecessor less
/// one (the ids are strictly increasing, so a run of consecutive ids is zeros).
fn put_ids(out: &mut Vec<u8>, ids: &[u64]) {
    put_varint(out, ids.len() as u64);
    let mut previous = None;
    for &id in ids {
        put_varint(out, previous.map_or(id, |p: u64| id - p - 1));
        previous = Some(id);
    }
}

/// A cursor over token bytes; every read is `None` past the end.
struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if n > self.bytes.len() {
            return None;
        }
        let (head, rest) = self.bytes.split_at(n);
        self.bytes = rest;
        Some(head)
    }

    fn byte(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    /// A LEB128 `u64`; `None` when it overflows or runs past the end.
    fn varint(&mut self) -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            let part = u64::from(byte & 0x7f);
            if shift == 63 && part > 1 {
                return None;
            }
            value |= part << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }

    /// The inverse of [`put_ids`]. A count larger than the bytes left (each id
    /// takes at least one) is refused before anything is allocated.
    fn ids(&mut self) -> Option<Vec<u64>> {
        let count = usize::try_from(self.varint()?).ok()?;
        if count > self.bytes.len() {
            return None;
        }
        let mut ids = Vec::with_capacity(count);
        let mut previous: Option<u64> = None;
        for _ in 0..count {
            let step = self.varint()?;
            let id = match previous {
                None => step,
                Some(p) => p.checked_add(1)?.checked_add(step)?,
            };
            ids.push(id);
            previous = Some(id);
        }
        Some(ids)
    }
}

#[cfg(target_os = "android")]
pub(crate) use jni_arm::{
    changes_since, current_change_token, start_change_observer, stop_change_observer,
};

/// The host calls; everything above is platform-free.
#[cfg(target_os = "android")]
mod jni_arm {
    use jni::errors::LogErrorAndDefault;
    use jni::objects::{JClass, JIntArray, JLongArray, JObject, JString, JValue};
    use jni::sys::jint;
    use jni::{Env, EnvUnowned, jni_sig, jni_str};

    use super::{Changes, Entry, IdKind, Row, Snapshot, asset_uri, compare, rows_from_columns};
    use crate::android::{NOT_INITIALIZED, ensure_off_ui_thread, error_for, run_jni, with_host};
    use crate::{AssetId, ChangeToken, LibraryChanges, PhotoError};

    /// A host call succeeded.
    const OK: jint = 7;
    /// No photo permission of the SDK set is granted.
    const PERMISSION_DENIED: jint = 8;
    /// A `MediaStore` query or observer registration failed.
    const QUERY_FAILED: jint = 10;
    /// `ChangesResult.kinds`: a still image.
    const KIND_IMAGE: jint = 0;
    /// `ChangesResult.kinds`: a video.
    const KIND_VIDEO: jint = 1;

    /// One `changeSnapshot` answer: the snapshot, its rows, and the two
    /// collection URIs ids are reported under.
    struct HostSnapshot {
        snapshot: Snapshot,
        rows: Vec<Row>,
        image_base: String,
        video_base: String,
    }

    impl HostSnapshot {
        fn token(&self) -> ChangeToken {
            ChangeToken::from_bytes(self.snapshot.encode())
        }

        fn asset_ids(&self, entries: Vec<Entry>) -> Vec<AssetId> {
            entries
                .into_iter()
                .map(|(kind, id)| {
                    let base = match kind {
                        IdKind::Image => &self.image_base,
                        IdKind::Video => &self.video_base,
                    };
                    AssetId::from_raw(asset_uri(base, id))
                })
                .collect()
        }
    }

    /// The crate root's `current_change_token` arm.
    pub(crate) fn current_change_token() -> Result<ChangeToken, PhotoError> {
        Ok(snapshot(false)?.token())
    }

    /// The crate root's `changes_since` arm (module doc, "What a delta is").
    pub(crate) fn changes_since(token: &ChangeToken) -> Result<LibraryChanges, PhotoError> {
        let stored = Snapshot::decode(token.as_bytes()).filter(|s| s.generation.is_some());
        let Some(stored) = stored else {
            let next = current_change_token()?;
            return Ok(LibraryChanges::FullRescan { next });
        };
        let host = snapshot(true)?;
        let next = host.token();
        Ok(match compare(&stored, &host.snapshot, &host.rows) {
            Changes::FullRescan => LibraryChanges::FullRescan { next },
            Changes::Delta {
                inserted,
                updated,
                deleted,
            } => LibraryChanges::Delta {
                inserted: host.asset_ids(inserted),
                updated: host.asset_ids(updated),
                deleted: host.asset_ids(deleted),
                next,
            },
        })
    }

    /// The crate root's `start_change_observer` hook: registers the host's
    /// `ContentObserver`. Runs under the registry's transition lock.
    pub(crate) fn start_change_observer() -> Result<(), PhotoError> {
        let code = with_host(|env, class| {
            run_jni(env, "FrustPhotoLibraryHost.startChangeObserver", |env| {
                env.call_static_method(
                    class,
                    jni_str!("startChangeObserver"),
                    jni_sig!("()I"),
                    &[],
                )?
                .i()
            })
        })?;
        if code == OK {
            Ok(())
        } else {
            Err(changes_error(code))
        }
    }

    /// The crate root's `stop_change_observer` hook: unregisters the host's
    /// `ContentObserver`. Safe from inside the observer's own callback.
    pub(crate) fn stop_change_observer() {
        let stopped = with_host(|env, class| {
            run_jni(env, "FrustPhotoLibraryHost.stopChangeObserver", |env| {
                env.call_static_method(class, jni_str!("stopChangeObserver"), jni_sig!("()V"), &[])?
                    .v()
            })
        });
        if let Err(err) = stopped {
            log::warn!("frust-photo-library: stopping the change observer failed: {err}");
        }
    }

    /// One `changeSnapshot` call, refusing the UI thread first.
    fn snapshot(with_generations: bool) -> Result<HostSnapshot, PhotoError> {
        with_host(|env, class| {
            ensure_off_ui_thread(env)?;
            run_jni(env, "FrustPhotoLibraryHost.changeSnapshot", |env| {
                let result = env
                    .call_static_method(
                        class,
                        jni_str!("changeSnapshot"),
                        jni_sig!("(Z)Ldev/frust/photolibrary/FrustPhotoLibraryHost$ChangesResult;"),
                        &[JValue::Bool(with_generations)],
                    )?
                    .l()?;
                let code = int_field(env, &result, jni_str!("code"))?;
                if code != OK {
                    return Ok(Err(changes_error(code)));
                }
                let version = string_field(env, &result, jni_str!("version"))?;
                let generation = env
                    .get_field(&result, jni_str!("generation"), jni_sig!("J"))?
                    .j()?;
                let ids = long_array_field(env, &result, jni_str!("ids"))?;
                let kinds = int_array_field(env, &result, jni_str!("kinds"))?;
                let added = long_array_field(env, &result, jni_str!("added"))?;
                let modified = long_array_field(env, &result, jni_str!("modified"))?;
                let image_base = string_field(env, &result, jni_str!("imageBase"))?;
                let video_base = string_field(env, &result, jni_str!("videoBase"))?;
                let (Some(version), Some(image_base), Some(video_base)) =
                    (version, image_base, video_base)
                else {
                    return Ok(Err(protocol_error()));
                };
                let Some(kinds) = kinds
                    .into_iter()
                    .map(kind_for_code)
                    .collect::<Option<Vec<_>>>()
                else {
                    return Ok(Err(protocol_error()));
                };
                let Some(rows) = rows_from_columns(&ids, &kinds, &added, &modified) else {
                    return Ok(Err(protocol_error()));
                };
                Ok(Ok(HostSnapshot {
                    snapshot: Snapshot::from_rows(version, generation, &rows),
                    rows,
                    image_base,
                    video_base,
                }))
            })?
        })
    }

    fn kind_for_code(code: jint) -> Option<IdKind> {
        match code {
            KIND_IMAGE => Some(IdKind::Image),
            KIND_VIDEO => Some(IdKind::Video),
            _ => None,
        }
    }

    fn protocol_error() -> PhotoError {
        PhotoError::Platform("the media library host returned a malformed change snapshot".into())
    }

    /// The typed error for a change call's non-[`OK`] code.
    fn changes_error(code: jint) -> PhotoError {
        match code {
            NOT_INITIALIZED => PhotoError::PlatformNotInitialized,
            PERMISSION_DENIED => PhotoError::PermissionDenied,
            QUERY_FAILED => PhotoError::Platform("the media library query failed".to_string()),
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

    /// A nullable `String` field; its local reference is released once read.
    fn string_field(
        env: &mut Env<'_>,
        object: &JObject<'_>,
        name: &'static jni::strings::JNIStr,
    ) -> Result<Option<String>, jni::errors::Error> {
        let value = env
            .get_field(object, name, jni_sig!("Ljava/lang/String;"))?
            .l()?;
        if value.is_null() {
            return Ok(None);
        }
        let text = env.cast_local::<JString>(value)?;
        let out = text.try_to_string(env)?;
        env.delete_local_ref(text);
        Ok(Some(out))
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
    ) -> Result<Vec<i64>, jni::errors::Error> {
        let array = env.get_field(object, name, jni_sig!("[J"))?.l()?;
        let array = env.cast_local::<JLongArray>(array)?;
        let mut values = vec![0; array.len(env)?];
        array.get_region(env, 0, &mut values)?;
        Ok(values)
    }

    /// `Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnLibraryChanged`
    /// — the host's `ContentObserver` saw a change. Wakes every subscriber; the
    /// call carries no data.
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnLibraryChanged<
        'local,
    >(
        mut env: EnvUnowned<'local>,
        _class: JClass<'local>,
    ) {
        env.with_env(|_env| {
            crate::changes::notify_all();
            Ok::<(), jni::errors::Error>(())
        })
        .resolve::<LogErrorAndDefault>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: u64, kind: IdKind, added: u64, modified: u64) -> Row {
        Row {
            id,
            kind,
            added,
            modified,
        }
    }

    fn snapshot(version: &str, generation: i64, rows: &[Row]) -> Snapshot {
        Snapshot::from_rows(version.to_string(), generation, rows)
    }

    fn images(ids: &[u64]) -> Vec<Entry> {
        ids.iter().map(|id| (IdKind::Image, *id)).collect()
    }

    /// The fixed cost of a token besides its ids, with a version string of the
    /// length `MediaStore` reports (a schema number and a UUID).
    const HEADER_BUDGET: usize = 100;
    const VERSION: &str = "1301:0f6c9a2e-5b1d-4d7e-9a43-2c8f7e1b6d55";

    #[test]
    fn a_token_round_trips() {
        let rows = [
            row(3, IdKind::Image, 1, 1),
            row(1, IdKind::Image, 1, 1),
            row(9, IdKind::Video, 1, 1),
            row(1_000_000, IdKind::Video, 1, 1),
            row(u64::MAX, IdKind::Image, 1, 1),
        ];
        let with_history = snapshot(VERSION, 42, &rows);
        assert_eq!(with_history.images, vec![1, 3, u64::MAX]);
        assert_eq!(with_history.videos, vec![9, 1_000_000]);
        assert_eq!(Snapshot::decode(&with_history.encode()), Some(with_history));

        let api_29 = snapshot("29:ü", NO_GENERATION, &[]);
        assert_eq!(api_29.generation, None);
        assert_eq!(Snapshot::decode(&api_29.encode()), Some(api_29));

        let empty = snapshot("", 0, &[]);
        assert_eq!(empty.generation, Some(0));
        assert_eq!(Snapshot::decode(&empty.encode()), Some(empty));
    }

    #[test]
    fn duplicate_rows_collapse_in_the_id_set() {
        let rows = [row(5, IdKind::Image, 0, 0), row(5, IdKind::Image, 0, 0)];
        assert_eq!(snapshot(VERSION, 1, &rows).images, vec![5]);
    }

    #[test]
    fn an_undecodable_token_is_none_never_a_panic() {
        let good = snapshot(
            VERSION,
            7,
            &[row(10, IdKind::Image, 0, 0), row(300, IdKind::Video, 0, 0)],
        )
        .encode();
        // Every truncation, including the empty token.
        for len in 0..good.len() {
            assert_eq!(Snapshot::decode(&good[..len]), None, "prefix of {len}");
        }
        let mut trailing = good.clone();
        trailing.push(0);
        assert_eq!(Snapshot::decode(&trailing), None);

        let mut magic = good.clone();
        magic[0] = b'X';
        assert_eq!(Snapshot::decode(&magic), None);
        let mut format = good.clone();
        format[TOKEN_MAGIC.len()] = TOKEN_FORMAT + 1;
        assert_eq!(Snapshot::decode(&format), None, "a newer format");
        let mut older = good.clone();
        older[TOKEN_MAGIC.len()] = 0;
        assert_eq!(Snapshot::decode(&older), None, "an older format");
        let mut flags = good.clone();
        flags[TOKEN_MAGIC.len() + 1] |= 0x80;
        assert_eq!(Snapshot::decode(&flags), None, "an unknown flag");

        // Foreign bytes: a binary property list (an archived Apple token) and a stub.
        assert_eq!(Snapshot::decode(b"bplist00\xd4\x01\x02\x03"), None);
        assert_eq!(Snapshot::decode(b"next"), None);
    }

    fn header(flags: u8) -> Vec<u8> {
        let mut out = TOKEN_MAGIC.to_vec();
        out.push(TOKEN_FORMAT);
        out.push(flags);
        out
    }

    #[test]
    fn a_malformed_body_is_none() {
        // Version not UTF-8.
        let mut bad_text = header(0);
        bad_text.extend_from_slice(&[2, 0xff, 0xfe, 0, 0]);
        assert_eq!(Snapshot::decode(&bad_text), None);

        // A generation varint that overflows `u64`.
        let mut overflow = header(FLAG_GENERATION);
        overflow.push(0);
        overflow.extend_from_slice(&[0xff; 9]);
        overflow.push(0x02);
        overflow.extend_from_slice(&[0, 0]);
        assert_eq!(Snapshot::decode(&overflow), None);

        // An id count larger than the bytes that follow.
        let mut count = header(0);
        count.push(0);
        put_varint(&mut count, u64::MAX);
        assert_eq!(Snapshot::decode(&count), None);

        // A gap that steps past `u64::MAX`.
        let mut past = header(0);
        past.push(0);
        past.push(2);
        put_varint(&mut past, u64::MAX);
        past.push(0);
        past.push(0);
        assert_eq!(Snapshot::decode(&past), None);
    }

    #[test]
    fn a_varint_round_trips_at_its_edges() {
        for value in [
            0,
            1,
            127,
            128,
            16_383,
            16_384,
            u64::from(u32::MAX),
            u64::MAX,
        ] {
            let mut out = Vec::new();
            put_varint(&mut out, value);
            let mut reader = Reader { bytes: &out };
            assert_eq!(reader.varint(), Some(value));
            assert!(reader.bytes.is_empty());
        }
    }

    #[test]
    fn a_version_mismatch_or_missing_history_is_a_full_rescan() {
        let rows = [row(1, IdKind::Image, 1, 1)];
        let stored = snapshot(VERSION, 5, &rows);
        assert_eq!(
            compare(&stored, &snapshot("1301:other", 9, &rows), &rows),
            Changes::FullRescan,
            "a rebuilt database"
        );
        assert_eq!(
            compare(&stored, &snapshot(VERSION, 4, &rows), &rows),
            Changes::FullRescan,
            "a generation that went back"
        );
        assert_eq!(
            compare(&stored, &snapshot(VERSION, NO_GENERATION, &rows), &rows),
            Changes::FullRescan,
            "no generations on this OS"
        );
        assert_eq!(
            compare(&snapshot(VERSION, NO_GENERATION, &rows), &stored, &rows),
            Changes::FullRescan,
            "a token taken without generations"
        );
        assert!(matches!(
            compare(&stored, &snapshot(VERSION, 5, &rows), &rows),
            Changes::Delta { .. }
        ));
    }

    #[test]
    fn generations_above_the_token_split_inserted_from_updated() {
        let before = [
            row(1, IdKind::Image, 2, 2),
            row(2, IdKind::Image, 3, 3),
            row(3, IdKind::Image, 4, 4),
        ];
        let stored = snapshot(VERSION, 10, &before);
        let rows = [
            // Untouched, and touched exactly at the token's generation.
            row(1, IdKind::Image, 2, 2),
            row(2, IdKind::Image, 3, 10),
            // Modified after the token.
            row(3, IdKind::Image, 4, 11),
            // Added after the token, and modified too: inserted only.
            row(4, IdKind::Image, 12, 13),
            row(5, IdKind::Image, 11, 11),
        ];
        let current = snapshot(VERSION, 13, &rows);
        assert_eq!(
            compare(&stored, &current, &rows),
            Changes::Delta {
                inserted: images(&[4, 5]),
                updated: images(&[3]),
                deleted: Vec::new(),
            }
        );
    }

    #[test]
    fn the_id_set_reports_deletions_and_newly_visible_rows() {
        let before = [
            row(1, IdKind::Image, 1, 1),
            row(2, IdKind::Video, 1, 1),
            row(3, IdKind::Image, 1, 1),
            row(4, IdKind::Video, 1, 1),
        ];
        let stored = snapshot(VERSION, 5, &before);
        let rows = [
            row(1, IdKind::Image, 1, 1),
            row(4, IdKind::Video, 1, 1),
            // Added before the token but not visible then (pending, trashed or not
            // selected): new to the app, even with a later modification.
            row(6, IdKind::Image, 3, 7),
        ];
        let current = snapshot(VERSION, 8, &rows);
        assert_eq!(
            compare(&stored, &current, &rows),
            Changes::Delta {
                inserted: images(&[6]),
                updated: Vec::new(),
                deleted: vec![(IdKind::Video, 2), (IdKind::Image, 3)],
            }
        );
    }

    #[test]
    fn an_id_is_matched_within_its_own_kind() {
        let stored = snapshot(VERSION, 5, &[row(7, IdKind::Image, 1, 1)]);
        let rows = [row(7, IdKind::Video, 1, 1)];
        let current = snapshot(VERSION, 5, &rows);
        assert_eq!(
            compare(&stored, &current, &rows),
            Changes::Delta {
                inserted: vec![(IdKind::Video, 7)],
                updated: Vec::new(),
                deleted: images(&[7]),
            }
        );
    }

    #[test]
    fn columns_become_rows_or_none() {
        let kinds = [IdKind::Image, IdKind::Video];
        assert_eq!(
            rows_from_columns(&[1, 2], &kinds, &[3, -1], &[4, 5]),
            Some(vec![
                row(1, IdKind::Image, 3, 4),
                row(2, IdKind::Video, 0, 5)
            ])
        );
        assert_eq!(
            rows_from_columns(&[1, 2], &kinds, &[], &[]),
            Some(vec![
                row(1, IdKind::Image, 0, 0),
                row(2, IdKind::Video, 0, 0)
            ]),
            "generations not read"
        );
        assert_eq!(rows_from_columns(&[1], &kinds, &[], &[]), None);
        assert_eq!(rows_from_columns(&[1, 2], &kinds, &[3], &[4]), None);
        assert_eq!(rows_from_columns(&[1, 2], &kinds, &[3, 3], &[]), None);
        assert_eq!(rows_from_columns(&[1, -2], &kinds, &[], &[]), None);
    }

    #[test]
    fn asset_uris_match_the_browser_shape() {
        assert_eq!(
            asset_uri("content://media/external/images/media", 42),
            "content://media/external/images/media/42"
        );
    }

    /// 100 000 ids, `gap(i)` apart, split between images and videos by `video(i)`.
    fn library(gap: impl Fn(u64) -> u64, video: impl Fn(u64) -> bool) -> Vec<Row> {
        let mut id = 1;
        (0..100_000)
            .map(|i| {
                id += gap(i);
                let kind = if video(i) {
                    IdKind::Video
                } else {
                    IdKind::Image
                };
                row(id, kind, 0, 0)
            })
            .collect()
    }

    fn token_len(rows: &[Row]) -> usize {
        let token = snapshot(VERSION, 1_234_567, rows);
        let bytes = token.encode();
        assert_eq!(Snapshot::decode(&bytes), Some(token));
        bytes.len()
    }

    /// A small deterministic generator, so the synthetic library is the same every run.
    fn xorshift(seed: u64) -> impl FnMut() -> u64 {
        let mut state = seed;
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        }
    }

    #[test]
    fn a_token_of_100k_ids_stays_within_its_documented_size() {
        // Consecutive ids: one byte each.
        let dense = token_len(&library(|_| 1, |_| false));
        assert!(
            (100_000..=100_000 + HEADER_BUDGET).contains(&dense),
            "{dense}"
        );

        // Ids with deletions in between, about one in five a video: one byte each but
        // for the rare video whose gap to the previous video passes 128.
        let mut next = xorshift(0x9e37_79b9_7f4a_7c15);
        let gaps: Vec<u64> = (0..100_000).map(|_| 1 + next() % 8).collect();
        let kinds: Vec<bool> = (0..100_000).map(|_| next().is_multiple_of(5)).collect();
        let mixed = token_len(&library(|i| gaps[i as usize], |i| kinds[i as usize]));
        assert!(mixed <= 101_000, "{mixed}");

        // The one-byte limit: gaps of exactly 128.
        let sparse = token_len(&library(|_| 128, |_| false));
        assert!(sparse <= 100_000 + HEADER_BUDGET, "{sparse}");

        // Two bytes each up to a gap of 16 384.
        let wide = token_len(&library(|_| 16_384, |_| false));
        assert!(
            (100_001..=200_000 + HEADER_BUDGET).contains(&wide),
            "{wide}"
        );
    }
}
