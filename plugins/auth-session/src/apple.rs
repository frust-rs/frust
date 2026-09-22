//! Apple (iOS + macOS) stub backend for [`crate::AuthSession::start`].
//!
//! Every session rejects immediately with
//! [`crate::AuthSessionError::Platform`]`("backend not implemented")` — this
//! crate's originating task builds only the host-testable core (the public
//! API, the `oneshot` future, the Busy guard, the URL/scheme validators) so
//! every real target compiles from day one; the real Apple backend
//! replaces this module with a real `ASWebAuthenticationSession`
//! (`objc2-authentication-services`) implementation — presenting on a
//! `UIWindow` anchor on iOS, an `NSWindow` anchor on macOS — without
//! touching `crate::AuthSession`'s public API.
//!
//! `target_vendor = "apple"`-gated (see `Cargo.toml`'s own comment) rather
//! than split further by `target_os` the way this crate's `android` module
//! has no macOS/iOS split to make: `ASWebAuthenticationSession` itself is
//! shared between iOS and macOS, only its presentation-context anchor type
//! differs (`objc2-ui-kit`'s `UIWindow` vs `objc2-app-kit`'s `NSWindow`,
//! each a `target_os`-gated dependency in `Cargo.toml`) — a distinction this
//! stub does not yet need to make since it never constructs either.

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
