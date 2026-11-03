//! Engine frame refusals as a [`crate::SurfaceRenderer`] reports them
//! ([`EngineRefusalReport`]), and the `FRUST_RENDER_INJECT_REFUSAL` knob that
//! forces one, so the skip paths a real refusal takes can be exercised on
//! demand.
//!
//! The knob is `plan:N` or `record:N` (`N >= 1`): every `N`th frame reaching
//! that stage is refused. `plan` is the encode-stage refusal, decided before
//! the swapchain is acquired; `record` is the stage after acquire, which the
//! renderer queries through [`RefusalInjector::refuses`]. Read once per
//! process, from the compile-time define or the runtime environment, a
//! non-empty runtime value winning. Unset, empty or unparsable means off.

use std::fmt;
use std::num::NonZeroU32;
use std::sync::OnceLock;

/// The knob's name, in either its compile-time or runtime half.
pub(crate) const INJECT_REFUSAL_VAR: &str = "FRUST_RENDER_INJECT_REFUSAL";

/// The reason an injected refusal is logged and reported under.
pub(crate) const INJECTED_REASON: &str = "injected by FRUST_RENDER_INJECT_REFUSAL";

/// The frames a [`crate::SurfaceRenderer`]'s engine refused.
///
/// `count` covers every refused frame of every surface this renderer held —
/// refused while the frame was planned, before acquire, or while it was
/// recorded after it — and `first_reason` latches the first one's reason and
/// is never overwritten. Non-exhaustive, so later fields are additive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct EngineRefusalReport {
    /// How many frames were refused.
    pub count: u32,
    /// The first refused frame's reason, as it was logged.
    pub first_reason: Option<String>,
}

impl EngineRefusalReport {
    /// Counts one refused frame, latching `reason` when it is the first.
    pub(crate) fn note(&mut self, reason: &dyn fmt::Display) {
        self.count = self.count.saturating_add(1);
        if self.first_reason.is_none() {
            self.first_reason = Some(reason.to_string());
        }
    }
}

/// Why a frame was refused: the engine's own error, or the knob.
#[derive(Debug)]
pub(crate) enum Refusal {
    /// The engine refused the frame.
    Engine(frust_engine::EngineError),
    /// `FRUST_RENDER_INJECT_REFUSAL` refused it.
    Injected,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Engine(error) => error.fmt(f),
            Self::Injected => f.write_str(INJECTED_REASON),
        }
    }
}

/// The stage of a frame an injected refusal lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefusalStage {
    /// The encode-stage plan, before the swapchain is acquired.
    Plan,
    /// The engine's recording, after the swapchain is acquired.
    Record,
}

/// A parsed `FRUST_RENDER_INJECT_REFUSAL`: refuse every `every`th frame
/// reaching `stage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Injection {
    pub(crate) stage: RefusalStage,
    pub(crate) every: NonZeroU32,
}

/// Parses a raw knob value; `None` — the knob off — for an unset, empty or
/// unparsable one. Surrounding whitespace and the stage name's case are
/// ignored; `N` must be a whole number of at least 1.
pub(crate) fn parse_injection(raw: Option<&str>) -> Option<Injection> {
    let (stage, every) = raw?.trim().split_once(':')?;
    let stage = if stage.eq_ignore_ascii_case("plan") {
        RefusalStage::Plan
    } else if stage.eq_ignore_ascii_case("record") {
        RefusalStage::Record
    } else {
        return None;
    };
    let every = every.parse::<NonZeroU32>().ok()?;
    Some(Injection { stage, every })
}

/// The value the process configuration selects: a non-empty runtime value,
/// else a non-empty compile-time one.
fn chosen<'a>(compile_time: Option<&'a str>, runtime: Option<&'a str>) -> Option<&'a str> {
    runtime
        .filter(|value| !value.is_empty())
        .or(compile_time.filter(|value| !value.is_empty()))
}

/// The injection the process configuration selects, resolved once. A set but
/// unparsable value warns once and leaves the knob off.
fn configured_injection() -> Option<Injection> {
    static INJECTION: OnceLock<Option<Injection>> = OnceLock::new();
    *INJECTION.get_or_init(|| {
        let runtime = std::env::var(INJECT_REFUSAL_VAR).ok();
        let raw = chosen(
            option_env!("FRUST_RENDER_INJECT_REFUSAL"),
            runtime.as_deref(),
        );
        let injection = parse_injection(raw);
        match (raw, injection) {
            (Some(value), None) => log::warn!(
                "frust-render: ignoring {INJECT_REFUSAL_VAR}={value:?} (expected plan:N or \
                 record:N with N >= 1)"
            ),
            (Some(value), Some(_)) => {
                log::warn!(
                    "frust-render: {INJECT_REFUSAL_VAR}={value} — refusing frames on purpose"
                );
            }
            (None, _) => {}
        }
        injection
    })
}

