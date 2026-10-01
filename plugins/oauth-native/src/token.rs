//! Token endpoint request bodies and response parsing (RFC 6749 §4.1.3,
//! §5, §6) — without an HTTP client. The app sends [`FormBody`] with its own
//! transport and hands the status and body bytes to
//! [`parse_token_response`].

use std::fmt;

use serde::Deserialize;

use crate::callback::AuthorizationCode;
use crate::encode::{form_encode, is_error_text, is_error_uri};
use crate::pkce::PkceVerifier;

/// The `Content-Type` every token request body uses.
pub const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

/// A token request body, ready for an HTTP `POST` to the token endpoint.
/// Its `Debug` redacts the body, which carries the authorization code, the
/// PKCE verifier or the refresh token.
#[derive(Clone, PartialEq, Eq)]
pub struct FormBody {
    /// Always [`FORM_CONTENT_TYPE`].
    pub content_type: &'static str,
    /// The `application/x-www-form-urlencoded` body.
    pub body: String,
}

impl fmt::Debug for FormBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FormBody")
            .field("content_type", &self.content_type)
            .field("body", &"<redacted>")
            .finish()
    }
}

/// The authorization-code grant body (RFC 6749 §4.1.3 with RFC 7636
/// §4.5's `code_verifier` and RFC 8707's `resource`):
/// `grant_type=authorization_code&code=…&redirect_uri=…&client_id=…&code_verifier=…`
/// followed by one `resource=…` per entry.
pub fn authorization_code_grant(
    code: &AuthorizationCode,
    redirect_uri: &str,
    client_id: &str,
    verifier: &PkceVerifier,
    resources: &[String],
) -> FormBody {
    let mut pairs = vec![
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", verifier.as_str()),
    ];
    pairs.extend(resources.iter().map(|r| ("resource", r.as_str())));
    FormBody {
        content_type: FORM_CONTENT_TYPE,
        body: form_encode(pairs),
    }
}

/// The refresh-token grant body (RFC 6749 §6):
/// `grant_type=refresh_token&refresh_token=…&client_id=…`, then `scope=…`
/// when set, then one `resource=…` per entry.
pub fn refresh_token_grant(
    refresh_token: &str,
    client_id: &str,
    scope: Option<&str>,
    resources: &[String],
) -> FormBody {
    let mut pairs = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    if let Some(scope) = scope {
        pairs.push(("scope", scope));
    }
    pairs.extend(resources.iter().map(|r| ("resource", r.as_str())));
    FormBody {
        content_type: FORM_CONTENT_TYPE,
        body: form_encode(pairs),
    }
}

/// A successful token response (RFC 6749 §5.1). Its `Debug` redacts
/// `access_token`, `refresh_token` and `id_token`. Persist a refresh token
/// in secure storage, never in plain preferences.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TokenResponse {
    /// The access token (non-empty).
    pub access_token: String,
    /// The token type, e.g. `"Bearer"`.
    pub token_type: String,
    /// Lifetime of the access token in seconds, if given.
    pub expires_in: Option<u64>,
    /// A refresh token, if issued.
    pub refresh_token: Option<String>,
    /// The granted scope, if it differs from the requested one.
    pub scope: Option<String>,
    /// An OpenID Connect ID token, if issued. Returned exactly as the server
    /// sent it, unvalidated: before trusting it, validate it per OpenID
    /// Connect Core §3.1.3.7 (signature against the issuer's keys, `iss`,
    /// `aud`, `exp`, and `nonce` if you sent one). This crate sends no `nonce`
    /// and performs no ID-token validation.
    pub id_token: Option<String>,
}

impl fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redact = |present: bool| if present { "<redacted>" } else { "None" };
        f.debug_struct("TokenResponse")
            .field("access_token", &"<redacted>")
            .field("token_type", &self.token_type)
            .field("expires_in", &self.expires_in)
            .field("refresh_token", &redact(self.refresh_token.is_some()))
            .field("scope", &self.scope)
            .field("id_token", &redact(self.id_token.is_some()))
            .finish()
    }
}

