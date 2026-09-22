//! Android stub backend for [`crate::AuthSession::start`].
//!
//! Every session rejects immediately with
//! [`crate::AuthSessionError::Platform`]`("backend not implemented")` — this
//! crate's originating task builds only the host-testable core (the public
//! API, the `oneshot` future, the Busy guard, the URL/scheme validators) so
//! every real target compiles from day one; the Android backend card
//! replaces this module with a real Chrome Custom Tabs
//! (`androidx.browser.customtabs`) implementation without touching
//! `crate::AuthSession`'s public API.

use crate::{AuthSessionError, AuthSessionRequest, SessionToken};

/// Reject every session — see this module's own doc.
pub(crate) fn start(
    _req: AuthSessionRequest,
    _token: SessionToken,
) -> Result<(), AuthSessionError> {
    Err(AuthSessionError::Platform(
        "backend not implemented".to_string(),
    ))
}
