//! The storage-core [`Backend`] conformance-suite harness: a plaintext JSON
//! file per named store, compiled only under `#[cfg(test)]` to validate every
//! backend implementation against a uniform contract. Production storage
//! routes to platform-specific backends (Keychain on Apple, Keystore on
//! Android, keyring on Linux/Windows).
//!
//! # Not secure at rest
//!
//! This backend stores plaintext JSON — it is **not** secure and must never
//! be used in production. It exists solely to exercise the [`Backend`] trait
//! contract in conformance tests.
//!
//! # On-disk shape
//!
//! A flat JSON object of `String` values, one entry per key:
//! `{"<key>": "<value>"}`. v1 stores only strings (see the crate's value
//! model), so no per-value type tagging is needed.
//!
//! # Concurrency & atomicity
//!
//! Every operation is a read-modify-write of the whole file, serialized by an
//! in-process [`Mutex`]. Writes go to a process-unique temp file then
//! `rename` into place, so a reader never observes a partial write and a
//! crash mid-write leaves the previous contents intact. A missing or
//! corrupted (non-JSON, or JSON that isn't a string-valued object) file is
//! treated as an empty store rather than an error — this module never panics
//! on bad on-disk data.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::{Backend, SecureStorageError};

/// A JSON-file-backed secure store, scoped to one store name.
pub(crate) struct FileStore {
    path: PathBuf,
    /// Guards every read-modify-write cycle (see module doc's Concurrency
    /// section).
    lock: Mutex<()>,
}

impl FileStore {
    /// Open the store at an explicit file path.
    ///
    /// Crate-private: the intended caller is the conformance suite
    /// (`#[cfg(test)]`), which opens tempdir-isolated stores so tests never
    /// touch the real user data directory.
    pub(crate) fn at_path(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    /// Load the on-disk map, treating any read/parse failure (missing file,
    /// corrupted JSON, JSON that isn't a string-valued object) as an empty
    /// store — see module doc.
    fn load(&self) -> BTreeMap<String, String> {
        match fs::read(&self.path) {
            Ok(bytes) => {
                serde_json::from_slice::<BTreeMap<String, String>>(&bytes).unwrap_or_default()
            }
            Err(_) => BTreeMap::new(),
        }
    }

    /// Persist `map`, atomically (see module doc).
    fn save(&self, map: &BTreeMap<String, String>) -> Result<(), SecureStorageError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(map)
            .map_err(|e| SecureStorageError::Storage(format!("encoding secure store: {e}")))?;
        let temp_path = self
            .path
            .with_extension(format!("json.tmp.{}", std::process::id()));
        fs::write(&temp_path, &bytes)?;
        let rename_result = fs::rename(&temp_path, &self.path);
        if rename_result.is_err() {
            // Best-effort cleanup so a failed rename doesn't leave the temp
            // file behind indefinitely.
            let _ = fs::remove_file(&temp_path);
        }
        rename_result.map_err(SecureStorageError::from)
    }
}

impl Backend for FileStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecureStorageError> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        Ok(self.load().get(key).cloned())
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecureStorageError> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut map = self.load();
        map.insert(key.to_string(), value.to_string());
        self.save(&map)
    }

    fn remove(&self, key: &str) -> Result<(), SecureStorageError> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut map = self.load();
        map.remove(key);
        self.save(&map)
    }

    fn clear(&self) -> Result<(), SecureStorageError> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        // A store owns its whole file exclusively (the store name is baked
        // into the filename), so clearing is simply an empty map — no
        // namespace filtering, unlike the OS-shared platform backends.
        self.save(&BTreeMap::new())
    }

    fn contains(&self, key: &str) -> Result<bool, SecureStorageError> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        Ok(self.load().contains_key(key))
    }

    fn keys(&self) -> Result<Vec<String>, SecureStorageError> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        Ok(self.load().into_keys().collect())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;

    use super::*;

    /// A scratch, per-test directory under the OS temp dir — every test opens
    /// `FileStore` via [`FileStore::at_path`] with a tempdir-isolated path.
    fn scratch_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "frust-secure-storage-test-{}-{tag}-{n}",
            std::process::id()
        ))
    }

    /// Open a `FileStore` for `store` under `dir` — the shape the conformance
    /// suite's factory needs, so two named stores under one `dir` exercise
    /// store-name isolation.
    fn store_at(dir: &std::path::Path, store: &str) -> FileStore {
        FileStore::at_path(dir.join(format!("frust.ss.{store}.json")))
    }

    /// The full cross-backend conformance suite, run against fresh
    /// tempdir-isolated file stores.
    #[test]
    fn conformance() {
        let dir = scratch_dir("conformance");
        crate::conformance::run_conformance_suite(&|store| Box::new(store_at(&dir, store)));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A missing file starts fresh (no panic, no error) — `get`/`keys` behave
    /// like an empty store.
    #[test]
    fn missing_file_is_fresh_start() {
        let dir = scratch_dir("missing-file");
        let store = store_at(&dir, "s");
        assert_eq!(store.get("k").unwrap(), None);
        assert!(store.keys().unwrap().is_empty());
    }

    /// A corrupted (non-JSON) file is treated as an empty store rather than
    /// erroring, and a subsequent write recovers it (never a panic).
    #[test]
    fn corrupted_file_is_fresh_start_and_recovers() {
        let dir = scratch_dir("corrupted");
        let store = store_at(&dir, "s");
        fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        fs::write(&store.path, b"not json at all {{{").unwrap();

        assert_eq!(store.get("k").unwrap(), None);
        assert!(store.keys().unwrap().is_empty());

        store.set("k", "v").unwrap();
        assert_eq!(store.get("k").unwrap(), Some("v".to_string()));

        let _ = fs::remove_dir_all(&dir);
    }

    /// A JSON file that parses but isn't a string-valued object (e.g. a bare
    /// array, or an object with non-string values) is also treated as an
    /// empty store.
    #[test]
    fn non_string_object_json_is_fresh_start() {
        let dir = scratch_dir("non-object");
        let store = store_at(&dir, "s");
        fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        fs::write(&store.path, br#"{"k": 123}"#).unwrap();

        assert_eq!(store.get("k").unwrap(), None);

        let _ = fs::remove_dir_all(&dir);
    }

    /// Concurrent same-process writers (multiple threads sharing one
    /// `FileStore` behind an `Arc`) never lose a write — the in-process
    /// `Mutex` serializes every read-modify-write cycle.
    #[test]
    fn concurrent_writers_all_land() {
        let dir = scratch_dir("concurrent");
        let store = Arc::new(store_at(&dir, "s"));
        let handles: Vec<_> = (0..16)
            .map(|i| {
                let store = Arc::clone(&store);
                thread::spawn(move || {
                    store.set(&format!("key-{i}"), &format!("v{i}")).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let mut keys = store.keys().unwrap();
        keys.sort();
        let mut expected: Vec<String> = (0..16).map(|i| format!("key-{i}")).collect();
        expected.sort();
        assert_eq!(keys, expected);
        for i in 0..16 {
            assert_eq!(
                store.get(&format!("key-{i}")).unwrap(),
                Some(format!("v{i}")),
                "writer {i}'s value must survive concurrent writes"
            );
        }

        let _ = fs::remove_dir_all(&dir);
    }
}
