//! Haptic feedback: a vocabulary and a process-wide sink. A widget fires
//! [`perform_haptic`]; the app installs one sink with [`set_haptic_sink`] that
//! forwards to its platform plugin, so no catalog depends on one. With no sink
//! firing is a silent no-op. The sink is thread-agnostic: widgets fire from the
//! event pass and the plugin owns any main-thread hop.
//!
//! ```ignore
//! use frust::{HapticFeedback, set_haptic_sink};
//! use frust_haptics::{HapticEffect, Haptics};
//! set_haptic_sink(Some(|feedback| {
//!     let effect = match feedback {
//!         HapticFeedback::Selection => HapticEffect::SelectionClick,
//!         HapticFeedback::Light => HapticEffect::ImpactLight,
//!         HapticFeedback::Medium => HapticEffect::ImpactMedium,
//!         HapticFeedback::Heavy => HapticEffect::ImpactHeavy,
//!         HapticFeedback::Success => HapticEffect::Success,
//!         HapticFeedback::Warning => HapticEffect::Warning,
//!         HapticFeedback::Error => HapticEffect::Error,
//!         _ => return,
//!     };
//!     let _ = Haptics::perform(effect);
//! }));
//! ```

use std::sync::Mutex;

/// What a control wants the user to feel.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HapticFeedback {
    /// A discrete selection tick, such as a stepped control crossing a step.
    Selection,
    /// A light tap for a subtle interaction.
    Light,
    /// A medium impact for a standard press.
    Medium,
    /// A heavy impact for a significant action.
    Heavy,
    /// An action completed.
    Success,
    /// An action needs attention.
    Warning,
    /// An action failed.
    Error,
}

/// The installed sink: a plain function, so it needs no allocation to copy out.
pub type HapticSink = fn(HapticFeedback);

static SINK: Mutex<Option<HapticSink>> = Mutex::new(None);

/// Install the process-wide sink, replacing any earlier one (last wins);
/// `None` uninstalls it.
pub fn set_haptic_sink(sink: Option<HapticSink>) {
    *SINK.lock().unwrap_or_else(|e| e.into_inner()) = sink;
}

/// Whether a sink is currently installed.
pub fn haptic_sink_installed() -> bool {
    SINK.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// Deliver `feedback` to the installed sink; a silent no-op without one. The
/// sink is copied out of the lock before it runs, so it may itself call
/// [`set_haptic_sink`].
pub fn perform_haptic(feedback: HapticFeedback) {
    let sink = *SINK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(sink) = sink {
        sink(feedback);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// The sink is process state; tests that touch it take this lock.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    static FIRST: AtomicU32 = AtomicU32::new(0);
    static SECOND: AtomicU32 = AtomicU32::new(0);

    fn first(_: HapticFeedback) {
        FIRST.fetch_add(1, Ordering::SeqCst);
    }

    fn second(_: HapticFeedback) {
        SECOND.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn perform_haptic_without_a_sink_is_a_no_op() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_haptic_sink(None);
        assert!(!haptic_sink_installed());
        perform_haptic(HapticFeedback::Heavy);
    }

    #[test]
    fn set_haptic_sink_is_last_wins() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        FIRST.store(0, Ordering::SeqCst);
        SECOND.store(0, Ordering::SeqCst);
        set_haptic_sink(Some(first));
        set_haptic_sink(Some(second));
        assert!(haptic_sink_installed());
        perform_haptic(HapticFeedback::Success);
        assert_eq!(FIRST.load(Ordering::SeqCst), 0);
        assert_eq!(SECOND.load(Ordering::SeqCst), 1);
        set_haptic_sink(None);
    }

    #[test]
    fn none_uninstalls_the_sink() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        FIRST.store(0, Ordering::SeqCst);
        set_haptic_sink(Some(first));
        perform_haptic(HapticFeedback::Light);
        set_haptic_sink(None);
        assert!(!haptic_sink_installed());
        perform_haptic(HapticFeedback::Light);
        assert_eq!(FIRST.load(Ordering::SeqCst), 1);
    }
}
