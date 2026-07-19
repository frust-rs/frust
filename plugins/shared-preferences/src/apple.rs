//! The Apple (iOS/macOS) [`Backend`] — `NSUserDefaults`-backed.
//!
//! **Stub for this task.** The real implementation (Phase 2 step 2 of the
//! plugin-system plan) lands in a later task: `standardUserDefaults`, ints
//! via `integerForKey`/`setInteger` (i64 precision), doubles native,
//! `Vec<String>` as a native `NSArray` of `NSString`, and `keys()` filtered
//! to (and stripped of) the `frust.` namespace prefix via
//! `dictionaryRepresentation` (the OS-shared-store namespacing Design
//! Decision 4 calls for — see [`crate`]'s module doc). The
//! `objc2`/`objc2-foundation` dependencies and `objc2-foundation`'s
//! explicit feature list are already declared in this crate's `Cargo.toml`
//! (verified against the vendored 0.3.2 source, not guessed) so that later
//! task only has to write source, never chase a feature-resolution error.
//!
//! Every method here reports [`PrefsError::Storage`] instead of running any
//! `objc2`/Foundation call — this keeps `SharedPreferences::standard()`
//! failing loudly and typed on Apple targets (including this repo's own
//! macOS dev hosts) rather than the crate silently landing partially-native
//! behavior. See [`crate`]'s module doc for why routing every platform
//! through this stub (rather than routing Apple/Android to [`crate::file`]
//! for now) was the choice made here.

use crate::{Backend, PrefValue, PrefsError};

/// Message every stub method reports — named once so a future replacement
/// can grep for exactly what still needs wiring.
const NOT_YET_IMPLEMENTED: &str =
    "the Apple (NSUserDefaults) shared-preferences backend is not implemented yet";

pub(crate) struct AppleStore;

impl AppleStore {
    /// Always succeeds — construction alone touches no Foundation API yet;
    /// every actual operation reports [`PrefsError::Storage`].
    pub(crate) fn standard() -> Result<Self, PrefsError> {
        Ok(Self)
    }
}

impl Backend for AppleStore {
    fn get(&self, _key: &str) -> Option<PrefValue> {
        None
    }

    fn set(&self, _key: &str, _value: PrefValue) -> Result<(), PrefsError> {
        Err(PrefsError::Storage(NOT_YET_IMPLEMENTED.into()))
    }

    fn remove(&self, _key: &str) -> Result<(), PrefsError> {
        Err(PrefsError::Storage(NOT_YET_IMPLEMENTED.into()))
    }

    fn clear(&self) -> Result<(), PrefsError> {
        Err(PrefsError::Storage(NOT_YET_IMPLEMENTED.into()))
    }

    fn keys(&self) -> Vec<String> {
        Vec::new()
    }
}