/// An RFC 6749 §5.2 token error code.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TokenErrorCode {
    /// `invalid_request`
    InvalidRequest,
    /// `invalid_client`
    InvalidClient,
    /// `invalid_grant`
    InvalidGrant,
    /// `unauthorized_client`
    UnauthorizedClient,
    /// `unsupported_grant_type`
    UnsupportedGrantType,
    /// `invalid_scope`
    InvalidScope,
    /// Any other (extension) error code, charset-validated.
    Other(String),
}

impl TokenErrorCode {
    fn from_code(code: &str) -> Self {
        match code {
            "invalid_request" => Self::InvalidRequest,
            "invalid_client" => Self::InvalidClient,
            "invalid_grant" => Self::InvalidGrant,
            "unauthorized_client" => Self::UnauthorizedClient,
            "unsupported_grant_type" => Self::UnsupportedGrantType,
            "invalid_scope" => Self::InvalidScope,
            other => Self::Other(other.to_string()),
        }
    }

    /// The wire form of the code (`"invalid_grant"`, …).
    pub fn as_str(&self) -> &str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InvalidClient => "invalid_client",
            Self::InvalidGrant => "invalid_grant",
            Self::UnauthorizedClient => "unauthorized_client",
            Self::UnsupportedGrantType => "unsupported_grant_type",
            Self::InvalidScope => "invalid_scope",
            Self::Other(code) => code,
        }
    }
}

impl fmt::Display for TokenErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The token response was not a usable success. No message includes the
/// response body; [`Endpoint`](Self::Endpoint) prints only the status and
/// the error code.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TokenError {
    /// A 200 response whose body is not a JSON object with a non-empty
    /// string `access_token` and a string `token_type` (or whose optional
    /// members have the wrong type).
    #[error("the token endpoint returned a malformed success response")]
    Malformed,
    /// A non-200 response carrying an RFC 6749 §5.2 error object.
    #[error("the token endpoint returned HTTP {status} with error `{error}`")]
    Endpoint {
        /// The HTTP status code.
        status: u16,
        /// The error code.
        error: TokenErrorCode,
        /// The `error_description`, if any.
        description: Option<String>,
        /// The `error_uri`, if any.
        uri: Option<String>,
    },
    /// A non-200 response without a valid RFC 6749 §5.2 error object.
    #[error("the token endpoint returned unexpected HTTP status {0}")]
    UnexpectedStatus(u16),
}

#[derive(Deserialize)]
struct RawSuccess {
    access_token: String,
    token_type: String,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
}

#[derive(Deserialize)]
struct RawError {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
    #[serde(default)]
    error_uri: Option<String>,
}

