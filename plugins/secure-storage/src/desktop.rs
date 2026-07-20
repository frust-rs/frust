//! Desktop secret-store backend (Linux/Windows), Plan Phase 4 (task S04).
//!
//! Routes through [`keyring-core`](https://docs.rs/keyring-core) — Linux via
//! `zbus-secret-service-keyring-store` (pure-Rust Secret Service client, no
//! libdbus to vendor), Windows via `windows-native-keyring-store` (Credential
//! Manager) — the platform-store crates task S01b pinned in `Cargo.toml`. Both
//! macOS desktop and iOS still share [`crate::apple`] (`target_vendor =
//! "apple"`), so this module is `#[cfg(any(target_os = "linux", target_os =
//! "windows"))]` only.
//!
//! # Entry naming
//!
//! `keyring-core` addresses a credential by an opaque `(service, user)` pair.
//! [`DesktopStore::standard`] fixes `service` to `frust.ss.<store>` (see
//! [`crate::KEY_NAMESPACE_PREFIX`]) for every entry it opens — the same
//! per-store namespace the module doc's *Named-store-handle model* documents
//! for every OS-shared backend — and every real key's `user` value is
//! [`KEY_ENTRY_PREFIX`] plus the caller's key, keeping data entries
//! structurally distinct from the reserved index entry below regardless of
//! what string an app picks as a key.
//!
//! # Enumeration strategy: a namespaced index entry
//!
//! `keyring-core`'s `CredentialStoreApi::search` is opt-in and inconsistent
//! across the two platform stores this backend pairs with: the Linux
//! secret-service store implements it unconditionally (an attribute search
//! over `service`/`username`), but the Windows store gates it behind a
//! `search` feature this crate's `Cargo.toml` does not enable (S01b froze
//! deps ahead of this task; adding a feature there is out of this task's
//! scope guard). Rather than diverge — a Linux-only enumeration path today,
//! a silently different feature footprint on Windows if that feature were
//! added later — [`DesktopStore`] maintains one **reserved index
//! credential** per store (`user =` [`INDEX_USER`], holding a JSON array of
//! every key currently stored): [`Backend::keys`]/[`Backend::clear`] read/
//! drive it, and every [`Backend::set`]/[`Backend::remove`] keeps it in sync.
//! This is the same tradeoff `SharedPreferences`' file backend and this
//! crate's own [`crate::file::FileStore`] don't have to make (both back a
//! real directory listing / JSON map they can enumerate directly) — an
//! OS-shared secret store has no such listing primitive to fall back on.
//!
//! # Biometric gate
//!
//! `AuthPolicy::Required` never reaches this backend — [`crate::SecureStorage::open_with`]
//! refuses it centrally before any backend is constructed, and stays refused
//! on Linux/Windows permanently (no platform biometric primitive exists to
//! back it here — see `PLAN.md` Phase 4).
//!
//! # Unverified pending a Linux/Windows host
//!
//! This module compiles on this macOS host only via a cross-target `cargo
//! check` (see `docs/DEVELOPMENT.md`'s Test section) — no Secret Service
//! daemon or Windows Credential Manager session exists here to actually run
//! against. [`tests::conformance`] (`#[cfg(any(target_os = "linux", target_os
//! = "windows"))]`) exercises the real store via the shared
//! [`crate::conformance`] suite and is `#[ignore]`d for the same headless
//! reason `frust-render`'s GPU smoke test is — run it on an actual Linux
//! desktop session / Windows machine to validate.

use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

#[cfg(target_os = "windows")]
use windows_native_keyring_store::Store as PlatformStore;
#[cfg(target_os = "linux")]
use zbus_secret_service_keyring_store::Store as PlatformStore;

use crate::{Backend, KEY_NAMESPACE_PREFIX, SecureStorageError};

/// Prefix applied to every real key's `user` value before it reaches the
/// platform store — see the module doc's *Entry naming*. Guarantees a data
/// entry's `user` can never equal [`INDEX_USER`], whatever string an app
/// picks as its key.
const KEY_ENTRY_PREFIX: &str = "k:";

/// The reserved `user` value for a store's index credential (see the module
/// doc's *Enumeration strategy*) — never [`KEY_ENTRY_PREFIX`]-prefixed, so it
/// can never collide with a real key's entry.
const INDEX_USER: &str = "index";

/// Install the process-wide `keyring-core` default store exactly once. Every
/// [`DesktopStore`] in the process shares the same underlying platform store
/// (Secret Service session / Credential Manager handle) via `keyring-core`'s
/// own default-store slot, mirroring how one `NSUserDefaults`/`SharedPreferences`
/// handle backs every [`crate::SecureStorage`] on the other OS-shared
/// backends.
fn ensure_default_store() -> Result<(), SecureStorageError> {
    static INIT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT.get_or_init(|| {
        PlatformStore::new()
            .map(|store| keyring_core::set_default_store(store))
            .map_err(|e| e.to_string())
    })
    .clone()
    .map_err(SecureStorageError::Storage)
}

/// Map a `keyring-core` error to the crate's error taxonomy. `keyring-core`
/// has no gate/biometric variants of its own (this backend never carries
/// [`crate::AuthPolicy::Required`] — see the module doc) so every failure
/// here is a generic storage failure a caller can display but not usefully
/// match further.
fn map_err(err: keyring_core::Error) -> SecureStorageError {
    SecureStorageError::Storage(err.to_string())
}

