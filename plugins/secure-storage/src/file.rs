//! The storage-core [`Backend`]: one JSON map file per named store.
//!
//! `<data_dir>/<app>/frust/secure/frust.ss.<store>.json`, where `data_dir`
//! follows the platform's own convention (`$XDG_DATA_HOME`/`~/.local/share`
//! on Unix, `%APPDATA%` on Windows — mirroring
//! `frust-shared-preferences`' file backend) and `<app>` is the current
//! executable's file stem (namespaced per binary). Baking the store name into
//! the filename is what gives store-name isolation for free (two
//! [`crate::SecureStorage`] handles with different names never share a file).
//!
//! # Not secure at rest
//!
//! This backend stores plaintext JSON — it is the storage-core fallback and
//! the `#[cfg(test)]` conformance target, **not** a secure store. The
//! secure platform backends (Apple Keychain, Android Keystore, desktop
//! keyring) arrive in later phases and supersede it on their targets; on
//! Linux/Windows a future phase may keep it strictly as a documented
//! dev-preview convenience.
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
    /// Open the store `name` at the OS-resolved default location.
    ///
    /// # Errors
    /// [`SecureStorageError::Storage`] if no data directory could be resolved
    /// (an unset `HOME`/`APPDATA`) — this module never guesses a fallback
    /// that could silently write into the process's current directory.
    pub(crate) fn standard(name: &str) -> Result<Self, SecureStorageError> {
        let dir = data_dir();
        if dir.as_os_str().is_empty() {
            return Err(SecureStorageError::Storage(
                "could not resolve a user data directory (HOME/APPDATA unset)".into(),
            ));
        }
        Ok(Self::at_path(store_path(dir, name)))
    }

    /// Open the store at an explicit file path, bypassing OS data-dir
    /// resolution.
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

/// Resolve the user's data directory, following XDG/platform conventions
/// (mirrors `frust-shared-preferences`' file backend). Never panics: an
/// unresolvable environment maps to an empty path, handled by
/// [`FileStore::standard`].
fn data_dir() -> PathBuf {
    #[cfg(unix)]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            let path = PathBuf::from(&xdg);
            if path.is_absolute() {
                return path;
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".local").join("share");
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata);
        }
    }
    PathBuf::new()
}

/// `<data_dir>/<app>/frust/secure/frust.ss.<store>.json`, namespaced per
/// binary (like the desktop shell's pipeline-cache file) and per store name
/// (see module doc).
fn store_path(data_dir: PathBuf, store: &str) -> PathBuf {
    let app = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_os_string()))
        .unwrap_or_else(|| "app".into());
    let file = format!(
        "{}{}.json",
        crate::KEY_NAMESPACE_PREFIX,
        sanitize_store(store)
    );
    data_dir.join(app).join("frust").join("secure").join(file)
}

/// Make a store name safe to use as a single path component: replace any
/// character that isn't alphanumeric, `-`, or `_` with `_`. Two distinct
/// store names could in principle collide after sanitization, but store
/// names are app-chosen identifiers, not arbitrary user input; this only
/// guards against a name that would otherwise escape the intended directory
/// or contain a path separator.
fn sanitize_store(store: &str) -> String {
    store
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;

    use super::*;

    /// A scratch, per-test directory under the OS temp dir — tests must NEVER
    /// resolve [`FileStore::standard`]'s real data directory, so every test
    /// opens `FileStore` via [`FileStore::at_path`] instead.
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

    /// `standard()` never writes to (or resolves, beyond the directory check)
    /// the real data directory in a way a test could observe — this just
    /// confirms the constructor itself doesn't panic or perform I/O.
    #[test]
    fn standard_resolves_without_io() {
        // Not asserting Ok/Err (hosts vary in HOME/APPDATA presence) — only
        // that construction is infallible-by-panic.
        let _ = FileStore::standard("s");
    }

    /// A store name with path-hostile characters is sanitized to a single
    /// safe path component (no separators, no traversal).
    #[test]
    fn store_name_is_sanitized() {
        assert_eq!(sanitize_store("a/b"), "a_b");
        assert_eq!(sanitize_store("../etc"), "___etc");
        assert_eq!(sanitize_store("ok-name_1"), "ok-name_1");
    }
}
