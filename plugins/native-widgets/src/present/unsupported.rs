//! The presentation arm for every target with no native modal UI this plugin
//! drives (desktop Linux, Windows, web): every request resolves
//! [`PresentError::Unsupported`] on its first poll, and nothing is ever live
//! to dismiss.

use super::{AlertHost, AlertOutcome, AlertSpec, PresentError, Sender};

/// The refusing host.
pub(crate) struct Host;

impl AlertHost for Host {
    fn show_alert(
        _spec: AlertSpec,
        _tx: Sender<AlertOutcome>,
        _generation: u64,
    ) -> Result<(), PresentError> {
        Err(PresentError::Unsupported)
    }

    fn dismiss(_generation: u64) {}
}

#[cfg(test)]
mod tests {
    use super::super::{ActionRole, oneshot};
    use super::*;

    /// Driven through the host directly rather than `show_alert`, so this
    /// test never touches the process-wide Busy slot `super::super`'s own
    /// serialized tests own.
    #[test]
    fn every_show_is_refused_unsupported() {
        // A generation nothing is ever live under (see `oneshot`'s tests).
        let (tx, _rx) = oneshot::channel(u64::MAX);
        let spec = AlertSpec::new("t", "m").with_action("ok", "OK", ActionRole::Default);
        assert_eq!(
            Host::show_alert(spec, tx, u64::MAX),
            Err(PresentError::Unsupported)
        );
        Host::dismiss(u64::MAX);
    }
}
