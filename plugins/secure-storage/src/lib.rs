//! `frust-secure-storage`: a platform-independent, secure key-value store —
//! iOS/macOS Keychain, Android Keystore (AES-GCM), Linux/Windows via the
//! `keyring` stack — with an **optional** per-store biometric gate.
//!
//! # Charter: a platform plugin
//!
//! Like [`frust-shared-preferences`](../frust_shared_preferences/index.html),
//! this is a **platform plugin** (see `docs/ARCHITECTURE.md`'s Module
//! Structure): it depends on `frust-plugin` (once its Android backend lands)
//! plus FFI crates only, and carries **no other `frust-*` framework
//! dependency**. An app adds this crate to its own `Cargo.toml` alongside
//! `frust`, the Flutter-pubspec model; the `frust` facade does not depend on
//! or re-export it.
//!
//! # Phased delivery
//!
//! This module is the **storage core** (plan Phase 1): the full public API,
//! a [`file`] backend, and a conformance suite, with **no platform FFI**.
//! Later phases route [`SecureStorage::open`] to native backends behind the
//! same API by `#[cfg(target_os = ...)]` (Phase 2 Apple Keychain, Phase 3
//! Android Keystore, Phase 4 desktop keyring), and add the biometric gate
//! (Phase 5). Until then every backend resolves to [`file`], and any store
//! opened with [`AuthPolicy::Required`] fails with
//! [`SecureStorageError::NotAvailable`]`(`[`Unavailability::UnsupportedPlatform`]`)`.
//!
//! # Named-store-handle model
//!
//! Unlike `SharedPreferences`' single standard store, secure storage is a
//! **named-store-handle** API ([`SecureStorage::open`]`(name)`): each store is
//! an independently-namespaced bag of secrets, so an app can partition
//! credentials (and later apply a different biometric policy per store). On
//! the OS-shared backends every store namespaces its keys `frust.ss.<store>`
//! ([`KEY_NAMESPACE_PREFIX`]) so a store can never collide with — nor
//! [`SecureStorage::clear`] ever remove — another library's entries.
//!
//! # v1 is sync, String-valued, non-reactive
//!
//! Values are `String` in v1 (`Vec<u8>` is a future enhancement). Calls are
//! synchronous; a gated call **blocks on the system biometric prompt**, so it
//! must never run on the UI thread — an app pairs it with
//! `frust-reactive`'s `spawn_blocking` (the plugin itself stays framework-free
//! per charter). See the plugin `README.md` (Phase 5) for the full contract.

// The `file` backend is the sole backend actually *dispatched to* today
// (see `open_with`'s selection point below), so it stays compiled
// unconditionally and also serves the conformance suite. S04 (Plan Phase 4)
// demotes it to a `#[cfg(test)]`-only conformance target once `desktop`
// takes over the linux/windows arm — mirroring
// `frust-shared-preferences`' backend-routing structure.
mod file;

// Backend stub modules (task S01b, Plan Phases 2-4 preamble): compiling,
// `#[cfg]`-gated `Backend` impls that every op `Err`s
// `NotAvailable(UnsupportedPlatform)`, added now so the platform-FFI deps
// below compile on every real target ahead of S02/S03/S04's work. None is
// constructed yet — `open_with`'s selection point below still routes every
// arm to `file::FileStore` until each task flips its own one line.
#[cfg(target_os = "android")]
mod android;
#[cfg(target_vendor = "apple")]
mod apple;
#[cfg(any(target_os = "linux", target_os = "windows"))]
mod desktop;

// Pure-Rust (FFI-free) helpers for the Android Keystore backend — Base64, IV
// framing, and store-id/alias derivation. Split out of `android` (which is
// `jni`-gated and so host-uncompilable) precisely so these are **host-tested**
// (task S03 requirement 6). Compiled only where used or tested — on Android, or
// in any `cargo test` build — so it is never dead code in a host non-test build.
#[cfg(any(target_os = "android", test))]
mod framing;

#[cfg(test)]
mod conformance;

use std::sync::Arc;
use std::time::Duration;

/// The namespace prefix every key gets on an OS-shared store
/// (iOS/macOS Keychain, Android Keystore-backed SharedPreferences) — the
/// concrete per-store namespace is `frust.ss.<store>` (see the module doc's
/// *Named-store-handle model*). The [`file`] backend scopes a store by
/// filename instead and does not apply this prefix inside the file.
pub const KEY_NAMESPACE_PREFIX: &str = "frust.ss.";

