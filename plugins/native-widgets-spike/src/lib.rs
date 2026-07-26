//! THROWAWAY — native-widgets Phase 0 spike crate.
//!
//! Proves, on device, the mechanisms `workflow/plans/features/frust-native-widgets/PLAN.md`
//! Phase 0 gates on:
//!
//! - **Spike 1 (Android)**: a `dev.frust.FrustNativeSpikeFactory` Kotlin factory
//!   whose `createView` calls this crate's `nativeCreateControl` export, which
//!   builds a REAL `android.widget.Button` (or a 50-child stress hierarchy)
//!   through direct JNI and hands it back; a click listener dispatching back
//!   into Rust ([`set_click_handler`]); and the perf floor
//!   ([`stress_tick`] — 500 property sets/frame, µs-timed, method-IDs cached).
//! - **Spike 2 (iOS)**: a `define_class!` factory class resolved via
//!   `NSClassFromString` from the host in a release build (see `apple` module).
//! - **Spike 4b rides the Kotlin factory** (system-bars hide on API 36 — see
//!   `FrustNativeSpikeFactory.kt`, it needs no Rust).
//!
//! Host (desktop) builds are inert stubs so the catalog keeps compiling
//! everywhere — the same shape as the shipped platform plugins.

#[cfg(target_os = "android")]
mod android;

#[cfg(all(target_os = "ios", target_vendor = "apple"))]
pub mod apple;

use std::sync::{Arc, Mutex, OnceLock};

/// The registered click handler — set by the app ([`set_click_handler`]),
/// invoked on the platform main thread by the native listener export with
/// `(control_id, event_kind)`.
#[allow(clippy::type_complexity)]
pub(crate) fn handler_slot() -> &'static Mutex<Option<Arc<dyn Fn(i64, i32) + Send + Sync>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<dyn Fn(i64, i32) + Send + Sync>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Register the (single, spike-scoped) native-event handler. Replaces any
/// previous handler; called from the catalog page's builder each rebuild.
pub fn set_click_handler(f: impl Fn(i64, i32) + Send + Sync + 'static) {
    *handler_slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
}

/// Spike 1's perf floor: perform `sets` direct-JNI property sets against the
/// retained stress views, returning the elapsed µs for the batch (`None` when
/// no stress hierarchy is live or off-Android). Called from the catalog page's
/// builder during rebuild — the exact main-thread-rebuild path Phase 1's `api`
/// layer will use.
pub fn stress_tick(sets: usize) -> Option<u64> {
    #[cfg(target_os = "android")]
    {
        android::stress_tick(sets)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = sets;
        None
    }
}

/// Live retained global-ref count (spike 1's leak accounting: must return to 0
/// after a dispose cycle). Always 0 off-Android.
pub fn live_ref_count() -> usize {
    #[cfg(target_os = "android")]
    {
        android::live_ref_count()
    }
    #[cfg(not(target_os = "android"))]
    {
        0
    }
}
