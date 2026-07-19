//! `frust-shared-preferences`: a platform-independent, Flutter-`shared_preferences`-shaped
//! key-value store — a NSUserDefaults/Android-SharedPreferences-backed handle
//! on Apple/Android, a JSON file on Linux/Windows.
//!
//! # Charter: a platform plugin
//!
//! Per `docs/spec.md`'s plugin-system Design Decision 1, this is a
//! **platform plugin**: it depends on [`frust_plugin`] plus FFI crates only,
//! and — like `frust-plugin` itself — carries **no other `frust-*`
//! framework dependency**. An app adds this crate to its own `Cargo.toml`
//! alongside `frust`, the same way a Flutter app adds a pub package; the
//! `frust` facade does not depend on or re-export it.
//!
//! # Value model
//!
//! Five value types (Design Decision 4 of the plugin-system plan — Flutter's
//! own `shared_preferences` set): `bool`, `i64`, `f64`, `String`,
//! `Vec<String>`. On-disk/on-platform-store Flutter compatibility is a
//! **non-goal** — every backend uses its own storage encoding (documented
//! per backend module).
//!
//! # v1 is sync, non-reactive
//!
//! Every platform store this wraps (`NSUserDefaults`, Android
//! `SharedPreferences`, this crate's own file backend) is documented
//! thread-safe with no main-thread requirement, and calls are cheap — so
//! v1 exposes a plain synchronous API (Design Decision 3), matching the
//! `frust-shell-common::theme_override` precedent. A reactive
//! `use_preference`-style wrapper is a future facade-glue addition, not
//! part of this crate.
//!
//! # Backend routing (this task)
//!
//! [`SharedPreferences::standard`] dispatches by `#[cfg(target_os = ...)]`
//! to one of three backend modules: [`file`] (Linux/Windows — the only real
//! implementation as of this task), [`android`], and [`apple`]. The
//! Android/Apple modules are **stubs** for now (every operation returns
//! [`PrefsError::Storage`]) — their real `SharedPreferences`/
//! `NSUserDefaults` implementations land in a later task; see each module's
//! doc comment. This crate still compiles (and its file-backend tests still
//! run) on every host, including this repo's macOS dev machines, where
//! `target_os = "macos"` routes through the (stub) `apple` module.

// `file` backs `standard()`'s dispatch on every target except
// Android/iOS/macOS (see below), but is also compiled under `cfg(test)` on
// *every* target (including this repo's macOS dev hosts) so its own unit
// tests and the conformance suite always run — see this crate's acceptance
// criteria ("cargo test -p frust-shared-preferences green ... on file
// backend"). Gating it this way (rather than leaving it unconditional) also
// means a non-test build on Android/iOS/macOS carries no dead-code warnings
// for a backend that build never dispatches to.
#[cfg(any(
    test,
    not(any(target_os = "android", target_os = "ios", target_os = "macos"))
))]
mod file;

#[cfg(target_os = "android")]
mod android;

#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;

#[cfg(test)]
mod conformance;

use std::sync::Arc;

/// Errors from a [`SharedPreferences`] operation.
///
/// `thiserror`-derived per `docs/CODE_STANDARDS.md`: callers match on the
/// variant (e.g. an app catching [`PrefsError::PlatformNotInitialized`] to
/// show a "re-scaffold your app" hint) rather than only displaying it.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum PrefsError {
    /// The Android host shell never installed the `(JavaVM, Context)`
    /// platform handles this backend needs (`frust-plugin`'s pre-init
    /// state). The wrapped error's message already names the fix
    /// (re-scaffold, or add the one-line `nativeInitPlatform` call).
    #[error(transparent)]
    PlatformNotInitialized(#[from] frust_plugin::PlatformHandleError),

    /// An underlying filesystem operation failed (file backend only).
    #[error("preferences I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A backend-specific storage failure that isn't an I/O error — a
    /// (de)serialization failure, or (for now) an unimplemented backend
    /// stub reporting "not yet supported".
    #[error("preferences storage error: {0}")]
    Storage(String),
}

/// The crate's internal value model — Flutter's five preference types
/// (module doc's Value model section) with Frust's own storage encodings.
/// Never part of the public API: [`SharedPreferences`]'s typed
/// `get_*`/`set_*` methods are the public surface.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PrefValue {
    Bool(bool),
    I64(i64),
    F64(f64),
    Str(String),
    StrList(Vec<String>),
}

/// One backend implementation ([`file`], and — once task 06 lands them —
/// `android`/`apple`).
///
/// Crate-private and deliberately minimal: [`SharedPreferences`]'s public
/// API translates to/from [`PrefValue`] at this seam, so a backend only
/// ever stores/retrieves the already-typed value. The conformance suite
/// ([`conformance::run_conformance_suite`], `#[cfg(test)]`) exercises any
/// `Backend` uniformly, so the same assertions this task locks in for
/// [`file::FileStore`] validate the Android/Apple backends once they land.
pub(crate) trait Backend: Send + Sync {
    /// The stored value for `key`, or `None` if absent.
    fn get(&self, key: &str) -> Option<PrefValue>;
    /// Store `value` at `key`, overwriting any existing value (including
    /// one of a different type — there is no type coexistence per key).
    fn set(&self, key: &str, value: PrefValue) -> Result<(), PrefsError>;
    /// Remove `key`, if present. Removing an absent key is a no-op success.
    fn remove(&self, key: &str) -> Result<(), PrefsError>;
    /// Remove every Frust-namespaced key this backend owns.
    fn clear(&self) -> Result<(), PrefsError>;
    /// Whether `key` currently has a stored value.
    fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
    /// Every currently-stored key (namespace prefix, if any, already
    /// stripped).
    fn keys(&self) -> Vec<String>;
}

