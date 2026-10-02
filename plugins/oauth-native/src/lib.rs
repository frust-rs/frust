//! `frust-oauth-native`: the security-critical protocol half of an RFC 8252
//! "OAuth 2.0 for Native Apps" authorization-code flow — PKCE, `state`, the
//! authorization URL, callback validation and token-grant bodies — as plain,
//! host-testable Rust.
//!
//! # Charter
//!
//! This crate has **no platform code, no HTTP client, no async runtime and
//! no `frust-*` dependency**. It pairs with `frust-auth-session` (which
//! presents the authorization URL in the platform's in-app browser tab and
//! hands back the raw callback URL) but does not depend on it, and it leaves
//! the token endpoint's HTTP transport to the app: [`authorization_code_grant`]
//! and [`refresh_token_grant`] produce a [`FormBody`] to `POST`, and
//! [`parse_token_response`] reads the status and body bytes the app's own
//! client received.
//!
//! # Flow
//!
//! 1. [`PkceVerifier::generate`] and [`State::generate`] — fresh per attempt.
//! 2. [`AuthorizationRequest::build`] — the URL to open.
//! 3. [`parse_callback`] — validates the redirect (exact redirect URI, RFC
//!    9207 `iss`, constant-time `state`, typed error responses) and yields an
//!    [`AuthorizationCode`].
//! 4. [`authorization_code_grant`] → app `POST` → [`parse_token_response`].
//!
//! # Security properties
//!
//! - **S256 only.** There is no `plain` PKCE method.
//! - **256-bit `state`**, compared in constant time.
//! - **No skip for `iss`.** [`IssuerCheck`] is either required or checked
//!   when present (RFC 9207 §2.4).
//! - **Fixed check order.** The issuer and `state` are verified before an
//!   `error` response is believed.
//! - **Redacted secrets.** Every secret-bearing type's `Debug` redacts the
//!   secret, and no error message includes a code, `state`, callback URL or
//!   token response body.

#![forbid(unsafe_code)]

mod authorize;
mod callback;
mod encode;
mod pkce;
mod token;

pub use authorize::{AuthorizationRequest, AuthorizeError};
pub use callback::{
    AuthorizationCode, AuthorizationErrorCode, CallbackError, CallbackExpectations, IssuerCheck,
    parse_callback,
};
pub use pkce::{
    MAX_VERIFIER_LEN, MIN_VERIFIER_LEN, PkceChallenge, PkceError, PkceVerifier, RandomError, State,
};
pub use token::{
    FORM_CONTENT_TYPE, FormBody, TokenError, TokenErrorCode, TokenResponse,
    authorization_code_grant, parse_token_response, refresh_token_grant,
};
