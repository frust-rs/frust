//! The Apple (iOS/macOS) [`Backend`] — Keychain-backed, via `objc2-security`.
//!
//! # Storage model — one `kSecClassGenericPassword` item per key
//!
//! Every entry is a generic-password Keychain item keyed by the pair
//! `(kSecAttrService, kSecAttrAccount)`:
//!
//! - **service** = `frust.ss.<store>` ([`crate::KEY_NAMESPACE_PREFIX`] + the
//!   store name) — the per-store namespace, so two [`crate::SecureStorage`]
//!   handles with different names never share an item, and
//!   [`clear`](AppleStore::clear) only ever deletes items for *this* service
//!   (never another library's, nor another store's).
//! - **account** = the caller's key, verbatim (unicode-safe).
//! - **value** = the caller's `String` UTF-8 bytes, stored as `kSecValueData`.
//!
//! Because the store name is baked into the service attribute, store-name
//! isolation is a property of the query itself: no key-prefixing inside the
//! item is needed (unlike the process-wide `NSUserDefaults` domain that
//! `frust-shared-preferences` shares — there keys carry a `frust.` prefix; a
//! Keychain query is already service-scoped).
//!
//! # Accessibility
//!
//! [`Accessibility`] maps to `kSecAttrAccessible` on write
//! ([`WhenUnlocked`](Accessibility::WhenUnlocked) →
//! `kSecAttrAccessibleWhenUnlocked`, the flutter_secure_storage-parity
//! default; [`AfterFirstUnlock`](Accessibility::AfterFirstUnlock) →
//! `kSecAttrAccessibleAfterFirstUnlock`). It is applied when an item is first
//! added; a later overwrite updates only the value data, leaving the
//! item's original accessibility in place.
//!
//! # Biometric gate — not in this phase
//!
//! `AuthPolicy::Required` is refused *before* a backend is ever constructed
//! (see `lib.rs`'s `open_with`), so this backend only ever serves
//! unauthenticated storage. The `SecAccessControl` biometric gate is Plan
//! Phase 5 (task S05).
//!
//! # macOS unbundled-preview caveat
//!
//! An unbundled binary (a `cargo run`/`cargo test` process with no
//! `CFBundleIdentifier`, and no data-protection-keychain entitlement) writes
//! to the login (file-based, "global") Keychain rather than an app-specific,
//! iOS-style data-protection Keychain — a storage-*location* difference from
//! a signed, bundled iOS/macOS app, not a behavioral one; the `frust.ss.`
//! service namespace keeps entries isolated either way. This backend
//! deliberately does *not* set `kSecUseDataProtectionKeychain`, which on
//! macOS requires an application-identifier entitlement an unsigned binary
//! lacks (it would fail `errSecMissingEntitlement`).
//!
//! # `unsafe`
//!
//! Confined to this module and each `# Safety`-noted, mirroring the
//! `frust-shared-preferences` `apple` backend precedent (`docs/CODE_STANDARDS.md`):
//! reading the Security-framework `extern` constant statics (edition-2024
//! requires `unsafe` to read an `extern` static — each is a linker-provided,
//! always-non-null CoreFoundation constant), and the `SecItem*` C calls
//! (`objc2-security` marks them `unsafe`). This backend holds no
//! CoreFoundation reference across calls (it rebuilds its `CFString`s per
//! operation from the Rust-`String` service name — the shared-preferences
//! precedent), so [`AppleStore`] is a plain value, trivially `Send + Sync`.

// `objc2-security`'s `errSec*` status codes are `camelCase` `const`s; using
// one in a `match` pattern trips `non_upper_case_globals` (an upstream naming
// choice, not ours). Silence it for this module — every such pattern is a
// deliberate match on a named Security-framework status.
#![allow(non_upper_case_globals)]

use std::ptr::NonNull;

use objc2_core_foundation::{
    CFArray, CFBoolean, CFData, CFDictionary, CFRetained, CFString, CFType, kCFBooleanTrue,
};
use objc2_security::{
    SecItemAdd, SecItemCopyMatching, SecItemDelete, SecItemUpdate, errSecAuthFailed, errSecDecode,
    errSecDuplicateItem, errSecInteractionNotAllowed, errSecItemNotFound, errSecMissingEntitlement,
    errSecNotAvailable, errSecParam, errSecSuccess, errSecUserCanceled, kSecAttrAccessible,
    kSecAttrAccessibleAfterFirstUnlock, kSecAttrAccessibleWhenUnlocked, kSecAttrAccount,
    kSecAttrService, kSecClass, kSecClassGenericPassword, kSecMatchLimit, kSecMatchLimitAll,
    kSecMatchLimitOne, kSecReturnAttributes, kSecReturnData, kSecValueData,
};

