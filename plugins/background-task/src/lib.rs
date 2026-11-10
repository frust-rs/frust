//! Backs `frust::BackgroundTask` with a platform service, for apps that opt in.
//!
//! Call [`install`] once, from `frust::app!(setup = { .. })` or the root
//! `Component::init`. Both run on the UI thread after the reactive runtime exists, and a
//! repeat call after Activity recreation returns `false` harmlessly. A guard begun before
//! `install` is the core no-op.
//!
//! - **Android**: a `shortService` foreground service holds the process while any guard
//!   is live. Adding this plugin puts `FOREGROUND_SERVICE` and the service in the app's
//!   manifest, so the Play Console needs a foreground-service declaration. An app can
//!   strip both with `tools:node="remove"`; `begin` is then refused and the work runs
//!   unprotected. See the README.
//! - **Everything else**: [`install`] returns `false`.
//!
//! `BackgroundTask::expired` is the app's signal on Android. It reads `true` when the
//! system refused the service, which `begin` reports through `is_granted`, and when the
//! protection ended: a 150 s watchdog (below the system's 3-minute `shortService` limit)
//! or the system's own timeout. The work is never cancelled for you.

#[cfg_attr(
    not(target_os = "android"),
    allow(dead_code, reason = "only the Android backend tracks live tasks")
)]
mod registry;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
use android as platform;

#[cfg(not(target_os = "android"))]
mod unsupported;
#[cfg(not(target_os = "android"))]
use unsupported as platform;

/// Install this platform's `frust::BackgroundTask` backend. Returns whether it was
/// installed: `false` on a repeat call, on a platform with no backend, and on Android when
/// the plugin's Gradle module is missing from the app. UI-thread-only once the reactive
/// runtime exists.
pub fn install() -> bool {
    platform::install()
}
