//! Apple Keychain backend — **stub** (Plan Phase 2, task S02 implements the
//! real `kSecClassGenericPassword` CRUD). This module exists on every Apple
//! target (`#[cfg(target_vendor = "apple")]` — iOS, macOS, tvOS, watchOS) so
//! the target-gated `objc2-security`/`objc2-local-authentication`/
//! `objc2-core-foundation` deps task S01b pinned in `Cargo.toml` compile and
//! link cleanly ahead of S02's/S05's work. Every [`Backend`] method here
//! returns
//! [`SecureStorageError::NotAvailable`]`(`[`Unavailability::UnsupportedPlatform`]`)`.
//!
//! `lib.rs`'s Apple selection arm still constructs [`crate::file::FileStore`]
//! today (S01b's single-flip selection-point contract — see that module's
//! `open_with`), so [`AppleStore`] is never constructed yet, hence the
//! `#[allow(dead_code)]` below. S02 flips that one line to
//! `Arc::new(apple::AppleStore)` and replaces this file's method bodies —
//! no other file needs to change for that.

use crate::{Backend, SecureStorageError, Unavailability};

/// Not yet constructed anywhere — S02 flips `lib.rs`'s Apple selection arm to
/// `Arc::new(apple::AppleStore::...)`, at which point this becomes real and
/// this attribute is removed.
#[allow(dead_code)]
pub(crate) struct AppleStore;

impl Backend for AppleStore {
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
