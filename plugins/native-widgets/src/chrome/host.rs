//! The `ChromeHost` seam a platform arm implements, the field-level
//! [`ChromeDiff`] it is handed, `NullChromeHost` — the production stand-in
//! on every target without an arm — and the test-only `RecordingChromeHost`,
//! which records calls the way the runtime's host `NativeCtx` does.

#[cfg(test)]
use std::collections::VecDeque;

use super::{ChromeGeneration, ChromeSpec, TopBarSpec};

/// Why the chrome slot or a host refused a publish.
#[derive(thiserror::Error, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ChromeError {
    /// The platform arm declined to install (an Android theme that is not
    /// Material3-derived, an inflate failure) — the app falls back to
    /// frust-drawn chrome. Only an arm constructs it.
    #[cfg_attr(not(target_os = "ios"), allow(dead_code))]
    #[error("native chrome refused: {0}")]
    Refused(String),
    /// No host surface exists yet (no resumed `Activity`, no key window).
    /// Transient: the slot retries the install on the generation's next
    /// publish, while a [`Self::Refused`] pins the fallback.
    #[error("no native chrome host is available")]
    NoHost,
    /// Another `native_scaffold` already owns the app chrome — one per app.
    #[error("another native_scaffold already owns the app chrome (one per app)")]
    Busy,
}

/// One field of a [`ChromeSpec`] a [`ChromeDiff`] can report as changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ChromeField {
    Tabs,
    Selected,
    /// The top bar appeared or went away.
    TopBar,
    Title,
    Subtitle,
    Leading,
    Actions,
    LargeTitle,
    Scroll,
    /// The app theme's brightness (`ChromeSpec::dark`).
    Brightness,
}

/// What changed between the retained spec and a newly published one. A
/// host reads the new values from [`Self::spec`] and touches only the
/// [`Self::changed`] fields.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ChromeDiff<'a> {
    /// The newly published spec, in full.
    pub(crate) spec: &'a ChromeSpec,
    /// The changed fields, in [`ChromeField`] declaration order.
    pub(crate) changed: Vec<ChromeField>,
}

impl<'a> ChromeDiff<'a> {
    /// The field-level difference from `old` to `new`. An absent top bar
    /// compares as an empty one, so a bar appearing reports
    /// [`ChromeField::TopBar`] plus whichever of its fields are non-empty.
    pub(crate) fn between(old: &ChromeSpec, new: &'a ChromeSpec) -> Self {
        let empty = TopBarSpec::default();
        let old_top = old.top.as_ref().unwrap_or(&empty);
        let new_top = new.top.as_ref().unwrap_or(&empty);
        let checks = [
            (ChromeField::Tabs, old.tabs != new.tabs),
            (ChromeField::Selected, old.selected != new.selected),
            (ChromeField::TopBar, old.top.is_some() != new.top.is_some()),
            (ChromeField::Title, old_top.title != new_top.title),
            (ChromeField::Subtitle, old_top.subtitle != new_top.subtitle),
            (ChromeField::Leading, old_top.leading != new_top.leading),
            (ChromeField::Actions, old_top.actions != new_top.actions),
            (
                ChromeField::LargeTitle,
                old_top.large_title != new_top.large_title,
            ),
            (ChromeField::Scroll, old.scroll != new.scroll),
            (ChromeField::Brightness, old.dark != new.dark),
        ];
        let changed = checks
            .into_iter()
            .filter_map(|(field, differs)| differs.then_some(field))
            .collect();
        Self { spec: new, changed }
    }

    /// Whether nothing changed — the slot then makes no host call at all.
    pub(crate) fn is_empty(&self) -> bool {
        self.changed.is_empty()
    }

    /// Whether `field` changed — an arm's question.
    #[cfg_attr(not(any(test, target_os = "ios")), allow(dead_code))]
    pub(crate) fn contains(&self, field: ChromeField) -> bool {
        self.changed.contains(&field)
    }
}