/// How accessible a stored secret is relative to the device lock state —
/// maps to `kSecAttrAccessible*` on Apple and to the Keystore unlock
/// requirement on Android (applied by the platform backends in Phases 2–3;
/// inert for the [`file`] backend).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Accessibility {
    /// Readable only while the device is unlocked (Apple
    /// `kSecAttrAccessibleWhenUnlocked`) — the safe default, matching
    /// flutter_secure_storage.
    #[default]
    WhenUnlocked,
    /// Readable after the first unlock following a boot, including while
    /// subsequently locked (Apple `kSecAttrAccessibleAfterFirstUnlock`) —
    /// for background access.
    AfterFirstUnlock,
}

/// Whether opening/using a store requires biometric (or device-credential)
/// authentication.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub enum AuthPolicy {
    /// No authentication — plain secure storage (the default).
    #[default]
    None,
    /// Every gated call blocks on the system biometric prompt.
    ///
    /// **Phase 1:** no platform backend implements the gate yet, so opening a
    /// store with this policy fails with
    /// [`SecureStorageError::NotAvailable`]`(`[`Unavailability::UnsupportedPlatform`]`)`.
    Required(AuthOptions),
}

/// Options controlling a store's biometric gate (only meaningful under
/// [`AuthPolicy::Required`]).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AuthOptions {
    /// Allow the device passcode/PIN/pattern as a fallback to biometrics
    /// (iOS `userPresence`, Android `DEVICE_CREDENTIAL`). When `false`, only a
    /// biometric may satisfy the gate.
    pub allow_device_credential: bool,
    /// Invalidate the store's key when the device's biometric enrollment
    /// changes (a fingerprint/face added or removed) — the safe default
    /// (`true`) surfaces a later read as [`SecureStorageError::KeyInvalidated`]
    /// rather than silently trusting a new enrollment.
    pub invalidate_on_enrollment: bool,
    /// How long a successful authentication is reused before the system
    /// prompts again; `None` re-prompts on every gated call.
    pub validity: Option<Duration>,
    /// The prompt's presentation strings.
    pub prompt: PromptSpec,
}

impl Default for AuthOptions {
    fn default() -> Self {
        Self {
            allow_device_credential: false,
            // Safe posture: a re-enrollment invalidates the key by default.
            invalidate_on_enrollment: true,
            validity: None,
            prompt: PromptSpec::default(),
        }
    }
}

/// The strings shown in the system authentication prompt. Android renders all
/// three; iOS shows [`PromptSpec::title`] as the operation prompt and ignores
/// the rest (the platform owns the button labels there).
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct PromptSpec {
    /// Prompt title (iOS operation prompt / Android `setTitle`).
    pub title: String,
    /// Prompt subtitle (Android `setSubtitle`; unused on iOS).
    pub subtitle: String,
    /// Negative-button label (Android `setNegativeButtonText`; unused on iOS,
    /// which provides its own Cancel affordance).
    pub negative_button: String,
}

/// Per-store options passed to [`SecureStorage::open_with`].
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct StoreOptions {
    /// When the store's secrets are accessible relative to device lock state.
    pub accessibility: Accessibility,
    /// Whether the store is behind a biometric gate.
    pub auth: AuthPolicy,
}

/// The result of [`SecureStorage::can_authenticate`] — an availability probe
/// that **never** shows a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CanAuthenticate {
    /// Biometric (or allowed device-credential) authentication can be
    /// performed on this device right now.
    Available,
    /// It cannot, for the given reason.
    Unavailable(Unavailability),
}

/// Why secure storage or its biometric gate is unavailable (the
/// [`SecureStorageError::NotAvailable`] payload and the
/// [`CanAuthenticate::Unavailable`] reason).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Unavailability {
    /// No biometric hardware exists on this device.
    NoHardware,
    /// Biometric hardware exists but is currently unavailable.
    HardwareUnavailable,
    /// No biometric is enrolled.
    NotEnrolled,
    /// A device passcode/credential is required but not set.
    PasscodeNotSet,
    /// The OS version is too old for the framework biometric API (Android
    /// `< 28`).
    UnsupportedApiLevel,
    /// The Android Kotlin biometric helper class is absent from the app (a
    /// project that added the plugin without the biometric setup step, or an
    /// R8-stripped build missing the keep rule).
    HelperMissing,
    /// The current platform/build has no implementation for this operation —
    /// including every biometric-gated open while the storage core (plan
    /// Phase 1) ships no platform backend yet.
    UnsupportedPlatform,
}

