//! Desktop secret-store backend (Linux/Windows) — **stub** (Plan Phase 4,
//! task S04 implements the real `keyring-core`-backed CRUD: Linux via
//! `zbus-secret-service-keyring-store`, Windows via
//! `windows-native-keyring-store`). This module exists on
//! `#[cfg(any(target_os = "linux", target_os = "windows"))]` so the
//! target-gated keyring deps task S01b pinned in `Cargo.toml` compile
//! cleanly ahead of S04's work. Every [`Backend`] method here returns
//! [`SecureStorageError::NotAvailable`]`(`[`Unavailability::UnsupportedPlatform`]`)`.
//!
//! macOS desktop is *not* covered here — it shares [`crate::apple`] with iOS
//! (`target_vendor = "apple"`).
//!
//! `lib.rs`'s linux/windows selection arm still constructs
//! [`crate::file::FileStore`] today (S01b's single-flip selection-point
//! contract — see that module's `open_with`), so [`DesktopStore`] is never
//! constructed yet, hence the `#[allow(dead_code)]` below. S04 flips that
//! one line to `Arc::new(desktop::DesktopStore)`, replaces this file's
//! method bodies, and demotes `file.rs` to `#[cfg(test)]`-only — no other
//! file needs to change for that.

use crate::{Backend, SecureStorageError, Unavailability};

/// Not yet constructed anywhere — S04 flips `lib.rs`'s linux/windows
/// selection arm to `Arc::new(desktop::DesktopStore::...)`, at which point
/// this becomes real and this attribute is removed.
#[allow(dead_code)]
pub(crate) struct DesktopStore;

impl Backend for DesktopStore {
    fn get(&self, _key: &str) -> Result<Option<String>, SecureStorageError> {
        Err(SecureStorageError::NotAvailable(
            Unavailability::UnsupportedPlatform,
        ))
    }

    fn set(&self, _key: &str, _value: &str) -> Result<(), SecureStorageError> {
        Err(SecureStorageError::NotAvailable(
            Unavailability::UnsupportedPlatform,
        ))
    }

    fn remove(&self, _key: &str) -> Result<(), SecureStorageError> {
        Err(SecureStorageError::NotAvailable(
            Unavailability::UnsupportedPlatform,
        ))
    }

    fn contains(&self, _key: &str) -> Result<bool, SecureStorageError> {
        Err(SecureStorageError::NotAvailable(
            Unavailability::UnsupportedPlatform,
        ))
    }

    fn keys(&self) -> Result<Vec<String>, SecureStorageError> {
        Err(SecureStorageError::NotAvailable(
            Unavailability::UnsupportedPlatform,
        ))
    }

    fn clear(&self) -> Result<(), SecureStorageError> {
        Err(SecureStorageError::NotAvailable(
            Unavailability::UnsupportedPlatform,
        ))
    }
}
