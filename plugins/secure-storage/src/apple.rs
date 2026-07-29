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
//! `kSecAttrAccessibleAfterFirstUnlock`). It is applied when an item is
//! first added; an overwrite's handling of the protection posture is
//! **asymmetric by store kind**, deliberately:
//!
//! - A **plain** (`auth: None`) store's overwrite updates only the value
//!   data (`SecItemUpdate`), leaving the item's original accessibility (or
//!   a pre-existing gated item's `SecAccessControl`) untouched — a plain
//!   `set` must never silently strip a gated item's protection.
//! - A **gated** (`auth: Some`) store's overwrite instead deletes and
//!   re-adds the item, re-using the freshly built protection pair — Keychain
//!   does not reliably support mutating `kSecAttrAccessControl` via
//!   `SecItemUpdate` (write-once-at-creation), so delete+re-add is the
//!   ecosystem-wide convention. This is what lets a key first written while
//!   the store was plain (or under stale gate flags) pick up the *current*
//!   `AuthOptions`-derived protection the moment it's rewritten through a
//!   gated store, rather than keeping whatever posture it was created
//!   under. See [`Backend::set`]'s `errSecDuplicateItem` arm.
//!
//! The residual limitation: a key only ever **read** (never rewritten)
//! after a plain→gated switch keeps its old, unprotected posture — the
//! re-assertion above only fires on a write. Retroactive protection for an
//! existing store requires rewriting (`get` + `set`) every key, not just
//! reopening the store gated.
//!
//! # Biometric gate
//!
//! When a store is opened with [`AuthPolicy::Required`](crate::AuthPolicy)
//! the [`AuthOptions`] travel in as [`AppleStore::auth`]. A gated store's
//! items are created with a `kSecAttrAccessControl` object instead of a plain
//! `kSecAttrAccessible` value: [`SecAccessControl::with_flags`] over the same
//! accessibility protection, with flags derived from the options —
//! [`BiometryCurrentSet`](SecAccessControlCreateFlags::BiometryCurrentSet)
//! when [`AuthOptions::invalidate_on_enrollment`] (the safe default: a
//! re-enrollment invalidates the item), else
//! [`BiometryAny`](SecAccessControlCreateFlags::BiometryAny), plus
//! `… | Or | DevicePasscode` when
//! [`AuthOptions::allow_device_credential`] permits the passcode fallback.
//! A gated **read** then attaches an [`LAContext`] to the
//! `SecItemCopyMatching` query (`kSecUseAuthenticationContext`, with
//! `kSecUseOperationPrompt` as the pre-context-era fallback): the context
//! carries [`PromptSpec::title`](crate::PromptSpec::title) as its
//! `localizedReason` and [`AuthOptions::validity`] as its
//! `touchIDAuthenticationAllowableReuseDuration` (the re-prompt window), so
//! the call **blocks while the system renders the Face ID/Touch ID dialog**.
//! [`can_authenticate`] probes availability without prompting via
//! [`LAContext::canEvaluatePolicy_error`], mapping the `LAError` code to an
//! [`Unavailability`]. All of this is compiled on the iOS/macOS gates but
//! only exercised by a **physical-device manual gate** — the Simulator
//! cannot render the biometric prompt (`docs/DEVELOPMENT.md`), and an
//! unauthenticated (`auth: None`) store is unaffected (host conformance).
//! The write side of the gate has the same host limitation, one step
//! further: on an unsigned/ad-hoc-signed host binary (no `TeamIdentifier`,
//! e.g. a plain `cargo test`/`cargo run`), `SecItemAdd`ing *any*
//! `kSecAttrAccessControl`-protected item — not just a gated read —
//! fails `errSecMissingEntitlement` (-34018), confirmed independent of any
//! particular code path here. A gated store's `set()` is host-conformance-
//! tested only up to that documented, expected failure (see
//! `apple::tests::gated_set_reprotects_plain_item`); a real gated write is
//! a signed-app-or-physical-device feature like the read side.
//!
//! A gated LAContext value is an Objective-C object, not a CoreFoundation
//! type; passing it into the CF keychain query dictionary needs one confined,
//! `# Safety`-noted pointer bridge ([`as_cf`]) — objc objects are
//! CFTypeRef-compatible at the ABI level (a thin, confined CF shim).
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
use objc2_foundation::NSString;
use objc2_local_authentication::{LAContext, LAError, LAPolicy};
use objc2_security::{
    SecAccessControl, SecAccessControlCreateFlags, SecItemAdd, SecItemCopyMatching, SecItemDelete,
    SecItemUpdate, errSecAuthFailed, errSecDecode, errSecDuplicateItem,
    errSecInteractionNotAllowed, errSecItemNotFound, errSecMissingEntitlement, errSecNotAvailable,
    errSecParam, errSecSuccess, errSecUserCanceled, kSecAttrAccessControl, kSecAttrAccessible,
    kSecAttrAccessibleAfterFirstUnlock, kSecAttrAccessibleWhenUnlocked, kSecAttrAccount,
    kSecAttrService, kSecClass, kSecClassGenericPassword, kSecMatchLimit, kSecMatchLimitAll,
    kSecMatchLimitOne, kSecReturnAttributes, kSecReturnData, kSecUseAuthenticationContext,
    kSecValueData,
};

