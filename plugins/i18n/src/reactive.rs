//! Signal-driven locale binding for a `frust` app.
//!
//! filled by a later task: an `RwSignal<Locale>` (or similar) a `Component`
//! reads reactively, with a provide/expect pair mirroring
//! `clean-signals-frust`'s `provide_controller`/`expect_controller` shape
//! (`docs/PLUGINS_ARCHITECTURE.md`'s Key Types). Only compiled with the
//! `frust-api` feature — the facade-glue half of this crate's charter (see
//! `src/lib.rs`'s crate doc's *Charter* section).

use crate::Locale;

/// The app's currently active locale, if one has been set.
///
/// filled by a later task — always `None` until then.
pub fn active_locale() -> Option<Locale> {
    None
}
