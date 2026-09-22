//! No platform authentication user agent on this target (Linux, Windows,
//! wasm, tvOS, …) — every session rejects immediately with
//! [`crate::AuthSessionError::NoHandler`], with no `oneshot` channel ever
//! genuinely left live (see [`crate::AuthSession::start`]'s doc's
//! *Ready-on-first-poll errors*).
//!
//! Neither desktop Linux nor desktop Windows ships a first-class in-app
//! browser-tab API the way Android's Custom Tabs or Apple's
//! `ASWebAuthenticationSession` do, and this crate has no plan to build one
//! (unlike, say, `frust-iap`'s desktop gap, which is a deliberate *v1*
//! deferral of a real, buildable platform API — see that crate's own
//! `Cargo.toml` comment). Callers on these targets should drive the same
//! RFC 8252 flow through
//! [`frust-url-launcher`](../frust_url_launcher/index.html)'s ordinary
//! system-browser launch instead, running a local loopback HTTP listener (or
//! an equivalent out-of-band code-retrieval step) to receive the identity
//! provider's redirect and hand the app a typed authorization code — the
//! same "loopback IP redirection" pattern RFC 8252 itself documents for
//! platforms with no in-app browser-tab primitive.

use crate::{AuthSessionError, AuthSessionRequest, SessionToken};

/// Reject every session — see this module's own doc.
pub(crate) fn start(
    _req: AuthSessionRequest,
    _token: SessionToken,
) -> Result<(), AuthSessionError> {
    Err(AuthSessionError::NoHandler)
}
