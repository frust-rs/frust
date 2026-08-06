//! Doctor panel state (PLAN D6/D6a): the titlebar toolchain chip and the
//! `d`-opened panel both read the same cached [`DoctorState`] — a flat mirror
//! of `frust-drive`'s [`Validator`](frust_drive::doctor::Validator) set run
//! off-thread (`spawn_blocking`, see `crate::runner`) at startup preflight and
//! again on demand. Today's flat [`frust_drive::doctor::Validation`] is the
//! source (the full D6a component-level report + guided commands are
//! future work).
//!
//! `frust_drive::doctor::Validation` doesn't derive `PartialEq`/`Eq`, and
//! [`crate::engine::Message`]/[`crate::engine::Effect`] both do — so
//! [`DoctorCheck`] is a small by-value mirror (name + status + messages)
//! rather than carrying the drive type directly, the same shape `RegisterSession`
//! carries session metadata the bare `SessionEvent` doesn't.

use frust_drive::doctor::Status;

/// One validator's result, flattened out of `frust_drive::doctor::Validation`
/// at the runner boundary (see the module doc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorCheck {
    /// The validator's name (`Validator::name`).
    pub name: String,
    /// Pass / Partial / Fail.
    pub status: Status,
    /// Actionable hint lines (only shown when non-passing, or in a verbose
    /// panel view — see `crate::ui::views::doctor`).
    pub messages: Vec<String>,
}

/// The doctor panel's cached state: the last full run's results (empty before
/// the first startup preflight completes) plus whether a re-run is in flight.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DoctorState {
    /// Every validator's last result, in `default_validators()` order.
    pub results: Vec<DoctorCheck>,
    /// Whether a validator run is currently in flight (spinner state).
    pub refreshing: bool,
}

impl DoctorState {
    /// The aggregate chip status (worst check wins: any `Fail` beats any
    /// `Partial` beats all-`Pass`), or `None` before the first result ever
    /// arrives (the startup preflight hasn't completed yet).
    pub fn overall(&self) -> Option<Status> {
        if self.results.is_empty() {
            return None;
        }
        let mut worst = Status::Pass;
        for check in &self.results {
            worst = match (worst, check.status) {
                (Status::Fail, _) | (_, Status::Fail) => Status::Fail,
                (Status::Partial, _) | (_, Status::Partial) => Status::Partial,
                _ => Status::Pass,
            };
        }
        Some(worst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(status: Status) -> DoctorCheck {
        DoctorCheck {
            name: "x".to_string(),
            status,
            messages: Vec::new(),
        }
    }

    #[test]
    fn overall_is_none_before_any_result() {
        assert_eq!(DoctorState::default().overall(), None);
    }

    #[test]
    fn overall_is_pass_when_every_check_passes() {
        let state = DoctorState {
            results: vec![check(Status::Pass), check(Status::Pass)],
            refreshing: false,
        };
        assert_eq!(state.overall(), Some(Status::Pass));
    }

    #[test]
    fn overall_is_partial_when_no_fail_but_a_partial_exists() {
        let state = DoctorState {
            results: vec![check(Status::Pass), check(Status::Partial)],
            refreshing: false,
        };
        assert_eq!(state.overall(), Some(Status::Partial));
    }

    #[test]
    fn overall_is_fail_when_any_check_fails_even_alongside_partial() {
        let state = DoctorState {
            results: vec![
                check(Status::Partial),
                check(Status::Fail),
                check(Status::Pass),
            ],
            refreshing: false,
        };
        assert_eq!(state.overall(), Some(Status::Fail));
    }
}
