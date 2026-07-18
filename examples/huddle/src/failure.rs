//! The app's closed failure enum.
//!
//! One `Failure` implementor for the whole app (small enough for a single
//! feature slice not to need its own). Every use case and repository maps
//! its errors into this type at the data-layer boundary — see
//! `features::team::data::repositories`.

use clean_signals::Failure;
use std::fmt;

/// Every failure mode `huddle`'s use cases can produce.
///
/// Matched exhaustively by presentation code (never string-matched) per
/// `docs/CODE_STANDARDS.md`'s "Generic-over-`F` failures" convention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TeamFailure {
    /// A transient transport/server error. Safe to retry.
    Network(String),
    /// Input rejected by domain rules (e.g. an empty/too-short name). Not
    /// retryable — retrying with the same bad input would just fail again.
    Validation(String),
}

impl fmt::Display for TeamFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TeamFailure::Network(msg) => write!(f, "network error: {msg}"),
            TeamFailure::Validation(msg) => write!(f, "validation error: {msg}"),
        }
    }
}

impl Failure for TeamFailure {
    fn user_message(&self) -> String {
        match self {
            TeamFailure::Network(_) => "Connection problem — please try again.".to_string(),
            TeamFailure::Validation(msg) => msg.clone(),
        }
    }

    fn is_retryable(&self) -> bool {
        matches!(self, TeamFailure::Network(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_is_retryable_with_generic_user_message() {
        let f = TeamFailure::Network("upstream 500".to_string());
        assert!(f.is_retryable());
        assert_eq!(f.user_message(), "Connection problem — please try again.");
    }

    #[test]
    fn validation_is_not_retryable_and_surfaces_its_own_message() {
        let f = TeamFailure::Validation("Name cannot be empty.".to_string());
        assert!(!f.is_retryable());
        assert_eq!(f.user_message(), "Name cannot be empty.");
    }
}