/// The seam a chrome arm implements: the real containers (iOS
/// `UITabBarController`, Android `MaterialToolbar` + `BottomNavigationView`)
/// live behind it, and nothing above it names a platform type.
///
/// Every method is called with the process-wide slot locked, so a host must
/// never call [`super::deliver`] synchronously from inside one — a platform
/// event arrives later, from the platform's own listener.
pub(crate) trait ChromeHost {
    /// Build the containers for `spec`, stamping events from them with
    /// `generation`. An `Err` leaves the slot claimed (the app renders
    /// fallback chrome) and this host untouched until the next install.
    fn install(
        &mut self,
        generation: ChromeGeneration,
        spec: &ChromeSpec,
    ) -> Result<(), ChromeError>;

    /// Update only `diff.changed`; never called with an empty diff.
    fn apply(&mut self, diff: &ChromeDiff<'_>);

    /// Tear the containers down; only called after a successful
    /// [`Self::install`]. Runs from scaffold teardown or from
    /// `ScaffoldWidget::drop`, so it must be panic-free and must not assume
    /// the host surface (window, `Activity`) is still alive.
    fn dispose(&mut self);
}

/// The production host on every target without a chrome arm: accepts every
/// install and does nothing, so the scaffold draws the fallback bars.
#[cfg(not(any(test, target_os = "ios")))]
#[derive(Debug, Default)]
pub(crate) struct NullChromeHost;

#[cfg(not(any(test, target_os = "ios")))]
impl NullChromeHost {
    /// The host (const, for the process-wide slot's static).
    pub(crate) const fn new() -> Self {
        Self
    }
}

#[cfg(not(any(test, target_os = "ios")))]
impl ChromeHost for NullChromeHost {
    fn install(&mut self, _: ChromeGeneration, _: &ChromeSpec) -> Result<(), ChromeError> {
        Ok(())
    }

    fn apply(&mut self, _: &ChromeDiff<'_>) {}

    fn dispose(&mut self) {}
}

/// One call a [`RecordingChromeHost`] saw.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum HostCall {
    Install {
        generation: ChromeGeneration,
        spec: ChromeSpec,
    },
    Apply {
        changed: Vec<ChromeField>,
        spec: ChromeSpec,
    },
    Dispose,
}

/// The most calls a [`RecordingChromeHost`] keeps — scroll publishes arrive
/// at frame rate, so the log drops its oldest entry rather than grow.
#[cfg(test)]
pub(crate) const RECORDED_CALLS_CAP: usize = 64;

/// The test host: accepts every install (unless built [`Self::refusing`])
/// and records each call.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct RecordingChromeHost {
    calls: VecDeque<HostCall>,
    refusal: Option<ChromeError>,
}

#[cfg(test)]
impl RecordingChromeHost {
    /// A host that accepts every install.
    pub(crate) const fn new() -> Self {
        Self {
            calls: VecDeque::new(),
            refusal: None,
        }
    }

    /// A host whose every install fails with `error` (still recorded).
    pub(crate) fn refusing(error: ChromeError) -> Self {
        Self {
            calls: VecDeque::new(),
            refusal: Some(error),
        }
    }

    /// Make every later install fail with `error`, or succeed for `None`.
    pub(crate) fn set_refusal(&mut self, error: Option<ChromeError>) {
        self.refusal = error;
    }

    /// The recorded calls, oldest first.
    pub(crate) fn calls(&self) -> impl Iterator<Item = &HostCall> {
        self.calls.iter()
    }

    /// Drain the recorded calls, oldest first.
    pub(crate) fn take_calls(&mut self) -> Vec<HostCall> {
        self.calls.drain(..).collect()
    }

    fn record(&mut self, call: HostCall) {
        if self.calls.len() == RECORDED_CALLS_CAP {
            self.calls.pop_front();
        }
        self.calls.push_back(call);
    }
}

#[cfg(test)]
impl ChromeHost for RecordingChromeHost {
    fn install(
        &mut self,
        generation: ChromeGeneration,
        spec: &ChromeSpec,
    ) -> Result<(), ChromeError> {
        self.record(HostCall::Install {
            generation,
            spec: spec.clone(),
        });
        self.refusal.clone().map_or(Ok(()), Err)
    }

    fn apply(&mut self, diff: &ChromeDiff<'_>) {
        self.record(HostCall::Apply {
            changed: diff.changed.clone(),
            spec: diff.spec.clone(),
        });
    }

    fn dispose(&mut self) {
        self.record(HostCall::Dispose);
    }
}