/// One renderer's injected-refusal clock: counts the frames reaching the
/// configured stage and answers whether the current one is refused.
#[derive(Debug, Clone, Default)]
pub(crate) struct RefusalInjector {
    injection: Option<Injection>,
    reached: u64,
}

impl RefusalInjector {
    /// An injector for `injection` (`None` refuses nothing).
    pub(crate) fn new(injection: Option<Injection>) -> Self {
        Self {
            injection,
            reached: 0,
        }
    }

    /// The injector the process configuration selects.
    pub(crate) fn from_config() -> Self {
        Self::new(configured_injection())
    }

    /// Whether the frame now reaching `stage` is refused, counting it when
    /// `stage` is the configured one. Always `false` with the knob off.
    pub(crate) fn refuses(&mut self, stage: RefusalStage) -> bool {
        let Some(injection) = self.injection.filter(|injection| injection.stage == stage) else {
            return false;
        };
        self.reached = self.reached.saturating_add(1);
        self.reached
            .is_multiple_of(u64::from(injection.every.get()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn injection(stage: RefusalStage, every: u32) -> Option<Injection> {
        Some(Injection {
            stage,
            every: NonZeroU32::new(every).expect("a test names a non-zero period"),
        })
    }

    #[test]
    fn the_knob_parses_a_stage_and_a_period() {
        assert_eq!(
            parse_injection(Some("plan:1")),
            injection(RefusalStage::Plan, 1)
        );
        assert_eq!(
            parse_injection(Some("record:7")),
            injection(RefusalStage::Record, 7)
        );
        assert_eq!(
            parse_injection(Some("  PLAN:3 ")),
            injection(RefusalStage::Plan, 3),
            "whitespace and the stage's case are ignored"
        );
    }

    #[test]
    fn an_unset_empty_or_unparsable_knob_is_off() {
        for raw in [
            None,
            Some(""),
            Some("plan"),
            Some("plan:"),
            Some("plan:0"),
            Some("plan:-1"),
            Some("plan:x"),
            Some("plan:1:2"),
            Some("submit:1"),
            Some(":1"),
            Some("1"),
        ] {
            assert_eq!(
                parse_injection(raw),
                None,
                "{raw:?} must leave the knob off"
            );
        }
    }

    #[test]
    fn a_non_empty_runtime_value_wins_over_the_compile_time_one() {
        assert_eq!(chosen(Some("plan:1"), Some("record:2")), Some("record:2"));
        assert_eq!(chosen(Some("plan:1"), Some("")), Some("plan:1"));
        assert_eq!(chosen(Some("plan:1"), None), Some("plan:1"));
        assert_eq!(chosen(Some(""), Some("")), None);
        assert_eq!(chosen(None, None), None);
    }

    #[test]
    fn the_injector_refuses_every_nth_frame_of_its_own_stage_only() {
        let mut injector = RefusalInjector::new(injection(RefusalStage::Plan, 3));
        let refused: Vec<bool> = (0..6)
            .map(|_| injector.refuses(RefusalStage::Plan))
            .collect();
        assert_eq!(refused, [false, false, true, false, false, true]);
        assert!(
            (0..6).all(|_| !injector.refuses(RefusalStage::Record)),
            "another stage is never refused, and does not advance the clock"
        );
        assert!(!injector.refuses(RefusalStage::Plan));
        assert!(!injector.refuses(RefusalStage::Plan));
        assert!(injector.refuses(RefusalStage::Plan));

        let mut off = RefusalInjector::default();
        assert!(!off.refuses(RefusalStage::Plan));
        assert!(!off.refuses(RefusalStage::Record));
    }

    #[test]
    fn the_report_counts_every_refusal_and_latches_the_first_reason() {
        let mut report = EngineRefusalReport::default();
        assert_eq!(report.count, 0);
        assert_eq!(report.first_reason, None);

        report.note(&Refusal::Injected);
        report.note(&Refusal::Engine(frust_engine::EngineError::TargetTooLarge));
        report.note(&"a later reason");
        assert_eq!(report.count, 3);
        assert_eq!(report.first_reason.as_deref(), Some(INJECTED_REASON));
    }
}
