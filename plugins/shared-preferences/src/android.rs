//! The Android [`Backend`] — `Context.getSharedPreferences`-backed.
//!
//! **Stub for this task.** The real implementation (Phase 2 step 3 of the
//! plugin-system plan) lands in a later task: every call routed through
//! [`frust_plugin::android::with_jni_env`];
//! `Context.getSharedPreferences("<app>_frust_prefs", MODE_PRIVATE)`;
//! writes via `edit()` → `put*` → `apply()`; `f64` stored as
//! `putLong(f64::to_bits())` for exact round trip; `Vec<String>` as a
//! JSON-encoded string (this crate already depends on `serde_json` for the
//! file backend, so the encoder is shared rather than hand-rolled); key
//! enumeration (`getAll`) under `push_local_frame`/`pop_local_frame`; keys
//! filtered to (and stripped of) the `frust.` namespace prefix (the
//! OS-shared-store namespacing Design Decision 4 calls for — see
//! [`crate`]'s module doc).
//!
//! Every method here reports [`PrefsError::Storage`] instead of touching
//! JNI — this keeps `SharedPreferences::standard()` failing loudly and
//! typed on Android (rather than the crate silently landing
//! partially-native behavior) until that later task lands.

use crate::{Backend, PrefValue, PrefsError};

/// Message every stub method reports — named once so a future replacement
/// can grep for exactly what still needs wiring.
const NOT_YET_IMPLEMENTED: &str =
    "the Android (SharedPreferences) shared-preferences backend is not implemented yet";

pub(crate) struct AndroidStore;

impl AndroidStore {
    /// Always succeeds — construction alone touches no JNI yet; every
    /// actual operation reports [`PrefsError::Storage`]. A future real
    /// implementation is expected to instead surface
    /// [`PrefsError::PlatformNotInitialized`] here when
    /// [`frust_plugin::android::with_jni_env`] reports the host shell never
    /// installed the platform handles.
    pub(crate) fn standard() -> Result<Self, PrefsError> {
        Ok(Self)
    }
}

impl Backend for AndroidStore {
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
