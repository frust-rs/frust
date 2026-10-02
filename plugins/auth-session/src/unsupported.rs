//! No in-app browser tab on this target (Linux, Windows, wasm, tvOS, …) —
//! every custom-scheme [`crate::AuthSession::start`] rejects immediately
//! with [`crate::AuthSessionError::NoHandler`], with no `oneshot` channel
//! ever genuinely left live (the future resolves on its first poll and the
//! slot is released).
//!
//! Desktop Linux and Windows ship no first-class in-app browser-tab API the
//! way Android's Custom Tabs or Apple's `ASWebAuthenticationSession` do, so
//! a custom-scheme callback has nothing to arrive through there. **On Linux
//! and Windows, use [`crate::LoopbackSession`] instead**: RFC 8252's
//! loopback interface redirection, which opens the system browser through
//! `frust-url-launcher` and receives the identity provider's redirect on
//! `http://127.0.0.1:<port>/<path>` (see the crate doc's *Desktop loopback*
//! section). [`crate::AuthSession::is_supported`] answers `false` on those
//! two targets — it describes this module's custom-scheme path, which cannot
//! work — and [`crate::LoopbackSession::is_supported`] answers `true` there.
//! Every other target this module builds for has neither backend.

use crate::{AuthSessionError, AuthSessionRequest, SessionToken};

/// Reject every custom-scheme session — see this module's own doc.
pub(crate) fn start(
    _req: AuthSessionRequest,
    _token: SessionToken,
) -> Result<(), AuthSessionError> {
    Err(AuthSessionError::NoHandler)
}
