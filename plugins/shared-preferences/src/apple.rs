//! The Apple (iOS/macOS) [`Backend`] — `NSUserDefaults`-backed, via `objc2`.
//!
//! # Storage encoding — native Foundation types
//!
//! Every value is stored as its natural `NSUserDefaults` type, so the store
//! stays inspectable by any other reader and a generic [`Backend::get`] can
//! recover the exact [`PrefValue`] variant from the stored object's runtime
//! class + numeric encoding:
//!
//! | [`PrefValue`]      | stored as                              | recovered by |
//! |--------------------|----------------------------------------|--------------|
//! | [`Bool`]           | `setBool:forKey:` (`NSNumber`, `Char`) | [`NSNumber::encoding`] == `Char`/`UChar` |
//! | [`I64`]            | `setInteger:forKey:` (`NSNumber`, `q`) | `NSNumber` numeric encoding |
//! | [`F64`]            | `setDouble:forKey:` (`NSNumber`, `d`)  | [`NSNumber::encoding`] == `Float`/`Double` |
//! | [`Str`]            | `NSString`                             | class is `NSString` |
//! | [`StrList`]        | `NSArray` of `NSString`                | class is `NSArray` |
//!
//! [`Bool`]: PrefValue::Bool
//! [`I64`]: PrefValue::I64
//! [`F64`]: PrefValue::F64
//! [`Str`]: PrefValue::Str
//! [`StrList`]: PrefValue::StrList
//!
//! We only ever write a value via `setBool:`/`setInteger:`/`setDouble:` (so
//! a `Char`-encoded `NSNumber` is unambiguously a Frust-written `bool`, never
//! a raw `i8`) — this is what makes the class/encoding read at [`get`] a
//! total, unambiguous mapping. `i64` uses `NSInteger` (= `isize`), exact on
//! the 64-bit-only Apple targets Frust supports. `f64` round-trips exactly
//! (finite values bit-for-bit; `NaN` stays `NaN`) because `NSNumber` stores
//! the IEEE-754 `double` verbatim.
//!
//! # Namespace + `clear`
//!
//! Keys are prefixed with [`crate::KEY_PREFIX`] (`"frust."`) so this backend
//! shares the process `NSUserDefaults` domain without colliding with the
//! app's own defaults; [`keys`](AppleStore::keys) strips it and
//! [`clear`](AppleStore::clear) removes only `frust.`-prefixed keys
//! (enumerated via `dictionaryRepresentation`), never another owner's.
//!
//! # macOS desktop preview
//!
//! `target_os = "macos"` routes here too, so `cargo run` on a dev mac
//! dogfoods this exact `objc2` path. An unbundled binary (no
//! `CFBundleIdentifier`) writes to the global `NSUserDefaults` domain rather
//! than an app-specific plist — a storage-*location* difference from a
//! bundled iOS/macOS app, not a behavioral one; the `frust.` namespace keeps
//! it isolated from unrelated global defaults either way.
//!
//! # Thread-safety
//!
//! `NSUserDefaults` is documented thread-safe with no main-thread
//! requirement. This backend stores no Objective-C reference across calls
//! (it re-fetches the shared defaults per operation — a cheap singleton
//! lookup), so [`AppleStore`] is a plain value and trivially `Send + Sync`.

// `AnyThread` provides `NSUserDefaults::alloc`, used only by the
// `#[cfg(test)]` suite-domain path ([`AppleStore::with_suite`]); gated so a
// non-test build (where that path is absent) carries no unused-import
// warning.
#[cfg(test)]
use objc2::AnyThread;
use objc2::encode::Encoding;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSArray, NSNumber, NSString, NSUserDefaults};

use crate::{Backend, KEY_PREFIX, PrefValue, PrefsError};

/// Which `NSUserDefaults` domain this store targets.
enum Domain {
    /// `standardUserDefaults` — the app's (or, unbundled, the global) domain.
    Standard,
    /// A named suite domain (`initWithSuiteName:`) — used only by the
    /// `#[cfg(test)]` conformance run so it never pollutes the host mac's
    /// standard defaults. The `String` is the suite name.
    #[cfg(test)]
    Suite(String),
}

/// `NSUserDefaults`-backed preferences store.
pub(crate) struct AppleStore {
    domain: Domain,
}

impl AppleStore {
    /// Open the app's standard preferences store (`standardUserDefaults`).
    ///
    /// # Errors
    /// Infallible today (fetching the shared defaults cannot fail); returns
    /// `Result` for signature parity with the other backends'
    /// `standard()`.
    pub(crate) fn standard() -> Result<Self, PrefsError> {
        Ok(Self {
            domain: Domain::Standard,
        })
    }

    /// Open a named-suite store — the `#[cfg(test)]` conformance run's
    /// isolation seam (mirrors [`crate::file::FileStore::at_path`]): the
    /// suite is a throwaway domain the test removes on completion, so the
    /// host mac's `standardUserDefaults` is never touched.
    #[cfg(test)]
    pub(crate) fn with_suite(name: impl Into<String>) -> Self {
        Self {
            domain: Domain::Suite(name.into()),
        }
    }

    /// Fetch the target `NSUserDefaults` instance for this store. Cheap: a
    /// shared-singleton lookup for [`Domain::Standard`], a suite-open for a
    /// test suite.
    fn defaults(&self) -> Retained<NSUserDefaults> {
        match &self.domain {
            Domain::Standard => NSUserDefaults::standardUserDefaults(),
            #[cfg(test)]
            Domain::Suite(name) => {
                let ns = NSString::from_str(name);
                NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), Some(&ns))
                    .expect("initWithSuiteName is non-nil for a non-reserved suite name")
            }
        }
    }
}