/// Parses a token endpoint response from its HTTP status and raw body.
/// A 200 must be an RFC 6749 §5.1 success object (unknown members are
/// ignored); any other status is an [`TokenError::Endpoint`] when the body
/// is a charset-valid §5.2 error object, else
/// [`TokenError::UnexpectedStatus`].
pub fn parse_token_response(http_status: u16, body: &[u8]) -> Result<TokenResponse, TokenError> {
    if http_status == 200 {
        let raw: RawSuccess = serde_json::from_slice(body).map_err(|_| TokenError::Malformed)?;
        if raw.access_token.is_empty() {
            return Err(TokenError::Malformed);
        }
        return Ok(TokenResponse {
            access_token: raw.access_token,
            token_type: raw.token_type,
            expires_in: raw.expires_in,
            refresh_token: raw.refresh_token,
            scope: raw.scope,
            id_token: raw.id_token,
        });
    }
    let Ok(raw) = serde_json::from_slice::<RawError>(body) else {
        return Err(TokenError::UnexpectedStatus(http_status));
    };
    let valid = !raw.error.is_empty()
        && is_error_text(&raw.error)
        && raw.error_description.as_deref().is_none_or(is_error_text)
        && raw.error_uri.as_deref().is_none_or(is_error_uri);
    if !valid {
        return Err(TokenError::UnexpectedStatus(http_status));
    }
    Err(TokenError::Endpoint {
        status: http_status,
        error: TokenErrorCode::from_code(&raw.error),
        description: raw.error_description,
        uri: raw.error_uri,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::callback::{CallbackExpectations, IssuerCheck, parse_callback};
    use crate::pkce::State;

    const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    const REDIRECT: &str = "com.example.app:/oauth/callback";
    const STATE: &str = "state-value-minimum-22chars";

    fn code(value: &str) -> AuthorizationCode {
        let exp = CallbackExpectations {
            redirect_uri: REDIRECT.to_string(),
            state: State::from_string(STATE.to_string()).unwrap(),
            issuer: IssuerCheck::IfPresent("https://as.example".to_string()),
        };
        parse_callback(&format!("{REDIRECT}?code={value}&state={STATE}"), &exp).unwrap()
    }

    #[test]
    fn authorization_code_grant_body() {
        let verifier = PkceVerifier::from_string(VERIFIER.to_string()).unwrap();
        let body = authorization_code_grant(
            &code("Splx+lO%2FB"),
            REDIRECT,
            "client 1",
            &verifier,
            &["https://api.example/".to_string()],
        );
        assert_eq!(body.content_type, "application/x-www-form-urlencoded");
        assert_eq!(
            body.body,
            format!(
                "grant_type=authorization_code&code=Splx+lO%2FB\
                 &redirect_uri=com.example.app%3A%2Foauth%2Fcallback&client_id=client+1\
                 &code_verifier={VERIFIER}&resource=https%3A%2F%2Fapi.example%2F"
            )
        );
        let body = authorization_code_grant(&code("c"), REDIRECT, "app", &verifier, &[]);
        assert_eq!(
            body.body,
            format!(
                "grant_type=authorization_code&code=c\
                 &redirect_uri=com.example.app%3A%2Foauth%2Fcallback&client_id=app\
                 &code_verifier={VERIFIER}"
            )
        );
    }

    #[test]
    fn refresh_token_grant_body() {
        let body = refresh_token_grant("rt/1+2", "app", Some("openid offline_access"), &[]);
        assert_eq!(
            body.body,
            "grant_type=refresh_token&refresh_token=rt%2F1%2B2&client_id=app\
             &scope=openid+offline_access"
        );
        let body = refresh_token_grant(
            "rt",
            "app",
            None,
            &["urn:a".to_string(), "https://b.example".to_string()],
        );
        assert_eq!(
            body.body,
            "grant_type=refresh_token&refresh_token=rt&client_id=app\
             &resource=urn%3Aa&resource=https%3A%2F%2Fb.example"
        );
        assert_eq!(body.content_type, FORM_CONTENT_TYPE);
    }

    #[test]
    fn parses_a_200_success() {
        let body = br#"{"access_token":"AT-123","token_type":"Bearer","expires_in":3600,
            "refresh_token":"RT-456","scope":"openid","id_token":"ID-789","extra":{"x":1}}"#;
        let token = parse_token_response(200, body).unwrap();
        assert_eq!(token.access_token, "AT-123");
        assert_eq!(token.token_type, "Bearer");
        assert_eq!(token.expires_in, Some(3600));
        assert_eq!(token.refresh_token.as_deref(), Some("RT-456"));
        assert_eq!(token.scope.as_deref(), Some("openid"));
        assert_eq!(token.id_token.as_deref(), Some("ID-789"));

        let minimal =
            parse_token_response(200, br#"{"access_token":"a","token_type":"x"}"#).unwrap();
        assert_eq!(minimal.expires_in, None);
        assert_eq!(minimal.refresh_token, None);
    }

    #[test]
    fn malformed_200_responses() {
        for body in [
            &br#"{"token_type":"Bearer"}"#[..],
            br#"{"access_token":"","token_type":"Bearer"}"#,
            br#"{"access_token":"a"}"#,
            br#"{"access_token":1,"token_type":"Bearer"}"#,
            br#"{"access_token":"a","token_type":"Bearer","expires_in":"3600"}"#,
            br#"{"access_token":"a","token_type":"Bearer","expires_in":-1}"#,
            br#"["access_token"]"#,
            b"not json",
            b"",
        ] {
            assert_eq!(
                parse_token_response(200, body).unwrap_err(),
                TokenError::Malformed,
                "{}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn error_object_becomes_endpoint_error() {
        let body = br#"{"error":"invalid_grant","error_description":"code expired",
            "error_uri":"https://as.example/e"}"#;
        let err = parse_token_response(400, body).unwrap_err();
        assert_eq!(
            err,
            TokenError::Endpoint {
                status: 400,
                error: TokenErrorCode::InvalidGrant,
                description: Some("code expired".to_string()),
                uri: Some("https://as.example/e".to_string()),
            }
        );
        assert_eq!(
            err.to_string(),
            "the token endpoint returned HTTP 400 with error `invalid_grant`"
        );
        assert_eq!(
            parse_token_response(401, br#"{"error":"invalid_client"}"#).unwrap_err(),
            TokenError::Endpoint {
                status: 401,
                error: TokenErrorCode::InvalidClient,
                description: None,
                uri: None,
            }
        );
        assert_eq!(
            parse_token_response(400, br#"{"error":"x_custom"}"#).unwrap_err(),
            TokenError::Endpoint {
                status: 400,
                error: TokenErrorCode::Other("x_custom".to_string()),
                description: None,
                uri: None,
            }
        );
    }

    #[test]
    fn non_error_bodies_are_unexpected_status() {
        assert_eq!(
            parse_token_response(500, b"Internal Server Error").unwrap_err(),
            TokenError::UnexpectedStatus(500)
        );
        assert_eq!(
            parse_token_response(502, br#"{"message":"bad gateway"}"#).unwrap_err(),
            TokenError::UnexpectedStatus(502)
        );
        assert_eq!(
            parse_token_response(400, br#"{"error":"bad\nchars"}"#).unwrap_err(),
            TokenError::UnexpectedStatus(400)
        );
        assert_eq!(
            parse_token_response(400, br#"{"error":""}"#).unwrap_err(),
            TokenError::UnexpectedStatus(400)
        );
        // A 201 carrying a success-shaped body is not a 200.
        assert_eq!(
            parse_token_response(201, br#"{"access_token":"a","token_type":"x"}"#).unwrap_err(),
            TokenError::UnexpectedStatus(201)
        );
    }

    #[test]
    fn errors_never_print_the_body() {
        let secret = "SECRET-BODY-TEXT";
        let bodies: [(u16, String); 3] = [
            (200, format!(r#"{{"token_type":"{secret}"}}"#)),
            (500, secret.to_string()),
            (
                400,
                format!(r#"{{"error":"invalid_grant","error_description":"{secret}"}}"#),
            ),
        ];
        for (status, body) in bodies {
            let err = parse_token_response(status, body.as_bytes()).unwrap_err();
            assert!(!err.to_string().contains(secret), "{err}");
        }
    }

    #[test]
    fn debug_redacts_token_secrets_and_form_body() {
        let body = br#"{"access_token":"AT-123","token_type":"Bearer",
            "refresh_token":"RT-456","id_token":"ID-789","scope":"openid"}"#;
        let token = parse_token_response(200, body).unwrap();
        let shown = format!("{token:?}");
        for secret in ["AT-123", "RT-456", "ID-789"] {
            assert!(!shown.contains(secret), "{shown}");
        }
        assert!(shown.contains("Bearer"));

        let verifier = PkceVerifier::from_string(VERIFIER.to_string()).unwrap();
        let form = authorization_code_grant(&code("CODE-XYZ"), REDIRECT, "app", &verifier, &[]);
        let shown = format!("{form:?}");
        assert!(!shown.contains("CODE-XYZ"), "{shown}");
        assert!(!shown.contains(VERIFIER), "{shown}");
        assert!(shown.contains(FORM_CONTENT_TYPE));
        let form = refresh_token_grant("RT-456", "app", None, &[]);
        assert!(!format!("{form:?}").contains("RT-456"));
    }
}
