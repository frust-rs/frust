//! The Linux/Windows [`Backend`]: a single JSON map file.
//!
//! `<data_dir>/<app>/frust/preferences.json`, where `data_dir` and `<app>`
//! come from `frust-paths` ([`frust_paths::data_dir`]/[`frust_paths::app_stem`])
//! — the shared platform-dir resolution + atomic-write substrate every
//! desktop-facing storage site delegates to (see `docs/ARCHITECTURE.md`'s
//! `frust-paths` row).
//!
//! This module is also the **temporary routing target** for
//! iOS/macOS/Android (see `crate`'s module doc) until task 06 lands the
//! native `apple`/`android` backends — its own JSON encoding is otherwise
//! irrelevant to those targets.
//!
//! # On-disk shape
//!
//! A flat JSON object, one entry per key: `{"<key>": {"t": "<type-tag>",
//! "v": <encoded-value>}}`. The explicit `"t"` tag (rather than relying on
//! the JSON value's own shape) disambiguates a `String` value from an
//! encoded `f64`'s hex string, and is what makes "overwrite with a
//! different type" and "get with the wrong accessor" both well-defined:
//! [`crate::SharedPreferences::get_bool`] et al. simply don't match a
//! [`crate::PrefValue`] variant that doesn't correspond to the tag.
//!
//! **`f64` encoding**: not a bare JSON number — `f64::to_bits()` as a
//! lowercase 16-hex-digit string. This is what gives exact round-trip
//! including negative values and `NaN` (JSON has no `NaN` literal, and a
//! bare JSON number would silently lose `NaN`/precision); a stored `NaN`
//! reads back bit-for-bit identical (`f64::to_bits()` equal), though per
//! IEEE 754 it will still compare unequal to itself with `==` — assert
//! `is_nan()` or bit-pattern equality when a test needs to check a `NaN`
//! round trip, never `==`.
//!
//! # Concurrency & atomicity
//!
//! Every operation is a read-modify-write of the whole file, serialized by
//! an in-process [`Mutex`] (same-process concurrent writers never
//! interleave). Writes go through [`frust_paths::atomic_write`] (temp file +
//! `rename`), so a reader never observes a partially-written file, and a
//! crash mid-write leaves the previous contents intact. A missing or
//! corrupted (non-JSON, or JSON that isn't an object) file is treated as an
//! empty store rather than an error — this module never panics on bad
//! on-disk data.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{Map, Value};

use crate::{Backend, PrefValue, PrefsError};

/// JSON-file-backed preferences store.
pub(crate) struct FileStore {
    path: PathBuf,
    /// Guards every read-modify-write cycle (see module doc's Concurrency
    /// section).
    lock: Mutex<()>,
}

impl FileStore {
    /// Open the store at the OS-resolved default location.
    ///
    /// # Errors
    /// [`PrefsError::Storage`] if no data directory could be resolved (an
    /// unset `HOME`/`APPDATA` — this module never guesses a fallback that
    /// could silently write into the process's current directory).
    pub(crate) fn standard() -> Result<Self, PrefsError> {
        let dir = frust_paths::data_dir().ok_or_else(|| {
            PrefsError::Storage(
                "could not resolve a user data directory (HOME/APPDATA unset)".into(),
            )
        })?;
        Ok(Self::at_path(preferences_path(dir)))
    }

    /// Open the store at an explicit path, bypassing OS data-dir
    /// resolution entirely.
    ///
    /// Crate-private: the sole intended caller is the conformance suite
    /// (`#[cfg(test)]`), which opens a tempdir-isolated store so tests
    /// never touch the real user data directory (per this crate's
    /// acceptance criteria).
    pub(crate) fn at_path(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    /// Load the on-disk map, treating any read/parse failure (missing file,
    /// corrupted JSON, JSON that isn't an object) as an empty store — see
    /// module doc.
    fn load(&self) -> Map<String, Value> {
        match fs::read(&self.path) {
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(Value::Object(map)) => map,
                _ => Map::new(),
            },
            Err(_) => Map::new(),
        }
    }

    /// Persist `map`, atomically (see module doc).
    fn save(&self, map: &Map<String, Value>) -> Result<(), PrefsError> {
        let bytes = serde_json::to_vec_pretty(&Value::Object(map.clone()))
            .map_err(|e| PrefsError::Storage(format!("encoding preferences: {e}")))?;
        frust_paths::atomic_write(&self.path, &bytes).map_err(PrefsError::from)
    }
}

impl Backend for FileStore {
    fn get(&self, key: &str) -> Option<PrefValue> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.load().get(key).and_then(decode)
    }

    fn set(&self, key: &str, value: PrefValue) -> Result<(), PrefsError> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut map = self.load();
        map.insert(key.to_string(), encode(&value));
        self.save(&map)
    }

    fn remove(&self, key: &str) -> Result<(), PrefsError> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut map = self.load();
        map.remove(key);
        self.save(&map)
    }

    fn clear(&self) -> Result<(), PrefsError> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        // The file backend owns the whole file exclusively (Design
        // Decision 4 — unlike the OS-shared NSUserDefaults/Android
        // SharedPreferences stores, no `frust.`-prefix filtering is needed
        // here), so clearing is simply an empty map.
        self.save(&Map::new())
    }

    fn contains(&self, key: &str) -> bool {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.load().contains_key(key)
    }

    fn keys(&self) -> Vec<String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.load().keys().cloned().collect()
    }
}