use crate::{
    Accessibility, AuthOptions, Backend, CanAuthenticate, KEY_NAMESPACE_PREFIX, SecureStorageError,
    Unavailability,
};

/// A Keychain-backed secure store, scoped to one `frust.ss.<store>` service.
pub(crate) struct AppleStore {
    /// The `kSecAttrService` value every item in this store carries:
    /// `frust.ss.<store>`. Held as a Rust `String` (not a retained
    /// `CFString`) so the store is trivially `Send + Sync` and holds no
    /// CoreFoundation reference across calls — rebuilt per operation.
    service: String,
    /// The `kSecAttrAccessible` posture applied to items this store adds.
    accessibility: Accessibility,
    /// The per-store biometric policy: `None` for a plain store, `Some` for a
    /// gated one. Held as plain data (a Rust struct of `bool`/`Option`/
    /// `String`s) so [`AppleStore`] stays `Send + Sync` — the `SecAccessControl`
    /// and `LAContext` objects are rebuilt per operation, never held across
    /// calls (mirroring `service`).
    auth: Option<AuthOptions>,
}

impl AppleStore {
    /// Open the Keychain store `store` with the given [`Accessibility`] and an
    /// optional biometric [`AuthOptions`] gate.
    ///
    /// `store` is the caller's store name; its service attribute is
    /// `frust.ss.<store>`.
    pub(crate) fn new(
        store: &str,
        accessibility: Accessibility,
        auth: Option<AuthOptions>,
    ) -> Self {
        Self {
            service: format!("{KEY_NAMESPACE_PREFIX}{store}"),
            accessibility,
            auth,
        }
    }

    /// The `kSecAttrAccessible` constant for this store's [`Accessibility`] —
    /// the "protection" both a plain `kSecAttrAccessible` value and a gated
    /// item's [`SecAccessControl`] are built from.
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

    /// The `SecAccessControl` object for this store's [`AuthOptions`], built
    /// per-write over the accessibility protection (see the module doc's
    /// *Biometric gate* for the flag mapping). Only called when
    /// [`Self::auth`] is `Some`.
    fn access_control(
        &self,
        opts: &AuthOptions,
    ) -> Result<CFRetained<SecAccessControl>, SecureStorageError> {
        let flags = access_flags(opts);
        let protection = as_type(self.accessible());
        let mut error: *mut objc2_core_foundation::CFError = std::ptr::null_mut();
        // SAFETY: `protection` is a valid `kSecAttrAccessible*` CFString (the
        // correct type this call requires); `error` is a valid out-pointer.
        // On failure the call returns `None` (and may set `*error`); on
        // success a +1-retained `SecAccessControl` we own.
        let control =
            unsafe { SecAccessControl::with_flags(None, protection, flags, &mut error as *mut _) };
        control.ok_or_else(|| {
            SecureStorageError::Storage(format!(
                "SecAccessControlCreateWithFlags failed for flags {:#x}",
                flags.0
            ))
        })
    }

