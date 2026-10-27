//! The Android activity lifecycle → `AppLifecycleState` derivation.
//!
//! Flutter's Android mapping: the target state is a pure function of four
//! activity facts. The JNI hooks (`jni_glue`) update the facts and push the
//! derived target through [`frust_reactive::push_app_lifecycle`], which dedups
//! and synthesizes the intermediate steps (`Inactive → Hidden → Paused` and
//! back). Android never requests an app exit — exits are not cancelable.

/// The states the Android mapping can target (a subset of
/// `frust_reactive::AppLifecycleState`; `Hidden` is synthesized by the state
/// machine). A local mirror keeps the derivation host-testable, since
/// `frust-reactive` is an Android-only dependency of this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    Detached,
    Paused,
    Inactive,
    Resumed,
}

#[cfg(target_os = "android")]
impl From<Target> for frust_reactive::AppLifecycleState {
    fn from(target: Target) -> Self {
        match target {
            Target::Detached => Self::Detached,
            Target::Paused => Self::Paused,
            Target::Inactive => Self::Inactive,
            Target::Resumed => Self::Resumed,
        }
    }
}

/// The four activity facts the target state is derived from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LifecycleFlags {
    /// Between `onStart` and `onStop`.
    pub started: bool,
    /// Between `onResume` and `onPause`.
    pub resumed: bool,
    /// The activity window holds input focus.
    pub window_focused: bool,
    /// `onDestroy` ran (the activity is finishing).
    pub destroyed: bool,
}

impl LifecycleFlags {
    /// The state these facts map to.
    ///
    /// Destroyed is `Detached`; not started is `Paused`; started, resumed and
    /// focused is `Resumed`; everything else (after `onPause`, a notification
    /// shade, a system dialog, the unfocused half of split-screen) is `Inactive`.
    pub(crate) fn target(&self) -> Target {
        if self.destroyed {
            Target::Detached
        } else if !self.started {
            Target::Paused
        } else if self.resumed && self.window_focused {
            Target::Resumed
        } else {
            Target::Inactive
        }
    }
}

#[cfg(target_os = "android")]
mod process {
    use std::sync::Mutex;

    use super::LifecycleFlags;

    /// Process-wide facts; only the UI thread touches them (the mutex is for
    /// `static` soundness, never contended).
    static FLAGS: Mutex<LifecycleFlags> = Mutex::new(LifecycleFlags {
        started: false,
        resumed: false,
        window_focused: false,
        destroyed: false,
    });

    /// Apply `update` to the facts, push the derived target unless `gate`
    /// rejects the new facts, and return nothing.
    pub(crate) fn apply(
        update: impl FnOnce(&mut LifecycleFlags),
        gate: impl FnOnce(&LifecycleFlags) -> bool,
    ) {
        let target = {
            let mut flags = FLAGS.lock().unwrap_or_else(|e| e.into_inner());
            update(&mut flags);
            gate(&flags).then(|| flags.target())
        };
        if let Some(target) = target {
            frust_reactive::push_app_lifecycle(target.into());
        }
    }

    /// Forget the facts (the activity was destroyed; a recreate starts clean).
    pub(crate) fn reset() {
        *FLAGS.lock().unwrap_or_else(|e| e.into_inner()) = LifecycleFlags::default();
    }
}

#[cfg(target_os = "android")]
pub(crate) use process::{apply, reset};

#[cfg(test)]
mod tests {
    use super::*;
    use Target::*;

    fn flags(
        started: bool,
        resumed: bool,
        window_focused: bool,
        destroyed: bool,
    ) -> LifecycleFlags {
        LifecycleFlags {
            started,
            resumed,
            window_focused,
            destroyed,
        }
    }

    #[test]
    fn destroyed_is_detached_whatever_else() {
        assert_eq!(flags(true, true, true, true).target(), Detached);
        assert_eq!(flags(false, false, false, true).target(), Detached);
    }

    #[test]
    fn not_started_is_paused() {
        assert_eq!(flags(false, false, false, false).target(), Paused);
        assert_eq!(flags(false, true, true, false).target(), Paused);
    }

    #[test]
    fn started_resumed_focused_is_resumed() {
        assert_eq!(flags(true, true, true, false).target(), Resumed);
    }

    #[test]
    fn started_but_paused_or_unfocused_is_inactive() {
        assert_eq!(flags(true, false, true, false).target(), Inactive);
        assert_eq!(flags(true, true, false, false).target(), Inactive);
        assert_eq!(flags(true, false, false, false).target(), Inactive);
    }

    #[test]
    fn home_and_return_sequences() {
        // Home: onPause, focus lost, onStop.
        assert_eq!(flags(true, false, true, false).target(), Inactive);
        assert_eq!(flags(true, false, false, false).target(), Inactive);
        assert_eq!(flags(false, false, false, false).target(), Paused);
        // Return: onStart, onResume, focus gained.
        assert_eq!(flags(true, false, false, false).target(), Inactive);
        assert_eq!(flags(true, true, false, false).target(), Inactive);
        assert_eq!(flags(true, true, true, false).target(), Resumed);
    }
}
