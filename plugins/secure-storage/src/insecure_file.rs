//! Opt-in, **debug-only, plaintext** file backend for desktop previews.
//!
//! Compiled only under `cfg(all(debug_assertions, any(target_os = "linux",
//! target_os = "windows", target_os = "macos")))`, so it does not exist in a
//! release build or on mobile. It lets `cargo run` previews work on a box with
//! no Secret Service daemon. Selection is explicit: the
//! [`crate::StoreOptions::with_insecure_file_fallback`] builder or
//! `FRUST_SECURE_STORAGE_INSECURE_FILE=1` (see [`crate::insecure_file_selected`]).
//!
//! # Not secure at rest
//!
//! Values are plaintext JSON at `data_dir()/frust-secure-storage/insecure-store.json`
//! (mode `0600` on unix), shaped `{ "<store>": { "<key>": "<value>" } }`. The
//! first selection in a process logs one `warn!` naming the path.
//!
//! # Biometric gate
//!
//! None. A gated open is refused by the caller with
//! `NotAvailable(UnsupportedPlatform)`, exactly as the native desktop backend.
//!
//! # Concurrency
//!
//! Every operation is a read-modify-write of the whole file under one
//! process-wide lock (all store handles share the file); writes go to a
//! temp file and `rename` into place. A missing or corrupt file reads as empty.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{Backend, SecureStorageError};

type Document = BTreeMap<String, BTreeMap<String, String>>;

/// Serializes every read-modify-write across all handles in the process.
static FILE_LOCK: Mutex<()> = Mutex::new(());

/// Directory name under `data_dir()`.
const DIR_NAME: &str = "frust-secure-storage";
/// File name inside [`DIR_NAME`].
const FILE_NAME: &str = "insecure-store.json";

/// The plaintext JSON-file store for one named store.
pub(crate) struct InsecureFileStore {
    path: PathBuf,
    name: String,
}

/// Claim the once-per-process warning; `true` only for the first caller.
fn claim_warning() -> bool {
    static WARNED: AtomicBool = AtomicBool::new(false);
    !WARNED.swap(true, Ordering::Relaxed)
}

impl InsecureFileStore {
    /// Open store `name` under `frust_paths::data_dir()`, logging the
    /// one-time plaintext warning.
    pub(crate) fn open(name: &str) -> Result<Self, SecureStorageError> {
        let dir = frust_paths::data_dir().ok_or_else(|| {
            SecureStorageError::Storage(
                "no usable data directory for the insecure file backend".into(),
            )
        })?;
        let store = Self::at_path(dir.join(DIR_NAME).join(FILE_NAME), name);
        if claim_warning() {
            log::warn!(
                "secure-storage: INSECURE file backend active (debug build); values are stored in plaintext at {}",
                store.path.display()
            );
        }
        Ok(store)
    }

    /// Open store `name` at an explicit file path (tests inject a temp path).
    pub(crate) fn at_path(path: PathBuf, name: &str) -> Self {
        Self {
            path,
            name: name.to_string(),
        }
    }

    fn load(&self) -> Document {
        fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Apply `f` to this store's map and persist when it reports a change.
    fn update<R>(
        &self,
        f: impl FnOnce(&mut BTreeMap<String, String>) -> (R, bool),
    ) -> Result<R, SecureStorageError> {
        let _guard = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut doc = self.load();
        let (out, changed) = f(doc.entry(self.name.clone()).or_default());
        if changed {
            if doc.get(&self.name).is_some_and(BTreeMap::is_empty) {
                doc.remove(&self.name);
            }
            let bytes = serde_json::to_vec_pretty(&doc).map_err(|e| {
                SecureStorageError::Storage(format!("encoding insecure store: {e}"))
            })?;
            write_private(&self.path, &bytes)?;
        }
        Ok(out)
    }
}

/// Atomically write `bytes` to `path` (temp file then rename), `0600` on unix.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("tmp.{}", std::process::id()));
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let result = opts
        .open(&temp)
        .and_then(|mut f| f.write_all(bytes).and_then(|()| f.sync_all()))
        .and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

impl Backend for InsecureFileStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecureStorageError> {
        let _guard = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        Ok(self
            .load()
            .get(&self.name)
            .and_then(|m| m.get(key).cloned()))
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecureStorageError> {
        self.update(|m| {
            m.insert(key.to_string(), value.to_string());
            ((), true)
        })
    }

    fn remove(&self, key: &str) -> Result<(), SecureStorageError> {
        self.update(|m| ((), m.remove(key).is_some()))
    }

    fn keys(&self) -> Result<Vec<String>, SecureStorageError> {
        let _guard = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        Ok(self
            .load()
            .get(&self.name)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default())
    }

    fn clear(&self) -> Result<(), SecureStorageError> {
        self.update(|m| {
            let changed = !m.is_empty();
            m.clear();
            ((), changed)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;

    use super::*;

    /// A unique temp data dir standing in for `data_dir()`; the tests never
    /// touch the real user data directory.
    fn temp_file() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir()
            .join(format!(
                "frust-ss-insecure-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ))
            .join(DIR_NAME)
            .join(FILE_NAME)
    }

    #[test]
    fn conformance_in_temp_data_dir() {
        let path = temp_file();
        crate::conformance::run_conformance_suite(&|store| {
            Box::new(InsecureFileStore::at_path(path.clone(), store))
        });
        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn layout_and_permissions() {
        let path = temp_file();
        let a = InsecureFileStore::at_path(path.clone(), "a");
        let b = InsecureFileStore::at_path(path.clone(), "b");
        a.set("k", "v").unwrap();
        b.set("k", "w").unwrap();
        let doc: Document = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(doc["a"]["k"], "v");
        assert_eq!(doc["b"]["k"], "w");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn warning_is_claimed_once() {
        // No other test calls `claim_warning` (they use `at_path`).
        let first = claim_warning();
        let second = claim_warning();
        assert!(first);
        assert!(!second);
    }
}