/// Errors from a [`SecureStorage`] operation.
///
/// `thiserror`-derived per `docs/CODE_STANDARDS.md`: callers match on the
/// variant (e.g. distinguishing [`Self::UserCanceled`] from
/// [`Self::AuthFailed`] to decide whether to retry) rather than only
/// displaying it. The union taxonomy spans the storage layer and the
/// biometric gate; the gate variants are produced by the platform backends
/// in later phases.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum SecureStorageError {
    /// The Android host shell never installed the `(JavaVM, Context)`
    /// platform handles the Keystore backend needs (`frust-plugin`'s pre-init
    /// state) — an old scaffold predating `nativeInitPlatform`. Never a
    /// panic; the caller falls back to defaults.
    #[error("secure storage platform not initialized")]
    PlatformNotInitialized,

    /// Secure storage, or its biometric gate, is unavailable for the given
    /// reason (see [`Unavailability`]).
    #[error("secure storage unavailable: {0:?}")]
    NotAvailable(Unavailability),

    /// The user dismissed the authentication prompt.
    #[error("authentication canceled by the user")]
    UserCanceled,

    /// The system canceled the authentication prompt (app backgrounded,
    /// another prompt took over).
    #[error("authentication canceled by the system")]
    SystemCanceled,

    /// Too many failed attempts — biometrics are locked out until the next
    /// successful device unlock.
    #[error("authentication temporarily locked out")]
    LockoutTemporary,

    /// Biometrics are locked out until a strong-auth (passcode) unlock.
    #[error("authentication permanently locked out")]
    LockoutPermanent,

    /// The store's key was invalidated by a biometric re-enrollment (see
    /// [`AuthOptions::invalidate_on_enrollment`]); the recovery is to remove
    /// and re-set the affected entries.
    #[error("the store key was invalidated by a biometric enrollment change")]
    KeyInvalidated,

    /// Authentication ran to completion but did not succeed.
    #[error("authentication failed")]
    AuthFailed,

    /// A backend-specific storage failure that isn't an I/O error — a
    /// (de)serialization failure, or a platform Keystore/Keychain error.
    #[error("secure storage error: {0}")]
    Storage(String),

    /// An underlying filesystem operation failed (file backend / desktop
    /// fallback).
    #[error("secure storage I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// One backend implementation — the [`file`] backend today, and the
/// `#[cfg]`-gated `apple`/`android`/`desktop` backends in later phases.
///
/// Crate-private and deliberately minimal: [`SecureStorage`]'s public API is
/// a thin, String-valued wrapper over one of these, scoped to a single named
/// store. The conformance suite ([`conformance::run_conformance_suite`],
/// `#[cfg(test)]`) exercises any backend factory uniformly, so the assertions
/// this task locks in for [`file::FileStore`] will validate the platform
/// backends once they land.
///
/// Every method returns a [`Result`] (unlike `SharedPreferences`' infallible
/// `get`): a secure-store read can genuinely fail — a locked Keychain, a
/// pending biometric prompt, an invalidated key.
pub(crate) trait Backend: Send + Sync {
    /// The stored value for `key`, or `None` if absent.
    fn get(&self, key: &str) -> Result<Option<String>, SecureStorageError>;
    /// Store `value` at `key`, overwriting any existing value.
    fn set(&self, key: &str, value: &str) -> Result<(), SecureStorageError>;
    /// Remove `key`, if present. Removing an absent key is a no-op success.
    fn remove(&self, key: &str) -> Result<(), SecureStorageError>;
    /// Whether `key` currently has a stored value.
    fn contains(&self, key: &str) -> Result<bool, SecureStorageError> {
        Ok(self.get(key)?.is_some())
    }
    /// Every currently-stored key in this store (namespace prefix, if any,
    /// already stripped).
    fn keys(&self) -> Result<Vec<String>, SecureStorageError>;
    /// Remove every key this store owns.
    fn clear(&self) -> Result<(), SecureStorageError>;
}

/// A handle onto one named secure store.
///
/// Cheap to [`Clone`] (an [`Arc`]-wrapped backend) and safe to share across
/// threads (`Send + Sync`). Open one with [`SecureStorage::open`] (no
/// authentication) or [`SecureStorage::open_with`] (custom
/// [`StoreOptions`]).
#[derive(Clone)]
pub struct SecureStorage {
    backend: Arc<dyn Backend>,
}

