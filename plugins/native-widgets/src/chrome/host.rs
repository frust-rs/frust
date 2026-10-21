//! The `ChromeHost` seam a platform arm implements, the field-level
//! [`ChromeDiff`] it is handed, and [`RecordingChromeHost`] — the stand-in
//! every target without an arm uses, recording calls the way the runtime's
//! host `NativeCtx` does.

use std::collections::VecDeque;

use super::{ChromeGeneration, ChromeSpec, TopBarSpec};

/// Why the chrome slot or a host refused a publish.
#[derive(thiserror::Error, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ChromeError {
    /// The platform arm declined to install (an Android theme that is not
    /// Material3-derived, an inflate failure) — the app falls back to
    /// frust-drawn chrome.
    #[error("native chrome refused: {0}")]
    Refused(String),
    /// No host surface exists yet (no resumed `Activity`, no key window).
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

    /// Whether `field` changed.
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

    /// Tear the containers down (scaffold teardown); only called after a
    /// successful [`Self::install`].
    fn dispose(&mut self);
}

/// One call a [`RecordingChromeHost`] saw.
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

/// The most calls a [`RecordingChromeHost`] keeps — it is the production
/// host on every target without an arm, where scroll publishes arrive at
/// frame rate, so the log drops its oldest entry rather than grow.
pub(crate) const RECORDED_CALLS_CAP: usize = 64;

/// The stand-in host for every target without a chrome arm: accepts every
/// install (unless built [`Self::refusing`]) and records each call.
#[derive(Debug, Default)]
pub(crate) struct RecordingChromeHost {
    calls: VecDeque<HostCall>,
    refusal: Option<ChromeError>,
}

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
