//! The `ChromeHost` seam a platform arm implements, the field-level
//! [`ChromeDiff`] it is handed, `NullChromeHost` — the production stand-in
//! on every target without an arm (neither iOS nor Android) — and the
//! test-only `RecordingChromeHost`,
//! which records calls the way the runtime's host `NativeCtx` does.

#[cfg(test)]
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{ChromeGeneration, ChromeSpec, TopBarSpec};

/// Why the chrome slot or a host refused a publish.
#[derive(thiserror::Error, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ChromeError {
    /// The platform arm declined to install (an Android theme that is not
    /// Material3-derived, an inflate failure) — the app falls back to
    /// frust-drawn chrome. Only an arm constructs it.
    #[cfg_attr(not(any(target_os = "ios", target_os = "android")), allow(dead_code))]
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
    #[cfg_attr(
        not(any(test, target_os = "ios", target_os = "android")),
        allow(dead_code)
    )]
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

    /// The failure channel: a failure the host found for `generation` after
    /// [`Self::install`] answered `Ok` (an arm that installs on a later turn
    /// and then finds no host, or panics), taken once. Reporting one forgets
    /// the install, so the slot never disposes it; a failure recorded for any
    /// other generation never answers here. Called under the slot lock on
    /// each of the generation's publishes and on its withdraw, so it must be
    /// cheap and lock-free.
    fn take_failure(&mut self, _generation: ChromeGeneration) -> Option<ChromeError> {
        None
    }

    /// `generation`, whose install failed, was withdrawn: stop any retry the
    /// host scheduled for it. [`Self::dispose`] covers an installed one.
    fn abandon(&mut self, _generation: ChromeGeneration) {}
}

/// The failure channel's cell for an arm whose failures arrive from a
/// platform callback (Android's `nativeOnChromeLost`): the failed generation
/// shifted past a small kind code, `0` for none. Written from the callback
/// and taken under the slot lock by [`ChromeHost::take_failure`], both
/// lock-free; a failure recorded for an older generation never replaces a
/// newer one's. The iOS arm's `FAILURE` cell packs the same way.
#[cfg_attr(not(any(test, target_os = "android")), allow(dead_code))]
#[derive(Debug)]
pub(crate) struct FailureCell(AtomicU64);

#[cfg_attr(not(any(test, target_os = "android")), allow(dead_code))]
impl FailureCell {
    /// The low bits a kind occupies; kinds are `1..=3`.
    const KIND_BITS: u32 = 2;

    /// An empty cell (const, for a `static`).
    pub(crate) const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    /// Record `kind` (`1..=3`) for `generation`, unless a newer
    /// generation's failure is already waiting. Generations stay far below
    /// `2^62`.
    pub(crate) fn record(&self, generation: u64, kind: u8) {
        debug_assert!((1..=3).contains(&kind), "failure kind {kind} out of range");
        let packed = (generation << Self::KIND_BITS) | u64::from(kind & 3);
        let _ = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |held| {
                (held >> Self::KIND_BITS <= generation).then_some(packed)
            });
    }

    /// The kind recorded for `generation`, taken once. An older
    /// generation's failure is spent (cleared, `None`); a newer one's stays
    /// for its own generation.
    pub(crate) fn take(&self, generation: u64) -> Option<u8> {
        let held = self.0.load(Ordering::Acquire);
        let kind = (held & ((1 << Self::KIND_BITS) - 1)) as u8;
        if kind == 0 {
            return None;
        }
        let failed = held >> Self::KIND_BITS;
        if failed > generation {
            return None;
        }
        self.0
            .compare_exchange(held, 0, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        (failed == generation).then_some(kind)
    }
}

/// The production host on every target without a chrome arm: accepts every
/// install and does nothing, so the scaffold draws the fallback bars.
#[cfg(not(any(test, target_os = "ios", target_os = "android")))]
#[derive(Debug, Default)]
pub(crate) struct NullChromeHost;

#[cfg(not(any(test, target_os = "ios", target_os = "android")))]
impl NullChromeHost {
    /// The host (const, for the process-wide slot's static).
    pub(crate) const fn new() -> Self {
        Self
    }
}

#[cfg(not(any(test, target_os = "ios", target_os = "android")))]
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
    /// A deferred failure [`Self::fail_deferred`] injected.
    failure: Option<(ChromeGeneration, ChromeError)>,
}

#[cfg(test)]
impl RecordingChromeHost {
    /// A host that accepts every install.
    pub(crate) const fn new() -> Self {
        Self {
            calls: VecDeque::new(),
            refusal: None,
            failure: None,
        }
    }

    /// A host whose every install fails with `error` (still recorded).
    pub(crate) fn refusing(error: ChromeError) -> Self {
        Self {
            calls: VecDeque::new(),
            refusal: Some(error),
            failure: None,
        }
    }

    /// Make every later install fail with `error`, or succeed for `None`.
    pub(crate) fn set_refusal(&mut self, error: Option<ChromeError>) {
        self.refusal = error;
    }

    /// Report `error` for `generation` through [`ChromeHost::take_failure`],
    /// as an arm whose install failed on a later turn does.
    pub(crate) fn fail_deferred(&mut self, generation: ChromeGeneration, error: ChromeError) {
        self.failure = Some((generation, error));
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

    /// Not recorded: the slot asks on every publish.
    fn take_failure(&mut self, generation: ChromeGeneration) -> Option<ChromeError> {
        match self.failure.take() {
            Some((failed, error)) if failed == generation => Some(error),
            // A newer generation's failure waits for it; an older one is dropped.
            Some(newer) if newer.0 > generation => {
                self.failure = Some(newer);
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FailureCell;

    #[test]
    fn a_failure_cell_answers_only_its_generation_once() {
        let cell = FailureCell::new();
        assert_eq!(cell.take(1), None);
        cell.record(4, 1);
        assert_eq!(cell.take(3), None, "a newer generation's failure waits");
        assert_eq!(cell.take(4), Some(1));
        assert_eq!(cell.take(4), None, "taken once");
    }

    #[test]
    fn an_older_failure_never_clobbers_a_newer_one_and_is_spent() {
        let cell = FailureCell::new();
        cell.record(7, 1);
        cell.record(6, 2);
        assert_eq!(cell.take(7), Some(1));
        cell.record(2, 1);
        assert_eq!(cell.take(5), None, "an older generation's failure is spent");
        assert_eq!(cell.take(2), None, "and gone");
    }
}