use crate::{Accessibility, Backend, KEY_NAMESPACE_PREFIX, SecureStorageError};

/// A Keychain-backed secure store, scoped to one `frust.ss.<store>` service.
pub(crate) struct AppleStore {
    /// The `kSecAttrService` value every item in this store carries:
    /// `frust.ss.<store>`. Held as a Rust `String` (not a retained
    /// `CFString`) so the store is trivially `Send + Sync` and holds no
    /// CoreFoundation reference across calls — rebuilt per operation.
    service: String,
    /// The `kSecAttrAccessible` posture applied to items this store adds.
    accessibility: Accessibility,
}

impl AppleStore {
    /// Open the Keychain store `store` with the given [`Accessibility`].
    ///
    /// `store` is the caller's store name; its service attribute is
    /// `frust.ss.<store>`.
    pub(crate) fn new(store: &str, accessibility: Accessibility) -> Self {
        Self {
            service: format!("{KEY_NAMESPACE_PREFIX}{store}"),
            accessibility,
        }
    }

    /// The `kSecAttrAccessible` constant for this store's [`Accessibility`].
    fn accessible(&self) -> &'static CFString {
        // SAFETY: reading `extern` Security-framework constant statics
        // (edition-2024 unsafe); each is a linker-provided, non-null
        // `&'static CFString`.
        unsafe {
            match self.accessibility {
                Accessibility::WhenUnlocked => kSecAttrAccessibleWhenUnlocked,
                Accessibility::AfterFirstUnlock => kSecAttrAccessibleAfterFirstUnlock,
            }
        }
    }
}

impl Backend for AppleStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecureStorageError> {
        let service = CFString::from_str(&self.service);
        let account = CFString::from_str(key);
        // SAFETY: reading `extern` Security-framework constant statics — each
        // a linker-provided, non-null `&'static CFString`.
        let (class, generic_pw, service_k, account_k, return_data_k, match_limit_k, match_one) = unsafe {
            (
                kSecClass,
                kSecClassGenericPassword,
                kSecAttrService,
                kSecAttrAccount,
                kSecReturnData,
                kSecMatchLimit,
                kSecMatchLimitOne,
            )
        };
        let query = dict(&[
            (as_type(class), as_type(generic_pw)),
            (as_type(service_k), as_type(&*service)),
            (as_type(account_k), as_type(&*account)),
            (as_type(return_data_k), cf_true()),
            (as_type(match_limit_k), as_type(match_one)),
        ]);