/// A handle onto the platform's shared/user-defaults-style key-value store.
///
/// Cheap to [`Clone`] (an [`Arc`]-wrapped backend) and safe to share across
/// threads (`Send + Sync`) — every backend this wraps is documented
/// thread-safe by its platform (`NSUserDefaults`, Android
/// `SharedPreferences`, or this crate's own `Mutex`-guarded file backend).
#[derive(Clone)]
pub struct SharedPreferences {
    backend: Arc<dyn Backend>,
}

impl SharedPreferences {
    /// Open the platform's standard preferences store.
    ///
    /// See the module doc's *Backend routing* section for which backend
    /// this resolves to on each target today.
    ///
    /// # Errors
    /// [`PrefsError::Storage`] if the store's location can't be resolved
    /// (file backend, no usable data directory) or the backend isn't
    /// implemented yet (Android/Apple stubs — this task); a future Android
    /// backend can additionally return
    /// [`PrefsError::PlatformNotInitialized`].
    pub fn standard() -> Result<Self, PrefsError> {
        #[cfg(target_os = "android")]
        let backend: Arc<dyn Backend> = Arc::new(android::AndroidStore::standard()?);
        #[cfg(any(target_os = "ios", target_os = "macos"))]
        let backend: Arc<dyn Backend> = Arc::new(apple::AppleStore::standard()?);
        #[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
        let backend: Arc<dyn Backend> = Arc::new(file::FileStore::standard()?);

        Ok(Self { backend })
    }

    /// The stored `bool` at `key`, or `None` if absent or stored as a
    /// different type.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        match self.backend.get(key) {
            Some(PrefValue::Bool(v)) => Some(v),
            _ => None,
        }
    }

    /// The stored `i64` at `key`, or `None` if absent or stored as a
    /// different type.
    pub fn get_i64(&self, key: &str) -> Option<i64> {
        match self.backend.get(key) {
            Some(PrefValue::I64(v)) => Some(v),
            _ => None,
        }
    }

    /// The stored `f64` at `key`, or `None` if absent or stored as a
    /// different type. Round-trips exactly, including negative values and
    /// `NaN` (every backend stores the raw bit pattern — see [`file`]'s
    /// module doc for the file backend's encoding).
    pub fn get_f64(&self, key: &str) -> Option<f64> {
        match self.backend.get(key) {
            Some(PrefValue::F64(v)) => Some(v),
            _ => None,
        }
    }

    /// The stored `String` at `key`, or `None` if absent or stored as a
    /// different type.
    pub fn get_string(&self, key: &str) -> Option<String> {
        match self.backend.get(key) {
            Some(PrefValue::Str(v)) => Some(v),
            _ => None,
        }
    }

    /// The stored `Vec<String>` at `key`, or `None` if absent or stored as
    /// a different type.
    pub fn get_string_list(&self, key: &str) -> Option<Vec<String>> {
        match self.backend.get(key) {
            Some(PrefValue::StrList(v)) => Some(v),
            _ => None,
        }
    }

    /// Store `value` at `key`, overwriting any existing value at that key
    /// (including one of a different type).
    pub fn set_bool(&self, key: &str, value: bool) -> Result<(), PrefsError> {
        self.backend.set(key, PrefValue::Bool(value))
    }

    /// Store `value` at `key`, overwriting any existing value at that key
    /// (including one of a different type).
    pub fn set_i64(&self, key: &str, value: i64) -> Result<(), PrefsError> {
        self.backend.set(key, PrefValue::I64(value))
    }

    /// Store `value` at `key`, overwriting any existing value at that key
    /// (including one of a different type). See [`Self::get_f64`] for the
    /// round-trip contract.
    pub fn set_f64(&self, key: &str, value: f64) -> Result<(), PrefsError> {
        self.backend.set(key, PrefValue::F64(value))
    }

    /// Store `value` at `key`, overwriting any existing value at that key
    /// (including one of a different type).
    pub fn set_string(&self, key: &str, value: impl Into<String>) -> Result<(), PrefsError> {
        self.backend.set(key, PrefValue::Str(value.into()))
    }

    /// Store `value` at `key`, overwriting any existing value at that key
    /// (including one of a different type). Empty strings and non-ASCII
    /// (unicode) entries round-trip exactly.
    pub fn set_string_list(
        &self,
        key: &str,
        value: impl Into<Vec<String>>,
    ) -> Result<(), PrefsError> {
        self.backend.set(key, PrefValue::StrList(value.into()))
    }

    /// Remove `key`, if present. Removing an absent key is a no-op success.
    pub fn remove(&self, key: &str) -> Result<(), PrefsError> {
        self.backend.remove(key)
    }

    /// Remove every Frust-namespaced key this handle owns. On the two
    /// OS-shared stores (`NSUserDefaults`, Android `SharedPreferences`)
    /// this clears only Frust's own `frust.`-prefixed entries, never
    /// another app/library's keys sharing the same store (Design
    /// Decision 4).
    pub fn clear(&self) -> Result<(), PrefsError> {
        self.backend.clear()
    }

    /// Whether `key` currently has a stored value (of any type).
    pub fn contains(&self, key: &str) -> bool {
        self.backend.contains(key)
    }

    /// Every currently-stored key.
    pub fn keys(&self) -> Vec<String> {
        self.backend.keys()
    }
}