/// `"frust."`-prefixed key as an `NSString`.
fn namespaced(key: &str) -> Retained<NSString> {
    NSString::from_str(&format!("{KEY_PREFIX}{key}"))
}

/// Recover a [`PrefValue`] from a stored `NSUserDefaults` object by its
/// runtime class + numeric encoding (see the module doc's encoding table).
/// Returns `None` for any object this backend never writes (e.g. a foreign
/// `NSData`), so a wrong-typed foreign entry degrades to "absent" rather
/// than panicking.
fn decode(obj: &AnyObject) -> Option<PrefValue> {
    if let Some(s) = obj.downcast_ref::<NSString>() {
        return Some(PrefValue::Str(s.to_string()));
    }
    if let Some(arr) = obj.downcast_ref::<NSArray>() {
        let mut out = Vec::with_capacity(arr.len());
        for element in arr.to_vec() {
            // Every element we ever write is an `NSString`; a foreign array
            // with a non-string element is not something we produced.
            let s = element.downcast::<NSString>().ok()?;
            out.push(s.to_string());
        }
        return Some(PrefValue::StrList(out));
    }
    if let Some(number) = obj.downcast_ref::<NSNumber>() {
        return Some(match number.encoding() {
            // `setBool:` is our only `Char`/`UChar` writer, so a `Char`
            // `NSNumber` is unambiguously a Frust-written `bool`.
            Encoding::Char | Encoding::UChar => PrefValue::Bool(number.as_bool()),
            Encoding::Float | Encoding::Double => PrefValue::F64(number.as_f64()),
            // Every remaining numeric encoding is an integer store
            // (`setInteger:` → `LongLong` on 64-bit).
            _ => PrefValue::I64(number.as_i64()),
        });
    }
    None
}

impl Backend for AppleStore {
    fn get(&self, key: &str) -> Option<PrefValue> {
        let defaults = self.defaults();
        let obj = defaults.objectForKey(&namespaced(key))?;
        decode(&obj)
    }

    fn set(&self, key: &str, value: PrefValue) -> Result<(), PrefsError> {
        let defaults = self.defaults();
        let nskey = namespaced(key);
        match value {
            PrefValue::Bool(b) => defaults.setBool_forKey(b, &nskey),
            PrefValue::I64(i) => defaults.setInteger_forKey(i as isize, &nskey),
            PrefValue::F64(f) => defaults.setDouble_forKey(f, &nskey),
            PrefValue::Str(s) => {
                let value = NSString::from_str(&s);
                // SAFETY: `setObject:forKey:` requires the value be a
                // property-list type; `NSString` is one. There is no safe
                // typed `setString:` on `NSUserDefaults`.
                unsafe { defaults.setObject_forKey(Some(&value), &nskey) };
            }
            PrefValue::StrList(list) => {
                let items: Vec<Retained<NSString>> =
                    list.iter().map(|s| NSString::from_str(s)).collect();
                let refs: Vec<&NSString> = items.iter().map(Retained::as_ref).collect();
                let array = NSArray::from_slice(&refs);
                // SAFETY: `setObject:forKey:` requires a property-list type;
                // an `NSArray` of `NSString` is one. There is no safe typed
                // array setter on `NSUserDefaults`.
                unsafe { defaults.setObject_forKey(Some(&array), &nskey) };
            }
        }
        Ok(())
    }

    fn remove(&self, key: &str) -> Result<(), PrefsError> {
        self.defaults().removeObjectForKey(&namespaced(key));
        Ok(())
    }

    fn clear(&self) -> Result<(), PrefsError> {
        let defaults = self.defaults();
        // Collect first (Rust `String`s), then remove — never mutate the
        // domain while enumerating its `dictionaryRepresentation`.
        let names: Vec<String> = defaults
            .dictionaryRepresentation()
            .keys()
            .map(|k| k.to_string())
            .filter(|k| k.starts_with(KEY_PREFIX))
            .collect();
        for name in names {
            defaults.removeObjectForKey(&NSString::from_str(&name));
        }
        Ok(())
    }

    fn keys(&self) -> Vec<String> {
        self.defaults()
            .dictionaryRepresentation()
            .keys()
            .filter_map(|k| k.to_string().strip_prefix(KEY_PREFIX).map(str::to_string))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use objc2_foundation::{NSString, NSUserDefaults};

    use super::*;

    /// A process-unique throwaway suite name, so a conformance run never
    /// collides with the host mac's `standardUserDefaults` (nor a parallel
    /// test binary's suite).
    fn unique_suite() -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!(
            "it.f0x.frust.shared-preferences.test.{}.{n}",
            std::process::id()
        )
    }

    /// Wipe the suite domain entirely (belt-and-braces on top of the
    /// conformance suite's own final `clear`), so nothing this test wrote
    /// persists on the host after the run.
    fn drop_suite(name: &str) {
        let defaults = NSUserDefaults::standardUserDefaults();
        defaults.removePersistentDomainForName(&NSString::from_str(name));
    }

    /// The full cross-backend conformance suite against a real, isolated
    /// `NSUserDefaults` suite domain on this host.
    #[test]
    fn conformance() {
        let suite = unique_suite();
        let store = AppleStore::with_suite(&suite);
        crate::conformance::run_conformance_suite(&store);
        drop_suite(&suite);
    }
}
