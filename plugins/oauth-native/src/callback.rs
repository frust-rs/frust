//! Authorization-response (callback) validation: redirect URI match, RFC
//! 9207 `iss`, constant-time `state`, RFC 6749 §4.1.2.1 error responses, and
//! finally the authorization code.
//!
//! [`parse_callback`]'s checks run in a fixed order so an attacker-supplied
//! error response cannot be mistaken for the real one: the issuer and
//! `state` are verified **before** an `error` parameter is believed, so an
//! error response carrying the wrong `state` is a
//! [`CallbackError::StateMismatch`], never a typed
//! [`CallbackError::Authorization`].

use std::fmt;

use subtle::ConstantTimeEq as _;

use crate::encode::{is_error_text, is_error_uri, parse_query};
use crate::pkce::State;

/// How the callback's `iss` parameter (RFC 9207) is checked. There is
/// deliberately no variant that skips the check: RFC 9207 §2.4 requires a
/// client that knows the issuer to compare any `iss` it receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssuerCheck {
    /// The authorization server advertises
    /// `authorization_response_iss_parameter_supported`: `iss` must be
    /// present and equal this issuer.
    Required(String),
    /// The server does not advertise `iss` support: an absent `iss` is
    /// accepted, but a present one must equal this issuer.
    IfPresent(String),
}

impl IssuerCheck {
    /// Picks the check from the server's metadata (RFC 8414 `issuer` and
    /// RFC 9207 `authorization_response_iss_parameter_supported`).
    pub fn from_metadata(issuer: String, iss_parameter_supported: bool) -> Self {
        if iss_parameter_supported {
            Self::Required(issuer)
        } else {
            Self::IfPresent(issuer)
        }
    }
}

/// What the callback must match: the redirect URI the request was built
/// with, the `state` it carried, and the issuer policy.
#[derive(Debug, Clone)]
pub struct CallbackExpectations {
    /// The exact `redirect_uri` sent in the authorization request. Must
    /// carry no query and no fragment.
    pub redirect_uri: String,
    /// The `state` sent in the authorization request.
    pub state: State,
    /// The `iss` policy for this authorization server.
    pub issuer: IssuerCheck,
}

/// An RFC 6749 §4.1.2.1 authorization error code.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthorizationErrorCode {
    /// `invalid_request`
    InvalidRequest,
    /// `unauthorized_client`
    UnauthorizedClient,
    /// `access_denied`
    AccessDenied,
    /// `unsupported_response_type`
    UnsupportedResponseType,
    /// `invalid_scope`
    InvalidScope,
    /// `server_error`
    ServerError,
    /// `temporarily_unavailable`
    TemporarilyUnavailable,
    /// Any other (extension) error code, charset-validated.
    Other(String),
}

impl AuthorizationErrorCode {
    fn from_code(code: &str) -> Self {
        match code {
            "invalid_request" => Self::InvalidRequest,
            "unauthorized_client" => Self::UnauthorizedClient,
            "access_denied" => Self::AccessDenied,
            "unsupported_response_type" => Self::UnsupportedResponseType,
            "invalid_scope" => Self::InvalidScope,
            "server_error" => Self::ServerError,
            "temporarily_unavailable" => Self::TemporarilyUnavailable,
            other => Self::Other(other.to_string()),
        }
    }

    /// The wire form of the code (`"access_denied"`, …).
    pub fn as_str(&self) -> &str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::UnauthorizedClient => "unauthorized_client",
            Self::AccessDenied => "access_denied",
            Self::UnsupportedResponseType => "unsupported_response_type",
            Self::InvalidScope => "invalid_scope",
            Self::ServerError => "server_error",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
            Self::Other(code) => code,
        }
    }
}

