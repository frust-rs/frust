//! Every target without a default-network monitor (Linux, Windows, web): the
//! monitor never starts and never reports, so no listener is ever called and
//! [`crate::Connectivity::current`] is always `None`.

/// Start nothing.
pub(crate) fn start() {}