/// A `keyring-core`-backed secret store, scoped to one store name via its
/// `service` value (see the module doc's *Entry naming*).
pub(crate) struct DesktopStore {
    service: String,
    /// Guards the index entry's read-modify-write cycle against concurrent
    /// same-process writers sharing this store behind an `Arc<dyn Backend>`
    /// (mirrors [`crate::file::FileStore`]'s in-process `Mutex`) — the
    /// platform store gives no read-modify-write atomicity across the two
    /// separate entries (a data key plus the index) a `set`/`remove` touches.
    index_lock: Mutex<()>,
}

impl DesktopStore {
    /// Open the store `name`, installing the process-wide default platform
    /// store on first use (see [`ensure_default_store`]).
    pub(crate) fn standard(name: &str) -> Result<Self, SecureStorageError> {
        ensure_default_store()?;
        Ok(Self {
            service: format!("{KEY_NAMESPACE_PREFIX}{name}"),
            index_lock: Mutex::new(()),
        })
    }

    /// The entry for real key `key` (see the module doc's *Entry naming*).
    fn entry(&self, key: &str) -> Result<keyring_core::Entry, SecureStorageError> {
        keyring_core::Entry::new(&self.service, &format!("{KEY_ENTRY_PREFIX}{key}"))
            .map_err(map_err)
    }

    /// This store's reserved index entry (see the module doc's *Enumeration
    /// strategy*).
    fn index_entry(&self) -> Result<keyring_core::Entry, SecureStorageError> {
        keyring_core::Entry::new(&self.service, INDEX_USER).map_err(map_err)
    }

    /// Load the index, locking first. Public-facing (used by [`Backend::keys`]).
    fn load_index(&self) -> Result<BTreeSet<String>, SecureStorageError> {
        let _guard = self.index_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.load_index_locked()
    }

    /// Load the index; caller must already hold `index_lock`. A missing
    /// index entry (a fresh store, or one just [`Backend::clear`]ed) reads as
    /// empty — never an error, matching every other backend's fresh-store
    /// contract.
    fn load_index_locked(&self) -> Result<BTreeSet<String>, SecureStorageError> {
        match self.index_entry()?.get_password() {
            Ok(json) => Ok(serde_json::from_str(&json).unwrap_or_default()),
            Err(keyring_core::Error::NoEntry) => Ok(BTreeSet::new()),
            Err(e) => Err(map_err(e)),
        }
    }

    /// Persist the index; caller must already hold `index_lock`.
    fn save_index_locked(&self, index: &BTreeSet<String>) -> Result<(), SecureStorageError> {
        let json = serde_json::to_string(index).map_err(|e| {
            SecureStorageError::Storage(format!("encoding secure-store index: {e}"))
        })?;
        self.index_entry()?.set_password(&json).map_err(map_err)
    }

    /// Add `key` to the index, if not already present.
    fn index_insert(&self, key: &str) -> Result<(), SecureStorageError> {
        let _guard = self.index_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut index = self.load_index_locked()?;
        if index.insert(key.to_string()) {
            self.save_index_locked(&index)?;
        }
        Ok(())
    }

    /// Remove `key` from the index, if present.
    fn index_remove(&self, key: &str) -> Result<(), SecureStorageError> {
        let _guard = self.index_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut index = self.load_index_locked()?;
        if index.remove(key) {
            self.save_index_locked(&index)?;
        }
        Ok(())
    }
}

impl Backend for DesktopStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecureStorageError> {
        match self.entry(key)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecureStorageError> {
        self.entry(key)?.set_password(value).map_err(map_err)?;
        self.index_insert(key)
    }

    fn remove(&self, key: &str) -> Result<(), SecureStorageError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => {}
            Err(e) => return Err(map_err(e)),
        }
        self.index_remove(key)
    }

    fn keys(&self) -> Result<Vec<String>, SecureStorageError> {
        Ok(self.load_index()?.into_iter().collect())
    }

    fn clear(&self) -> Result<(), SecureStorageError> {
        let _guard = self.index_lock.lock().unwrap_or_else(|e| e.into_inner());
        let index = self.load_index_locked()?;
        for key in &index {
            match self.entry(key)?.delete_credential() {
                Ok(()) | Err(keyring_core::Error::NoEntry) => {}
                Err(e) => return Err(map_err(e)),
            }
        }
        match self.index_entry()?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(map_err(e)),
        }
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "windows")))]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    /// The full cross-backend conformance suite ([`crate::conformance`])
    /// against the real platform store. Needs an interactive desktop session
    /// (a running Secret Service daemon reachable over the session dbus on
    /// Linux, an interactive Windows Credential Manager session) — not
    /// available in a headless CI/container run, hence `#[ignore]` (see the
    /// module doc's *Unverified pending a Linux/Windows host* and
    /// `docs/DEVELOPMENT.md`'s manual/gated tests). Each store name gets a
    /// process-unique suffix so repeated runs never collide with stale state
    /// left behind by a previous run.
    #[test]
    #[ignore = "needs a real platform secret store (Secret Service dbus session on Linux, an interactive Windows Credential Manager session) — not available headless; run manually with `cargo test -p frust-secure-storage -- --ignored` on a Linux/Windows desktop"]
    fn conformance() {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let run = std::process::id();
        crate::conformance::run_conformance_suite(&|store| {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            Box::new(
                DesktopStore::standard(&format!("test-{run}-{n}-{store}"))
                    .expect("open desktop keyring store"),
            )
        });
    }
}