    /// An [`LAContext`] configured for this store's gated **reads**: its
    /// `localizedReason` is the prompt title and its
    /// `touchIDAuthenticationAllowableReuseDuration` is the options' re-prompt
    /// window (`0` when [`AuthOptions::validity`] is `None`, i.e. prompt every
    /// time). Kept alive by the caller for the duration of the keychain call
    /// it is attached to. Only called when [`Self::auth`] is `Some`.
    fn auth_context(&self, opts: &AuthOptions) -> objc2::rc::Retained<LAContext> {
        let reason = NSString::from_str(&opts.prompt.title);
        let reuse = opts.validity.map(|d| d.as_secs_f64()).unwrap_or(0.0);
        // SAFETY: `LAContext::new` returns a fresh, valid retained context;
        // `reason` is a valid `NSString`; both setters are plain property
        // writes with in-range args (`objc2` marks the allocation and setters
        // unsafe).
        unsafe {
            let context = LAContext::new();
            context.setLocalizedReason(&reason);
            context.setTouchIDAuthenticationAllowableReuseDuration(reuse);
            context
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
        let mut pairs: Vec<(&CFType, &CFType)> = vec![
            (as_type(class), as_type(generic_pw)),
            (as_type(service_k), as_type(&*service)),
            (as_type(account_k), as_type(&*account)),
            (as_type(return_data_k), cf_true()),
            (as_type(match_limit_k), as_type(match_one)),
        ];

        // A gated read attaches the biometric prompt context: `LAContext`
        // (reuse-duration + reason) under `kSecUseAuthenticationContext`, plus
        // the pre-context-era `kSecUseOperationPrompt` message. Both the
        // context and the prompt CFString must outlive the keychain call, so
        // they are bound here and referenced (never moved) into `pairs`.
        let context;
        let op_prompt;
        if let Some(opts) = &self.auth {
            // `kSecUseOperationPrompt` is deprecated in favor of the
            // `LAContext.localizedReason` we also set, but is kept as the
            // pre-context-era fallback this backend specifies; the scoped
            // allow covers exactly that one deliberate use.
            #[allow(deprecated)]
            // SAFETY: reading `extern` Security-framework constant statics.
            let (auth_ctx_k, op_prompt_k) = unsafe {
                (
                    kSecUseAuthenticationContext,
                    objc2_security::kSecUseOperationPrompt,
                )
            };
            context = self.auth_context(opts);
            op_prompt = CFString::from_str(&opts.prompt.title);
            pairs.push((as_type(auth_ctx_k), as_cf(&context)));
            pairs.push((as_type(op_prompt_k), as_type(&*op_prompt)));
        }
        let query = dict(&pairs);

        let mut result: *const CFType = std::ptr::null();
        // SAFETY: `query` is a valid CFDictionary of CF values; `result` is a
        // valid out-pointer. On `errSecSuccess` with a `kSecReturnData` query
        // it is set to a +1-retained CFData (or left null for empty data). A
        // gated read blocks here while the system renders the biometric prompt.
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
        // SAFETY: reading `extern` Security-framework constant statics.
        let (class, generic_pw, service_k, account_k, accessible_k, access_control_k, value_k) = unsafe {
            (
                kSecClass,
                kSecClassGenericPassword,
                kSecAttrService,
                kSecAttrAccount,
                kSecAttrAccessible,
                kSecAttrAccessControl,
                kSecValueData,
            )
        };

        // A gated store protects the item with a `SecAccessControl` object
        // (`kSecAttrAccessControl`); a plain store uses a `kSecAttrAccessible`
        // value. The two are mutually exclusive, so build exactly one — held
        // in `control` (kept alive across the add) only in the gated case.
        let control = match &self.auth {
            Some(opts) => Some(self.access_control(opts)?),
            None => None,
        };
        let protection_pair = match &control {
            Some(control) => (as_type(access_control_k), as_type(&**control)),
            None => (as_type(accessible_k), as_type(self.accessible())),
        };

        let add = dict(&[
            (as_type(class), as_type(generic_pw)),
            (as_type(service_k), as_type(&*service)),
            (as_type(account_k), as_type(&*account)),
            protection_pair,
            (as_type(value_k), as_type(&*data)),
        ]);
        // SAFETY: `add` is a valid CFDictionary of CF values; null result
        // pointer (we don't need a reference to the added item).
        let status = unsafe { SecItemAdd(as_dict(&add), std::ptr::null_mut()) };
        match status {
            errSecSuccess => Ok(()),
            // The item already exists. A gated store re-asserts protection
            // (delete + re-add — see below); a plain store keeps the
            // existing value-only update.
            errSecDuplicateItem if self.auth.is_some() => {
                // Keychain does not reliably support mutating
                // `kSecAttrAccessControl` via `SecItemUpdate` — it's a
                // write-once-at-creation attribute, so the ecosystem-wide
                // convention (and this store's) is delete-then-re-add,
                // re-using `add` above (it already carries the correct
                // protection pair: a fresh `SecAccessControl` built from
                // this store's current `AuthOptions`). This is how a plain
                // item written before the store was reopened gated (or a
                // stale-flags gated item) picks up the current protection
                // on every write, rather than keeping whatever posture it
                // was created under.
                let query = dict(&[
                    (as_type(class), as_type(generic_pw)),
                    (as_type(service_k), as_type(&*service)),
                    (as_type(account_k), as_type(&*account)),
                ]);
                // SAFETY: `query` is a valid CFDictionary of CF values.
                let del_status = unsafe { SecItemDelete(as_dict(&query)) };
                if del_status != errSecSuccess && del_status != errSecItemNotFound {
                    return Err(map_status(del_status));
                }
                // SAFETY: `add` is a valid CFDictionary of CF values; null
                // result pointer (we don't need a reference to the added
                // item).
                let add_status = unsafe { SecItemAdd(as_dict(&add), std::ptr::null_mut()) };
                if add_status == errSecSuccess {
                    Ok(())
                } else {
                    // Distinct from `map_status`: the delete already
                    // succeeded, so this is no longer a value-preserving
                    // update — the item may now be entirely absent rather
                    // than left at its old value.
                    Err(SecureStorageError::Storage(format!(
                        "keychain re-add after delete failed during gated set — item may now be absent: {}",
                        describe(add_status)
                    )))
                }
            }
            // Plain store: update only the value data, leaving the
            // original accessibility posture untouched. A plain-store
            // overwrite must never delete-and-strip a pre-existing gated
            // item's protection — only a gated store (above) re-asserts it.
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

/// Borrow an [`LAContext`] (an Objective-C object) as a `&CFType` so it can be
/// a value in the CoreFoundation keychain query dictionary
/// (`kSecUseAuthenticationContext`). This is the single confined CF bridge:
/// an Objective-C `id` and a `CFTypeRef` are the same thing
/// at the ABI level, so an `LAContext` pointer *is* a valid `CFType` pointer.
///
/// # Safety
/// The returned reference borrows `context`; the caller keeps `context` alive
/// for at least as long as the returned `&CFType` is used (here, until the
/// `SecItemCopyMatching` call that reads the query completes). No ownership is
/// transferred — the reference is non-owning, matching every other `&CFType`
/// value placed in the query dictionary.
fn as_cf(context: &LAContext) -> &CFType {
    // SAFETY: `LAContext` is an Objective-C object, hence a valid CFTypeRef;
    // reinterpreting `&LAContext` as `&CFType` is sound (identical ABI, same
    // lifetime), and the CF query only ever *reads* it (no CF mutation).
    unsafe { &*(std::ptr::from_ref(context).cast::<CFType>()) }
}

/// The [`SecAccessControlCreateFlags`] for a gated store's [`AuthOptions`]
/// (see the module doc's *Biometric gate*): a
/// biometry constraint, current-set (auto-invalidating on re-enrollment) when
/// [`AuthOptions::invalidate_on_enrollment`] else any-enrollment, widened with
/// a device-passcode fallback when [`AuthOptions::allow_device_credential`].
fn access_flags(opts: &AuthOptions) -> SecAccessControlCreateFlags {
    let base = if opts.invalidate_on_enrollment {
        SecAccessControlCreateFlags::BiometryCurrentSet
    } else {
        SecAccessControlCreateFlags::BiometryAny
    };
    if opts.allow_device_credential {
        base | SecAccessControlCreateFlags::Or | SecAccessControlCreateFlags::DevicePasscode
    } else {
        base
    }
}

/// Probe whether biometric authentication can be evaluated on this device,
/// **without** prompting — [`crate::SecureStorage::can_authenticate`]'s Apple
/// arm. Uses [`LAContext::canEvaluatePolicy_error`] with
/// `DeviceOwnerAuthenticationWithBiometrics`; a failure's `LAError` code maps
/// to an [`Unavailability`] (see [`la_unavailability`]).
pub(crate) fn can_authenticate() -> CanAuthenticate {
    // SAFETY: `LAContext::new` returns a fresh, valid retained context;
    // `canEvaluatePolicy_error` does not prompt — it only reports whether the
    // policy *could* be evaluated, returning `Ok(())` when it can or an
    // `NSError` (carrying an `LAError` code) when it cannot.
    let result = unsafe {
        let context = LAContext::new();
        context.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthenticationWithBiometrics)
    };
    match result {
        Ok(()) => CanAuthenticate::Available,
        Err(error) => CanAuthenticate::Unavailable(la_unavailability(LAError(error.code()))),
    }
}

/// Map an [`LAError`] from a `canEvaluatePolicy` failure to the
/// [`Unavailability`] reason [`can_authenticate`] reports.
fn la_unavailability(code: LAError) -> Unavailability {
    match code {
        LAError::BiometryNotAvailable => Unavailability::HardwareUnavailable,
        LAError::BiometryNotEnrolled => Unavailability::NotEnrolled,
        LAError::PasscodeNotSet => Unavailability::PasscodeNotSet,
        LAError::BiometryNotPaired | LAError::BiometryDisconnected => Unavailability::NoHardware,
        // Lockout / any other probe failure: biometrics exist but can't be
        // used right now. No `Unavailability` lockout variant exists (that is a
        // per-call `SecureStorageError`), so this is the closest availability
        // answer.
        _ => Unavailability::HardwareUnavailable,
    }
}

/// Map an [`LAError`] to a typed [`SecureStorageError`] — the reference
/// taxonomy for the gate's failure modes. The gated *keychain* path surfaces
/// these as `OSStatus` codes through [`map_status`] rather than `LAError`, so
/// this is used where an `LAContext` policy evaluation itself reports an
/// `NSError` (retained here for completeness and to document the mapping
/// this backend locks in).
///
/// Not wired into the gated keychain read path (which surfaces `OSStatus`
/// through [`map_status`]), so it is dead in a non-test build — kept as the
/// canonical, test-verified `LAError` taxonomy this module enumerates, hence
/// the explicit allow.
#[allow(dead_code)]
fn map_la_error(code: LAError) -> SecureStorageError {
    match code {
        LAError::UserCancel | LAError::UserFallback => SecureStorageError::UserCanceled,
        LAError::SystemCancel | LAError::AppCancel => SecureStorageError::SystemCanceled,
        LAError::AuthenticationFailed => SecureStorageError::AuthFailed,
        LAError::PasscodeNotSet => SecureStorageError::NotAvailable(Unavailability::PasscodeNotSet),
        LAError::BiometryNotAvailable => {
            SecureStorageError::NotAvailable(Unavailability::HardwareUnavailable)
        }
        LAError::BiometryNotEnrolled => {
            SecureStorageError::NotAvailable(Unavailability::NotEnrolled)
        }
        LAError::BiometryLockout => SecureStorageError::LockoutTemporary,
        other => SecureStorageError::Storage(format!("LAError {}", other.0)),
    }
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
                Box::new(AppleStore::new(&store, Accessibility::WhenUnlocked, None))
            };
            crate::conformance::run_conformance_suite(&factory);
        }
        // Belt-and-braces: service-scoped clear of every store the suite
        // opened (the suite leaves some entries behind by design).
        for store in opened.lock().unwrap().iter() {
            let _ = AppleStore::new(store, Accessibility::WhenUnlocked, None).clear();
        }
    }