impl fmt::Display for AuthorizationErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The callback was rejected. No message prints the authorization code, the
/// `state`, or the callback URL; [`Authorization`](Self::Authorization)
/// prints only the server's charset-validated error code and description.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CallbackError {
    /// The expected redirect URI carries a query or fragment, so an exact
    /// match is impossible.
    #[error("the expected redirect URI must not contain a query or fragment")]
    InvalidRedirectUri,
    /// The callback carries a fragment, a malformed query, or an error
    /// response with out-of-charset fields.
    #[error("the callback URL is malformed")]
    Malformed,
    /// The callback's scheme, authority or path differs from the expected
    /// redirect URI.
    #[error("the callback URL does not match the expected redirect URI")]
    RedirectMismatch,
    /// A response parameter appears more than once.
    #[error("the callback carries the `{0}` parameter more than once")]
    DuplicateParameter(&'static str),
    /// `iss` is required by the issuer policy but absent.
    #[error("the callback carries no `iss` parameter but the issuer requires one")]
    IssuerMissing,
    /// `iss` differs from the expected issuer.
    #[error("the callback's `iss` does not match the expected issuer")]
    IssuerMismatch,
    /// `state` is absent.
    #[error("the callback carries no `state` parameter")]
    StateMissing,
    /// `state` differs from the one sent.
    #[error("the callback's `state` does not match the request")]
    StateMismatch,
    /// The authorization server returned an error response (after the
    /// issuer and `state` checks passed).
    #[error("the authorization server returned `{error}`{}", describe(description.as_deref()))]
    Authorization {
        /// The error code.
        error: AuthorizationErrorCode,
        /// The `error_description`, if any.
        description: Option<String>,
        /// The `error_uri`, if any.
        uri: Option<String>,
    },
    /// The response has neither an error nor a non-empty `code`.
    #[error("the callback carries no authorization code")]
    CodeMissing,
}

fn describe(description: Option<&str>) -> String {
    description.map_or_else(String::new, |d| format!(": {d}"))
}

/// A validated authorization code, for the token request. Its `Debug`
/// redacts the code.
pub struct AuthorizationCode(String);

impl AuthorizationCode {
    /// The code text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the wrapper, returning the code text.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Debug for AuthorizationCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthorizationCode(<redacted>)")
    }
}

/// Validates the redirect the in-app browser session (or loopback listener)
/// delivered and extracts the authorization code. Checks, in order:
///
/// 1. the expected redirect URI has no query or fragment;
/// 2. the callback has no fragment;
/// 3. the callback (before `?`) equals the expected redirect URI — the
///    scheme and, for `http`/`https`, the host compare ASCII
///    case-insensitively, everything else exactly;
/// 4. the query decodes, and none of `code`, `state`, `iss`, `error`,
///    `error_description`, `error_uri` repeats;
/// 5. `iss` per [`IssuerCheck`];
/// 6. `state` is present and equal, compared in constant time;
/// 7. an `error` response becomes [`CallbackError::Authorization`];
/// 8. a non-empty `code` is returned.
pub fn parse_callback(
    callback_url: &str,
    exp: &CallbackExpectations,
) -> Result<AuthorizationCode, CallbackError> {
    if exp.redirect_uri.contains(['?', '#']) {
        return Err(CallbackError::InvalidRedirectUri);
    }
    if callback_url.contains('#') {
        return Err(CallbackError::Malformed);
    }
    let (base, query) = callback_url.split_once('?').unwrap_or((callback_url, ""));
    if !redirect_matches(base, &exp.redirect_uri) {
        return Err(CallbackError::RedirectMismatch);
    }

    let params = ResponseParams::from_query(query)?;

    match (&exp.issuer, &params.iss) {
        (IssuerCheck::Required(_), None) => return Err(CallbackError::IssuerMissing),
        (IssuerCheck::Required(expected) | IssuerCheck::IfPresent(expected), Some(iss))
            if iss != expected =>
        {
            return Err(CallbackError::IssuerMismatch);
        }
        _ => {}
    }

    let Some(state) = &params.state else {
        return Err(CallbackError::StateMissing);
    };
    let state_equal: bool = state.as_bytes().ct_eq(exp.state.as_str().as_bytes()).into();
    if !state_equal {
        return Err(CallbackError::StateMismatch);
    }

    if let Some(error) = params.error {
        let description_ok = params
            .error_description
            .as_deref()
            .is_none_or(is_error_text);
        let uri_ok = params.error_uri.as_deref().is_none_or(is_error_uri);
        if error.is_empty() || !is_error_text(&error) || !description_ok || !uri_ok {
            return Err(CallbackError::Malformed);
        }
        return Err(CallbackError::Authorization {
            error: AuthorizationErrorCode::from_code(&error),
            description: params.error_description,
            uri: params.error_uri,
        });
    }

    match params.code {
        Some(code) if !code.is_empty() => Ok(AuthorizationCode(code)),
        _ => Err(CallbackError::CodeMissing),
    }
}