impl SecureStorage {
    /// Open the named secure store with default options (no biometric gate,
    /// [`Accessibility::WhenUnlocked`]).
    ///
    /// See the module doc's *Named-store-handle model* for how a store name
    /// is namespaced on each backend.
    ///
    /// # Errors
    /// [`SecureStorageError::Storage`] if the store's location can't be
    /// resolved (file backend, no usable data directory). On Android (Phase
    /// 3), [`SecureStorageError::PlatformNotInitialized`] for an old scaffold.
    pub fn open(name: &str) -> Result<Self, SecureStorageError> {
        Self::open_with(name, StoreOptions::default())
    }

    /// Open the named secure store with explicit [`StoreOptions`].
    ///
    /// # Errors
    /// As [`Self::open`], plus — in this phase — every store opened with
    /// [`AuthPolicy::Required`] fails with
    /// [`SecureStorageError::NotAvailable`]`(`[`Unavailability::UnsupportedPlatform`]`)`,
    /// because no platform backend implements the biometric gate yet.
    pub fn open_with(name: &str, options: StoreOptions) -> Result<Self, SecureStorageError> {
        if matches!(options.auth, AuthPolicy::Required(_)) {
            // Storage core (Phase 1): the biometric gate has no platform
            // implementation yet — a typed, non-panicking refusal, not a
            // silent downgrade to unauthenticated storage.
            return Err(SecureStorageError::NotAvailable(
                Unavailability::UnsupportedPlatform,
            ));
        }

        // FINAL backend routing (task S01b, Plan Phases 2-4 preamble): apple
        // targets -> `apple`, android -> `android`, linux/windows ->
        // `desktop`. Until each platform backend lands, every arm below
        // still constructs `file::FileStore` — this is the single,
        // clearly-marked selection point each task flips its own one line
        // of, and no other line in this file (or any other shared file)
        // needs to change to land a platform backend:
        //   - S02 flips the apple arm to `Arc::new(apple::AppleStore)`.
        //   - S03 flips the android arm to `Arc::new(android::AndroidStore)`.
        //   - S04 flips the linux/windows arm to
        //     `Arc::new(desktop::DesktopStore)` (and demotes `file.rs` to
        //     `#[cfg(test)]`-only).
        #[cfg(target_vendor = "apple")]
        let backend: Arc<dyn Backend> = Arc::new(file::FileStore::standard(name)?);
        #[cfg(target_os = "android")]
        let backend: Arc<dyn Backend> = Arc::new(file::FileStore::standard(name)?);
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        let backend: Arc<dyn Backend> = Arc::new(file::FileStore::standard(name)?);

        Ok(Self { backend })
    }

    /// Probe whether biometric (or allowed device-credential) authentication
    /// is available on this device, **without** showing a prompt.
    ///
    /// In this phase no platform backend implements the gate, so this always
    /// reports [`CanAuthenticate::Unavailable`]`(`[`Unavailability::UnsupportedPlatform`]`)`.
    pub fn can_authenticate() -> CanAuthenticate {
        CanAuthenticate::Unavailable(Unavailability::UnsupportedPlatform)
    }

    /// The stored value at `key`, or `None` if absent.
    ///
    /// # Errors
    /// A backend read failure ([`SecureStorageError::Storage`]/
    /// [`SecureStorageError::Io`]) or, for a gated store on a platform
    /// backend, an authentication error.
    pub fn get(&self, key: &str) -> Result<Option<String>, SecureStorageError> {
        self.backend.get(key)
    }

    /// Store `value` at `key`, overwriting any existing value.
    pub fn set(&self, key: &str, value: &str) -> Result<(), SecureStorageError> {
        self.backend.set(key, value)
    }

    /// Remove `key`, if present. Removing an absent key is a no-op success.
    pub fn remove(&self, key: &str) -> Result<(), SecureStorageError> {
        self.backend.remove(key)
    }

    /// Whether `key` currently has a stored value.
    pub fn contains(&self, key: &str) -> Result<bool, SecureStorageError> {
        self.backend.contains(key)
    }

    /// Every currently-stored key in this store.
    pub fn keys(&self) -> Result<Vec<String>, SecureStorageError> {
        self.backend.keys()
    }

    /// Remove every key this store owns. On an OS-shared backend this clears
    /// only this store's `frust.ss.<store>`-namespaced entries, never another
    /// library's keys.
    pub fn clear(&self) -> Result<(), SecureStorageError> {
        self.backend.clear()
    }
}