    /// A store opened with [`Accessibility::AfterFirstUnlock`] round-trips
    /// values the same way — the accessibility posture is a write-time
    /// attribute, not a read gate — and is cleaned up.
    #[test]
    fn after_first_unlock_round_trips() {
        let store = format!("{}.afu", unique_run());
        let backend = AppleStore::new(&store, Accessibility::AfterFirstUnlock, None);
        backend.clear().unwrap();

        backend.set("token", "value").unwrap();
        assert_eq!(backend.get("token").unwrap(), Some("value".to_string()));
        assert!(backend.contains("token").unwrap());

        backend.clear().unwrap();
        assert_eq!(backend.get("token").unwrap(), None);
    }

    /// Regression test: a value written while a store is **plain** gets
    /// its Keychain protection re-asserted the moment it's overwritten
    /// through a **gated** store opened on the same service — exercising
    /// the `errSecDuplicateItem` arm's gated delete+re-add path. Host-safe:
    /// `set()` never attaches an `LAContext` (only a gated `get()` does —
    /// see the module doc's *Biometric gate*), so this triggers no prompt
    /// either way.
    ///
    /// This host (an unsigned/ad-hoc-signed `cargo test` binary, no
    /// `TeamIdentifier`) cannot actually create a `kSecAttrAccessControl`
    /// item at all — even a fresh, non-duplicate `SecItemAdd` with an
    /// access-control pair fails `errSecMissingEntitlement` (-34018),
    /// confirmed independent of this fix (same failure on the unmodified
    /// success-path `SecItemAdd` a few lines above the `errSecDuplicateItem`
    /// arm this fix touches) — a stronger, write-side sibling of the
    /// module doc's already-documented gated-*read* physical-device-only
    /// caveat. So the exact outcome this test can assert on *this* host is
    /// that the gated overwrite exercises the delete+re-add branch to
    /// completion (the prior plain item is gone either way) and fails with
    /// exactly that documented, expected status — not a panic, not a
    /// different error, and not a silent success that skipped the branch.
    /// On a properly entitled/signed host (or a real device) the same
    /// assertions accept `Ok(())` instead. The ACL contents themselves are
    /// never assertable through any public API (per the task's constraint).
    #[test]
    fn gated_set_reprotects_plain_item() {
        let store = format!("{}.gate-reprotect", unique_run());
        let plain = AppleStore::new(&store, Accessibility::WhenUnlocked, None);
        plain.clear().unwrap();

        // Write while plain — a normal, unprotected-accessibility item.
        plain.set("token", "v1").unwrap();

        // Reopen the same service gated and overwrite: this must hit the
        // `errSecDuplicateItem` arm's gated branch (delete the plain item,
        // re-add with the fresh `SecAccessControl`).
        let gated = AppleStore::new(
            &store,
            Accessibility::WhenUnlocked,
            Some(AuthOptions::default()),
        );
        match gated.set("token", "v2") {
            Ok(()) => {}
            Err(SecureStorageError::Storage(msg)) => {
                assert!(
                    msg.contains("missing keychain entitlement") || msg.contains("-34018"),
                    "expected the documented missing-entitlement failure for a \
                     `SecAccessControl` item on this unsigned host, got: {msg}"
                );
            }
            Err(other) => panic!("expected Ok or a Storage(missing entitlement), got: {other:?}"),
        }

        // Either way the delete already ran — the plain item is gone.
        // Belt-and-braces cleanup via the plain handle's service-scoped
        // clear (delete-by-query needs no authentication context).
        plain.clear().unwrap();
    }

