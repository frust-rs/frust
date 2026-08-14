//! System-locale detection.
//!
//! filled by a later task: routes by `#[cfg(target_os = ...)]` to a real
//! backend — Android (`frust_plugin::android`'s JNI handle, reading
//! `Resources.getSystem().getConfiguration().getLocales()`), Apple
//! (`objc2-foundation`'s `NSLocale.preferredLanguages`, iOS only — macOS
//! shares the desktop backend instead, see `Cargo.toml`'s target-dependency
//! comments), and desktop (`sys-locale::locale`) — the same target-gated-FFI
//! shape every plugin's `Cargo.toml` splits on
//! (`docs/PLUGINS_ARCHITECTURE.md`'s Layer Dependencies).

use crate::Locale;

/// Returns the platform's configured locale list, most-preferred first.
///
/// filled by a later task — always empty until then.
#[allow(dead_code)] // not yet wired into `engine`; a later task calls this from there
pub fn system_locales() -> Vec<Locale> {
    Vec::new()
}
