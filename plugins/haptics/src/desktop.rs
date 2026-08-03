//! The desktop (macOS/Linux/Windows) [`Backend`] — unavailable, by design,
//! in v1.
//!
//! `#[cfg(any(target_os = "macos", target_os = "linux", target_os =
//! "windows"))]`: unlike `frust-clipboard`, which shares a real desktop
//! backend (`arboard`) across these three targets, this crate ships no
//! desktop haptics implementation at all. macOS trackpads do expose
//! `NSHapticFeedbackManager` and some Windows/Linux hardware exposes force
//! feedback, but none of it maps onto this crate's mobile-first
//! [`HapticEffect`] vocabulary, and a desktop-preview build is not this
//! plugin's motivating use case (`docs/PLUGINS_ARCHITECTURE.md`'s OS-capability
//! plugin charter). A future desktop backend is additive, not a breaking
//! change — every call site already treats [`Haptics::perform`] as
//! fire-and-forget and may ignore the `Result`.
//!
//! Every [`HapticEffect`] variant is still matched explicitly below (no
//! wildcard arm) rather than short-circuited before the match — the crate's
//! compile-level conformance guarantee (the crate doc's *Backends* section)
//! holds on every cfg arm, this one included, so a future variant forces an
//! update here too.

use crate::{Backend, HapticEffect, HapticsError, Unavailability};

/// The desktop backend. Stateless — there is nothing to open or hold; every
/// call reports [`HapticsError::NotAvailable`] synchronously.
pub(crate) struct DesktopHaptics;

impl Backend for DesktopHaptics {
    fn perform(&self, effect: HapticEffect) -> Result<(), HapticsError> {
        // Exhaustive, no wildcard: every variant reaches the same outcome
        // today, but a future variant added without touching this arm fails
        // to compile rather than silently falling through (see the module
        // doc).
        match effect {
            HapticEffect::SelectionClick
            | HapticEffect::ImpactLight
            | HapticEffect::ImpactMedium
            | HapticEffect::ImpactHeavy
            | HapticEffect::Success
            | HapticEffect::Warning
            | HapticEffect::Error => Err(HapticsError::NotAvailable(
                Unavailability::UnsupportedPlatform,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Haptics;

    /// Every effect this crate defines reports [`HapticsError::NotAvailable`]
    /// on desktop — never a panic, never `Ok(())` (this platform never has a
    /// mechanism to have silently succeeded through).
    #[test]
    fn every_effect_reports_not_available() {
        let effects = [
            HapticEffect::SelectionClick,
            HapticEffect::ImpactLight,
            HapticEffect::ImpactMedium,
            HapticEffect::ImpactHeavy,
            HapticEffect::Success,
            HapticEffect::Warning,
            HapticEffect::Error,
        ];
        for effect in effects {
            let result = Haptics::perform(effect);
            assert!(
                matches!(
                    result,
                    Err(HapticsError::NotAvailable(
                        Unavailability::UnsupportedPlatform
                    ))
                ),
                "{effect:?}: {result:?}"
            );
        }
    }
}