    /// Regression guard for the asymmetry above: a **plain** store's
    /// overwrite must still be the original value-only `SecItemUpdate` path
    /// (never delete+re-add) — i.e. the plain conformance arm stays green
    /// after the gated-arm change. `conformance` above already exercises a
    /// plain-store `set`-then-overwrite via [`crate::conformance`]'s
    /// generic suite; this asserts the same directly against a repeated
    /// `set` on one plain-store key.
    #[test]
    fn plain_set_overwrite_still_value_only_update() {
        let store = format!("{}.plain-overwrite", unique_run());
        let backend = AppleStore::new(&store, Accessibility::WhenUnlocked, None);
        backend.clear().unwrap();

        backend.set("token", "v1").unwrap();
        assert_eq!(backend.get("token").unwrap(), Some("v1".to_string()));
        backend.set("token", "v2").unwrap();
        assert_eq!(backend.get("token").unwrap(), Some("v2".to_string()));

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

    /// The `AuthOptions` → `SecAccessControlCreateFlags` mapping the gate
    /// locks in (pure logic — no Keychain access, runs everywhere). Bit
    /// values per `objc2-security`: BiometryAny = 1<<1, BiometryCurrentSet =
    /// 1<<3, DevicePasscode = 1<<4, Or = 1<<14.
    #[test]
    fn access_flags_mapping() {
        let f = |allow_device_credential, invalidate_on_enrollment| {
            access_flags(&AuthOptions {
                allow_device_credential,
                invalidate_on_enrollment,
                ..AuthOptions::default()
            })
        };
        // Default safe posture: current-set biometry only (auto-invalidating).
        assert_eq!(
            f(false, true),
            SecAccessControlCreateFlags::BiometryCurrentSet
        );
        // Opt out of enrollment invalidation: any enrolled biometry.
        assert_eq!(f(false, false), SecAccessControlCreateFlags::BiometryAny);
        // Passcode fallback widens with `Or | DevicePasscode`.
        assert_eq!(
            f(true, true),
            SecAccessControlCreateFlags::BiometryCurrentSet
                | SecAccessControlCreateFlags::Or
                | SecAccessControlCreateFlags::DevicePasscode
        );
        assert_eq!(
            f(true, false),
            SecAccessControlCreateFlags::BiometryAny
                | SecAccessControlCreateFlags::Or
                | SecAccessControlCreateFlags::DevicePasscode
        );
    }

    /// The `LAError` → availability / error taxonomy the gate locks in
    /// (pure logic — no LocalAuthentication call, runs everywhere).
    #[test]
    fn la_error_mapping() {
        // Availability probe reasons.
        assert_eq!(
            la_unavailability(LAError::BiometryNotEnrolled),
            Unavailability::NotEnrolled
        );
        assert_eq!(
            la_unavailability(LAError::PasscodeNotSet),
            Unavailability::PasscodeNotSet
        );
        assert_eq!(
            la_unavailability(LAError::BiometryNotAvailable),
            Unavailability::HardwareUnavailable
        );

        // Per-call error taxonomy.
        assert!(matches!(
            map_la_error(LAError::UserCancel),
            SecureStorageError::UserCanceled
        ));
        assert!(matches!(
            map_la_error(LAError::SystemCancel),
            SecureStorageError::SystemCanceled
        ));
        assert!(matches!(
            map_la_error(LAError::BiometryLockout),
            SecureStorageError::LockoutTemporary
        ));
        assert!(matches!(
            map_la_error(LAError::AuthenticationFailed),
            SecureStorageError::AuthFailed
        ));
        match map_la_error(LAError::InvalidContext) {
            SecureStorageError::Storage(msg) => assert!(msg.contains("LAError")),
            other => panic!("expected Storage, got {other:?}"),
        }
    }
}