        let mut result: *const CFType = std::ptr::null();
        // SAFETY: `query` is a valid CFDictionary of CF values; `result` is a
        // valid out-pointer. On `errSecSuccess` with a `kSecReturnData` query
        // it is set to a +1-retained CFData (or left null for empty data).
        let status = unsafe { SecItemCopyMatching(as_dict(&query), &mut result) };
        match status {
            errSecItemNotFound => Ok(None),
            errSecSuccess => match NonNull::new(result.cast_mut()) {
                // Item exists but carried no (or empty) data payload.
                None => Ok(Some(String::new())),
                Some(ptr) => {
                    // SAFETY: `SecItemCopyMatching` returned a +1-retained CF
                    // value for our `kSecReturnData` query; take ownership.
                    let value: CFRetained<CFType> = unsafe { CFRetained::from_raw(ptr) };
                    let data = value.downcast::<CFData>().map_err(|_| {
                        SecureStorageError::Storage("keychain value was not CFData".into())
                    })?;
                    let text = String::from_utf8(data.to_vec()).map_err(|e| {
                        SecureStorageError::Storage(format!("keychain value not UTF-8: {e}"))
                    })?;
                    Ok(Some(text))
                }
            },
            other => Err(map_status(other)),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecureStorageError> {
        let service = CFString::from_str(&self.service);
        let account = CFString::from_str(key);
        let data = CFData::from_bytes(value.as_bytes());
        let accessible = self.accessible();
        // SAFETY: reading `extern` Security-framework constant statics.
        let (class, generic_pw, service_k, account_k, accessible_k, value_k) = unsafe {
            (
                kSecClass,
                kSecClassGenericPassword,
                kSecAttrService,
                kSecAttrAccount,
                kSecAttrAccessible,
                kSecValueData,
            )
        };

        let add = dict(&[
            (as_type(class), as_type(generic_pw)),
            (as_type(service_k), as_type(&*service)),
            (as_type(account_k), as_type(&*account)),
            (as_type(accessible_k), as_type(accessible)),
            (as_type(value_k), as_type(&*data)),
        ]);
        // SAFETY: `add` is a valid CFDictionary of CF values; null result
        // pointer (we don't need a reference to the added item).
        let status = unsafe { SecItemAdd(as_dict(&add), std::ptr::null_mut()) };
        match status {
            errSecSuccess => Ok(()),
            // The item already exists — update only its value data, leaving
            // the original accessibility posture untouched.
            errSecDuplicateItem => {
                let query = dict(&[
                    (as_type(class), as_type(generic_pw)),
                    (as_type(service_k), as_type(&*service)),
                    (as_type(account_k), as_type(&*account)),
                ]);
                let update = dict(&[(as_type(value_k), as_type(&*data))]);
                // SAFETY: both are valid CFDictionaries of CF values.
                let status = unsafe { SecItemUpdate(as_dict(&query), as_dict(&update)) };
                if status == errSecSuccess {
                    Ok(())
                } else {
                    Err(map_status(status))
                }
            }
            other => Err(map_status(other)),
        }
    }

    fn remove(&self, key: &str) -> Result<(), SecureStorageError> {
        let service = CFString::from_str(&self.service);
        let account = CFString::from_str(key);
        // SAFETY: reading `extern` Security-framework constant statics.
        let (class, generic_pw, service_k, account_k) = unsafe {
            (
                kSecClass,
                kSecClassGenericPassword,
                kSecAttrService,
                kSecAttrAccount,
            )
        };
        let query = dict(&[
            (as_type(class), as_type(generic_pw)),
            (as_type(service_k), as_type(&*service)),
            (as_type(account_k), as_type(&*account)),
        ]);
        // SAFETY: `query` is a valid CFDictionary of CF values.
        let status = unsafe { SecItemDelete(as_dict(&query)) };
        match status {
            // Removing an absent key is a no-op success.
            errSecSuccess | errSecItemNotFound => Ok(()),
            other => Err(map_status(other)),
        }
    }

    fn contains(&self, key: &str) -> Result<bool, SecureStorageError> {
        let service = CFString::from_str(&self.service);
        let account = CFString::from_str(key);
        // SAFETY: reading `extern` Security-framework constant statics.
        let (class, generic_pw, service_k, account_k, match_limit_k, match_one) = unsafe {
            (
                kSecClass,
                kSecClassGenericPassword,
                kSecAttrService,
                kSecAttrAccount,
                kSecMatchLimit,
                kSecMatchLimitOne,
            )
        };
        // No return type requested: a match yields `errSecSuccess` with no
        // payload (no data copied), so `contains` never pulls the value.
        let query = dict(&[
            (as_type(class), as_type(generic_pw)),
            (as_type(service_k), as_type(&*service)),
            (as_type(account_k), as_type(&*account)),
            (as_type(match_limit_k), as_type(match_one)),
        ]);
        // SAFETY: `query` is a valid CFDictionary of CF values; null result
        // pointer (no return type requested).
        let status = unsafe { SecItemCopyMatching(as_dict(&query), std::ptr::null_mut()) };
        match status {
            errSecSuccess => Ok(true),
            errSecItemNotFound => Ok(false),
            other => Err(map_status(other)),
        }
    }

    fn keys(&self) -> Result<Vec<String>, SecureStorageError> {
        let service = CFString::from_str(&self.service);
        // SAFETY: reading `extern` Security-framework constant statics.
        let (class, generic_pw, service_k, return_attrs_k, match_limit_k, match_all) = unsafe {
            (
                kSecClass,
                kSecClassGenericPassword,
                kSecAttrService,
                kSecReturnAttributes,
                kSecMatchLimit,
                kSecMatchLimitAll,
            )
        };
        let query = dict(&[
            (as_type(class), as_type(generic_pw)),
            (as_type(service_k), as_type(&*service)),
            (as_type(return_attrs_k), cf_true()),
            (as_type(match_limit_k), as_type(match_all)),
        ]);

        let mut result: *const CFType = std::ptr::null();
        // SAFETY: `query` is a valid CFDictionary of CF values; `result` is a
        // valid out-pointer. On `errSecSuccess` for a `kSecReturnAttributes`
        // + `kSecMatchLimitAll` query it is a +1-retained CFArray of
        // attribute CFDictionaries.
        let status = unsafe { SecItemCopyMatching(as_dict(&query), &mut result) };
        match status {
            errSecItemNotFound => Ok(Vec::new()),
            errSecSuccess => {
                let Some(ptr) = NonNull::new(result.cast_mut().cast::<CFArray<CFType>>()) else {
                    return Ok(Vec::new());
                };
                // SAFETY: take ownership of the +1-retained CFArray. Every
                // element is a Keychain attribute CFDictionary (we requested
                // `kSecReturnAttributes`).
                let array: CFRetained<CFArray<CFType>> = unsafe { CFRetained::from_raw(ptr) };
                // SAFETY: reading the `extern` `kSecAttrAccount` constant.
                let account_k = unsafe { kSecAttrAccount };
                let mut out = Vec::with_capacity(array.len());
                for item in array.to_vec() {
                    // SAFETY: each array element is a CFDictionary (attribute
                    // dictionary); reinterpret it as such to read its account.
                    let attrs: CFRetained<CFDictionary<CFType, CFType>> =
                        unsafe { CFRetained::cast_unchecked(item) };
                    if let Some(account) = attrs.get(as_type(account_k))
                        && let Ok(name) = account.downcast::<CFString>()
                    {
                        out.push(name.to_string());
                    }
                }
                Ok(out)
            }
            other => Err(map_status(other)),
        }
    }

    fn clear(&self) -> Result<(), SecureStorageError> {
        let service = CFString::from_str(&self.service);
        // SAFETY: reading `extern` Security-framework constant statics.
        let (class, generic_pw, service_k) =
            unsafe { (kSecClass, kSecClassGenericPassword, kSecAttrService) };
        // Service-scoped delete (no account) removes exactly this store's
        // items — never another service's, per the module doc.
        let query = dict(&[
            (as_type(class), as_type(generic_pw)),
            (as_type(service_k), as_type(&*service)),
        ]);
        // The macOS login (file-based) Keychain deletes only ONE matching
        // item per `SecItemDelete` call, unlike the iOS/data-protection
        // Keychain's delete-all — so loop until nothing matches. On the
        // delete-all backends the first call clears everything and the
        // second returns `errSecItemNotFound`, ending the loop immediately.
        loop {
            // SAFETY: `query` is a valid CFDictionary of CF values.
            let status = unsafe { SecItemDelete(as_dict(&query)) };
            match status {
                errSecSuccess => continue,
                errSecItemNotFound => break Ok(()),
                other => break Err(map_status(other)),
            }
        }
    }
}

/// Borrow a CoreFoundation value as its base `CFType` — disambiguates the
/// blanket `AsRef<Self>` / `AsRef<CFType>` impls every CF type carries (a
/// bare `.as_ref()` at the call site would be ambiguous).
fn as_type<T: AsRef<CFType> + ?Sized>(value: &T) -> &CFType {
    value.as_ref()
}

/// Borrow a typed `CFDictionary<CFType, CFType>` as the untyped `CFDictionary`
/// the `SecItem*` C API expects.
fn as_dict(value: &CFDictionary<CFType, CFType>) -> &CFDictionary {
    value.as_ref()
}

/// `kCFBooleanTrue` as a `&CFType` — the value for boolean query flags
/// (`kSecReturnData`, `kSecReturnAttributes`).
fn cf_true() -> &'static CFType {
    // SAFETY: reading the `extern` `kCFBooleanTrue` constant static — a
    // linker-provided, always-`Some` CoreFoundation singleton.
    let boolean: &'static CFBoolean =
        unsafe { kCFBooleanTrue }.expect("kCFBooleanTrue is non-null");
    as_type(boolean)
}