/// The response parameters [`parse_callback`] reads; anything else in the
/// query is ignored.
#[derive(Default)]
struct ResponseParams {
    code: Option<String>,
    state: Option<String>,
    iss: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
    error_uri: Option<String>,
}

impl ResponseParams {
    fn from_query(query: &str) -> Result<Self, CallbackError> {
        let pairs = parse_query(query).map_err(|_| CallbackError::Malformed)?;
        let mut params = Self::default();
        for (name, value) in pairs {
            let (slot, name): (&mut Option<String>, &'static str) = match name.as_str() {
                "code" => (&mut params.code, "code"),
                "state" => (&mut params.state, "state"),
                "iss" => (&mut params.iss, "iss"),
                "error" => (&mut params.error, "error"),
                "error_description" => (&mut params.error_description, "error_description"),
                "error_uri" => (&mut params.error_uri, "error_uri"),
                _ => continue,
            };
            if slot.is_some() {
                return Err(CallbackError::DuplicateParameter(name));
            }
            *slot = Some(value);
        }
        Ok(params)
    }
}

/// Compares the callback's base (no query, no fragment) with the expected
/// redirect URI: the scheme always ASCII case-insensitively, the host too
/// for `http`/`https`, everything else (userinfo, port, path, an opaque
/// private-use path) exactly.
fn redirect_matches(actual: &str, expected: &str) -> bool {
    let (Some((a_scheme, a_rest)), Some((e_scheme, e_rest))) =
        (actual.split_once(':'), expected.split_once(':'))
    else {
        return actual == expected;
    };
    if !a_scheme.eq_ignore_ascii_case(e_scheme) {
        return false;
    }
    let web = e_scheme.eq_ignore_ascii_case("http") || e_scheme.eq_ignore_ascii_case("https");
    if !web {
        return a_rest == e_rest;
    }
    let (Some(a_hier), Some(e_hier)) = (a_rest.strip_prefix("//"), e_rest.strip_prefix("//"))
    else {
        return a_rest == e_rest;
    };
    let (a_authority, a_path) = split_authority(a_hier);
    let (e_authority, e_path) = split_authority(e_hier);
    a_path == e_path && authority_matches(a_authority, e_authority)
}

fn split_authority(hier: &str) -> (&str, &str) {
    let end = hier.find('/').unwrap_or(hier.len());
    hier.split_at(end)
}

fn authority_matches(actual: &str, expected: &str) -> bool {
    let (a_userinfo, a_hostport) = split_userinfo(actual);
    let (e_userinfo, e_hostport) = split_userinfo(expected);
    let (a_host, a_port) = split_port(a_hostport);
    let (e_host, e_port) = split_port(e_hostport);
    a_userinfo == e_userinfo && a_port == e_port && a_host.eq_ignore_ascii_case(e_host)
}

fn split_userinfo(authority: &str) -> (Option<&str>, &str) {
    match authority.rsplit_once('@') {
        Some((userinfo, hostport)) => (Some(userinfo), hostport),
        None => (None, authority),
    }
}