/// `<data_dir>/<app>/frust/preferences.json`, namespaced per binary via
/// [`frust_paths::app_stem`] (see module doc).
fn preferences_path(data_dir: PathBuf) -> PathBuf {
    data_dir
        .join(frust_paths::app_stem())
        .join("frust")
        .join("preferences.json")
}

/// Encode one [`PrefValue`] into its tagged on-disk shape (module doc).
fn encode(value: &PrefValue) -> Value {
    match value {
        PrefValue::Bool(v) => tagged("bool", Value::Bool(*v)),
        PrefValue::I64(v) => tagged("i64", Value::from(*v)),
        PrefValue::F64(v) => tagged("f64", Value::String(format!("{:016x}", v.to_bits()))),
        PrefValue::Str(v) => tagged("string", Value::String(v.clone())),
        PrefValue::StrList(v) => tagged(
            "string_list",
            Value::Array(v.iter().cloned().map(Value::String).collect()),
        ),
    }
}

fn tagged(tag: &str, value: Value) -> Value {
    let mut obj: HashMap<&str, Value> = HashMap::with_capacity(2);
    obj.insert("t", Value::String(tag.to_string()));
    obj.insert("v", value);
    serde_json::to_value(obj).expect("a two-entry string-keyed map always serializes")
}

/// Decode one tagged on-disk entry back into a [`PrefValue`]; `None` for
/// anything that doesn't match the expected tagged shape (a corrupted
/// single entry degrades to "absent", not a panic — mirroring
/// [`FileStore::load`]'s whole-file corruption handling).
fn decode(value: &Value) -> Option<PrefValue> {
    let tag = value.get("t")?.as_str()?;
    let v = value.get("v")?;
    match tag {
        "bool" => v.as_bool().map(PrefValue::Bool),
        "i64" => v.as_i64().map(PrefValue::I64),
        "f64" => {
            let hex = v.as_str()?;
            let bits = u64::from_str_radix(hex, 16).ok()?;
            Some(PrefValue::F64(f64::from_bits(bits)))
        }
        "string" => v.as_str().map(str::to_string).map(PrefValue::Str),
        "string_list" => v
            .as_array()?
            .iter()
            .map(|entry| entry.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .map(PrefValue::StrList),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;

    use super::*;

    /// A scratch, per-test path under the OS temp dir — tests must NEVER
    /// resolve `FileStore::standard()`'s real data directory (per this
    /// crate's acceptance criteria), so every test opens `FileStore` via
    /// [`FileStore::at_path`] instead.
    fn scratch_path(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!(
                "frust-shared-preferences-test-{}-{tag}-{n}",
                std::process::id()
            ))
            .join("preferences.json")
    }

    fn scratch_store(tag: &str) -> FileStore {
        FileStore::at_path(scratch_path(tag))
    }

    /// The full cross-backend conformance suite, run against a fresh
    /// tempdir-isolated file store.
    #[test]
    fn conformance() {
        let store = scratch_store("conformance");
        crate::conformance::run_conformance_suite(&store);
    }

    /// A missing file starts fresh (no panic, no error) — `get`/`keys`
    /// behave like an empty store.
    #[test]
    fn missing_file_is_fresh_start() {
        let store = scratch_store("missing-file");
        assert_eq!(store.get("k"), None);
        assert!(store.keys().is_empty());
    }

    /// A corrupted (non-JSON) file is treated as an empty store rather than
    /// erroring, and a subsequent write recovers it (never a panic).
    #[test]
    fn corrupted_file_is_fresh_start_and_recovers() {
        let store = scratch_store("corrupted");
        fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        fs::write(&store.path, b"not json at all {{{").unwrap();

        assert_eq!(store.get("k"), None);
        assert!(store.keys().is_empty());

        store.set("k", PrefValue::Bool(true)).unwrap();
        assert_eq!(store.get("k"), Some(PrefValue::Bool(true)));

        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    /// A JSON file that parses but isn't an object (e.g. a bare array) is
    /// also treated as an empty store.
    #[test]
    fn non_object_json_is_fresh_start() {
        let store = scratch_store("non-object");
        fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        fs::write(&store.path, b"[1, 2, 3]").unwrap();

        assert_eq!(store.get("k"), None);

        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    /// Concurrent same-process writers (multiple threads sharing one
    /// `FileStore` behind an `Arc`) never lose a write — the in-process
    /// `Mutex` serializes every read-modify-write cycle.
    #[test]
    fn concurrent_writers_all_land() {
        let store = Arc::new(scratch_store("concurrent"));
        let handles: Vec<_> = (0..16)
            .map(|i| {
                let store = Arc::clone(&store);
                thread::spawn(move || {
                    store.set(&format!("key-{i}"), PrefValue::I64(i)).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let mut keys = store.keys();
        keys.sort();
        let mut expected: Vec<String> = (0..16).map(|i| format!("key-{i}")).collect();
        expected.sort();
        assert_eq!(keys, expected);
        for i in 0..16 {
            assert_eq!(
                store.get(&format!("key-{i}")),
                Some(PrefValue::I64(i)),
                "writer {i}'s value must survive concurrent writes"
            );
        }

        let _ = fs::remove_dir_all(store.path.parent().unwrap());
    }

    /// `standard()` never writes to (or even resolves, beyond the
    /// directory check) the real data directory in a way a test could
    /// observe — this just confirms the constructor itself doesn't panic
    /// or perform I/O.
    #[test]
    fn standard_resolves_without_io() {
        // Not asserting Ok/Err (CI hosts vary in HOME/APPDATA presence) —
        // only that construction is infallible-by-panic.
        let _ = FileStore::standard();
    }
}