/// Build an immutable `CFDictionary<CFType, CFType>` from key/value pairs.
fn dict(pairs: &[(&CFType, &CFType)]) -> CFRetained<CFDictionary<CFType, CFType>> {
    let keys: Vec<&CFType> = pairs.iter().map(|(k, _)| *k).collect();
    let values: Vec<&CFType> = pairs.iter().map(|(_, v)| *v).collect();
    CFDictionary::from_slices(&keys, &values)
}

/// Map a non-success `OSStatus` to a typed [`SecureStorageError`].
///
/// The two unambiguously authentication-domain statuses map to their typed
/// variants ([`errSecUserCanceled`] → [`SecureStorageError::UserCanceled`],
/// [`errSecAuthFailed`] → [`SecureStorageError::AuthFailed`]) — forward-useful
/// once the Phase-5 biometric gate produces them; every other status
/// (including `errSecItemNotFound`/`errSecDuplicateItem`, which each op
/// handles before reaching here) is tolerantly folded into
/// [`SecureStorageError::Storage`] carrying a human name plus the raw code,
/// so an unknown Keychain error is never lost.
fn map_status(status: i32) -> SecureStorageError {
    match status {
        errSecUserCanceled => SecureStorageError::UserCanceled,
        errSecAuthFailed => SecureStorageError::AuthFailed,
        other => SecureStorageError::Storage(describe(other)),
    }
}