/// Splits `host[:port]` (including a bracketed IPv6 literal) into the host
/// and the `:port` suffix (empty when absent).
fn split_port(hostport: &str) -> (&str, &str) {
    if hostport.starts_with('[') {
        if let Some(close) = hostport.find(']') {
            return hostport.split_at(close + 1);
        }
        return (hostport, "");
    }
    match hostport.rfind(':') {
        Some(colon) => hostport.split_at(colon),
        None => (hostport, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUSTOM: &str = "com.example.app:/oauth/callback";
    const LOOPBACK: &str = "http://127.0.0.1:53121/oauth/callback";
    const ISSUER: &str = "https://as.example";
    const ISS_ENC: &str = "https%3A%2F%2Fas.example";
    const STATE: &str = "xyzSTATE-123_~";
    const CODE: &str = "SplxlOBeZQQYbYS6WxSbIA";

    fn exp(redirect: &str, issuer: IssuerCheck) -> CallbackExpectations {
        CallbackExpectations {
            redirect_uri: redirect.to_string(),
            state: State::from_string(STATE.to_string()).unwrap(),
            issuer,
        }
    }

    fn required() -> IssuerCheck {
        IssuerCheck::Required(ISSUER.to_string())
    }

    fn if_present() -> IssuerCheck {
        IssuerCheck::IfPresent(ISSUER.to_string())
    }

    #[test]
    fn issuer_check_from_metadata() {
        assert_eq!(IssuerCheck::from_metadata(ISSUER.into(), true), required());
        assert_eq!(
            IssuerCheck::from_metadata(ISSUER.into(), false),
            if_present()
        );
    }

    #[test]
    fn success_via_private_use_scheme() {
        let url = format!("{CUSTOM}?code={CODE}&state={STATE}&iss={ISS_ENC}");
        let code = parse_callback(&url, &exp(CUSTOM, required())).unwrap();
        assert_eq!(code.as_str(), CODE);
        assert_eq!(code.into_string(), CODE);
        // The scheme compares case-insensitively; the path does not.
        let url = format!("COM.EXAMPLE.APP:/oauth/callback?code={CODE}&state={STATE}");
        assert!(parse_callback(&url, &exp(CUSTOM, if_present())).is_ok());
        let url = format!("com.example.app:/OAuth/callback?code={CODE}&state={STATE}");
        assert_eq!(
            parse_callback(&url, &exp(CUSTOM, if_present())).unwrap_err(),
            CallbackError::RedirectMismatch
        );
    }

    #[test]
    fn success_via_loopback() {
        let url = format!("{LOOPBACK}?state={STATE}&code={CODE}&iss={ISS_ENC}&extra=1");
        let code = parse_callback(&url, &exp(LOOPBACK, required())).unwrap();
        assert_eq!(code.as_str(), CODE);
        // Scheme and host compare case-insensitively for http(s).
        let url = format!("HTTP://LocalHost:53121/oauth/callback?code={CODE}&state={STATE}");
        let e = exp("http://localhost:53121/oauth/callback", if_present());
        assert!(parse_callback(&url, &e).is_ok());
        // IPv6 loopback.
        let url = format!("http://[::1]:53121/oauth/callback?code={CODE}&state={STATE}");
        let e = exp("http://[::1]:53121/oauth/callback", if_present());
        assert!(parse_callback(&url, &e).is_ok());
    }

    #[test]
    fn redirect_mismatch_on_path_port_and_scheme() {
        let e = exp(LOOPBACK, if_present());
        for base in [
            "http://127.0.0.1:53121/oauth/callbackx",
            "http://127.0.0.1:53121/oauth",
            "http://127.0.0.1:53122/oauth/callback",
            "http://127.0.0.1/oauth/callback",
            "https://127.0.0.1:53121/oauth/callback",
            "http://127.0.0.2:53121/oauth/callback",
            "http://user@127.0.0.1:53121/oauth/callback",
            "com.example.app:/oauth/callback",
            "no-scheme",
        ] {
            let url = format!("{base}?code={CODE}&state={STATE}");
            assert_eq!(
                parse_callback(&url, &e).unwrap_err(),
                CallbackError::RedirectMismatch,
                "{base}"
            );
        }
    }

    #[test]
    fn invalid_expected_redirect_and_fragment() {
        let url = format!("{CUSTOM}?code={CODE}&state={STATE}");
        let e = exp("com.example.app:/oauth/callback?x=1", if_present());
        assert_eq!(
            parse_callback(&url, &e).unwrap_err(),
            CallbackError::InvalidRedirectUri
        );
        let e = exp("com.example.app:/oauth/callback#f", if_present());
        assert_eq!(
            parse_callback(&url, &e).unwrap_err(),
            CallbackError::InvalidRedirectUri
        );
        let url = format!("{CUSTOM}?code={CODE}&state={STATE}#frag");
        assert_eq!(
            parse_callback(&url, &exp(CUSTOM, if_present())).unwrap_err(),
            CallbackError::Malformed
        );
        let url = format!("{CUSTOM}?code=%zz&state={STATE}");
        assert_eq!(
            parse_callback(&url, &exp(CUSTOM, if_present())).unwrap_err(),
            CallbackError::Malformed
        );
    }

    #[test]
    fn duplicate_parameters_are_refused() {
        let e = exp(CUSTOM, if_present());
        let url = format!("{CUSTOM}?code={CODE}&state={STATE}&state={STATE}");
        assert_eq!(
            parse_callback(&url, &e).unwrap_err(),
            CallbackError::DuplicateParameter("state")
        );
        let url = format!("{CUSTOM}?code=a&code=b&state={STATE}");
        assert_eq!(
            parse_callback(&url, &e).unwrap_err(),
            CallbackError::DuplicateParameter("code")
        );
        let url = format!("{CUSTOM}?code={CODE}&state={STATE}&iss=a&iss=b");
        assert_eq!(
            parse_callback(&url, &e).unwrap_err(),
            CallbackError::DuplicateParameter("iss")
        );
        // An unrelated parameter may repeat.
        let url = format!("{CUSTOM}?x=1&x=2&code={CODE}&state={STATE}");
        assert!(parse_callback(&url, &e).is_ok());
    }

    #[test]
    fn issuer_required() {
        let e = exp(CUSTOM, required());
        let url = format!("{CUSTOM}?code={CODE}&state={STATE}");
        assert_eq!(
            parse_callback(&url, &e).unwrap_err(),
            CallbackError::IssuerMissing
        );
        let url = format!("{CUSTOM}?code={CODE}&state={STATE}&iss=https%3A%2F%2Fevil.example");
        assert_eq!(
            parse_callback(&url, &e).unwrap_err(),
            CallbackError::IssuerMismatch
        );
        // Exact string comparison after decoding: a trailing slash differs.
        let url = format!("{CUSTOM}?code={CODE}&state={STATE}&iss={ISS_ENC}%2F");
        assert_eq!(
            parse_callback(&url, &e).unwrap_err(),
            CallbackError::IssuerMismatch
        );
        let url = format!("{CUSTOM}?code={CODE}&state={STATE}&iss={ISSUER}");
        assert!(parse_callback(&url, &e).is_ok());
    }

    #[test]
    fn issuer_if_present() {
        let e = exp(CUSTOM, if_present());
        let url = format!("{CUSTOM}?code={CODE}&state={STATE}");
        assert!(parse_callback(&url, &e).is_ok());
        let url = format!("{CUSTOM}?code={CODE}&state={STATE}&iss=https%3A%2F%2Fevil.example");
        assert_eq!(
            parse_callback(&url, &e).unwrap_err(),
            CallbackError::IssuerMismatch
        );
    }

    #[test]
    fn issuer_is_checked_before_state() {
        let e = exp(CUSTOM, required());
        let url = format!("{CUSTOM}?code={CODE}&state=wrong");
        assert_eq!(
            parse_callback(&url, &e).unwrap_err(),
            CallbackError::IssuerMissing
        );
    }

    #[test]
    fn state_missing_and_mismatch() {
        let e = exp(CUSTOM, if_present());
        let url = format!("{CUSTOM}?code={CODE}");
        assert_eq!(
            parse_callback(&url, &e).unwrap_err(),
            CallbackError::StateMissing
        );
        let url = format!("{CUSTOM}?code={CODE}&state=xyzSTATE-123_");
        assert_eq!(
            parse_callback(&url, &e).unwrap_err(),
            CallbackError::StateMismatch
        );
        let url = format!("{CUSTOM}?code={CODE}&state={STATE}x");
        assert_eq!(
            parse_callback(&url, &e).unwrap_err(),
            CallbackError::StateMismatch
        );
        let url = format!("{CUSTOM}?code={CODE}&state=");
        assert_eq!(
            parse_callback(&url, &e).unwrap_err(),
            CallbackError::StateMismatch
        );
    }

    #[test]
    fn error_response_with_matching_state_is_typed() {
        let e = exp(CUSTOM, required());
        let url = format!(
            "{CUSTOM}?error=access_denied&error_description=The+user+said+no\
             &error_uri=https%3A%2F%2Fas.example%2Ferr&state={STATE}&iss={ISS_ENC}"
        );
        let err = parse_callback(&url, &e).unwrap_err();
        assert_eq!(
            err,
            CallbackError::Authorization {
                error: AuthorizationErrorCode::AccessDenied,
                description: Some("The user said no".to_string()),
                uri: Some("https://as.example/err".to_string()),
            }
        );
        assert_eq!(
            err.to_string(),
            "the authorization server returned `access_denied`: The user said no"
        );

        let url = format!("{CUSTOM}?error=custom_thing&state={STATE}&iss={ISS_ENC}");
        assert_eq!(
            parse_callback(&url, &e).unwrap_err(),
            CallbackError::Authorization {
                error: AuthorizationErrorCode::Other("custom_thing".to_string()),
                description: None,
                uri: None,
            }
        );
    }

    #[test]
    fn error_response_with_wrong_state_is_state_mismatch() {
        let e = exp(CUSTOM, if_present());
        let url = format!("{CUSTOM}?error=access_denied&state=attacker");
        assert_eq!(
            parse_callback(&url, &e).unwrap_err(),
            CallbackError::StateMismatch
        );
        let url = format!("{CUSTOM}?error=access_denied");
        assert_eq!(
            parse_callback(&url, &e).unwrap_err(),
            CallbackError::StateMissing
        );
    }

    #[test]
    fn error_response_with_bad_charset_is_malformed() {
        let e = exp(CUSTOM, if_present());
        for query in [
            "error=",
            "error=bad%22quote",
            "error=access_denied&error_description=line%0Abreak",
            "error=access_denied&error_description=back%5Cslash",
            "error=access_denied&error_uri=has+space",
        ] {
            let url = format!("{CUSTOM}?{query}&state={STATE}");
            assert_eq!(
                parse_callback(&url, &e).unwrap_err(),
                CallbackError::Malformed,
                "{query}"
            );
        }
    }

    #[test]
    fn code_missing_or_empty() {
        let e = exp(CUSTOM, if_present());
        let url = format!("{CUSTOM}?state={STATE}");
        assert_eq!(
            parse_callback(&url, &e).unwrap_err(),
            CallbackError::CodeMissing
        );
        let url = format!("{CUSTOM}?code=&state={STATE}");
        assert_eq!(
            parse_callback(&url, &e).unwrap_err(),
            CallbackError::CodeMissing
        );
        assert_eq!(
            parse_callback(CUSTOM, &e).unwrap_err(),
            CallbackError::StateMissing
        );
    }

    #[test]
    fn plus_decodes_to_space() {
        let e = CallbackExpectations {
            redirect_uri: CUSTOM.to_string(),
            state: State::from_string(STATE.to_string()).unwrap(),
            issuer: IssuerCheck::Required("https://as.example/tenant one".to_string()),
        };
        let url = format!(
            "{CUSTOM}?code=a+b%2Bc&state={STATE}&iss=https%3A%2F%2Fas.example%2Ftenant+one"
        );
        assert_eq!(parse_callback(&url, &e).unwrap().as_str(), "a b+c");
    }

    #[test]
    fn secrets_never_reach_debug_or_display() {
        let e = exp(CUSTOM, required());
        let url = format!("{CUSTOM}?code={CODE}&state={STATE}&iss={ISS_ENC}");
        let code = parse_callback(&url, &e).unwrap();
        let shown = format!("{code:?}");
        assert_eq!(shown, "AuthorizationCode(<redacted>)");
        assert!(!shown.contains(CODE));

        let shown = format!("{e:?}");
        assert!(!shown.contains(STATE));

        let failures = [
            format!("{CUSTOM}?code={CODE}&state=wrong&iss={ISS_ENC}"),
            format!("{CUSTOM}?code={CODE}&state={STATE}&state={STATE}"),
            format!("{CUSTOM}?code={CODE}&state={STATE}"),
            format!("{LOOPBACK}?code={CODE}&state={STATE}"),
            format!("{CUSTOM}?code={CODE}&state={STATE}#x"),
        ];
        for url in failures {
            let err = parse_callback(&url, &e).unwrap_err();
            for shown in [err.to_string(), format!("{err:?}")] {
                assert!(!shown.contains(CODE), "{shown}");
                assert!(!shown.contains(STATE), "{shown}");
                assert!(!shown.contains("com.example"), "{shown}");
            }
        }
    }
}
