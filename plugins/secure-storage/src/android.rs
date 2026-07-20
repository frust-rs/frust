//! Android Keystore backend — **stub** (Plan Phase 3, task S03 implements
//! the real AES-256-GCM-in-`AndroidKeyStore` CRUD via JNI). This module
//! exists on `#[cfg(target_os = "android")]` so the target-gated
//! `frust-plugin`/`jni` deps task S01b pinned in `Cargo.toml` compile and
//! link cleanly ahead of S03's/S05's work. Every [`Backend`] method here
//! returns
//! [`SecureStorageError::NotAvailable`]`(`[`Unavailability::UnsupportedPlatform`]`)`.
//!
//! `lib.rs`'s Android selection arm still constructs [`crate::file::FileStore`]
//! today (S01b's single-flip selection-point contract — see that module's
//! `open_with`), so [`AndroidStore`] is never constructed yet, hence the
//! `#[allow(dead_code)]` below. S03 flips that one line to
//! `Arc::new(android::AndroidStore)` and replaces this file's method bodies
//! — no other file needs to change for that.

use crate::{Backend, SecureStorageError, Unavailability};

/// Not yet constructed anywhere — S03 flips `lib.rs`'s Android selection arm
/// to `Arc::new(android::AndroidStore::...)`, at which point this becomes
/// real and this attribute is removed.
#[allow(dead_code)]
pub(crate) struct AndroidStore;

impl Backend for AndroidStore {
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