/// A human-readable `"<name> (OSStatus <code>)"` for the Keychain statuses
/// this backend can plausibly surface (named where known, raw code always
/// included so an unrecognized status is still actionable).
fn describe(status: i32) -> String {
    let name = match status {
        errSecItemNotFound => "item not found",
        errSecDuplicateItem => "duplicate item",
        errSecParam => "invalid parameter",
        errSecInteractionNotAllowed => "interaction not allowed (keychain locked)",
        errSecMissingEntitlement => "missing keychain entitlement",
        errSecNotAvailable => "keychain not available",
        errSecDecode => "decode error",
        _ => "keychain error",
    };
    format!("{name} (OSStatus {status})")
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    /// A process-unique test-run tag so a conformance run's Keychain items
    /// never collide with the host's real entries, a prior run's leftovers,
    /// or a concurrently-running test binary's items. The login Keychain
    /// persists across runs, so uniqueness plus the run's own cleanup keeps
    /// the host pristine.
    fn unique_run() -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("test.{}.{n}", std::process::id())
    }

    /// The full cross-backend conformance suite against real, isolated
    /// login-Keychain services on this host — every service opened is torn
    /// down afterwards so nothing this test wrote survives the run.
    #[test]
    fn conformance() {
        let run = unique_run();
        let opened = Mutex::new(Vec::<String>::new());
        {
            let factory = |name: &str| -> Box<dyn Backend> {
                let store = format!("{run}.{name}");
                opened.lock().unwrap().push(store.clone());
                Box::new(AppleStore::new(&store, Accessibility::WhenUnlocked))
            };
            crate::conformance::run_conformance_suite(&factory);
        }
        // Belt-and-braces: service-scoped clear of every store the suite
        // opened (the suite leaves some entries behind by design).
        for store in opened.lock().unwrap().iter() {
            let _ = AppleStore::new(store, Accessibility::WhenUnlocked).clear();
        }
    }

    /// A store opened with [`Accessibility::AfterFirstUnlock`] round-trips
    /// values the same way — the accessibility posture is a write-time
    /// attribute, not a read gate — and is cleaned up.
    #[test]
    fn after_first_unlock_round_trips() {
        let store = format!("{}.afu", unique_run());
        let backend = AppleStore::new(&store, Accessibility::AfterFirstUnlock);
        backend.clear().unwrap();

        backend.set("token", "value").unwrap();
        assert_eq!(backend.get("token").unwrap(), Some("value".to_string()));
        assert!(backend.contains("token").unwrap());

        backend.clear().unwrap();
        assert_eq!(backend.get("token").unwrap(), None);
    }

    /// Error mapping: the two authentication-domain statuses map to typed
    /// variants; everything else folds into `Storage` carrying the raw code.
    /// (Pure logic — no Keychain access, so this runs everywhere.)
    #[test]
    fn status_mapping() {
        assert!(matches!(
            map_status(errSecUserCanceled),
            SecureStorageError::UserCanceled
        ));
        assert!(matches!(
            map_status(errSecAuthFailed),
            SecureStorageError::AuthFailed
        ));
        match map_status(errSecInteractionNotAllowed) {
            SecureStorageError::Storage(msg) => {
                assert!(msg.contains(&errSecInteractionNotAllowed.to_string()));
                assert!(msg.contains("keychain locked"));
            }
            other => panic!("expected Storage, got {other:?}"),
        }
        // An unrecognized status still carries its raw code.
        match map_status(-99999) {
            SecureStorageError::Storage(msg) => assert!(msg.contains("-99999")),
            other => panic!("expected Storage, got {other:?}"),
        }
    }
}
